//! Sandbox service (containerd 2.x) —— 占位实现。
//!
//! 仅满足 `server::build_and_start` 的注册签名 (需要 `Arc<dyn Sandbox>`)，
//! 全部方法走 trait 默认 (NOT_FOUND)。真下层接入后再实现，详见 ADR/清单。

use std::sync::Arc;

use async_trait::async_trait;
use containerd_shim::asynchronous::ExitSignal;
use containerd_shim_protos::sandbox_async::Sandbox;

/// 占位 Sandbox service：空实现，所有方法返回 NOT_FOUND (trait 默认)。
pub struct SandboxService {
    #[allow(dead_code)]
    vm: Arc<dyn crate::common::vm::SandboxVm>,
    #[allow(dead_code)]
    exit: Arc<ExitSignal>,
}

impl SandboxService {
    pub fn new(vm: Arc<dyn crate::common::vm::SandboxVm>, exit: Arc<ExitSignal>) -> Self {
        Self { vm, exit }
    }
}

#[async_trait]
impl Sandbox for SandboxService {}
