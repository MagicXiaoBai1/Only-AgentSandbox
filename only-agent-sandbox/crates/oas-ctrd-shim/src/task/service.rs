//! Task service: 对接 containerd 1.6.33 (无 Sandbox API, pod 走经典 v2 Task service).

//! 一个 shim 进程内可挂两条 task（见 ADR 决策5/6）：
//! - **sandbox-Task** (`container-type=sandbox`)：代表 pod/pause，打到 [`SandboxVm`] (起/管一个 microVM)。
//! - **container-Task** (`container-type=container`)：代表业务容器，P1 = 假 exit (driver no-op,
//!   Wait 立即返回 exit=0)，真进程留待 vsoc guest agent (§2.8)。

//! 两条靠 bundle `config.json` 的 `io.kubernetes.cri.container-type` annotation 区分。
//! 注册进 `HashMap<task_id, TaskEntry>`。退出门控（ADR 决策8）：`shutdown`/`delete` 后只在
//! 注册表清空时才 `exit.signal()`——一个 shim 挂 sandbox+container 两条，须都 Delete 才退。

//! 第一阶段实现 8 方法 (create/start/wait/state/kill/delete/connect/shutdown)，其余 11 个
//! 走 `Task` trait 默认 (NOT_FOUND)。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use containerd_shim::asynchronous::ExitSignal;
use containerd_shim::protos::ttrpc::r#async::TtrpcContext;
use containerd_shim::protos::api;
use containerd_shim::protos::types::task::Status;
use containerd_shim_protos::shim_async::Task;
use tokio::sync::Mutex;

use crate::common::bundle::{load_bundle_spec, ContainerType};
use crate::common::vm::{SandboxVm, VmState};

// 空 Task service：保留给 2.x (Sandbox 路径) 注册用——那条路径不走 Task，全 NOT_FOUND。
pub struct NoopTask;

impl Task for NoopTask {}

// ———— 注册表条目 ———————————————————————————————————————————————
/// 注册表里一条 task：pod (打到 VM) 或业务容器 (假 exit)。
enum TaskEntry {
    /// pod/pause task：生命周期打到真/假 microVM。
    Sandbox { vm: Arc<dyn SandboxVm> },
    /// 业务容器 task：P1 无真进程，start 即「运行」，kill/delete 决定退出码。
    Container(ContainerState),
}

/// 假容器状态：合成 pid + exit 通知 (watch)。
struct ContainerState {
    /// 合成非零 pid (1.6.33 把 task.Pid 当不透明值，见 ADR 附记)。
    pid: u32,
    /// 是否已退出 (start→false; kill/delete→true)。
    exited: bool,
    exit_code: u32,
    /// 退出通知：wait() 挂在上面，kill/delete 触发。
    exit_tx: tokio::sync::watch::Sender<Option<u32>>,
    exit_rx: tokio::sync::watch::Receiver<Option<u32>>,
}

impl ContainerState {
    fn new(pid: u32) -> Self {
        let (tx, rx) = tokio::sync::watch::channel(None);
        Self {
            pid,
            exited: false,
            exit_code: 0,
            exit_tx: tx,
            exit_rx: rx,
        }
    }

    fn mark_exited(&mut self, code: u32) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.exit_code = code;
        let _ = self.exit_tx.send(Some(code));
    }
}

// 合成容器 pid（同 MockVm 的占位哲学：1.6.33 不 /proc 查、不能它 kill）。
const FAKE_CONTAINER_PID: u32 = 424243;

// ———— TaskService ———————————————————————————————————————————————
/// service + task_id 注册表，挂 `vm_factory` 造 sandbox-Task 的 VM (P1 = MockVm)。
pub struct TaskService {
    /// task_id -> 注册表条目 (sandbox-Task 与 container-Task 都进这里)。
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    /// 造 sandbox VM 的工厂（测试注入 MockVm，真实现注入 RealVm）。
    vm_factory: Arc<dyn Fn() -> Arc<dyn SandboxVm> + Send + Sync>,
    /// 注册表空时触发 shim 进程退出。
    exit: Arc<ExitSignal>,
}

impl TaskService {
    pub fn new(
        vm_factory: Arc<dyn Fn() -> Arc<dyn SandboxVm> + Send + Sync>,
        exit: Arc<ExitSignal>,
    ) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            vm_factory,
            exit,
        }
    }
}

fn ttrpc_err(msg: impl Into<String>) -> ttrpc::Error {
    ttrpc::Error::Others(msg.into())
}

