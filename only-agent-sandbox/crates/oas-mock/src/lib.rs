//! Mock 后端：driver / net / storage 的手写假实现 + `VmReadiness` 假实现。
//!
//! 设计目标：
//! - **不引 mockall**：每个 mock 用 `Arc<Mutex<…>>` 记录调用 + `AtomicBool` 注入失败，
//!   完全可控、零额外依赖。
//! - `MockNet` 持真实 `Store` 做 IPAM（§4.2：net 层 `lease_ip`/`release_ip`），让
//!   IPAM「崩溃后不重复分配」的性质在 mock 模式下也成立。
//! - 同时供 `oas-runtime`（`main.rs` 在真实 Firecracker 后端就绪前的临时装配）与
//!   `oas-e2e`（测试）共用，单一事实源。
//!
//! ⚠️ 这是占位后端：`create_vm` 不起任何 firecracker 进程，`setup` 不建 netns/tap，
//! `provision` 不制备 ext4。仅用于让上层（CRI/Manager/Store）在协议层跑通。真实底层
//! 实现就绪后 `main.rs` 改用真后端，本 crate 退回仅测试用。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use oas_driver::{DriverError, FirecrackerDriver, VmId, VmLifecycle, VmSpec, VmStatus};
use oas_manager::{OasError, VmReadiness};
use oas_net::{NetConfig, NetError, NetworkManager};
use oas_storage::{DiskConfig, StorageError, StorageManager};
use oas_store::Store;

// ---- MockDriver ------------------------------------------------------------

/// 不起真实 firecracker 进程；记录 create/get/delete/list 调用，可注入 create 失败。
pub struct MockDriver {
    fail_create: AtomicBool,
    creates: Mutex<Vec<(String, u8)>>,
    deletes: Mutex<Vec<u64>>,
    gets: Mutex<Vec<u64>>,
    lists: Mutex<Vec<()>>,
    statuses: Mutex<HashMap<u64, VmStatus>>,
    next_vm_id: AtomicU64,
}

impl MockDriver {
    pub fn new() -> Self {
        Self {
            fail_create: AtomicBool::new(false),
            creates: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
            gets: Mutex::new(Vec::new()),
            lists: Mutex::new(Vec::new()),
            statuses: Mutex::new(HashMap::new()),
            next_vm_id: AtomicU64::new(1),
        }
    }
    pub fn fail_create(&self, v: bool) {
        self.fail_create.store(v, Ordering::Relaxed);
    }
    pub fn create_count(&self) -> usize {
        self.creates.lock().unwrap().len()
    }
    pub fn delete_count(&self) -> usize {
        self.deletes.lock().unwrap().len()
    }
    pub fn get_count(&self) -> usize {
        self.gets.lock().unwrap().len()
    }
    pub fn list_count(&self) -> usize {
        self.lists.lock().unwrap().len()
    }
}

impl Default for MockDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FirecrackerDriver for MockDriver {
    async fn create_vm(
        &self,
        netns_path: &str,
        type_id: u8,
        _spec: VmSpec,
        _event_fd: std::os::fd::RawFd,
    ) -> Result<VmId, DriverError> {
        if self.fail_create.load(Ordering::Relaxed) {
            return Err(DriverError::Other("injected create_vm failure".into()));
        }
        let id = self.next_vm_id.fetch_add(1, Ordering::Relaxed);
        self.creates
            .lock()
            .unwrap()
            .push((netns_path.to_string(), type_id));
        self.statuses.lock().unwrap().insert(
            id,
            VmStatus {
                started: true,
                healthy: true,
                lifecycle: VmLifecycle::Running,
            },
        );
        Ok(VmId(id))
    }

    async fn get_vm(&self, id: VmId) -> Result<VmStatus, DriverError> {
        self.gets.lock().unwrap().push(id.0);
        match self.statuses.lock().unwrap().get(&id.0) {
            Some(s) => Ok(s.clone()),
            None => Err(DriverError::NotFound(id.0)),
        }
    }

    async fn delete_vm(&self, id: VmId) -> Result<(), DriverError> {
        self.deletes.lock().unwrap().push(id.0);
        let mut st = self.statuses.lock().unwrap();
        if let Some(s) = st.get_mut(&id.0) {
            s.lifecycle = VmLifecycle::Stopped;
        }
        Ok(())
    }

    async fn list_vm(&self) -> Result<Vec<VmStatus>, DriverError> {
        self.lists.lock().unwrap().push(());
        Ok(self.statuses.lock().unwrap().values().cloned().collect())
    }
}

// ---- MockNet ---------------------------------------------------------------

