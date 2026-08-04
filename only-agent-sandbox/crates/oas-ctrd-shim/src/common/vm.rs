//! 下层 VM 抽象 + Mock 实现 (假下层, 不起 firecracker).
//!
//! 复刻 "oas-mock" 的哲学: 先用假下层把 `containerd→shim 上层` 协议/状态机跑通,
//! 真 firecracker restore (复用 "oas-driver::vm_core") 后替换本 trait 的真实实现.
//!
//! shim 是「单 VM 守护进程」: 一个进程管一个 sandbox, 故 VM 状态是进程内单例.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oas_config::Config;
use oas_driver::vm_core::{RestoreInputs, VmCore, VmCoreState};

/// VM 生命周期状态 (映射到 containerd sandbox 期望的 state 字符串).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    /// 尚未 create.
    NotExist,
    /// create 完成, 已 resume (snapshot restore), 等价 "running".
    Running,
    /// 已 stop / shutdown.
    Stopped,
}

impl VmState {
    /// containerd sandbox 期望的 state 字符串 (见 SandboxStatusResponse.state).
    pub fn as_ctrld_str(self) -> &'static str {
        match self {
            VmState::NotExist => "notexist",
            VmState::Running => "running",
            VmState::Stopped => "stopped",
        }
    }
}

/// create 时从 CreateSandboxRequest 提炼的入参 (P1 mock 只用到 id/type).
#[derive(Debug, Clone, Default)]
pub struct VmCreateArgs {
    pub sandbox_id: String,
    /// containerd 传入的 netns (真下层会进入它补 tap0; mock 仅记录).
    pub netns_path: String,
    /// 从 annotations["agent-sandbox/type"] 解析的 bundle type (mock 仅记录).
    pub type_id: u8,
}

/// start 后回给 containerd 的运行信息.
#[derive(Debug, Clone, Default)]
pub struct VmRunInfo {
    /// host 侧 firecracker pid (mock 返回一个占位非零值).
    pub pid: u32,
}

/// 下层 VM 抽象: Sandbox service 8 方法挂在它上面.
///
/// 真实实现 ("RealVm") 后续复用 "oas-driver" 的 fresh_restore/re_attach/cleanup/watch;
/// "MockVm" 不起任何进程, 仅推进内存状态机 + 记录调用, 供 A 路进程内 e2e.
#[async_trait]
pub trait SandboxVm: Send + Sync {
    /// materialize + jailer + snapshot/load (mock: 仅置 Running).
    async fn create(&self, args: VmCreateArgs) -> Result<(), String>;
    /// 确认已 resume (mock: 返回占位 pid).
    async fn start(&self) -> Result<VmRunInfo, String>;
    /// 当前状态 + pid.
    async fn status(&self) -> (VmState, u32);
    /// 停 firecracker + 清 jail root (mock: 置 Stopped + 通知 waiter).
    async fn stop(&self, _timeout_secs: u32) -> Result<(), String>;
    /// 阻塞至 VM 退出, 返回 exit_status (mock: 等 stop/shutdown 触发).
    async fn wait(&self) -> u32;
    /// 触发 shim 进程退出的收尾 (mock: 同 stop).
    async fn shutdown(&self) -> Result<(), String>;
}

// ---- MockVm ----------------------------------------------------------------

struct MockInner {
    state: VmState,
    pid: u32,
    args: Option<VmCreateArgs>,
    /// create/start/stop/shutdown/wait 调用计数, 供 e2e 断言.
    calls: Vec<&'static str>,
}

/// 假下层: 不起 firecracker, 仅推进内存状态机.
pub struct MockVm {
    inner: Mutex<MockInner>,
    /// VM 退出通知 (stop/shutdown 触发 exit_status).
    exit: tokio::sync::watch::Sender<Option<u32>>,
    exit_rx: tokio::sync::watch::Receiver<Option<u32>>,
}

impl Default for MockVm {
    fn default() -> Self {
        Self::new()
    }
}

impl MockVm {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(None);
        Self {
            inner: Mutex::new(MockInner {
                state: VmState::NotExist,
                pid: 0,
                args: None,
                calls: Vec::new(),
            }),
            exit: tx,
            exit_rx: rx,
        }
    }

    /// 测试辅助: 返回调用序列快照.
    pub fn calls(&self) -> Vec<&'static str> {
        self.inner.lock().unwrap().calls.clone()
    }
}

