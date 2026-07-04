//! 真 `Manager` 实现（§3.2）：持 driver/net/storage/store + readiness + 类型表 + 锁 + 时钟。
//!
//! 容器层经 driver 下沉（§2.8）：`create/start/stop/remove_container` 调 driver 对应原语，
//! `container_status` 调 `driver.get_container_status` 做 reconcile 对账；声明态事实源仍在
//! store。真实 vsock/GuestAgent 通道待 §2.8 落地，届时改动收敛在 driver impl 内。
//! 红线 2 保留：`stop_container` 显式停止置 EXITED（exit_code=0/Completed，非「VM 还活着」反推）；
//! 红线 3 保留：`list_containers` 永不调 driver。

use std::sync::Arc;
use std::time::Duration;

use oas_driver::{ContainerSpec, DriverError, FirecrackerDriver, VmId, VmLifecycle, VmSpec};
use oas_net::{NetConfig, NetError, NetworkManager};
use oas_storage::{DiskConfig, StorageError, StorageManager};
use oas_store::{Store, StoreError};
use oas_types::{
    ContainerExitReason, ContainerFilter, ContainerRecord, ContainerState, IpLease,
    SandboxFilter, SandboxRecord, SandboxState,
};

use crate::{
    Clock, CreateContainerRequest, CreateSandboxRequest, IdGenerator, ImageInfo, Manager,
    OasError, PerKeyLock, RuntimeCondition, RuntimeStatusInfo, SandboxTypeTable, VersionInfo,
    VmReadiness,
};

/// 就绪等待超时（真实 eventfd 实现用；假实现可忽略）。
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
/// `stop_sandbox` 轮询 `get_vm` 到 Stopped 的最大次数 / 间隔。
const MAX_STOP_POLLS: usize = 50;
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 真编排层 Manager。
pub struct OasManager {
    driver: Arc<dyn FirecrackerDriver>,
    net: Arc<dyn NetworkManager>,
    storage: Arc<dyn StorageManager>,
    store: Arc<dyn Store>,
    types: SandboxTypeTable,
    locks: PerKeyLock,
    clock: Arc<dyn Clock>,
    id_gen: IdGenerator,
    readiness: Arc<dyn VmReadiness>,
    // 容器原语经 driver 下沉（§2.8）：真实 vsock/GuestAgent 通道待落地，无需在 manager 持 agent。
    // TODO(§4.6): reconcile 后台 task。
}

impl OasManager {
    pub fn new(
        driver: Arc<dyn FirecrackerDriver>,
        net: Arc<dyn NetworkManager>,
        storage: Arc<dyn StorageManager>,
        store: Arc<dyn Store>,
        readiness: Arc<dyn VmReadiness>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            driver,
            net,
            storage,
            store,
            types: SandboxTypeTable::default(),
            locks: PerKeyLock::new(),
            clock,
            id_gen: IdGenerator::new(),
            readiness,
        }
    }

    /// 取 sandbox 的 vm_id（容器原语下沉 driver 时定位 VM）。
    fn vm_id_of(&self, sandbox_id: &str) -> Result<VmId, OasError> {
        let sb = self.store.get_sandbox(sandbox_id).map_err(map_store_err)?;
        Ok(VmId(sb.vm_id))
    }
}

// ---- 错误映射（下层 → OasError，自包含）----

fn map_driver_err(e: DriverError) -> OasError {
    let msg = e.to_string();
    match e {
        DriverError::NotFound(_) => OasError::NotFound(msg),
        DriverError::Exists(_) => OasError::Conflict(msg),
        _ => OasError::Unavailable(msg),
    }
}

fn map_net_err(e: NetError) -> OasError {
    let msg = e.to_string();
    match e {
        NetError::IpamExhausted(_) => OasError::Unavailable(msg),
        _ => OasError::Internal(msg),
    }
}

fn map_storage_err(e: StorageError) -> OasError {
    let msg = e.to_string();
    match e {
        StorageError::CloudDiskNotFound(_) => OasError::InvalidArgument(msg),
        _ => OasError::Internal(msg),
    }
}

fn map_store_err(e: StoreError) -> OasError {
    let msg = e.to_string();
    match e {
        StoreError::NotFound(_) => OasError::NotFound(msg),
        _ => OasError::Internal(msg),
    }
}