/// 不建真实 netns/tap；持真实 `Store` 做 IPAM（lease/release）。
pub struct MockNet {
    store: Arc<dyn Store>,
    cidr: String,
    gateway: String,
    setups: Mutex<Vec<String>>,
    teardowns: Mutex<Vec<String>>,
    fail_setup: AtomicBool,
}

impl MockNet {
    pub fn new(store: Arc<dyn Store>, cidr: &str, gateway: &str) -> Self {
        Self {
            store,
            cidr: cidr.to_string(),
            gateway: gateway.to_string(),
            setups: Mutex::new(Vec::new()),
            teardowns: Mutex::new(Vec::new()),
            fail_setup: AtomicBool::new(false),
        }
    }
    pub fn fail_setup(&self, v: bool) {
        self.fail_setup.store(v, Ordering::Relaxed);
    }
    pub fn setup_count(&self) -> usize {
        self.setups.lock().unwrap().len()
    }
    pub fn teardown_count(&self) -> usize {
        self.teardowns.lock().unwrap().len()
    }
}

#[async_trait]
impl NetworkManager for MockNet {
    async fn setup(&self, sandbox_id: &str) -> Result<NetConfig, NetError> {
        if self.fail_setup.load(Ordering::Relaxed) {
            return Err(NetError::Other("injected setup failure".into()));
        }
        let mut lease = self
            .store
            .lease_ip(&self.cidr)
            .map_err(|e| NetError::Other(e.to_string()))?;
        lease.sandbox_id = sandbox_id.to_string();
        self.setups.lock().unwrap().push(sandbox_id.to_string());
        Ok(NetConfig {
            netns_path: format!("/var/run/netns/oas-{sandbox_id}"),
            tap_name: "tap0".into(),
            mac: format!("02:oas:{sandbox_id}"),
            pod_ip: lease.ip.clone(),
            gateway: self.gateway.clone(),
            lease,
        })
    }

    async fn teardown(&self, net: &NetConfig) -> Result<(), NetError> {
        self.teardowns
            .lock()
            .unwrap()
            .push(net.lease.ip.clone());
        self.store
            .release_ip(&net.lease)
            .map_err(|e| NetError::Other(e.to_string()))?;
        Ok(())
    }
}

// ---- MockStorage -----------------------------------------------------------

/// 不制备真实 ext4 / 绑云盘；记录 provision/cleanup 调用，可注入 provision 失败。
pub struct MockStorage {
    fail_provision: AtomicBool,
    provisions: Mutex<Vec<(String, u8)>>,
    cleanups: Mutex<Vec<()>>,
}

impl MockStorage {
    pub fn new() -> Self {
        Self {
            fail_provision: AtomicBool::new(false),
            provisions: Mutex::new(Vec::new()),
            cleanups: Mutex::new(Vec::new()),
        }
    }
    pub fn fail_provision(&self, v: bool) {
        self.fail_provision.store(v, Ordering::Relaxed);
    }
    pub fn provision_count(&self) -> usize {
        self.provisions.lock().unwrap().len()
    }
    pub fn cleanup_count(&self) -> usize {
        self.cleanups.lock().unwrap().len()
    }
}

impl Default for MockStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StorageManager for MockStorage {
    async fn provision(
        &self,
        sandbox_id: &str,
        type_id: u8,
        cloud_disk_ref: Option<&str>,
    ) -> Result<DiskConfig, StorageError> {
        self.provisions
            .lock()
            .unwrap()
            .push((sandbox_id.to_string(), type_id));
        if self.fail_provision.load(Ordering::Relaxed) {
            return Err(StorageError::Other("injected provision failure".into()));
        }
        Ok(DiskConfig {
            rw_layer_path: Some(format!("/var/lib/oas/rw/{sandbox_id}.ext4")),
            cloud_disk_dev: cloud_disk_ref.map(|s| s.to_string()),
        })
    }

    async fn cleanup(&self, _disk: &DiskConfig) -> Result<(), StorageError> {
        self.cleanups.lock().unwrap().push(());
        Ok(())
    }
}

// ---- VmReadiness 假实现 ----------------------------------------------------

/// 立即就绪（happy path）。
pub struct ImmediateReadiness;

#[async_trait]
impl VmReadiness for ImmediateReadiness {
    async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), OasError> {
        Ok(())
    }
}

/// 永不就绪：短暂等待后返回 `Unavailable`（用于测 readiness 超时回滚，不挂起）。
pub struct NeverReadyReadiness;

#[async_trait]
impl VmReadiness for NeverReadyReadiness {
    async fn wait_ready(&self, _vm_id: VmId, _timeout: Duration) -> Result<(), OasError> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Err(OasError::Unavailable(
            "vm not ready (never-ready fake)".into(),
        ))
    }
}
