//! redb 后端 `Store`（§3.6 五张表中的 sandbox / container / ipam）。
//!
//! 记录经 `serde_json` 序列化为 `&[u8]` 存入；ipam 表 key=ip、value=cidr。
//! 表在构造时一次性建好，避免读路径撞 `TableDoesNotExist`。事务回滚靠 redb
//! `WriteTransaction` drop-without-commit 原生语义。

use std::collections::HashSet;

use oas_types::{
    ContainerFilter, ContainerRecord, IpLease, SandboxFilter, SandboxRecord,
};
use redb::{ReadableDatabase, ReadableTable};

use crate::{ip_to_string, matches_container, matches_sandbox, parse_cidr, Store, StoreError, Txn};

const SANDBOX: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("sandbox");
const CONTAINER: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("container");
const IPAM: redb::TableDefinition<&str, &str> = redb::TableDefinition::new("ipam");

/// redb 持久化 `Store`。
pub struct RedbStore {
    db: redb::Database,
}

fn map_err<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Db(e.to_string())
}

impl RedbStore {
    /// 打开（或创建）文件型 redb 数据库。
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let db = redb::Database::create(path).map_err(map_err)?;
        Self::from_db(db)
    }

    /// 内存型 redb（测试用，每实例独立、无文件）。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let db = redb::Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(map_err)?;
        Self::from_db(db)
    }

    /// 由已打开的 `Database` 构造，并预建全部表。
    pub fn from_db(db: redb::Database) -> Result<Self, StoreError> {
        let wtxn = db.begin_write().map_err(map_err)?;
        {
            let _ = wtxn.open_table(SANDBOX).map_err(map_err)?;
            let _ = wtxn.open_table(CONTAINER).map_err(map_err)?;
            let _ = wtxn.open_table(IPAM).map_err(map_err)?;
        }
        wtxn.commit().map_err(map_err)?;
        Ok(Self { db })
    }
}

/// 事务句柄：包裹 `&mut WriteTransaction`，方法各自 `open_table`。
struct RedbTxn<'a> {
    wtxn: &'a mut redb::WriteTransaction,
}

impl<'a> Txn for RedbTxn<'a> {
    fn put_sandbox(&mut self, r: &SandboxRecord) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(r).map_err(|e| StoreError::Db(e.to_string()))?;
        let mut t = self.wtxn.open_table(SANDBOX).map_err(map_err)?;
        t.insert(r.sandbox_id.as_str(), bytes.as_slice())
            .map_err(map_err)?;
        Ok(())
    }
    fn put_container(&mut self, r: &ContainerRecord) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(r).map_err(|e| StoreError::Db(e.to_string()))?;
        let mut t = self.wtxn.open_table(CONTAINER).map_err(map_err)?;
        t.insert(r.container_id.as_str(), bytes.as_slice())
            .map_err(map_err)?;
        Ok(())
    }
    fn delete_sandbox(&mut self, id: &str) -> Result<(), StoreError> {
        let mut t = self.wtxn.open_table(SANDBOX).map_err(map_err)?;
        t.remove(id).map_err(map_err)?;
        Ok(())
    }
    fn delete_container(&mut self, id: &str) -> Result<(), StoreError> {
        let mut t = self.wtxn.open_table(CONTAINER).map_err(map_err)?;
        t.remove(id).map_err(map_err)?;
        Ok(())
    }
    fn lease_ip(&mut self, cidr: &str) -> Result<IpLease, StoreError> {
        let mut t = self.wtxn.open_table(IPAM).map_err(map_err)?;
        lease_ip_redb(&mut t, cidr)
    }
    fn release_ip(&mut self, lease: &IpLease) -> Result<(), StoreError> {
        let mut t = self.wtxn.open_table(IPAM).map_err(map_err)?;
        t.remove(lease.ip.as_str()).map_err(map_err)?;
        Ok(())
    }
}