fn now_ts() -> protobuf::well_known_types::timestamp::Timestamp {
    let now = std::time::SystemTime::now();
    let duration_since = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let mut ts = protobuf::well_known_types::timestamp::Timestamp::new();
    ts.seconds = duration_since.as_secs() as i64;
    ts.nanos = duration_since.subsec_nanos() as i32;
    ts
}

#[async_trait]
impl Task for TaskService {
    /// create: 读 bundle config.json 分流——sandbox-Task 造 VM 并 create；container-Task 建假状态。
    async fn create(
        &self,
        _ctx: &TtrpcContext,
        req: api::CreateTaskRequest,
    ) -> ttrpc::Result<api::CreateTaskResponse> {
        // 1.6.33 每条 task 都带 bundle；读它拿 container-type + netns。
        let spec = load_bundle_spec(&req.bundle)
            .map_err(|e| ttrpc_err(format!("create: load bundle {}: {}", req.bundle, e)))?;

        let mut tasks = self.tasks.lock().await;
        let pid = match spec.container_type {
            ContainerType::Sandbox => {
                let vm = (self.vm_factory)();
                let args = crate::common::vm::VmCreateArgs {
                    sandbox_id: req.id.clone(),
                    netns_path: spec.netns_path.clone().unwrap_or_default(),
                    type_id: 0, // Task 路径 P1 固定 bundle type 0 (annotation 选型留后期)。
                };
                vm.create(args)
                    .await
                    .map_err(|e| ttrpc_err(format!("create: vm create: {e}")))?;
                let (_state, pid) = vm.status().await;
                tasks.insert(req.id.clone(), TaskEntry::Sandbox { vm });
                pid
            }
            ContainerType::Container => {
                let st = ContainerState::new(FAKE_CONTAINER_PID);
                let pid = st.pid;
                tasks.insert(req.id.clone(), TaskEntry::Container(st));
                pid
            }
        };

        let mut resp = api::CreateTaskResponse::new();
        resp.pid = pid;
        Ok(resp)
    }

    /// start: sandbox-Task → vm.start 拿 pid；container-Task → 返回合成 pid (假运行)。
    async fn start(
        &self,
        _ctx: &TtrpcContext,
        req: api::StartRequest,
    ) -> ttrpc::Result<api::StartResponse> {
        let tasks = self.tasks.lock().await;
        let entry = tasks
            .get(&req.id)
            .ok_or_else(|| ttrpc_err(format!("start: task {} not found", req.id)))?;

        let pid = match entry {
            TaskEntry::Sandbox { vm } => {
                vm.start()
                    .await
                    .map_err(|e| ttrpc_err(format!("start: vm start: {e}")))?;
                let (_state, pid) = vm.status().await;
                pid
            }
            TaskEntry::Container(st) => st.pid, // 假运行：返回合成 pid
        };

        let mut resp = api::StartResponse::new();
        resp.pid = pid;
        Ok(resp)
    }

    /// state: 当前状态 (RUNNING/STOPPED) + pid + exit_status。
    async fn state(
        &self,
        _ctx: &TtrpcContext,
        req: api::StateRequest,
    ) -> ttrpc::Result<api::StateResponse> {
        let mut tasks = self.tasks.lock().await;
        let entry = tasks
            .get_mut(&req.id)
            .ok_or_else(|| ttrpc_err(format!("state: task {} not found", req.id)))?;

        let mut resp = api::StateResponse::new();
        match entry {
            TaskEntry::Sandbox { vm } => {
                let (vm_state, pid) = vm.status().await;
                resp.pid = pid;
                resp.status = match vm_state {
                    VmState::Running => Status::RUNNING,
                    VmState::Stopped => Status::STOPPED,
                    VmState::NotExist => Status::CREATED,
                }
                .into();
            }
            TaskEntry::Container(st) => {
                resp.pid = st.pid;
                if st.exited {
                    resp.status = Status::STOPPED.into();
                    resp.exit_status = st.exit_code;
                    resp.exited_at = protobuf::MessageField::some(now_ts());
                } else {
                    resp.status = Status::RUNNING.into();
                }
            }
        }
        Ok(resp)
    }

