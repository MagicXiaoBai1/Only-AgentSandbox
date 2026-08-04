//! 进程无关的 firecracker 恢复原语集 —— Path A `ShimImpl` 与 Path B `RealVm` 共享的底层。
//!
//! 设计约束（见 ADR 0009）：
//! - **不含 ttrpc**：恢复逻辑与传输形态解耦，`ShimImpl` 把它包成 ttrpc `Create/State/Stop`，
//!   `RealVm` 把它包成 `SandboxVm` trait。
//! - **不含 condvar / 不含 tokio**：纯同步库。`RealVm`（异步 `SandboxVm`）经
//!   `tokio::task::spawn_blocking` 桥接；`ShimImpl` 本就在 ttrpc sync 线程里直接调。
//! - **幂等 `create`**：先 `find_fc_pid(jail_root)`，活则 re-attach、无则全量 restore。
//!   `shim.meta` 记 fc_pid + mntns inode，供应急杀做身份校验。
//!
//! in-VM exec 的扩展 seam（`VmHandle::guest`）见 ADR 0011，现恒为 `None`。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::unistd::{chown, Gid, Uid};
use oas_config::Config;

use crate::firecracker::FirecrackerClient;
use crate::identity::{find_fc_pid, mntns_inode, pid_alive, terminate_pid, ShimMeta};

// ---------------------------------------------------------------------------
// 输入 / 产物
// ---------------------------------------------------------------------------

/// 恢复输入：`CreateRequest`（ttrpc）的进程无关等价物。
///
/// `ShimImpl` 从 `CreateRequest` 翻译出本结构；`RealVm` 从 `(Config, sid, type_id)` 构造。
#[derive(Debug, Clone)]
pub struct RestoreInputs {
    /// bundle 目录（`vmlinux`/`rootfs.ext4`/`vmstate`/`mem` 四件套所在）。
    pub bundle_dir: PathBuf,
    /// jailer `--netns` 指向的 netns 路径。
    pub netns_path: String,
    /// per-VM 可写 ext4（宿主路径），materialize 到 jail 内 `/data.ext4`；无则 None。
    pub rw_layer_path: Option<PathBuf>,
    pub jailer_uid: u32,
    pub jailer_gid: u32,
    pub chroot_base_dir: PathBuf,
    pub firecracker_bin: PathBuf,
    pub jailer_bin: PathBuf,
    /// per-VM 云盘块设备。MVP 不支持（非 None → restore 报错）。
    pub cloud_disk_dev: Option<String>,
}

/// 恢复产物。
#[derive(Debug, Clone)]
pub struct VmHandle {
    /// firecracker 进程的 host pid。
    pub fc_pid: u32,
    /// create 时 `stat(/proc/<fc_pid>/ns/mnt).st_ino`（身份锚点）。
    pub mntns_inode: u64,
    /// 未来 in-VM exec 的传输句柄；现恒为 `None`（见 ADR 0011）。
    pub guest: Option<GuestEndpoint>,
}

/// 未来 guest-agent 传输方式。预留 seam，暂不填充。
#[derive(Debug, Clone)]
pub enum GuestEndpoint {
    Vsock { cid: u32, port: u16 },
    UnixSock { path: PathBuf },
}

/// VM 生命周期状态（`ShimImpl` 与 `RealVm` 各自映射到自己的对外状态枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmCoreState {
    /// 尚未 create。
    NotExist,
    Running,
    Stopped,
    Failed,
}

struct CoreInner {
    fc_pid: Option<u32>,
    state: VmCoreState,
    created: bool,
}

/// 进程无关的 VM 恢复/看护核心。持有 `(Config, sandbox_id)`，构造时算好各路径。
pub struct VmCore {
    cfg: Arc<Config>,
    sandbox_id: String,
    sandbox_dir: PathBuf,
    jail_root: PathBuf,
    meta_path: PathBuf,
    inner: Mutex<CoreInner>,
}

impl VmCore {
    pub fn new(cfg: Arc<Config>, sandbox_id: impl Into<String>) -> Self {
        let sandbox_id = sandbox_id.into();
        let sandbox_dir = cfg.sandbox_dir(&sandbox_id);
        let jail_root = cfg.jail_root(&sandbox_id);
        let meta_path = sandbox_dir.join("shim.meta");
        Self {
            cfg,
            sandbox_id,
            sandbox_dir,
            jail_root,
            meta_path,
            inner: Mutex::new(CoreInner {
                fc_pid: None,
                state: VmCoreState::NotExist,
                created: false,
            }),
        }
    }

