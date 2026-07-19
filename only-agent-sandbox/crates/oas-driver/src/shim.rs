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
use std::time::Duration;

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
        let bundle = Path::new(&req.bundle_dir);
        // materialize 进 jail root 固定路径。
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
        // chown + chmod（跟随实验：0700 dir / 0444 ro / 0666 rw）。
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

        // spawn jailer（--daemonize 后 jailer 自身很快退出）。
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

        // 等 API socket。
        let sock = self.fc_socket();
        let mut waited = 0;
        while !sock.exists() {
            std::thread::sleep(Duration::from_millis(100));
            waited += 1;
            if waited > 100 {
                return Err("timeout waiting for firecracker socket".into());
            }
        }

        // 找到 firecracker 的 host pid（按 jail root 命中 /proc/pid/root）。
        let fc_pid = find_fc_pid(&self.jail_root)
            .ok_or_else(|| "firecracker pid not found after jailer spawn".to_string())?;

        // logger + snapshot/load + 写 meta。任一失败都要终止已拉起的 firecracker，
        // 否则进程会泄漏（fc_pid 此刻还没写入 state，create 的错误分支不会清理）。
        let result = (|| -> Result<(), String> {
            let fc = FirecrackerClient::new(&sock);
            fc.put_logger(&self.cfg.log_dir.join(format!("fc-{}.log", self.sandbox_id)));
            fc.put_snapshot_load("/vmstate.src", "/mem.src")
                .map_err(|e| e.to_string())?;

            // 写 shim.meta（mntns inode 身份锚点）。
            let ino = mntns_inode(fc_pid).map_err(|e| format!("stat mntns: {e}"))?;
            let meta = ShimMeta {
                fc_pid,
                mntns_inode: ino,
                started_at: 0,
                shim_pid: std::process::id(),
            };
            let _ = meta.write(&self.meta_path);

            let mut st = self.state.lock().unwrap();
            st.fc_pid = Some(fc_pid);
            st.lifecycle = Lifecycle::Running;
            st.created = true;
            Ok(())
        })();
        if result.is_err() {
            terminate_pid(fc_pid);
        }
        result
    }

    /// re-attach 已存活的 firecracker（runtime re-spawn shim 后调用）。
    fn re_attach(&self) -> Result<(), String> {
        let fc_pid = find_fc_pid(&self.jail_root)
            .ok_or_else(|| "no live firecracker to re-attach".to_string())?;
        let ino = mntns_inode(fc_pid).map_err(|e| format!("stat mntns: {e}"))?;
        let meta = ShimMeta {
            fc_pid,
            mntns_inode: ino,
            started_at: 0,
            shim_pid: std::process::id(),
        };
        let _ = meta.write(&self.meta_path);
        let mut st = self.state.lock().unwrap();
        st.fc_pid = Some(fc_pid);
        st.lifecycle = Lifecycle::Running;
        st.created = true;
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
        // 幂等快速路径。
        {
            let st = self.state.lock().unwrap();
            if st.created && st.fc_pid.is_some_and(pid_alive) {
                return Ok(CreateResponse {
                    state: st.lifecycle.as_str().into(),
                    error: String::new(),
                    ..Default::default()
                });
            }
        }
        // re-attach 判定：jail root 下已有活 firecracker。
        let result = if find_fc_pid(&self.jail_root).is_some() {
            self.re_attach()
        } else {
            self.fresh_restore(&req)
        };
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

fn copy_file(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), String> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    std::fs::copy(src, dst).map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    Ok(())
}

fn chmod(path: &Path, mode: u32) {
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn chown_path(path: &Path, uid: u32, gid: u32) {
    let _ = chown(path, Some(Uid::from(uid)), Some(Gid::from(gid)));
}
