//! Shim 侧逻辑：ttrpc server + `Create`/`State`/`Stop` + jailer + materialize + snapshot/load + re-attach。
//!
//! 进程模型：shim 是单 VM 守护进程。ttrpc sync server 起 handler 线程池，`Create` 在其中
//! 同步完成恢复（re-attach 或全量 restore）。`run()` 起 server 后阻塞在 exit condvar 上，
//! `Stop` 清理后触发退出。
//!
//! 幂等：`Create` 先 `find_fc_pid(jail_root)`，若已有活着的 firecracker（其 `/proc/pid/root`
//! 命中 jail root）则 re-attach，否则全量 restore。`shim.meta` 记 fc_pid + mntns inode，
//! 供 runtime 应急杀做身份校验。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nix::unistd::{chown, Gid, Uid};
use oas_config::Config;
use ttrpc::Server;

use crate::firecracker::FirecrackerClient;
use crate::generated::shim::{
    CreateRequest, CreateResponse, StateRequest, StateResponse, StopRequest, StopResponse,
};
use crate::generated::shim_ttrpc::{create_shim, Shim};
use crate::identity::{find_fc_pid, mntns_inode, pid_alive, terminate_pid, ShimMeta};

/// shim 启动参数（由 main.rs 解析 clap 后构造）。
pub struct ShimArgs {
    pub config_path: PathBuf,
    pub sandbox_id: String,
    pub socket: PathBuf,
    pub log_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Pending,
    Running,
    Stopped,
    Failed,
}

impl Lifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Pending => "Pending",
            Lifecycle::Running => "Running",
            Lifecycle::Stopped => "Stopped",
            Lifecycle::Failed => "Failed",
        }
    }
}

struct Inner {
    fc_pid: Option<u32>,
    lifecycle: Lifecycle,
    created: bool,
}

struct ShimImpl {
    cfg: Arc<Config>,
    sandbox_id: String,
    sandbox_dir: PathBuf,
    jail_root: PathBuf,
    meta_path: PathBuf,
    state: Mutex<Inner>,
    exit: Arc<(Mutex<bool>, Condvar)>,
    watch_started: AtomicBool,
}

/// shim 进程入口。阻塞至 `Stop` 触发退出。
pub fn run(args: ShimArgs) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Arc::new(Config::load(&args.config_path));
    let sandbox_dir = cfg.sandbox_dir(&args.sandbox_id);
    let jail_root = cfg.jail_root(&args.sandbox_id);
    std::fs::create_dir_all(&sandbox_dir)?;
    // socket 父目录
    if let Some(p) = args.socket.parent() {
        std::fs::create_dir_all(p)?;
    }
    let _ = std::fs::remove_file(&args.socket);

    let exit = Arc::new((Mutex::new(false), Condvar::new()));
    let impl_ = Arc::new(ShimImpl {
        cfg,
        sandbox_id: args.sandbox_id.clone(),
        sandbox_dir: sandbox_dir.clone(),
        jail_root,
        meta_path: sandbox_dir.join("shim.meta"),
        state: Mutex::new(Inner {
            fc_pid: None,
            lifecycle: Lifecycle::Pending,
            created: false,
        }),
        exit: exit.clone(),
        watch_started: AtomicBool::new(false),
    });

    let methods = create_shim(impl_.clone() as Arc<dyn Shim + Send + Sync>);
    let sockaddr = format!("unix://{}", args.socket.display());
    let mut server = Server::new()
        .bind(&sockaddr)?
        .register_service(methods);
    server.start()?;

    tracing::info!(target: "oas-shim", sid = %args.sandbox_id, "shim serving, waiting for Create");

    // 阻塞至 Stop 触发 exit（Stop 置位 *m=true 并 notify_all）。
    let (m, c) = &*exit;
    let mut g = m.lock().unwrap();
    while !*g {
        g = c.wait(g).unwrap();
    }
    drop(g);
    // 给 ttrpc 把 Stop 响应发回去留点时间。
    std::thread::sleep(Duration::from_millis(150));
    Ok(())
}

