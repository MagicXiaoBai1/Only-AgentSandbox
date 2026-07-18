//! 编排层 → Firecracker 原语层 契约（§2.1 / §3.3 / §2.8）+ 真 impl。
//!
//! 本 crate 同时承载：
//! - **契约**（`FirecrackerDriver` trait + 类型）——runtime 侧调用、mock 实现。
//! - **runtime 侧真 impl**（`driver::RealDriver`）：ttrpc client + spawn shim + 再发现 + 应急杀。
//! - **shim 侧逻辑**（`shim`）：ttrpc server + jailer + materialize + snapshot/load + re-attach。
//! - **ttrpc 生成代码**（`gen`）：`Shim` service（Create/State/Stop）。
//!
//! 回收边界：VM/firecracker 进程由 shim 侧回收（经 jailer daemonize，shim 崩 ≠ fc 崩）；
//! netns/ext4/IP 租约由编排层调 net/storage 回收，职责不重叠。
//!
//! 容器原语 MVP no-op（§2.8），真实 vsock/GuestAgent 待落地。

use oas_types::SandboxId;

pub mod driver;
pub mod firecracker;
pub mod generated;
pub mod identity;
pub mod shim;

pub use driver::RealDriver;

// ---------------------------------------------------------------------------
// 网络规格（create_vm 入参，向前兼容 CNI）
// ---------------------------------------------------------------------------

/// 创建 VM 时网络上下文。MVP `tap_name` 恒为固定常量；CNI 落地后可为 per-sandbox 名。
#[derive(Debug, Clone, Default)]
pub struct VmNet {
    /// jailer `--netns` 指向的 netns 路径。
    pub netns_path: String,
    /// netns 内固定 tap 名（MVP `tap0`）。
    pub tap_name: String,
}

// ---------------------------------------------------------------------------
// VM 生命周期 / 状态
// ---------------------------------------------------------------------------

/// VM 生命周期状态（§3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmLifecycle {
    Creating,
    Restoring,
    Running,
    Degraded,
    Stopping,
    Stopped,
}

/// VM 实际态（由 driver `get_vm` / `list_vm` 返回，供 reconcile 对账声明态 vs 实际态）。
#[derive(Debug, Clone)]
pub struct VmStatus {
    pub id: SandboxId,
    pub started: bool,
    pub healthy: bool,
    pub lifecycle: VmLifecycle,
}

/// 每 VM 的盘拓扑覆盖：把可写层 / 云盘 materialize 进 jail root 的固定路径（§3.3 / §8.2）。
#[derive(Debug, Clone, Default)]
pub struct VmSpec {
    /// per-VM 可写 ext4 路径（宿主），materialize 到 jail 内 `/data.ext4`。
    pub rw_layer_path: Option<String>,
    /// per-VM 云盘块设备。MVP 不支持（type 2 恢复返回错误）。
    pub cloud_disk_dev: Option<String>,
}

/// Firecracker 原语层错误。
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("vm not found: {0}")]
    NotFound(String),
    #[error("vm already exists: {0}")]
    Exists(String),
    #[error("snapshot error: {0}")]
    Snapshot(String),
    #[error("firecracker api error: {0}")]
    FirecrackerApi(String),
    #[error("shim error: {0}")]
    Shim(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------------------
// 容器原语类型（§2.8）——driver 本地类型。
// ---------------------------------------------------------------------------

/// 待在 VM 内启动的容器进程规格。
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub cwd: String,
}

/// driver 视角的容器运行态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntimeState {
    Created,
    Running,
    Exited,
}

/// `get_container_status` 返回的实际态。MVP 无 vsock 时为 `None`。
#[derive(Debug, Clone, Default)]
pub struct ContainerRuntimeStatus {
    pub state: Option<ContainerRuntimeState>,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub exit_code: Option<i32>,
}

/// 编排层 → Firecracker 原语层接口（§2.1）。
///
/// `id`（`SandboxId`）由编排层生成并传入，是 VM 的唯一句柄——driver 不再造 id。
/// `create_vm` 原子幂等：内部 spawn shim 并同步 `Create`（re-attach 或全量 restore），
/// 返回时 VM 已 Running（或 `Err`）。`delete_vm` 幂等：shim 不可达时走应急杀。
#[async_trait::async_trait]
pub trait FirecrackerDriver: Send + Sync {
    /// 原子幂等建 VM。`net` 给 jailer `--netns` + tap 名；`type_id` 决定 bundle；
    /// `spec` 把 per-VM 可写层 materialize 进 jail root。
    async fn create_vm(
        &self,
        id: &SandboxId,
        net: &VmNet,
        type_id: u8,
        spec: VmSpec,
    ) -> Result<(), DriverError>;

    async fn get_vm(&self, id: &SandboxId) -> Result<VmStatus, DriverError>;

    async fn delete_vm(&self, id: &SandboxId) -> Result<(), DriverError>;

    async fn list_vm(&self) -> Result<Vec<VmStatus>, DriverError>;

    // --- 容器原语（§2.8，MVP no-op）---

    async fn create_container(
        &self,
        vm_id: &SandboxId,
        container_id: &str,
        spec: ContainerSpec,
    ) -> Result<(), DriverError>;

    async fn start_container(&self, vm_id: &SandboxId, container_id: &str)
        -> Result<(), DriverError>;

    async fn stop_container(
        &self,
        vm_id: &SandboxId,
        container_id: &str,
        timeout_sec: i64,
    ) -> Result<(), DriverError>;

    async fn remove_container(&self, vm_id: &SandboxId, container_id: &str)
        -> Result<(), DriverError>;

    async fn get_container_status(
        &self,
        vm_id: &SandboxId,
        container_id: &str,
    ) -> Result<ContainerRuntimeStatus, DriverError>;
}