    pub fn sandbox_id(&self) -> &str {
        &self.sandbox_id
    }
    pub fn sandbox_dir(&self) -> &Path {
        &self.sandbox_dir
    }
    pub fn jail_root(&self) -> &Path {
        &self.jail_root
    }
    pub fn meta_path(&self) -> &Path {
        &self.meta_path
    }

    /// fc API socket（jail 内 `/run/firecracker.socket`）。
    fn fc_socket(&self) -> PathBuf {
        self.jail_root.join("run").join("firecracker.socket")
    }

    /// 幂等 create：已有活 fc → re-attach；否则全量 restore。同步阻塞到 Running/Failed。
    pub fn create(&self, inputs: &RestoreInputs) -> Result<VmHandle, String> {
        // 幂等快速路径：已 create 且 fc 仍活 → 直接回句柄。
        {
            let st = self.inner.lock().unwrap();
            if st.created {
                if let Some(pid) = st.fc_pid {
                    if pid_alive(pid) {
                        let ino = mntns_inode(pid)
                            .map_err(|e| format!("stat mntns: {e}"))?;
                        return Ok(VmHandle {
                            fc_pid: pid,
                            mntns_inode: ino,
                            guest: None,
                        });
                    }
                }
            }
        }

        let result = if find_fc_pid(&self.jail_root).is_some() {
            tracing::info!(target: "oas-shim", sid = %self.sandbox_id, "create: re-attach path");
            self.re_attach()
        } else {
            tracing::info!(target: "oas-shim", sid = %self.sandbox_id, "create: fresh-restore path");
            self.fresh_restore(inputs)
        };

        match &result {
            Ok(h) => {
                let mut st = self.inner.lock().unwrap();
                st.fc_pid = Some(h.fc_pid);
                st.state = VmCoreState::Running;
                st.created = true;
            }
            Err(_) => {
                self.inner.lock().unwrap().state = VmCoreState::Failed;
            }
        }
        result
    }

    /// re-attach 已存活的 firecracker（runtime re-spawn shim 后调用）。
    pub fn re_attach(&self) -> Result<VmHandle, String> {
        let t0 = Instant::now();
        let t = Instant::now();
        let fc_pid = find_fc_pid(&self.jail_root)
            .ok_or_else(|| "no live firecracker to re-attach".to_string())?;
        log_step(&self.sandbox_id, "re_attach:find_fc_pid", t);

        let t = Instant::now();
        let ino = mntns_inode(fc_pid).map_err(|e| format!("stat mntns: {e}"))?;
        let meta = ShimMeta {
            fc_pid,
            mntns_inode: ino,
            started_at: 0,
            shim_pid: std::process::id(),
        };
        let _ = meta.write(&self.meta_path);
        log_step(&self.sandbox_id, "re_attach:write_meta", t);

        log_step(&self.sandbox_id, "re_attach_total", t0);
        Ok(VmHandle {
            fc_pid,
            mntns_inode: ino,
            guest: None,
        })
    }

