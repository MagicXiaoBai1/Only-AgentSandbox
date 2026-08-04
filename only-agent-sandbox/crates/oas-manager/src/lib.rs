//! 编排层 (oas-manager) —— CRI 层的上游契约。
//!
//! 这里只定义 `Manager` 公开 API（trait）、领域 request/response 类型与统一错误
//! `OasError`，外加一个 `StubManager` 供 `oas-cri` 在 §3.2 真 Manager 落地前装配起
//! 服务。真 Manager 实现（持 driver/net/storage/store 四个 trait、状态机、reconcile）
//! 在 §3.2。
//!
//! 依赖方向：`oas-cri → oas-manager → oas-types`，单向无环。本 crate 契约只依赖
//! `oas-types`，不直接依赖 driver/net/storage/store（下层错误在真 Manager impl 里
//! 映射进 `OasError`）。

use std::collections::HashMap;

use oas_types::{
    ContainerFilter, ContainerMetadata, ContainerRecord, DnsConfig, KeyValue, LinuxResources,
    Mount, SandboxFilter, SandboxMetadata, SandboxRecord,
};

pub mod clock;
pub mod id;
pub mod lock;
pub mod manager;
pub mod readiness;

pub use clock::{Clock, FakeClock, SystemClock};
pub use id::IdGenerator;
pub use lock::PerKeyLock;
pub use manager::OasManager;
pub use readiness::VmReadiness;

// ---------------------------------------------------------------------------
// 统一领域错误
// ---------------------------------------------------------------------------

/// 统一领域错误。CRI 层 `error.rs` 把它映射成 `tonic::Status`（§3.1）。
///
/// 自包含：不 `From` 下层 `StoreError`/`DriverError`/…，以免本契约反向依赖下层 crate。
/// 下层错误在真 Manager impl 里映射进这里的变体。
#[derive(Debug, thiserror::Error)]
pub enum OasError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("image not in whitelist: {0}")]
    ImageNotInList(String),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("internal: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// 领域 request / response 类型（proto ⇄ domain 的 domain 侧）
// ---------------------------------------------------------------------------

/// `RunPodSandbox` 的领域请求。annotation 里的 `agent-sandbox/type`、`cloud-disk`、
/// `rw-size`（§9）由 CRI convert 解析进 `type_id`/`cloud_disk_ref`/`rw_size`。
#[derive(Debug, Clone)]
pub struct CreateSandboxRequest {
    pub metadata: SandboxMetadata,
    pub labels: HashMap<String, String>,
    pub annotations: HashMap<String, String>,
    pub hostname: String,
    pub log_directory: String,
    pub dns_config: Option<DnsConfig>,
    pub cgroup_parent: String,
    /// 从 annotation `agent-sandbox/type` 解析。
    pub type_id: u8,
    /// 从 annotation `agent-sandbox/cloud-disk` 解析（type2 必填）。
    pub cloud_disk_ref: Option<String>,
    /// 从 annotation `agent-sandbox/rw-size` 解析。
    pub rw_size: Option<u64>,
    /// CRI `runtime_handler`（空串视为默认 `oas`）。
    pub runtime_handler: String,
}

/// `CreateContainer` 的领域请求。
#[derive(Debug, Clone)]
pub struct CreateContainerRequest {
    pub pod_sandbox_id: String,
    pub metadata: ContainerMetadata,
    pub image: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub working_dir: String,
    pub envs: Vec<KeyValue>,
    pub mounts: Vec<Mount>,
    pub labels: HashMap<String, String>,
    pub annotations: HashMap<String, String>,
    pub log_path: String,
    pub resources: LinuxResources,
    pub tty: bool,
    pub stdin: bool,
    pub stdin_once: bool,
}

/// 镜像信息（对应 CRI `Image`）。
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub size: u64,
    pub username: String,
    /// 工作镜像引用（= `ImageSpec.image`）。
    pub image_ref: String,
    pub pinned: bool,
}

/// `Version` 返回。
#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub runtime_name: String,
    pub runtime_version: String,
    pub runtime_api_version: String,
}

/// `Status` 的运行时 condition。
#[derive(Debug, Clone)]
pub struct RuntimeCondition {
    pub r#type: String,
    pub status: bool,
    pub reason: String,
    pub message: String,
}

/// `Status` 返回。
#[derive(Debug, Clone)]
pub struct RuntimeStatusInfo {
    pub conditions: Vec<RuntimeCondition>,
}

// ---------------------------------------------------------------------------
// Manager 公开 API（CRI → manager）
// ---------------------------------------------------------------------------

/// 编排层对 CRI 层的统一入口（Facade，§5）。CRI 持 `Arc<dyn Manager>`。
///
/// 方法返回 `oas-types` 的 `SandboxRecord`/`ContainerRecord`，由 CRI convert 翻译回 proto。
#[async_trait::async_trait]
pub trait Manager: Send + Sync {
    async fn version(&self) -> Result<VersionInfo, OasError>;
    async fn status(&self) -> Result<RuntimeStatusInfo, OasError>;
    async fn update_runtime_config(&self, pod_cidr: Option<&str>) -> Result<(), OasError>;

