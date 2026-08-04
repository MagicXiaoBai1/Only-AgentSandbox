//! containerd 2.x Sandbox service：把 Sandbox API 8 方法映射到下层 [`SandboxVm`]。
//!
//! 与 `task` 路径对称：Sandbox 路径一个 shim 管一个 sandbox，VM 即进程内单例，
//! 故 `SandboxService` 直接持有同一个 `vm`（与 `server::build_and_start` 注入的一致）。

pub mod service;
