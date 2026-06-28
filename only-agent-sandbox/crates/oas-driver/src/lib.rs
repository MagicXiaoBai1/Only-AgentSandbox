//! 编排层 → Firecracker 原语层 契约（§2.1 / §3.3）。
//!
//! 4 原语：`create_vm` / `get_vm` / `delete_vm` / `list_vm`。
//! 回收边界：VM 进程由本层回收；netns / ext4 / IP 租约由编排层调网络、存储层回收，
//! 职责不重叠。

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
}
