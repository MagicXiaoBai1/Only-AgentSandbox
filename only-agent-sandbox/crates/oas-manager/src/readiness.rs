//! VM 就绪等待抽象（§4.2 step 6-7）。
//!
//! 设计上 Manager 建 VM 后在 `event_fd` 上等就绪再落 READY。本迭代把「等就绪」抽象成
//! `VmReadiness`，真实 `eventfd` + `AsyncFd` 实现随 driver 专题落地；测试用 `Immediate` /
//! `NeverReady` 两种假实现覆盖成功与超时回滚两条路径（不挂起）。

use std::time::Duration;

use oas_driver::VmId;

use crate::OasError;

/// VM 就绪等待。
#[async_trait::async_trait]
pub trait VmReadiness: Send + Sync {
    /// 等待 `vm_id` 就绪，超时返回 `Err`（→ 回滚 `delete_vm` + cleanup + teardown）。
    async fn wait_ready(&self, vm_id: VmId, timeout: Duration) -> Result<(), OasError>;
}
