//! e2e 共享：测试装配（`TestEnv`）+ 请求构造助手。mock 后端来自 `oas_mock` crate。

#![allow(dead_code)]
#![allow(unused_imports)]

use std::sync::Arc;

use oas_manager::{FakeClock, OasManager, VmReadiness};
use oas_store::MemoryStore;

// mock 后端统一从 oas_mock 复用（与 main.rs 同源）。
pub use oas_mock::{ImmediateReadiness, MockDriver, MockNet, MockStorage, NeverReadyReadiness};

// ---- 测试装配 --------------------------------------------------------------

/// 各 test 二进制按需读取字段子集。
pub struct TestEnv {
    pub mgr: Arc<OasManager>,
    pub store: Arc<MemoryStore>,
    pub driver: Arc<MockDriver>,
    pub net: Arc<MockNet>,
    pub storage: Arc<MockStorage>,
    pub clock: Arc<FakeClock>,
}

impl TestEnv {
    pub fn new() -> Self {
        Self::with_readiness(Arc::new(ImmediateReadiness))
    }

    pub fn with_readiness(readiness: Arc<dyn VmReadiness>) -> Self {
        let store = Arc::new(MemoryStore::new());
        let driver = Arc::new(MockDriver::new());
        let net = Arc::new(MockNet::new(store.clone(), "10.244.0.0/24", "10.244.0.254"));
        let storage = Arc::new(MockStorage::new());
        let clock = Arc::new(FakeClock::new(1_700_000_000));
        let cfg = Arc::new(oas_config::Config::default());
        let mgr = Arc::new(OasManager::new(
            driver.clone(),
            net.clone(),
            storage.clone(),
            store.clone(),
            cfg,
            readiness,
            clock.clone(),
        ));
        Self {
            mgr,
            store,
            driver,
            net,
            storage,
            clock,
        }
    }
}

// ---- 请求构造助手 ----

pub fn sandbox_req(uid: &str, type_id: u8) -> oas_manager::CreateSandboxRequest {
    use std::collections::HashMap;
    oas_manager::CreateSandboxRequest {
        metadata: oas_types::SandboxMetadata {
            name: format!("pod-{uid}"),
            namespace: "default".into(),
            uid: uid.into(),
            attempt: 0,
        },
        labels: HashMap::new(),
        annotations: HashMap::new(),
        hostname: "host".into(),
        log_directory: "/logs".into(),
        dns_config: None,
        cgroup_parent: String::new(),
        type_id,
        cloud_disk_ref: None,
        rw_size: None,
        runtime_handler: "oas".into(),
    }
}

pub fn container_req(sandbox_id: &str, image: &str) -> oas_manager::CreateContainerRequest {
    use std::collections::HashMap;
    oas_manager::CreateContainerRequest {
        pod_sandbox_id: sandbox_id.into(),
        metadata: oas_types::ContainerMetadata {
            name: "c0".into(),
            attempt: 0,
        },
        image: image.into(),
        command: vec!["sh".into()],
        args: vec![],
        working_dir: String::new(),
        envs: vec![oas_types::KeyValue {
            key: "K".into(),
            value: "V".into(),
        }],
        mounts: vec![],
        labels: HashMap::new(),
        annotations: HashMap::new(),
        log_path: String::new(),
        resources: oas_types::LinuxResources::default(),
        tty: false,
        stdin: false,
        stdin_once: false,
    }
}
