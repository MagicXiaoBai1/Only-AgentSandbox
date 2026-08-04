//! ttrpc server 装配: 手动 register Sandbox service (containerd-shim 0.11 的 run())
//! 只自动挂 Task, 不挂 Sandbox——见清单坑#1, 故照 kata 手动 register。
//!
//! 抽成独立函数供两处复用:
//! - `main.rs` 的 `run` 子命令 (真 containerd 驱动)
//! - `tests/` 进程内 e2e (扮 containerd 的 ttrpc client 打进来)

use std::sync::Arc;

use containerd_shim::asynchronous::ExitSignal;
use containerd_shim_protos::sandbox_async;
use containerd_shim_protos::shim_async;

use crate::common::vm::SandboxVm;
use crate::sandbox::service::SandboxService;
use crate::task::service::TaskService;

/// 在 `socket_addr` (形如 `unix:///run/xxx.sock`) 上起 ttrpc server, 注册 Sandbox + Task service。
///
/// 双协议单二进制: 同时挂 Sandbox (2.x) 与 Task (1.6.33), 被哪版 containerd 驱动就只走对应一条。
/// 两条共用同一个 `vm`——一个 shim 管一个 sandbox, 故 Task 路径的 sandbox-Task 用同一 VM
/// (工厂闭包 `move || vm.clone()`; container-Task 不碰 VM, 走假 exit)。
///
/// 返回 (server, exit): server 已 `start()`; exit 被 ShutdownSandbox (2.x) 或注册表清空 (Task)
/// 触发后, 调用方应 `exit.wait().await` 阻塞至退出, 再 `server.shutdown()`。
pub async fn build_and_start(
    socket_addr: &str,
    vm: Arc<dyn SandboxVm>,
) -> Result<(ttrpc::asynchronous::Server, Arc<ExitSignal>), Box<dyn std::error::Error>> {
    let exit = Arc::new(ExitSignal::default());

    // Sandbox service (2.x): 直接持这个 VM。
    let svc = SandboxService::new(vm.clone(), exit.clone());
    let sandbox_methods =
        sandbox_async::create_sandbox(Arc::new(svc) as Arc<dyn sandbox_async::Sandbox + Send + Sync>);

    // Task service (1.6.33): sandbox-Task 经工厂拿同一个 VM。
    let vm_for_task = vm.clone();
    let vm_factory: Arc<dyn Fn() -> Arc<dyn SandboxVm> + Send + Sync> =
        Arc::new(move || vm_for_task.clone());
    let task_svc = TaskService::new(vm_factory, exit.clone());
    let task_methods =
        shim_async::create_task(Arc::new(task_svc) as Arc<dyn shim_async::Task + Send + Sync>);

    let mut server = ttrpc::asynchronous::Server::new()
        .bind(socket_addr)?
        .register_service(sandbox_methods)
        .register_service(task_methods);
    server.start().await?;
    Ok((server, exit))
}