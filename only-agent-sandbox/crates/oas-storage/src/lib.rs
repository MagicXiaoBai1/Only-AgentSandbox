//! 编排层 → 存储层 契约（§2.3 / §3.5）。
//!
//! 按 `type` 制备每 VM 可写 ext4（COW/overlay over base，高密度关键）、绑定云盘、清理。

/// 盘配置：`provision` 返回，`cleanup` 据此幂等清理。
#[derive(Debug, Clone, Default)]
pub struct DiskConfig {
    /// `/var/lib/oas/rw/<sandbox_id>.ext4`。
    pub rw_layer_path: Option<String>,
    /// `/dev/xxx`，来自 annotation。
    pub cloud_disk_dev: Option<String>,
}

/// 存储层错误。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cloud disk not found: {0}")]
    CloudDiskNotFound(String),
    #[error("{0}")]
    Other(String),
}

/// 编排层 → 存储层接口（§2.3）。
///
/// `provision` 按 type 制备每 VM 可写 ext4（COW/overlay），解析 annotation 里的云盘设备；
/// `cleanup` 幂等清理可写层文件、解绑云盘。
#[async_trait::async_trait]
pub trait StorageManager: Send + Sync {
    async fn provision(
        &self,
        sandbox_id: &str,
        type_id: u8,
        cloud_disk_ref: Option<&str>,
    ) -> Result<DiskConfig, StorageError>;

    async fn cleanup(&self, disk: &DiskConfig) -> Result<(), StorageError>;
}
