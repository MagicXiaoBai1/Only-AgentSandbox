//! 内存版 `Store`：HashMap 后端，无依赖、确定性，用于测试与开发。

use std::collections::HashMap;
use std::sync::Mutex;

use oas_types::{
    ContainerFilter, ContainerRecord, IpLease, SandboxFilter, SandboxRecord,
};

use crate::{ip_to_string, matches_container, matches_sandbox, parse_cidr, Store, StoreError, Txn};

/// 内存表集合。
#[derive(Clone, Default)]
struct MemoryTables {
    sandboxes: HashMap<String, SandboxRecord>,
    containers: HashMap<String, ContainerRecord>,
    /// key = ip。
    ipam: HashMap<String, IpLease>,
}

/// 内存 `Store`。
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<MemoryTables>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 事务句柄：直接操作 `&mut MemoryTables`；回滚靠调用方快照恢复。
struct MemoryTxn<'a> {
    tables: &'a mut MemoryTables,
}

impl<'a> Txn for MemoryTxn<'a> {
    fn put_sandbox(&mut self, r: &SandboxRecord) -> Result<(), StoreError> {
        self.tables
            .sandboxes
            .insert(r.sandbox_id.clone(), r.clone());
        Ok(())
    }
    fn put_container(&mut self, r: &ContainerRecord) -> Result<(), StoreError> {
        self.tables
            .containers
            .insert(r.container_id.clone(), r.clone());
        Ok(())
    }
    fn delete_sandbox(&mut self, id: &str) -> Result<(), StoreError> {
        self.tables.sandboxes.remove(id);
        Ok(())
    }
    fn delete_container(&mut self, id: &str) -> Result<(), StoreError> {
        self.tables.containers.remove(id);
        Ok(())
    }
    fn lease_ip(&mut self, cidr: &str) -> Result<IpLease, StoreError> {
        lease_ip_in(&mut self.tables.ipam, cidr)
    }
    fn release_ip(&mut self, lease: &IpLease) -> Result<(), StoreError> {
        self.tables.ipam.remove(&lease.ip);
        Ok(())
    }
}

impl Store for MemoryStore {
    fn get_sandbox(&self, id: &str) -> Result<SandboxRecord, StoreError> {
        let tables = self.inner.lock().unwrap();
        tables
            .sandboxes
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("sandbox {id}")))
    }

    fn get_sandbox_by_uid(&self, pod_uid: &str) -> Result<Option<SandboxRecord>, StoreError> {
        let tables = self.inner.lock().unwrap();
        Ok(tables
            .sandboxes
            .values()
            .find(|s| s.pod_uid == pod_uid)
            .cloned())
    }

    fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<SandboxRecord>, StoreError> {
        let tables = self.inner.lock().unwrap();
        let mut out: Vec<SandboxRecord> = tables
            .sandboxes
            .values()
            .filter(|s| matches_sandbox(s, filter))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.sandbox_id.cmp(&b.sandbox_id));
        Ok(out)
    }

    fn get_container(&self, id: &str) -> Result<ContainerRecord, StoreError> {
        let tables = self.inner.lock().unwrap();
        tables
            .containers
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("container {id}")))
    }

    fn list_containers(&self, filter: &ContainerFilter) -> Result<Vec<ContainerRecord>, StoreError> {
        let tables = self.inner.lock().unwrap();
        let mut out: Vec<ContainerRecord> = tables
            .containers
            .values()
            .filter(|c| matches_container(c, filter))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.container_id.cmp(&b.container_id));
        Ok(out)
    }

    fn lease_ip(&self, cidr: &str) -> Result<IpLease, StoreError> {
        let mut tables = self.inner.lock().unwrap();
        lease_ip_in(&mut tables.ipam, cidr)
    }

    fn release_ip(&self, lease: &IpLease) -> Result<(), StoreError> {
        let mut tables = self.inner.lock().unwrap();
        tables.ipam.remove(&lease.ip);
        Ok(())
    }

    fn transaction(
        &self,
        f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError> {
        let mut guard = self.inner.lock().unwrap();
        // 快照用于回滚（MemoryTables 全是 Clone 的记录，量小可接受）。
        let snapshot = guard.clone();
        let mut txn = MemoryTxn {
            tables: &mut *guard,
        };
        let res = f(&mut txn);
        if res.is_err() {
            // 回滚：丢弃事务内改动。
            *guard = snapshot;
        }
        res
    }
}

// ---- IPAM 分配（Memory/Redb 共用语义）-------------------------------------

/// 在给定 ipam 占用集上分配下一个可用 IP。占用集以 `used` 传入（key=ip），
/// 命中即写入并返回租约；`sandbox_id` 留空，由调用方（net 层）填充。
pub(crate) fn lease_ip_in(
    ipam: &mut HashMap<String, IpLease>,
    cidr: &str,
) -> Result<IpLease, StoreError> {
    let (base, prefix) = parse_cidr(cidr)?;
    if prefix == 32 {
        return Err(StoreError::Db(format!("ipam exhausted: {cidr}")));
    }
    let host_bits = 32 - prefix;
    let count: u64 = 1u64 << host_bits;
    let mask = if prefix == 0 { 0u32 } else { (!0u32) << host_bits };
    let network = base & mask;
    // host_bits >= 2 时跳过网络地址(.0)与广播(末地址)；/31 用两个地址。
    let (start, end) = if host_bits >= 2 {
        (1u64, count - 1)
    } else {
        (0u64, count)
    };
    for off in start..end {
        let candidate = network.wrapping_add(off as u32);
        let ip = ip_to_string(candidate);
        if !ipam.contains_key(&ip) {
            let lease = IpLease {
                ip: ip.clone(),
                cidr: cidr.to_string(),
                sandbox_id: String::new(),
            };
            ipam.insert(ip, lease.clone());
            return Ok(lease);
        }
    }
    Err(StoreError::Db(format!("ipam exhausted: {cidr}")))
}
