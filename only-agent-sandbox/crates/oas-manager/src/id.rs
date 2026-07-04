//! ID 分配：进程内单调计数器，无新依赖。
//!
//! 跨进程 / 崩溃恢复的持久化 `sandbox_id` 分配留待后续（迁入 store `meta` 表）。

use std::sync::atomic::{AtomicU64, Ordering};

use oas_types::SandboxId;

/// sandbox_id / container_id 生成器。
pub struct IdGenerator {
    sb: AtomicU64,
    ct: AtomicU64,
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self {
            sb: AtomicU64::new(1),
            ct: AtomicU64::new(1),
        }
    }
}

impl IdGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sandbox_id(&self) -> SandboxId {
        let n = self.sb.fetch_add(1, Ordering::Relaxed);
        SandboxId(format!("sb-{n:016x}"))
    }

    pub fn container_id(&self) -> String {
        let n = self.ct.fetch_add(1, Ordering::Relaxed);
        format!("ct-{n:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn sandbox_ids_unique() {
        let g = IdGenerator::new();
        let set: HashSet<String> = (0..1000).map(|_| g.sandbox_id().0).collect();
        assert_eq!(set.len(), 1000);
        assert!(g.sandbox_id().as_str().starts_with("sb-"));
    }

    #[test]
    fn container_ids_unique_and_prefixed() {
        let g = IdGenerator::new();
        let a = g.container_id();
        let b = g.container_id();
        assert_ne!(a, b);
        assert!(a.starts_with("ct-") && b.starts_with("ct-"));
    }
}
