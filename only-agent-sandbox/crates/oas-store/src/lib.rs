//! 编排层 → 持久化层 契约（§2.7 / §3.6）。
//!
//! redb 5 张表（sandbox / container / ipam / image / meta）的 Repository 抽象。
//! 业务逻辑与 redb 存储解耦，事务边界 = 幂等与恢复的原子点。
//!
//! object-safety：`transaction` 取 `Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>`，
//! 无泛型，故 `Store` object-safe，可 `Arc<dyn Store>`（§3.2 Manager 持有）。

mod memory;
mod redb;

pub use memory::MemoryStore;
pub use redb::RedbStore;

/// 单事务句柄：事务内可见的写接口。
///
/// 各后端自行实现（`MemoryTxn` / `RedbTxn`），通过 `&mut dyn Txn` 传给 `Store::transaction`
/// 的闭包。`lease_ip` / `release_ip` 也在事务内可用，以便「删 sandbox + 释放 IP」一类原子点。
pub trait Txn {
    fn put_sandbox(&mut self, r: &oas_types::SandboxRecord) -> Result<(), StoreError>;
    fn put_container(&mut self, r: &oas_types::ContainerRecord) -> Result<(), StoreError>;
    fn delete_sandbox(&mut self, id: &str) -> Result<(), StoreError>;
    fn delete_container(&mut self, id: &str) -> Result<(), StoreError>;
    fn lease_ip(&mut self, cidr: &str) -> Result<oas_types::IpLease, StoreError>;
    fn release_ip(&mut self, lease: &oas_types::IpLease) -> Result<(), StoreError>;
}

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

/// 编排层 → 持久化层接口（§2.7）。
///
/// sandbox / container 同构 CRUD + IPAM 租约原子分配 + 单事务执行。
/// 读路径（`list_*` / `get_*`）只读 store，靠 redb 事务的 MVCC / 快照一致性，
/// 不等写锁，保证 PLEG 不被阻塞（§4.3）。
///
/// 四个写方法（`put_sandbox` / `put_container` / `delete_sandbox` / `delete_container`）
/// 有默认实现：经 `transaction` 走单写事务，故后端只需实现读路径 + IPAM + `transaction`。
/// `delete_*` 删不存在返回 `Ok(())`（幂等），与 CRI `swallow_not_found` 双保险。
pub trait Store: Send + Sync {
    // ---- 读 ----
    fn get_sandbox(&self, id: &str) -> Result<oas_types::SandboxRecord, StoreError>;
    fn get_sandbox_by_uid(
        &self,
        pod_uid: &str,
    ) -> Result<Option<oas_types::SandboxRecord>, StoreError>;
    fn list_sandboxes(
        &self,
        filter: &oas_types::SandboxFilter,
    ) -> Result<Vec<oas_types::SandboxRecord>, StoreError>;
    fn get_container(&self, id: &str) -> Result<oas_types::ContainerRecord, StoreError>;
    fn list_containers(
        &self,
        filter: &oas_types::ContainerFilter,
    ) -> Result<Vec<oas_types::ContainerRecord>, StoreError>;

    // ---- IPAM 单操作（各自一个原子事务）----
    fn lease_ip(&self, cidr: &str) -> Result<oas_types::IpLease, StoreError>;
    fn release_ip(&self, lease: &oas_types::IpLease) -> Result<(), StoreError>;

    // ---- 事务 ----
    /// 单事务执行：建资源 + 落库的原子点（§3.6 事务边界）。闭包返回 `Err` 则整体回滚。
    fn transaction(
        &self,
        f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError>;

    // ---- 写（默认经 transaction）----
    fn put_sandbox(&self, r: &oas_types::SandboxRecord) -> Result<(), StoreError> {
        let r = r.clone();
        self.transaction(Box::new(move |t| t.put_sandbox(&r)))
    }
    fn put_container(&self, r: &oas_types::ContainerRecord) -> Result<(), StoreError> {
        let r = r.clone();
        self.transaction(Box::new(move |t| t.put_container(&r)))
    }
    fn delete_sandbox(&self, id: &str) -> Result<(), StoreError> {
        let id = id.to_string();
        self.transaction(Box::new(move |t| t.delete_sandbox(&id)))
    }
    fn delete_container(&self, id: &str) -> Result<(), StoreError> {
        let id = id.to_string();
        self.transaction(Box::new(move |t| t.delete_container(&id)))
    }
}

// ---- IPAM CIDR 解析（Memory/Redb 共用）------------------------------------

/// 解析 `a.b.c.d/prefix` → (网络基址 u32, prefix)。失败返回 `Other`。
pub(crate) fn parse_cidr(cidr: &str) -> Result<(u32, u8), StoreError> {
    let (ip, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| StoreError::Other(format!("bad cidr: {cidr}")))?;
    let octets: Vec<u8> = ip
        .split('.')
        .map(|s| s.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StoreError::Other(format!("bad cidr: {cidr}")))?;
    if octets.len() != 4 {
        return Err(StoreError::Other(format!("bad cidr: {cidr}")));
    }
    let ip_u32 = u32::from_be_bytes([octets[0], octets[1], octets[2], octets[3]]);
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| StoreError::Other(format!("bad cidr: {cidr}")))?;
    if prefix > 32 {
        return Err(StoreError::Other(format!("bad cidr: {cidr}")));
    }
    Ok((ip_u32, prefix))
}

pub(crate) fn ip_to_string(ip: u32) -> String {
    let b = ip.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

// ---- 过滤匹配（Memory/Redb 共用）------------------------------------------

pub(crate) fn matches_sandbox(s: &oas_types::SandboxRecord, f: &oas_types::SandboxFilter) -> bool {
    if let Some(id) = &f.id {
        if &s.sandbox_id != id {
            return false;
        }
    }
    if let Some(uid) = &f.pod_uid {
        if &s.pod_uid != uid {
            return false;
        }
    }
    if let Some(state) = f.state {
        if s.state != state {
            return false;
        }
    }
    for (k, v) in &f.label_selector {
        if s.labels.get(k) != Some(v) {
            return false;
        }
    }
    true
}

pub(crate) fn matches_container(
    c: &oas_types::ContainerRecord,
    f: &oas_types::ContainerFilter,
) -> bool {
    if let Some(id) = &f.id {
        if &c.container_id != id {
            return false;
        }
    }
    if let Some(sid) = &f.sandbox_id {
        if &c.sandbox_id != sid {
            return false;
        }
    }
    if let Some(state) = f.state {
        if c.state != state {
            return false;
        }
    }
    for (k, v) in &f.label_selector {
        if c.labels.get(k) != Some(v) {
            return false;
        }
    }
    true
}