#[async_trait]
impl SandboxVm for MockVm {
    async fn create(&self, args: VmCreateArgs) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        g.calls.push("create");
        // 幂等: 已 Running 直接返回.
        if g.state == VmState::Running {
            return Ok(());
        }
        g.args = Some(args);
        g.state = VmState::Running;
        g.pid = 424242; // 占位非零 pid (真下层是 fc host pid).
        Ok(())
    }

    async fn start(&self) -> Result<VmRunInfo, String> {
        let mut g = self.inner.lock().unwrap();
        g.calls.push("start");
        if g.state != VmState::Running {
            return Err(format!("start: vm not running (state={:?})", g.state));
        }
        Ok(VmRunInfo { pid: g.pid })
    }

    async fn status(&self) -> (VmState, u32) {
        let g = self.inner.lock().unwrap();
        (g.state, g.pid)
    }

    async fn stop(&self, _timeout_secs: u32) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        g.calls.push("stop");
        if g.state == VmState::Running {
            g.state = VmState::Stopped;
            let _ = self.exit.send(Some(0));
        }
        Ok(())
    }

    async fn wait(&self) -> u32 {
        // 已退出则立即返回; 否则等 stop/shutdown.
        let mut rx = self.exit_rx.clone();
        loop {
            if let Some(code) = *rx.borrow() {
                return code;
            }
            if rx.changed().await.is_err() {
                return 0;
            }
        }
    }

    async fn shutdown(&self) -> Result<(), String> {
        {
            let mut g = self.inner.lock().unwrap();
            g.calls.push("shutdown");
            if g.state == VmState::Running {
                g.state = VmState::Stopped;
            }
        }
        let _ = self.exit.send(Some(0));
        Ok(())
    }
}

// ---- RealVm ----------------------------------------------------------------

/// 真下层: 经 `oas_driver::vm_core` 在进程内拉起真实 firecracker 做 snapshot 恢复。
///
/// 与 `MockVm` 实现**同一 `SandboxVm` 契约**, 故上层 `TaskService`/`server` 无需改动——
/// `run_server` 注入 `RealVm` 即从「假下层」切到「真 firecracker」。详见 ADR 0009。
///
/// `VmCore` 是同步库(做阻塞 FS/进程/HTTP-UDS 工作), 本 impl 经 `spawn_blocking` 桥接到
/// 异步 `SandboxVm`, 不占 tokio worker 线程。
pub struct RealVm {
    cfg: Arc<Config>,
    /// 懒构造: 首次 `create` 拿到 sandbox_id 才建 `VmCore`（一个 shim = 一个 sandbox）。
    core: Mutex<Option<Arc<VmCore>>>,
    /// VM 退出通知 (stop/shutdown 触发, 喂给 `wait`)。wait=(b): 不观察 fc 自然退出。
    exit: tokio::sync::watch::Sender<Option<u32>>,
    exit_rx: tokio::sync::watch::Receiver<Option<u32>>,
}

impl RealVm {
    pub fn new(cfg: Arc<Config>) -> Self {
        let (tx, rx) = tokio::sync::watch::channel(None);
        Self {
            cfg,
            core: Mutex::new(None),
            exit: tx,
            exit_rx: rx,
        }
    }

    /// 确保 `VmCore` 已建（用 `args.sandbox_id` 派生 jail_root/meta 等路径）, 返回其 `Arc` 句柄。
    fn ensure_core(&self, sandbox_id: &str) -> Arc<VmCore> {
        let mut g = self.core.lock().unwrap();
        if g.is_none() {
            *g = Some(Arc::new(VmCore::new(self.cfg.clone(), sandbox_id)));
        }
        g.as_ref().unwrap().clone()
    }

    fn core_clone(&self) -> Option<Arc<VmCore>> {
        self.core.lock().unwrap().clone()
    }
}

