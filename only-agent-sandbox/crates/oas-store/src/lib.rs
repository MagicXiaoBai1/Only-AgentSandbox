//! 编排层 → 持久化层 契约（§2.4 / §3.6）。
//!
//! redb 5 张表（sandbox / container / ipam / image / meta）的 Repository 抽象。
//! 业务逻辑与 redb 存储解耦，事务边界 = 幂等与恢复的原子点。

/// 单事务句柄。
///
/// 占位：真实实现将包裹 `redb::WriteTransaction`（契约阶段不引 redb）。
///
/// ⚠️ object-safety：`Store::transaction` 是泛型方法，导致 `Store` 当前 **非
/// object-safe**。§3.2 设计 `Manager` 持 `Arc<dyn Store>`，与此冲突。本次忠实保留
/// §2 的泛型签名；object-safety 留到 §3.2 实现时再决定（改 `Box<dyn FnOnce>` 或
/// 拆 `begin_txn` / `commit`）。
pub struct Txn;

/// 持久化层错误。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(String),
    #[error("{0}")]
    Other(String),
}

/// 编排层 → 持久化层接口（§2.4）。
///
/// sandbox / container 同构 CRUD + IPAM 租约原子分配 + 单事务执行。
/// 读路径（`list_*` / `get_*`）只读 store，靠 redb 事务的 MVCC / 快照一致性，
/// 不等写锁，保证 PLEG 不被阻塞（§10.1）。
pub trait Store: Send + Sync {
    fn put_sandbox(&self, r: &oas_types::SandboxRecord) -> Result<(), StoreError>;
    fn get_sandbox(&self, id: &str) -> Result<oas_types::SandboxRecord, StoreError>;
    fn get_sandbox_by_uid(
        &self,
        pod_uid: &str,
    ) -> Result<Option<oas_types::SandboxRecord>, StoreError>;
    fn list_sandboxes(
        &self,
        filter: &oas_types::SandboxFilter,
    ) -> Result<Vec<oas_types::SandboxRecord>, StoreError>;
    fn delete_sandbox(&self, id: &str) -> Result<(), StoreError>;

    fn put_container(&self, r: &oas_types::ContainerRecord) -> Result<(), StoreError>;
    fn get_container(&self, id: &str) -> Result<oas_types::ContainerRecord, StoreError>;
    fn list_containers(
        &self,
        filter: &oas_types::ContainerFilter,
    ) -> Result<Vec<oas_types::ContainerRecord>, StoreError>;
    fn delete_container(&self, id: &str) -> Result<(), StoreError>;

    /// IPAM 租约原子分配（崩溃后不重复分配）。
    fn lease_ip(&self, cidr: &str) -> Result<oas_types::IpLease, StoreError>;
    fn release_ip(&self, lease: &oas_types::IpLease) -> Result<(), StoreError>;

    /// 单事务执行：建资源 + 落库的原子点（§3.6 事务边界）。
    fn transaction<R, F>(&self, f: F) -> Result<R, StoreError>
    where
        F: FnOnce(&mut Txn) -> Result<R, StoreError>;
}