    /// 全量恢复：materialize → jailer → 等 socket → snapshot/load → 写 meta。
    pub fn fresh_restore(&self, req: &RestoreInputs) -> Result<VmHandle, String> {
        if req.cloud_disk_dev.as_deref().is_some_and(|s| !s.is_empty()) {
            return Err("cloud disk restore not supported in MVP".into());
        }
        let t0 = Instant::now();
        let bundle = &req.bundle_dir;
        // materialize 进 jail root 固定路径。
        let t = Instant::now();
        std::fs::create_dir_all(&self.jail_root).map_err(|e| format!("mkdir jail root: {e}"))?;
        copy_file(bundle.join("vmlinux"), self.jail_root.join("vmlinux"))?;
        copy_file(bundle.join("rootfs.ext4"), self.jail_root.join("rootfs.ext4"))?;
        copy_file(bundle.join("vmstate"), self.jail_root.join("vmstate.src"))?;
        copy_file(bundle.join("mem"), self.jail_root.join("mem.src"))?;
        if let Some(rw) = &req.rw_layer_path {
            copy_file(rw, self.jail_root.join("data.ext4"))?;
        }
        log_step(&self.sandbox_id, "materialize", t);

        // chown + chmod（跟随实验：0700 dir / 0444 ro / 0666 rw）。
        let t = Instant::now();
        chmod(&self.jail_root, 0o0700);
        chown_path(&self.jail_root, req.jailer_uid, req.jailer_gid);
        for name in ["vmlinux", "rootfs.ext4", "vmstate.src", "mem.src"] {
            let p = self.jail_root.join(name);
            chmod(&p, 0o0444);
            chown_path(&p, req.jailer_uid, req.jailer_gid);
        }
        if req.rw_layer_path.is_some() {
            let p = self.jail_root.join("data.ext4");
            chmod(&p, 0o0666);
            chown_path(&p, req.jailer_uid, req.jailer_gid);
        }
        log_step(&self.sandbox_id, "chmod_chown", t);

        // spawn jailer（--daemonize 后 jailer 自身很快退出）。
        let t = Instant::now();
        let mut j = Command::new(&req.jailer_bin);
        j.arg("--id")
            .arg(&self.sandbox_id)
            .arg("--exec-file")
            .arg(&req.firecracker_bin)
            .arg("--uid")
            .arg(req.jailer_uid.to_string())
            .arg("--gid")
            .arg(req.jailer_gid.to_string())
            .arg("--chroot-base-dir")
            .arg(&req.chroot_base_dir);
        // cgroup2-only host 必须显式 --cgroup-version=2：jailer 默认 v1 在 cgroup2 上
        // cgroup 设置异常，firecracker 会被 device 控制器拒访 /dev/kvm（Kvm Permission denied）。
        if host_cgroup_v2() {
            j.arg("--cgroup-version").arg("2");
        }
        j.arg("--new-pid-ns")
            .arg("--netns")
            .arg(&req.netns_path)
            .arg("--daemonize")
            .arg("--")
            .arg("--api-sock")
            .arg("run/firecracker.socket")
            // 让 firecracker 从启动起就写日志（独立于 API PUT /logger），便于诊断不响应/退出。
            .arg("--log-path")
            .arg(format!("fc-{}.log", self.sandbox_id))
            .arg("--level")
            .arg("Debug");
        let output = j.output().map_err(|e| format!("spawn jailer: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "jailer exited {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        log_step(&self.sandbox_id, "jailer_spawn", t);

        // 等 API socket。5ms 轮询，上限 10s。
        let t = Instant::now();
        let sock = self.fc_socket();
        let mut waited = 0;
        std::thread::sleep(Duration::from_millis(5));
        while !sock.exists() {
            std::thread::sleep(Duration::from_millis(5));
            waited += 1;
            if waited > 2000 {
                return Err("timeout waiting for firecracker socket".into());
            }
        }
        log_step(&self.sandbox_id, "wait_fc_socket", t);

        // 找到 firecracker 的 host pid（按 jail root 命中 /proc/pid/root）。
        let t = Instant::now();
        let fc_pid = find_fc_pid(&self.jail_root)
            .ok_or_else(|| "firecracker pid not found after jailer spawn".to_string())?;
        log_step(&self.sandbox_id, "find_fc_pid", t);

        // [DEBUG] OAS_DEBUG=1 时 dump shim/fc 的 cgroup + caps + jail /dev/kvm，定位 /dev/kvm 拒访。
        if std::env::var("OAS_DEBUG").as_deref() == Ok("1") {
            self.dump_kvm_diag(fc_pid);
        }

        // logger + snapshot/load + 写 meta。任一失败都要终止已拉起的 firecracker，
        // 否则进程会泄漏（fc_pid 此刻还没写入 state，create 的错误分支不会清理）。
        let result = (|| -> Result<VmHandle, String> {
            let fc = FirecrackerClient::new(&sock);

            let t = Instant::now();
            // firecracker 被 chroot，logger 路径必须 jail 内相对路径（cwd=jail_root）。
            // 用与 --log-path 同名，写 jail_root/fc-<sid>.log；host 侧日志由 save_fc_log 拷出。
            fc.put_logger(Path::new(&format!("fc-{}.log", self.sandbox_id)));
            log_step(&self.sandbox_id, "put_logger", t);

            let t = Instant::now();
            fc.put_snapshot_load("/vmstate.src", "/mem.src")
                .map_err(|e| e.to_string())?;
            log_step(&self.sandbox_id, "snapshot_load", t);

            // A1：可选等待 guest-agent TCP 可达后再报 Running。
            if self.cfg.net.wait_guest_agent {
                let t = Instant::now();
                wait_tcp_in_netns(
                    &req.netns_path,
                    &self.cfg.net.guest_ip,
                    self.cfg.net.guest_agent_port,
                    self.cfg.net.guest_agent_wait_secs,
                )?;
                log_step(&self.sandbox_id, "wait_guest_agent", t);
            }

            // 写 shim.meta（mntns inode 身份锚点）。
            let t = Instant::now();
            let ino = mntns_inode(fc_pid).map_err(|e| format!("stat mntns: {e}"))?;
            let meta = ShimMeta {
                fc_pid,
                mntns_inode: ino,
                started_at: 0,
                shim_pid: std::process::id(),
            };
            let _ = meta.write(&self.meta_path);
            log_step(&self.sandbox_id, "write_meta", t);

            Ok(VmHandle {
                fc_pid,
                mntns_inode: ino,
                guest: None,
            })
        })();
        log_step(&self.sandbox_id, "fresh_restore_total", t0);
        if result.is_err() {
            terminate_pid(fc_pid);
            // firecracker --log-path 写到 jail 内（cwd=jail_root）；jail root 随后会被
            // emergency_kill 清掉，故把 fc 日志拷到 host 路径留底便于诊断。
            self.save_fc_log();
        }
        result
    }

    /// 把 jail 内的 firecracker 日志（`<jail_root>/fc-<sid>.log`）拷到 `cfg.log_dir`。
    /// firecracker 被 chroot，`PUT /logger` 只能写 jail 内路径；host 侧日志靠本函数拷出。
    fn save_fc_log(&self) {
        let jail_log = self.jail_root.join(format!("fc-{}.log", self.sandbox_id));
        if !jail_log.exists() {
            return;
        }
        let host_log = self.cfg.log_dir.join(format!("fc-{}.log", self.sandbox_id));
        let _ = std::fs::create_dir_all(&self.cfg.log_dir);
        let _ = std::fs::copy(&jail_log, &host_log);
        tracing::debug!(target: "oas-shim", sid = %self.sandbox_id, "fc log saved to {}", host_log.display());
    }

    /// [DEBUG] dump shim 与 firecracker 的 cgroup/caps/namespace + /dev/kvm 可见性,
    /// 定位 containerd 拉起的 shim 上下文是否拒访 /dev/kvm。写到 /tmp + stderr。
    fn dump_kvm_diag(&self, fc_pid: u32) {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| format!("<{e}>"));
        let rlink = |p: &str| {
            std::fs::read_link(p)
                .map(|x| x.display().to_string())
                .unwrap_or_else(|e| format!("<{e}>"))
        };
        let shim_cgroup = read("/proc/self/cgroup");
        let shim_mntns = rlink("/proc/self/ns/mnt");
        let fc_cgroup = read(&format!("/proc/{fc_pid}/cgroup"));
        let fc_status = read(&format!("/proc/{fc_pid}/status"));
        let fc_caps: String = fc_status
            .lines()
            .filter(|l| l.starts_with("Cap"))
            .collect::<Vec<_>>()
            .join("\n");
        let fc_mntns = rlink(&format!("/proc/{fc_pid}/ns/mnt"));
        let kvm_meta = std::fs::metadata("/dev/kvm")
            .map(|m| format!("exists (mode {:o})", m.permissions().mode()))
            .unwrap_or_else(|e| format!("<{e}>"));
        // jail 内 /dev/kvm（firecracker chroot 看到的）：权限/属主决定 1234 能否 open。
        use std::os::unix::fs::MetadataExt;
        let jail_dev = self.jail_root.join("dev");
        let jail_dev_ls = std::fs::read_dir(&jail_dev)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|e| format!("<{e}>"));
        let jail_kvm = jail_dev.join("kvm");
        let jail_kvm_info = std::fs::metadata(&jail_kvm)
            .map(|m| {
                format!(
                    "exists mode={:o} uid={} gid={} rdev={}:{}",
                    m.permissions().mode(),
                    m.uid(),
                    m.gid(),
                    m.rdev() >> 8,
                    m.rdev() & 0xff
                )
            })
            .unwrap_or_else(|e| format!("<{e}>"));
        let diag = format!(
            "=== oas-shim kvm diag (sid={}) ===\n\
             [shim] pid={}\n[shim] cgroup={}\n[shim] mntns={}\n\
             [fc]   pid={fc_pid}\n[fc]   cgroup={}\n[fc]   mntns={}\n[fc]   caps:\n{fc_caps}\n\
             [host] /dev/kvm: {kvm_meta}\n\
             [jail] /dev/ : {jail_dev_ls}\n\
             [jail] /dev/kvm: {jail_kvm_info}\n",
            self.sandbox_id,
            std::process::id(),
            shim_cgroup.trim(),
            shim_mntns,
            fc_cgroup.trim(),
            fc_mntns,
        );
        let _ = std::fs::write(format!("/tmp/oas-kvm-diag-{}.txt", self.sandbox_id), &diag);
        eprintln!("{diag}");
    }

    /// 当前状态 + pid（实时探测 fc 存活：Running 但 fc 已死 → Stopped）。
    pub fn liveness(&self) -> (VmCoreState, u32) {
        let st = self.inner.lock().unwrap();
        let mut state = st.state;
        let pid = st.fc_pid.unwrap_or(0);
        drop(st);
        if state == VmCoreState::Running && pid != 0 && !pid_alive(pid) {
            state = VmCoreState::Stopped;
        }
        (state, pid)
    }

    /// kill fc + 清 jail root + 清 meta。幂等。不改动 wait 通知（由调用方包 watch）。
    pub fn cleanup(&self) {
        if let Some(pid) = self.inner.lock().unwrap().fc_pid {
            terminate_pid(pid);
        }
        let _ = std::fs::remove_dir_all(&self.jail_root);
        let _ = std::fs::remove_file(&self.meta_path);
        self.inner.lock().unwrap().state = VmCoreState::Stopped;
    }
}

// ---- 辅助 ------------------------------------------------------------------

/// host 是否为 cgroup v2（unified）。`/sys/fs/cgroup/cgroup.controllers` 在 cgroup2
/// 根挂载下存在；纯 cgroup v1 无此文件。用于决定 jailer 的 `--cgroup-version`。
fn host_cgroup_v2() -> bool {
    std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}

/// 在沙箱 netns 内轮询 TCP 连通 `guest_ip:port`（guest-agent Ready）。
fn wait_tcp_in_netns(
    netns_path: &str,
    guest_ip: &str,
    port: u16,
    timeout_secs: u64,
) -> Result<(), String> {
    let ns = netns_path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(netns_path);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let py = format!(
        "import socket; s=socket.create_connection(('{guest_ip}',{port}),1); s.close()"
    );
    while Instant::now() < deadline {
        let ok = Command::new("ip")
            .args(["netns", "exec", ns, "python3", "-c", &py])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        // bash /dev/tcp 兜底（无 python3 时）。
        let bash_ok = Command::new("ip")
            .args([
                "netns",
                "exec",
                ns,
                "bash",
                "-c",
                &format!("echo >/dev/tcp/{guest_ip}/{port}"),
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if bash_ok {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "timeout waiting for guest-agent {guest_ip}:{port} in netns {ns} ({timeout_secs}s)"
    ))
}

/// 记录单个恢复子步骤的耗时（ms）。
fn log_step(sid: &str, step: &str, start: Instant) {
    tracing::info!(
        target: "oas-shim",
        sid = %sid,
        step = %step,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "restore step"
    );
}

/// reflink 优先的文件拷贝（写时复制 materialize）。
fn copy_file(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), String> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    match reflink_copy(src, dst) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::debug!(
                target: "oas-shim",
                src = %src.display(), dst = %dst.display(), err = %e,
                "reflink unavailable, fallback to full copy"
            );
            std::fs::copy(src, dst)
                .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
            Ok(())
        }
    }
}

/// `FICLONE` = `_IOW(0x94, 9, int)` = 0x40049409，见 Linux `<fs.h>`。
const FICLONE: nix::libc::c_ulong = 0x40049409;

fn reflink_copy(src: &Path, dst: &Path) -> Result<(), String> {
    use std::os::unix::io::AsRawFd;
    let s = std::fs::File::open(src).map_err(|e| format!("open src: {e}"))?;
    let d = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dst)
        .map_err(|e| format!("open dst: {e}"))?;
    // ioctl(dst_fd, FICLONE, src_fd) — 让 dst 成为 src 的 CoW 克隆。
    let r = unsafe { nix::libc::ioctl(d.as_raw_fd(), FICLONE, s.as_raw_fd()) };
    if r == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    Err(format!("ioctl FICLONE: {err}"))
}

fn chmod(path: &Path, mode: u32) {
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn chown_path(path: &Path, uid: u32, gid: u32) {
    let _ = chown(path, Some(Uid::from(uid)), Some(Gid::from(gid)));
}
