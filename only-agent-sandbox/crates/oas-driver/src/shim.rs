//! Shim 侧逻辑：ttrpc server + `Create`/`State`/`Stop` —— Path A 的 per-sandbox 守护进程。
//!
//! 进程模型：shim 是单 VM 守护进程。ttrpc sync server 起 handler 线程池，`Create` 在其中
//! 同步完成恢复（re-attach 或全量 restore）。`run()` 起 server 后阻塞在 exit condvar 上，
//! `Stop` 清理后触发退出。
//!
//! 恢复原语（materialize / jailer / snapshot/load / re-attach / cleanup / 身份校验）已抽到
//! [`crate::vm_core::VmCore`]（见 ADR 0009）：本文件只负责 ttrpc 契约 + 进程退出编排，
//! 把 `CreateRequest` 翻译成 `RestoreInputs` 交给 `VmCore`。Path B 的 `RealVm` 共享同一核心。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use oas_config::Config;
use ttrpc::Server;

use crate::generated::shim::{
    CreateRequest, CreateResponse, StateRequest, StateResponse, StopRequest, StopResponse,
};
use crate::generated::shim_ttrpc::{create_shim, Shim};
use crate::identity::pid_alive;
use crate::vm_core::{RestoreInputs, VmCore, VmCoreState};

/// shim 启动参数（由 main.rs 解析 clap 后构造）。
pub struct ShimArgs {
    pub config_path: PathBuf,
    pub sandbox_id: String,
    pub socket: PathBuf,
    #[allow(dead_code)]
    pub log_file: PathBuf,
}

struct ShimImpl {
    core: VmCore,
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
        core: VmCore::new(cfg, args.sandbox_id.clone()),
        exit: exit.clone(),
        watch_started: AtomicBool::new(false),
    });

    let methods = create_shim(impl_.clone() as Arc<dyn Shim + Send + Sync>);
    let sockaddr = format!("unix://{}", args.socket.display());
    let mut server = Server::new()
        .bind(&sockaddr)?
        .register_service(methods);
    server.start()?;

    tracing::info!(target: "oas-shim", sid = %args.sandbox_id, jail = %jail_root.display(), "shim serving, waiting for Create");

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
    fn start_watch(&self, fc_pid: u32) {
        if self.watch_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let sid = self.core.sandbox_id().to_string();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if !pid_alive(fc_pid) {
                    tracing::warn!(target: "oas-shim", sid = %sid, fc_pid, "firecracker died");
                    // 标记由 Stop / State 时按需查询；此处仅记日志，避免跨 Mutex 复杂度。
                    return;
                }
            }
        });
    }

    /// 把 ttrpc `CreateRequest` 翻译成进程无关的 `RestoreInputs`。
    fn inputs_from(req: &CreateRequest) -> RestoreInputs {
        let rw_layer_path = if req.rw_layer_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(&req.rw_layer_path))
        };
        let cloud_disk_dev = if req.cloud_disk_dev.is_empty() {
            None
        } else {
            Some(req.cloud_disk_dev.clone())
        };
        RestoreInputs {
            bundle_dir: PathBuf::from(&req.bundle_dir),
            netns_path: req.netns_path.clone(),
            rw_layer_path,
            jailer_uid: req.jailer_uid,
            jailer_gid: req.jailer_gid,
            chroot_base_dir: PathBuf::from(&req.chroot_base_dir),
            firecracker_bin: PathBuf::from(&req.firecracker_bin),
            jailer_bin: PathBuf::from(&req.jailer_bin),
            cloud_disk_dev,
        }
    }
}

impl Shim for ShimImpl {
    fn create(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: CreateRequest,
    ) -> ttrpc::Result<CreateResponse> {
        let t_create = Instant::now();
        let inputs = Self::inputs_from(&req);
        let result = self.core.create(&inputs);
        tracing::info!(
            target: "oas-shim",
            sid = %self.core.sandbox_id(),
            total_ms = t_create.elapsed().as_millis() as u64,
            "create done"
        );
        match result {
            Ok(handle) => {
                self.start_watch(handle.fc_pid);
                Ok(CreateResponse {
                    state: "Running".into(),
                    error: String::new(),
                    ..Default::default()
                })
            }
            Err(e) => {
                tracing::error!(target: "oas-shim", sid = %self.core.sandbox_id(), "create failed: {e}");
                Ok(CreateResponse {
                    state: "Failed".into(),
                    error: e,
                    ..Default::default()
                })
            }
        }
    }

    fn state(&self, _ctx: &ttrpc::TtrpcContext, _req: StateRequest) -> ttrpc::Result<StateResponse> {
        let (state, _pid) = self.core.liveness();
        Ok(StateResponse {
            state: state.as_resp_str().into(),
            ..Default::default()
        })
    }

    fn stop(&self, _ctx: &ttrpc::TtrpcContext, _req: StopRequest) -> ttrpc::Result<StopResponse> {
        self.core.cleanup();
        // 触发 run() 退出。
        let (m, c) = &*self.exit;
        *m.lock().unwrap() = true;
        c.notify_all();
        Ok(StopResponse::default())
    }
}

impl VmCoreState {
    /// 映射到 ttrpc `StateResponse.state` 字符串（对齐原 Lifecycle::as_str）。
    fn as_resp_str(self) -> &'static str {
        match self {
            VmCoreState::NotExist => "Pending",
            VmCoreState::Running => "Running",
            VmCoreState::Stopped => "Stopped",
            VmCoreState::Failed => "Failed",
        }
    }
}