fn image_info(image: &str) -> ImageInfo {
    ImageInfo {
        id: format!("sha256:{image}"),
        repo_tags: vec![image.to_string()],
        repo_digests: vec![],
        size: 1_048_576, // 非 0：kubelet 校验 ImageStatus 要求 id≠"" 且 size>0
        username: String::new(),
        image_ref: image.to_string(),
        pinned: true,
    }
}

/// 从 SandboxRecord 还原 NetConfig（供 `stop_sandbox` 调 `net.teardown` 释放 IP）。
/// `lease.cidr` 留空：`release_ip` 以 ip 为键，cidr 仅信息字段。
fn net_cfg_from(rec: &SandboxRecord) -> NetConfig {
    NetConfig {
        netns_path: rec.netns_path.clone(),
        tap_name: rec.tap_name.clone(),
        mac: rec.mac.clone(),
        pod_ip: rec.pod_ip.clone(),
        gateway: rec.gateway.clone(),
        lease: IpLease {
            ip: rec.pod_ip.clone(),
            cidr: String::new(),
            sandbox_id: rec.sandbox_id.clone(),
        },
    }
}

#[async_trait::async_trait]
impl Manager for OasManager {
    async fn version(&self) -> Result<VersionInfo, OasError> {
        Ok(VersionInfo {
            runtime_name: "only-agent-sandbox".into(),
            runtime_version: "0.1.0".into(),
            runtime_api_version: "v1".into(),
        })
    }

    async fn status(&self) -> Result<RuntimeStatusInfo, OasError> {
        Ok(RuntimeStatusInfo {
            conditions: vec![
                RuntimeCondition {
                    r#type: "RuntimeReady".into(),
                    status: true,
                    reason: String::new(),
                    message: String::new(),
                },
                RuntimeCondition {
                    r#type: "NetworkReady".into(),
                    status: true,
                    reason: String::new(),
                    message: String::new(),
                },
            ],
        })
    }

    async fn update_runtime_config(&self, _pod_cidr: Option<&str>) -> Result<(), OasError> {
        // TODO: 持久化 podCIDR 到 store `meta` 表。
        Ok(())
    }

