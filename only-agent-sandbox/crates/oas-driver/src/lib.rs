//! 编排层 → Firecracker 原语层 契约（§2.1 / §3.3 / §2.8）。
//!
//! VM 原语：`create_vm` / `get_vm` / `delete_vm` / `list_vm`。
//! 容器原语：`create_container` / `start_container` / `stop_container` /
//! `remove_container` / `get_container_status`。
//!
//! 回收边界：VM 进程由本层回收；netns / ext4 / IP 租约由编排层调网络、存储层回收，
//! 职责不重叠。
//!
//! 容器原语是 VM 内外带外通信（vsock GuestAgent）的对外屏蔽点（§2.8）：不同 VMM
//! 可能用不同通信方式，编排层只调本 trait，不感知 vsock / 串口 / 通道细节。
//! **MVP 阶段容器原语为 no-op**（声明态事实源仍在 `oas-store`），真实 vsock 通道
//! 与 GuestAgent 待 §2.8 落地——届时改动收敛在本层 impl 内，编排层与 CRI 零改动。

use std::os::fd::RawFd;

/// VM 标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VmId(pub u64);

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

/// VM 实际态（由 driver `get_vm` 返回，供 reconcile 对账声明态 vs 实际态）。
#[derive(Debug, Clone)]
pub struct VmStatus {
    pub started: bool,
    pub healthy: bool,
    pub lifecycle: VmLifecycle,
}

/// 每 VM 的盘拓扑覆盖：把可写层 / 云盘 PATCH 进 snapshot 预留的 drive slot（§3.3 / §8.2）。
#[derive(Debug, Clone, Default)]
pub struct VmSpec {
    /// per-VM 可写 ext4 路径。
    pub rw_layer_path: Option<String>,
    /// per-VM 云盘块设备。
    pub cloud_disk_dev: Option<String>,
}

/// Firecracker 原语层错误。
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("vm not found: {0}")]
    NotFound(u64),
    #[error("vm already exists: {0}")]
    Exists(u64),
    #[error("snapshot error: {0}")]
    Snapshot(String),
    #[error("firecracker api error: {0}")]
    FirecrackerApi(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------------------
// 容器原语类型（§2.8）——driver 本地类型，不依赖 oas-types，保持依赖方向不反转。
// ---------------------------------------------------------------------------

/// 待在 VM 内启动的容器进程规格（编排层 `CreateContainerRequest` 映射而来）。
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    pub command: Vec<String>,
    pub args: Vec<String>,
    /// `KEY=VAL` 形式。
    pub env: Vec<String>,
    pub cwd: String,
}

/// driver 视角的容器运行态（用于 reconcile 对账声明态 vs 实际态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntimeState {
    Created,
    Running,
    Exited,
}

/// `get_container_status` 返回的实际态。pid / exit_code 在 MVP 无 vsock 时为 `None`。
#[derive(Debug, Clone, Default)]
pub struct ContainerRuntimeStatus {
    pub state: Option<ContainerRuntimeState>,
    pub pid: Option<u32>,
    /// Unix 时间戳（秒）。
    pub started_at: Option<i64>,
    pub exit_code: Option<i32>,
}

/// 编排层 → Firecracker 原语层接口（§2.1）。
///
/// `create_vm` 原子幂等：同 `(type_id, netns)` 已在建则返回旧 `VmId`；
/// `delete_vm` 幂等：删不存在返回 `Ok`，不保证立即停止，调用方轮询 `get_vm` 到 `Stopped`。
#[async_trait::async_trait]
pub trait FirecrackerDriver: Send + Sync {
    /// 原子幂等建 VM。`type_id` 决定 vCPU/mem/rootfs/drive 拓扑；
    /// `spec` 把 per-VM 可写层 / 云盘 PATCH 进 snapshot 预留的 drive slot；
    /// 就绪时写 `event_fd` 通知。
    async fn create_vm(
        &self,
        netns_path: &str,
        type_id: u8,
        spec: VmSpec,
        event_fd: RawFd,
    ) -> Result<VmId, DriverError>;

    async fn get_vm(&self, id: VmId) -> Result<VmStatus, DriverError>;

    async fn delete_vm(&self, id: VmId) -> Result<(), DriverError>;

    async fn list_vm(&self) -> Result<Vec<VmStatus>, DriverError>;

    // --- 容器原语（§2.8，VM 内外带外通信屏蔽点）---
    // MVP 阶段为 no-op：真实 vsock 通道待落地，编排层声明态事实源仍在 store。

    /// 在 `vm_id` 对应的 VM 内登记一个容器（MVP no-op）。
    async fn create_container(
        &self,
        vm_id: VmId,
        container_id: &str,
        spec: ContainerSpec,
    ) -> Result<(), DriverError>;

    /// 在 VM 内启动已登记容器进程（MVP no-op）。
    async fn start_container(&self, vm_id: VmId, container_id: &str) -> Result<(), DriverError>;

    /// 在 VM 内停止容器进程（MVP no-op）。
    async fn stop_container(
        &self,
        vm_id: VmId,
        container_id: &str,
        timeout_sec: i64,
    ) -> Result<(), DriverError>;

    /// 在 VM 内移除容器登记（MVP no-op）。
    async fn remove_container(&self, vm_id: VmId, container_id: &str) -> Result<(), DriverError>;

    /// 查 VM 内容器实际态，供编排层 reconcile 对账（MVP 返回缺省态）。
    async fn get_container_status(
        &self,
        vm_id: VmId,
        container_id: &str,
    ) -> Result<ContainerRuntimeStatus, DriverError>;
}