impl ShimImpl {
    /// fc API socket 路径（jail 内 `/run/firecracker.socket`）。
    fn fc_socket(&self) -> PathBuf {
        self.jail_root.join("run").join("firecracker.socket")
    }

    fn start_watch(&self, fc_pid: u32) {
        if self.watch_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = Arc::new(());
        let _ = state;
        let jail_root = self.jail_root.clone();
        let sid = self.sandbox_id.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if !pid_alive(fc_pid) {
                    tracing::warn!(target: "oas-shim", sid = %sid, fc_pid, "firecracker died");
                    let _ = jail_root;
                    // 标记由 Stop / State 时按需查询；此处仅记日志，避免跨 Mutex 复杂度。
                    return;
                }
            }
        });
    }

    /// 全量恢复：materialize → jailer → 等 socket → snapshot/load → 写 meta。
    fn fresh_restore(&self, req: &CreateRequest) -> Result<(), String> {
        if !req.cloud_disk_dev.is_empty() {
            return Err("cloud disk restore not supported in MVP".into());
        }
        let t0 = Instant::now();
        let bundle = Path::new(&req.bundle_dir);
        // materialize 进 jail root 固定路径。
        let t = Instant::now();
        std::fs::create_dir_all(&self.jail_root).map_err(|e| format!("mkdir jail root: {e}"))?;
        copy_file(bundle.join("vmlinux"), self.jail_root.join("vmlinux"))?;
        copy_file(bundle.join("rootfs.ext4"), self.jail_root.join("rootfs.ext4"))?;
        copy_file(bundle.join("vmstate"), self.jail_root.join("vmstate.src"))?;
        copy_file(bundle.join("mem"), self.jail_root.join("mem.src"))?;
        if !req.rw_layer_path.is_empty() {
            copy_file(
                Path::new(&req.rw_layer_path),
                self.jail_root.join("data.ext4"),
            )?;
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
        if !req.rw_layer_path.is_empty() {
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
            .arg(&req.chroot_base_dir)
            .arg("--new-pid-ns")
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
        let output = j
            .output()
            .map_err(|e| format!("spawn jailer: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "jailer exited {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        log_step(&self.sandbox_id, "jailer_spawn", t);

        // 等 API socket。5ms 轮询:既测准 firecracker 冷启动到 bind 的真实耗时,
        // 又避免 100ms 粒度最多白等 ~95ms。上限 100×100ms=10s 不变。
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

        // logger + snapshot/load + 写 meta。任一失败都要终止已拉起的 firecracker，
        // 否则进程会泄漏（fc_pid 此刻还没写入 state，create 的错误分支不会清理）。
        let result = (|| -> Result<(), String> {
            let fc = FirecrackerClient::new(&sock);

            let t = Instant::now();
            fc.put_logger(&self.cfg.log_dir.join(format!("fc-{}.log", self.sandbox_id)));
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

            let mut st = self.state.lock().unwrap();
            st.fc_pid = Some(fc_pid);
            st.lifecycle = Lifecycle::Running;
            st.created = true;
            Ok(())
        })();
        log_step(&self.sandbox_id, "fresh_restore_total", t0);
        if result.is_err() {
            terminate_pid(fc_pid);
        }
        result
    }

    /// re-attach 已存活的 firecracker（runtime re-spawn shim 后调用）。
    fn re_attach(&self) -> Result<(), String> {
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

        let mut st = self.state.lock().unwrap();
        st.fc_pid = Some(fc_pid);
        st.lifecycle = Lifecycle::Running;
        st.created = true;
        log_step(&self.sandbox_id, "re_attach_total", t0);
        Ok(())
    }

    fn cleanup(&self) {
        if let Some(pid) = self.state.lock().unwrap().fc_pid {
            terminate_pid(pid);
        }
        let _ = std::fs::remove_dir_all(&self.jail_root);
        let _ = std::fs::remove_file(&self.meta_path);
    }
}

impl Shim for ShimImpl {
    fn create(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: CreateRequest,
    ) -> ttrpc::Result<CreateResponse> {
        let t_create = Instant::now();
        // 幂等快速路径。
        {
            let st = self.state.lock().unwrap();
            if st.created && st.fc_pid.is_some_and(pid_alive) {
                tracing::info!(target: "oas-shim", sid = %self.sandbox_id, path = "idempotent", total_ms = t_create.elapsed().as_millis() as u64, "create done");
                return Ok(CreateResponse {
                    state: st.lifecycle.as_str().into(),
                    error: String::new(),
                    ..Default::default()
                });
            }
        }
        // re-attach 判定：jail root 下已有活 firecracker。
        let result = if find_fc_pid(&self.jail_root).is_some() {
            tracing::info!(target: "oas-shim", sid = %self.sandbox_id, "create: re-attach path");
            self.re_attach()
        } else {
            tracing::info!(target: "oas-shim", sid = %self.sandbox_id, "create: fresh-restore path");
            self.fresh_restore(&req)
        };
        tracing::info!(
            target: "oas-shim",
            sid = %self.sandbox_id,
            total_ms = t_create.elapsed().as_millis() as u64,
            "create done"
        );
        if let Some(fc_pid) = self.state.lock().unwrap().fc_pid {
            self.start_watch(fc_pid);
        }
        match result {
            Ok(()) => Ok(CreateResponse {
                state: "Running".into(),
                error: String::new(),
                ..Default::default()
            }),
            Err(e) => {
                self.state.lock().unwrap().lifecycle = Lifecycle::Failed;
                tracing::error!(target: "oas-shim", sid = %self.sandbox_id, "create failed: {e}");
                Ok(CreateResponse {
                    state: "Failed".into(),
                    error: e,
                    ..Default::default()
                })
            }
        }
    }

    fn state(&self, _ctx: &ttrpc::TtrpcContext, _req: StateRequest) -> ttrpc::Result<StateResponse> {
        let st = self.state.lock().unwrap();
        let mut life = st.lifecycle;
        // 若 fc 曾在但已死，反映 Stopped。
        if matches!(life, Lifecycle::Running) && st.fc_pid.is_some_and(|p| !pid_alive(p)) {
            life = Lifecycle::Stopped;
        }
        Ok(StateResponse {
            state: life.as_str().into(),
            ..Default::default()
        })
    }

    fn stop(&self, _ctx: &ttrpc::TtrpcContext, _req: StopRequest) -> ttrpc::Result<StopResponse> {
        self.cleanup();
        self.state.lock().unwrap().lifecycle = Lifecycle::Stopped;
        // 触发 run() 退出。
        let (m, c) = &*self.exit;
        *m.lock().unwrap() = true;
        c.notify_all();
        Ok(StopResponse::default())
    }
}

// ---- 辅助 ------------------------------------------------------------------

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

/// 记录单个恢复子步骤的耗时（ms）。供 `fresh_restore`/`re_attach` 分段打点，
/// 落入 shim 日志（target=oas-shim），便于定位 restore 链路瓶颈。
fn log_step(sid: &str, step: &str, start: Instant) {
    tracing::info!(
        target: "oas-shim",
        sid = %sid,
        step = %step,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "restore step"
    );
}

/// reflink 优先的文件拷贝。
///
/// 同 CoW 文件系统(xfs reflink=1 / btrfs)上 `FICLONE` 仅复制元数据,瞬时完成,
/// 不搬 3GB 数据 —— 这是设计要求的“写时复制 materialize”(见 概要设计 §存储、
/// 实现计划-oas-driver-shim)。跨文件系统或不支持时(EXDEV/EOPNOTSUPP/ENOTTY)
/// 自动降级为 `std::fs::copy` 全量拷贝,并记 debug 日志便于诊断为何没走 COW。
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

/// `FICLONE` = `_IOW(0x94, 9, int)` = 0x40049409,见 Linux `<fs.h>`。
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