    async fn run_sandbox(&self, req: CreateSandboxRequest) -> Result<String, OasError> {
        // 锁 pod_uid（幂等键）。
        let _guard = self.locks.lock(&req.metadata.uid).await;

        // 幂等短路：同 pod_uid 已存在。
        if let Some(existing) = self
            .store
            .get_sandbox_by_uid(&req.metadata.uid)
            .map_err(map_store_err)?
        {
            if existing.state == SandboxState::Ready {
                return Ok(existing.sandbox_id);
            }
            return Err(OasError::Conflict(format!(
                "sandbox {} exists but not ready; remove first",
                existing.sandbox_id
            )));
        }

        let ty = self.types.get(req.type_id)?;
        // 云盘一致性。
        if ty.has_cloud_disk && req.cloud_disk_ref.is_none() {
            return Err(OasError::InvalidArgument(format!(
                "type {} requires cloud-disk annotation",
                req.type_id
            )));
        }
        if !ty.has_cloud_disk && req.cloud_disk_ref.is_some() {
            return Err(OasError::InvalidArgument(format!(
                "type {} does not accept cloud-disk",
                req.type_id
            )));
        }

        let sandbox_id = self.id_gen.sandbox_id();

        // net.setup（内部 lease_ip，崩溃后不重复分配）。
        let net_cfg = match self.net.setup(&sandbox_id).await {
            Ok(c) => c,
            Err(e) => return Err(map_net_err(e)),
        };

        // storage.provision。
        let disk = match self
            .storage
            .provision(&sandbox_id, req.type_id, req.cloud_disk_ref.as_deref())
            .await
        {
            Ok(d) => d,
            Err(e) => {
                let _ = self.net.teardown(&net_cfg).await; // 回滚网络
                return Err(map_storage_err(e));
            }
        };

        // driver.create_vm（event_fd 本轮传哨兵 -1，假实现忽略；真实 eventfd 随 driver 专题）。
        let spec = VmSpec {
            rw_layer_path: disk.rw_layer_path.clone(),
            cloud_disk_dev: disk.cloud_disk_dev.clone(),
        };
        let vm_id = match self
            .driver
            .create_vm(&net_cfg.netns_path, req.type_id, spec, -1)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                let oe = map_driver_err(e);
                self.rollback_create(&net_cfg, &disk, None).await;
                return Err(oe);
            }
        };

        // 等就绪（超时 → 回滚）。
        if let Err(oe) = self.readiness.wait_ready(vm_id, READINESS_TIMEOUT).await {
            self.rollback_create(&net_cfg, &disk, Some(vm_id)).await;
            return Err(oe);
        }

        // 落库 READY（单事务）。
        let rec = SandboxRecord {
            sandbox_id: sandbox_id.clone(),
            pod_uid: req.metadata.uid.clone(),
            metadata: req.metadata.clone(),
            labels: req.labels.clone(),
            annotations: req.annotations.clone(),
            type_id: req.type_id,
            vm_id: vm_id.0,
            netns_path: net_cfg.netns_path.clone(),
            tap_name: net_cfg.tap_name.clone(),
            mac: net_cfg.mac.clone(),
            pod_ip: net_cfg.pod_ip.clone(),
            gateway: net_cfg.gateway.clone(),
            rw_layer_path: disk.rw_layer_path.clone(),
            cloud_disk_dev: disk.cloud_disk_dev.clone(),
            state: SandboxState::Ready,
            created_at: self.clock.now_unix_secs(),
        };
        let rec_clone = rec.clone();
        if let Err(e) = self
            .store
            .transaction(Box::new(move |t| t.put_sandbox(&rec_clone)))
        {
            self.rollback_create(&net_cfg, &disk, Some(vm_id)).await;
            return Err(map_store_err(e));
        }
        Ok(sandbox_id)
    }

    async fn stop_sandbox(&self, sandbox_id: &str) -> Result<(), OasError> {
        let _guard = self.locks.lock(sandbox_id).await;
        let rec = self.store.get_sandbox(sandbox_id).map_err(map_store_err)?; // NotFound 透传，CRI 吞
        let vm_id = VmId(rec.vm_id);

        let _ = self.driver.delete_vm(vm_id).await; // 幂等
        for _ in 0..MAX_STOP_POLLS {
            match self.driver.get_vm(vm_id).await {
                Ok(s) if s.lifecycle == VmLifecycle::Stopped => break,
                Ok(_) => tokio::time::sleep(STOP_POLL_INTERVAL).await,
                Err(DriverError::NotFound(_)) => break,
                Err(_) => break, // best-effort
            }
        }

        let _ = self.net.teardown(&net_cfg_from(&rec)).await; // 释放 IP
        let mut rec2 = rec.clone();
        rec2.state = SandboxState::NotReady;
        let rec_clone = rec2.clone();
        self.store
            .transaction(Box::new(move |t| t.put_sandbox(&rec_clone)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn remove_sandbox(&self, sandbox_id: &str) -> Result<(), OasError> {
        let _guard = self.locks.lock(sandbox_id).await;
        let rec = match self.store.get_sandbox(sandbox_id) {
            Ok(r) => r,
            Err(StoreError::NotFound(_)) => return Ok(()), // 幂等
            Err(e) => return Err(map_store_err(e)),
        };
        let disk = DiskConfig {
            rw_layer_path: rec.rw_layer_path.clone(),
            cloud_disk_dev: rec.cloud_disk_dev.clone(),
        };
        let _ = self.storage.cleanup(&disk).await; // best-effort
        let sid = sandbox_id.to_string();
        self.store
            .transaction(Box::new(move |t| t.delete_sandbox(&sid)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn sandbox_status(&self, sandbox_id: &str) -> Result<SandboxRecord, OasError> {
        self.store.get_sandbox(sandbox_id).map_err(map_store_err)
    }

    async fn list_sandboxes(&self, filter: SandboxFilter) -> Result<Vec<SandboxRecord>, OasError> {
        self.store.list_sandboxes(&filter).map_err(map_store_err)
    }

    async fn create_container(&self, req: CreateContainerRequest) -> Result<String, OasError> {
        let _guard = self.locks.lock(&req.pod_sandbox_id).await;
        let sb = self
            .store
            .get_sandbox(&req.pod_sandbox_id)
            .map_err(map_store_err)?;
        let ty = self.types.get(sb.type_id)?;

        // 镜像白名单（ImageNotInList → CRI not_found）。
        if !ty.image_allowed(&req.image) {
            return Err(OasError::ImageNotInList(req.image.clone()));
        }
        // 资源 vs type 预算（§9 一致性）。
        if let Some(mem) = req.resources.memory_limit_bytes {
            let budget = (ty.mem_mib as i64) * 1_048_576;
            if mem > budget {
                return Err(OasError::TypeMismatch(format!(
                    "memory {mem}B exceeds type {} budget {budget}B",
                    ty.type_id
                )));
            }
        }
        if let Some(cpu) = req.resources.cpu_shares {
            let budget = (ty.vcpu as u64) * 1024;
            if cpu > budget {
                return Err(OasError::TypeMismatch(format!(
                    "cpu_shares {cpu} exceeds type {} budget {budget}",
                    ty.type_id
                )));
            }
        }

        // 硬限制：单沙箱仅单容器（ADR）。已有未删除容器 → Conflict。
        let existing = self
            .store
            .list_containers(&ContainerFilter {
                sandbox_id: Some(req.pod_sandbox_id.clone()),
                ..Default::default()
            })
            .map_err(map_store_err)?;
        if !existing.is_empty() {
            return Err(OasError::Conflict(format!(
                "sandbox {} already has a container (single-container-per-sandbox)",
                req.pod_sandbox_id
            )));
        }

        let container_id = self.id_gen.container_id();
        let env: Vec<String> = req.envs.iter().map(|kv| format!("{}={}", kv.key, kv.value)).collect();
        let rec = ContainerRecord {
            container_id: container_id.clone(),
            sandbox_id: req.pod_sandbox_id.clone(),
            metadata: req.metadata.clone(),
            image: req.image.clone(),
            command: req.command.clone(),
            args: req.args.clone(),
            env,
            mounts: req.mounts.clone(),
            resources: req.resources.clone(),
            state: ContainerState::Created,
            created_at: self.clock.now_unix_secs(),
            started_at: None,
            finished_at: None,
            exit_code: 0,
            reason: None,
            message: String::new(),
            labels: req.labels.clone(),
            annotations: req.annotations.clone(),
        };
        let rec_clone = rec.clone();
        self.store
            .transaction(Box::new(move |t| t.put_container(&rec_clone)))
            .map_err(map_store_err)?;

        // 经 driver 下沉（§2.8）：在 VM 内登记容器。MVP no-op，失败回滚刚写入的记录。
        let spec = ContainerSpec {
            command: req.command.clone(),
            args: req.args.clone(),
            env: rec.env.clone(),
            cwd: req.working_dir.clone(),
        };
        if let Err(e) = self
            .driver
            .create_container(VmId(sb.vm_id), &container_id, spec)
            .await
        {
            let cid = container_id.clone();
            let _ = self
                .store
                .transaction(Box::new(move |t| t.delete_container(&cid)));
            return Err(map_driver_err(e));
        }
        Ok(container_id)
    }

    async fn start_container(&self, container_id: &str) -> Result<(), OasError> {
        let sid = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?
            .sandbox_id;
        let _guard = self.locks.lock(&sid).await;
        let c = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?;
        match c.state {
            ContainerState::Running => Ok(()), // 幂等
            ContainerState::Created => {
                let vm_id = self.vm_id_of(&c.sandbox_id)?;
                // 经 driver 下沉（§2.8）：在 VM 内启动容器进程。MVP no-op。
                self.driver
                    .start_container(vm_id, container_id)
                    .await
                    .map_err(map_driver_err)?;
                let mut c2 = c.clone();
                c2.state = ContainerState::Running;
                c2.started_at = Some(self.clock.now_unix_secs());
                let cc = c2.clone();
                self.store
                    .transaction(Box::new(move |t| t.put_container(&cc)))
                    .map_err(map_store_err)?;
                Ok(())
            }
            _ => Err(OasError::Conflict(format!(
                "container {container_id} not in Created/Running state"
            ))),
        }
    }

    async fn stop_container(&self, container_id: &str, timeout: i64) -> Result<(), OasError> {
        let sid = match self.store.get_container(container_id) {
            Ok(c) => c.sandbox_id,
            Err(StoreError::NotFound(_)) => return Ok(()), // 幂等
            Err(e) => return Err(map_store_err(e)),
        };
        let _guard = self.locks.lock(&sid).await;
        let c = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?;
        if c.state == ContainerState::Exited {
            return Ok(()); // 幂等
        }
        let vm_id = self.vm_id_of(&c.sandbox_id)?;
        // 经 driver 下沉（§2.8）：在 VM 内停止容器进程。best-effort：失败不阻断置 EXITED
        // （红线 2：显式停止，非 VM-alive 反推 exit_code）。
        if let Err(e) = self
            .driver
            .stop_container(vm_id, container_id, timeout)
            .await
        {
            tracing::warn!(target: "oas-manager", "driver stop_container failed (proceeding): {e}");
        }
        let mut c2 = c.clone();
        c2.state = ContainerState::Exited;
        c2.finished_at = Some(self.clock.now_unix_secs());
        c2.exit_code = 0;
        c2.reason = Some(ContainerExitReason::Completed);
        c2.message = "explicit stop".into();
        let cc = c2.clone();
        self.store
            .transaction(Box::new(move |t| t.put_container(&cc)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn remove_container(&self, container_id: &str) -> Result<(), OasError> {
        let sid = match self.store.get_container(container_id) {
            Ok(c) => c.sandbox_id,
            Err(StoreError::NotFound(_)) => return Ok(()), // 幂等
            Err(e) => return Err(map_store_err(e)),
        };
        let _guard = self.locks.lock(&sid).await;
        let vm_id = self.vm_id_of(&sid)?;
        // 经 driver 下沉（§2.8）：在 VM 内移除容器登记。best-effort。
        if let Err(e) = self.driver.remove_container(vm_id, container_id).await {
            tracing::warn!(target: "oas-manager", "driver remove_container failed (proceeding): {e}");
        }
        let cid = container_id.to_string();
        self.store
            .transaction(Box::new(move |t| t.delete_container(&cid)))
            .map_err(map_store_err)?;
        Ok(())
    }

    async fn container_status(&self, container_id: &str) -> Result<ContainerRecord, OasError> {
        let c = self
            .store
            .get_container(container_id)
            .map_err(map_store_err)?;
        // 经 driver 下沉（§2.8）：拉 VM 内实际态做 reconcile 对账。声明态事实源仍是 store，
        // driver 实际态仅用于对账/补全 pid——MVP 不自动改 store 状态机（红线 2：不反推 exit_code）。
        if let Ok(vm_id) = self.vm_id_of(&c.sandbox_id) {
            if let Ok(rt) = self.driver.get_container_status(vm_id, container_id).await {
                tracing::debug!(
                    target: "oas-manager",
                    container_id,
                    declared_state = ?c.state,
                    runtime_state = ?rt.state,
                    "container_status reconcile",
                );
            }
        }
        Ok(c)
    }

    async fn list_containers(
        &self,
        filter: ContainerFilter,
    ) -> Result<Vec<ContainerRecord>, OasError> {
        self.store.list_containers(&filter).map_err(map_store_err)
    }

    async fn image_status(&self, image: &str) -> Result<Option<ImageInfo>, OasError> {
        if self.types.image_allowed_any(image) {
            Ok(Some(image_info(image)))
        } else {
            Ok(None)
        }
    }

    async fn pull_image(&self, image: &str) -> Result<String, OasError> {
        if self.types.image_allowed_any(image) {
            Ok(image.to_string())
        } else {
            Err(OasError::ImageNotInList(image.to_string()))
        }
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>, OasError> {
        Ok(self
            .types
            .all_images()
            .into_iter()
            .map(image_info)
            .collect())
    }

    async fn remove_image(&self, _image: &str) -> Result<(), OasError> {
        Ok(()) // 白名单来自配置，幂等空操作。
    }
}

impl OasManager {
    /// `run_sandbox` 失败回滚已建资源（顺序：driver → storage → net）。
    async fn rollback_create(
        &self,
        net_cfg: &NetConfig,
        disk: &DiskConfig,
        vm_id: Option<VmId>,
    ) {
        if let Some(id) = vm_id {
            let _ = self.driver.delete_vm(id).await;
        }
        let _ = self.storage.cleanup(disk).await;
        let _ = self.net.teardown(net_cfg).await;
    }
}
