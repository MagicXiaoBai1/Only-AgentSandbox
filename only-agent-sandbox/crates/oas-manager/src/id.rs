//! ID 分配：进程内单调计数器，无新依赖。
//!
//! 跨进程 / 崩溃恢复的持久化 `vm_id` 分配留待后续（迁入 store `meta` 表）。

use std::sync::atomic::{AtomicU64, Ordering};

/// sandbox_id / container_id / vm_id 生成器。
pub struct IdGenerator {
    sb: AtomicU64,
    ct: AtomicU64,
    vm: AtomicU64,
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self {
            sb: AtomicU64::new(1),
            ct: AtomicU64::new(1),
            vm: AtomicU64::new(1), // 避开 VmId(0) 哨兵
        }
    }
}

impl IdGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sandbox_id(&self) -> String {
        let n = self.sb.fetch_add(1, Ordering::Relaxed);
        format!("sb-{n:016x}")
    }

    pub fn container_id(&self) -> String {
        let n = self.ct.fetch_add(1, Ordering::Relaxed);
        format!("ct-{n:016x}")
    }

    pub fn vm_id(&self) -> u64 {
        self.vm.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn sandbox_ids_unique() {
        let g = IdGenerator::new();
        let set: HashSet<String> = (0..1000).map(|_| g.sandbox_id()).collect();
        assert_eq!(set.len(), 1000);
        assert!(g.sandbox_id().starts_with("sb-"));
    }

    #[test]
    fn container_ids_unique_and_prefixed() {
        let g = IdGenerator::new();
        let a = g.container_id();
        let b = g.container_id();
        assert_ne!(a, b);
        assert!(a.starts_with("ct-") && b.starts_with("ct-"));
    }

    #[test]
    fn vm_id_starts_at_one_and_increments() {
        let g = IdGenerator::new();
        assert_eq!(g.vm_id(), 1);
        assert_eq!(g.vm_id(), 2);
        assert_eq!(g.vm_id(), 3);
    }
}