    async fn run_sandbox(&self, req: CreateSandboxRequest) -> Result<String, OasError>;
    async fn stop_sandbox(&self, sandbox_id: &str) -> Result<(), OasError>;
    async fn remove_sandbox(&self, sandbox_id: &str) -> Result<(), OasError>;
    async fn sandbox_status(&self, sandbox_id: &str) -> Result<SandboxRecord, OasError>;
    async fn list_sandboxes(&self, filter: SandboxFilter) -> Result<Vec<SandboxRecord>, OasError>;

    async fn create_container(&self, req: CreateContainerRequest) -> Result<String, OasError>;
    async fn start_container(&self, container_id: &str) -> Result<(), OasError>;
    async fn stop_container(&self, container_id: &str, timeout: i64) -> Result<(), OasError>;
    async fn remove_container(&self, container_id: &str) -> Result<(), OasError>;
    async fn container_status(&self, container_id: &str) -> Result<ContainerRecord, OasError>;
    async fn list_containers(
        &self,
        filter: ContainerFilter,
    ) -> Result<Vec<ContainerRecord>, OasError>;

    async fn image_status(&self, image: &str) -> Result<Option<ImageInfo>, OasError>;
    async fn pull_image(&self, image: &str) -> Result<String, OasError>;
    async fn list_images(&self) -> Result<Vec<ImageInfo>, OasError>;
    async fn remove_image(&self, image: &str) -> Result<(), OasError>;
}

// ---------------------------------------------------------------------------
// StubManager：§3.2 真 Manager 落地前的装配占位
// ---------------------------------------------------------------------------

/// 空实现 Manager：握手类（version/status）返回固定值；读路径返回空；幂等 stop/remove
/// 返回 `Ok`；写操作返回 `Unavailable`（kubelet 起沙箱会失败，预期，待 §3.2）。
#[derive(Debug, Default, Clone, Copy)]
pub struct StubManager;

impl StubManager {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Manager for StubManager {
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
                    reason: "".into(),
                    message: "".into(),
                },
                RuntimeCondition {
                    r#type: "NetworkReady".into(),
                    status: true,
                    reason: "".into(),
                    message: "".into(),
                },
            ],
        })
    }

    async fn update_runtime_config(&self, _pod_cidr: Option<&str>) -> Result<(), OasError> {
        // 空实现接住（§3.1）。
        Ok(())
    }

    async fn run_sandbox(&self, _req: CreateSandboxRequest) -> Result<String, OasError> {
        Err(OasError::Unavailable(
            "manager not implemented (§3.2)".into(),
        ))
    }

    async fn stop_sandbox(&self, _sandbox_id: &str) -> Result<(), OasError> {
        // 幂等：删不存在 = Ok。
        Ok(())
    }

    async fn remove_sandbox(&self, _sandbox_id: &str) -> Result<(), OasError> {
        Ok(())
    }

    async fn sandbox_status(&self, sandbox_id: &str) -> Result<SandboxRecord, OasError> {
        Err(OasError::NotFound(sandbox_id.into()))
    }

    async fn list_sandboxes(&self, _filter: SandboxFilter) -> Result<Vec<SandboxRecord>, OasError> {
        Ok(Vec::new())
    }

    async fn create_container(&self, _req: CreateContainerRequest) -> Result<String, OasError> {
        Err(OasError::Unavailable(
            "manager not implemented (§3.2)".into(),
        ))
    }

    async fn start_container(&self, container_id: &str) -> Result<(), OasError> {
        Err(OasError::NotFound(container_id.into()))
    }

    async fn stop_container(&self, _container_id: &str, _timeout: i64) -> Result<(), OasError> {
        Ok(())
    }

    async fn remove_container(&self, _container_id: &str) -> Result<(), OasError> {
        Ok(())
    }

    async fn container_status(&self, container_id: &str) -> Result<ContainerRecord, OasError> {
        Err(OasError::NotFound(container_id.into()))
    }

    async fn list_containers(
        &self,
        _filter: ContainerFilter,
    ) -> Result<Vec<ContainerRecord>, OasError> {
        Ok(Vec::new())
    }

    async fn image_status(&self, _image: &str) -> Result<Option<ImageInfo>, OasError> {
        Ok(None)
    }

    async fn pull_image(&self, _image: &str) -> Result<String, OasError> {
        Err(OasError::Unavailable(
            "manager not implemented (§3.2)".into(),
        ))
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>, OasError> {
        Ok(Vec::new())
    }

    async fn remove_image(&self, _image: &str) -> Result<(), OasError> {
        Ok(())
    }
}