/// 从 `(Config, VmCreateArgs)` 构造恢复输入。`netns_path` 缺省时由 `cfg.netns_path(sid)` 派生。
fn build_restore_inputs(cfg: &Config, args: &VmCreateArgs) -> Result<RestoreInputs, String> {
    let ty = cfg
        .get_type(args.type_id)
        .ok_or_else(|| format!("unknown type_id {}", args.type_id))?;
    if ty.has_cloud_disk {
        return Err(format!(
            "type {} cloud-disk restore not supported in MVP",
            args.type_id
        ));
    }
    let bundle_dir = cfg.bundle_dir(&ty.bundle);
    // netns 优先用 containerd 在 bundle config.json 里提供的 sandbox netns（CRI 路径）；
    // 仅当未给（裸 ctr run / e2e）时回落 cfg.netns_path(sid)。shim 不自建 netns。
    let netns_path = if args.netns_path.is_empty() {
        cfg.netns_path(&args.sandbox_id)
            .to_string_lossy()
            .into_owned()
    } else {
        args.netns_path.clone()
    };
    let rw_layer_path = if ty.has_rw_layer {
        Some(cfg.rw_base_dir.join(format!("{}.ext4", args.sandbox_id)))
    } else {
        None
    };
    Ok(RestoreInputs {
        bundle_dir,
        netns_path,
        rw_layer_path,
        jailer_uid: cfg.jailer_uid,
        jailer_gid: cfg.jailer_gid,
        chroot_base_dir: cfg.chroot_base_dir.clone(),
        firecracker_bin: cfg.firecracker_bin.clone(),
        jailer_bin: cfg.jailer_bin.clone(),
        cloud_disk_dev: None,
    })
}

/// `VmCoreState` → `VmState`（Failed 归并到 Stopped）。
fn map_state(s: VmCoreState) -> VmState {
    match s {
        VmCoreState::Running => VmState::Running,
        VmCoreState::NotExist => VmState::NotExist,
        VmCoreState::Stopped | VmCoreState::Failed => VmState::Stopped,
    }
}

#[async_trait]
impl SandboxVm for RealVm {
    async fn create(&self, args: VmCreateArgs) -> Result<(), String> {
        let inputs = build_restore_inputs(&self.cfg, &args)?;
        let core = self.ensure_core(&args.sandbox_id);
        let core2 = core.clone();
        // 同步 VmCore::create（materialize+jailer+snapshot/load, 可能数秒）放 blocking 池。
        tokio::task::spawn_blocking(move || core2.create(&inputs))
            .await
            .map_err(|e| format!("blocking join: {e}"))?
            .map(|_handle| ())
    }

    async fn start(&self) -> Result<VmRunInfo, String> {
        let core = self.core_clone().ok_or("start: vm not created")?;
        let (state, pid) = tokio::task::spawn_blocking(move || core.liveness())
            .await
            .map_err(|e| format!("blocking join: {e}"))?;
        if state != VmCoreState::Running {
            return Err(format!("start: vm not running (state={:?})", state));
        }
        Ok(VmRunInfo { pid })
    }

    async fn status(&self) -> (VmState, u32) {
        match self.core_clone() {
            Some(core) => {
                let (state, pid) = tokio::task::spawn_blocking(move || core.liveness())
                    .await
                    .unwrap_or((VmCoreState::NotExist, 0));
                (map_state(state), pid)
            }
            None => (VmState::NotExist, 0),
        }
    }

    async fn stop(&self, _timeout_secs: u32) -> Result<(), String> {
        if let Some(core) = self.core_clone() {
            tokio::task::spawn_blocking(move || core.cleanup())
                .await
                .map_err(|e| format!("blocking join: {e}"))?;
        }
        // wait=(b): 唤醒所有 waiter, wait 返回 0。
        let _ = self.exit.send(Some(0));
        Ok(())
    }

    async fn wait(&self) -> u32 {
        // 不观察 firecracker 自然退出(ADR: wait=(b)); status 是探测 fc 存活的安全阀。
        let mut rx = self.exit_rx.clone();
        loop {
            if let Some(code) = *rx.borrow() {
                return code;
            }
            if rx.changed().await.is_err() {
                return 0;
            }
        }
    }

    async fn shutdown(&self) -> Result<(), String> {
        // 委托给 stop: shutdown 当前无独立调用方（TaskService::shutdown 不调本方法）。
        self.stop(0).await
    }
}