    /// wait: 阻塞至该 task 退出，返回 exit_status。
    async fn wait(
        &self,
        _ctx: &TtrpcContext,
        req: api::WaitRequest,
    ) -> ttrpc::Result<api::WaitResponse> {
        // 取出等待句柄后立刻释放锁，避免 wait 长期持锁阻塞其他方法。
        enum Waiter {
            Vm(Arc<dyn SandboxVm>),
            Container(tokio::sync::watch::Receiver<Option<u32>>),
        }

        let waiter = {
            let tasks = self.tasks.lock().await;
            match tasks.get(&req.id) {
                Some(TaskEntry::Sandbox { vm }) => Waiter::Vm(vm.clone()),
                Some(TaskEntry::Container(st)) => Waiter::Container(st.exit_rx.clone()),
                None => return Err(ttrpc_err(format!("wait: task {} not found", req.id))),
            }
        };

        let exit_status = match waiter {
            Waiter::Vm(vm) => {
                vm.wait().await;
                0 // vm.wait() 已阻塞至退出；exit code 简化为 0 (真实现从 vm.status 提取)。
            }
            Waiter::Container(mut rx) => {
                let mut code = 0u32;
                loop {
                    if let Some(c) = rx.borrow().clone() {
                        code = c;
                        break;
                    }
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                code
            }
        };

        let mut resp = api::WaitResponse::new();
        resp.exit_status = exit_status;
        resp.exited_at = protobuf::MessageField::some(now_ts());
        Ok(resp)
    }

    /// kill: sandbox-Task → vm.stop；container-Task → 标记假退出。返回 Empty (对齐 v2 Task)。
    async fn kill(
        &self,
        _ctx: &TtrpcContext,
        req: api::KillRequest,
    ) -> ttrpc::Result<api::Empty> {
        let mut tasks = self.tasks.lock().await;
        let entry = tasks
            .get_mut(&req.id)
            .ok_or_else(|| ttrpc_err(format!("kill: task {} not found", req.id)))?;

        match entry {
            TaskEntry::Sandbox { vm } => {
                vm.stop(0)
                    .await
                    .map_err(|e| ttrpc_err(format!("kill: vm stop: {e}")))?;
            }
            TaskEntry::Container(st) => {
                st.mark_exited(0);
            }
        }

        Ok(api::Empty::new())
    }

    /// delete: 从注册表删除该 task，回其 exit_status；注册表空 → 触发 shim 退出。
    async fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: api::DeleteRequest,
    ) -> ttrpc::Result<api::DeleteResponse> {
        let mut tasks = self.tasks.lock().await;
        let (pid, exit_status) = match tasks.get_mut(&req.id) {
            Some(TaskEntry::Sandbox { vm }) => {
                let _ = vm.stop(0).await; // 确保停止
                let (_state, pid) = vm.status().await;
                (pid, 0)
            }
            Some(TaskEntry::Container(st)) => {
                st.mark_exited(0);
                (st.pid, st.exit_code)
            }
            None => (0, 0),
        };

        tasks.remove(&req.id);

        let empty = tasks.is_empty();
        drop(tasks);

        // 退出门控 (ADR 决策8)：两条 task 都 Delete 后注册表空，才真正退 shim。
        if empty {
            self.exit.signal();
        }

        let mut resp = api::DeleteResponse::new();
        resp.pid = pid;
        resp.exit_status = exit_status;
        resp.exited_at = protobuf::MessageField::some(now_ts());
        Ok(resp)
    }

    /// connect: 回 shim_pid (本进程) + task_pid (不透明合成值)。
    async fn connect(
        &self,
        _ctx: &TtrpcContext,
        req: api::ConnectRequest,
    ) -> ttrpc::Result<api::ConnectResponse> {
        let tasks = self.tasks.lock().await;
        let task_pid = match tasks.get(&req.id) {
            Some(TaskEntry::Sandbox { vm }) => {
                let (_state, pid) = vm.status().await;
                pid
            }
            Some(TaskEntry::Container(st)) => st.pid,
            None => 0,
        };

        let mut resp = api::ConnectResponse::new();
        resp.shim_pid = std::process::id();
        resp.task_pid = task_pid;
        Ok(resp)
    }

    /// shutdown: 对齐 runc 参考 shim——只在注册表空时才 `exit.signal()`。
    async fn shutdown(
        &self,
        _ctx: &TtrpcContext,
        _req: api::ShutdownRequest,
    ) -> ttrpc::Result<api::Empty> {
        let empty = self.tasks.lock().await.is_empty();
        if empty {
            self.exit.signal();
        }
        Ok(api::Empty::new())
    }
}