impl Store for RedbStore {
    fn get_sandbox(&self, id: &str) -> Result<SandboxRecord, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(SANDBOX).map_err(map_err)?;
        match t.get(id).map_err(map_err)? {
            Some(g) => serde_json::from_slice(g.value())
                .map_err(|e| StoreError::Db(e.to_string())),
            None => Err(StoreError::NotFound(format!("sandbox {id}"))),
        }
    }

    fn get_sandbox_by_uid(&self, pod_uid: &str) -> Result<Option<SandboxRecord>, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(SANDBOX).map_err(map_err)?;
        for item in t.iter().map_err(map_err)? {
            let (_k, v) = item.map_err(map_err)?;
            let rec: SandboxRecord =
                serde_json::from_slice(v.value()).map_err(|e| StoreError::Db(e.to_string()))?;
            if rec.pod_uid == pod_uid {
                return Ok(Some(rec));
            }
        }
        Ok(None)
    }

    fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<SandboxRecord>, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(SANDBOX).map_err(map_err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(map_err)? {
            let (_k, v) = item.map_err(map_err)?;
            let rec: SandboxRecord =
                serde_json::from_slice(v.value()).map_err(|e| StoreError::Db(e.to_string()))?;
            if matches_sandbox(&rec, filter) {
                out.push(rec);
            }
        }
        out.sort_by(|a, b| a.sandbox_id.cmp(&b.sandbox_id));
        Ok(out)
    }

    fn get_container(&self, id: &str) -> Result<ContainerRecord, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(CONTAINER).map_err(map_err)?;
        match t.get(id).map_err(map_err)? {
            Some(g) => serde_json::from_slice(g.value())
                .map_err(|e| StoreError::Db(e.to_string())),
            None => Err(StoreError::NotFound(format!("container {id}"))),
        }
    }

    fn list_containers(&self, filter: &ContainerFilter) -> Result<Vec<ContainerRecord>, StoreError> {
        let rtxn = self.db.begin_read().map_err(map_err)?;
        let t = rtxn.open_table(CONTAINER).map_err(map_err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(map_err)? {
            let (_k, v) = item.map_err(map_err)?;
            let rec: ContainerRecord =
                serde_json::from_slice(v.value()).map_err(|e| StoreError::Db(e.to_string()))?;
            if matches_container(&rec, filter) {
                out.push(rec);
            }
        }
        out.sort_by(|a, b| a.container_id.cmp(&b.container_id));
        Ok(out)
    }

    fn lease_ip(&self, cidr: &str) -> Result<IpLease, StoreError> {
        let wtxn = self.db.begin_write().map_err(map_err)?;
        let result = {
            let mut t = wtxn.open_table(IPAM).map_err(map_err)?;
            lease_ip_redb(&mut t, cidr)
        };
        match result {
            Ok(lease) => {
                wtxn.commit().map_err(map_err)?;
                Ok(lease)
            }
            Err(e) => Err(e), // wtxn drop → 回滚
        }
    }

    fn release_ip(&self, lease: &IpLease) -> Result<(), StoreError> {
        let wtxn = self.db.begin_write().map_err(map_err)?;
        {
            let mut t = wtxn.open_table(IPAM).map_err(map_err)?;
            t.remove(lease.ip.as_str()).map_err(map_err)?;
        }
        wtxn.commit().map_err(map_err)?;
        Ok(())
    }

    fn transaction(
        &self,
        f: Box<dyn FnOnce(&mut dyn Txn) -> Result<(), StoreError>>,
    ) -> Result<(), StoreError> {
        let mut wtxn = self.db.begin_write().map_err(map_err)?;
        let result = {
            let mut txn = RedbTxn { wtxn: &mut wtxn };
            f(&mut txn)
        };
        match result {
            Ok(()) => {
                wtxn.commit().map_err(map_err)?;
                Ok(())
            }
            Err(e) => Err(e), // wtxn drop → 回滚
        }
    }
}

/// 在 ipam 表上分配下一个可用 IP（事务内）。命中即 insert；耗尽返回 `Db`。
fn lease_ip_redb(
    t: &mut redb::Table<'_, &str, &str>,
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
    let (start, end) = if host_bits >= 2 {
        (1u64, count - 1)
    } else {
        (0u64, count)
    };

    // 收集已占用的 ip。
    let mut used: HashSet<String> = HashSet::new();
    for item in t.iter().map_err(map_err)? {
        let (k, _v) = item.map_err(map_err)?;
        used.insert(k.value().to_string());
    }

    for off in start..end {
        let candidate = network.wrapping_add(off as u32);
        let ip = ip_to_string(candidate);
        if !used.contains(&ip) {
            t.insert(ip.as_str(), cidr).map_err(map_err)?;
            return Ok(IpLease {
                ip,
                cidr: cidr.to_string(),
                sandbox_id: String::new(),
            });
        }
    }
    Err(StoreError::Db(format!("ipam exhausted: {cidr}")))
}
