//! per-key 串行化锁（§3.2 / §5）。
//!
//! 锁粒度 per-`sandbox_id`；容器写锁父 sandbox（固定 sandbox→container 顺序）；
//! 读路径不加锁（§4.3）。锁表项不回收（少量泄漏，后续可周期压实）。

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use tokio::sync::{Mutex, OwnedMutexGuard};

/// per-key 异步互斥锁表。
#[derive(Default)]
pub struct PerKeyLock {
    map: StdMutex<HashMap<String, std::sync::Arc<Mutex<()>>>>,
}

impl PerKeyLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取 `key` 对应的锁；同 key 串行，不同 key 并行。返回 `OwnedMutexGuard`。
    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let arc = {
            let mut m = self.map.lock().unwrap();
            m.entry(key.to_string())
                .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
                .clone()
        };
        arc.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn same_key_serializes() {
        let lk = Arc::new(PerKeyLock::new());
        let g = lk.lock("k").await;

        // 另一任务锁同一 key 应被阻塞：spawn 后短等仍持锁。
        let lk2 = lk.clone();
        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hit2 = hit.clone();
        let join = tokio::spawn(async move {
            let _g2 = lk2.lock("k").await;
            hit2.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!hit.load(std::sync::atomic::Ordering::Relaxed));

        drop(g); // 释放后另一任务应能拿到
        join.await.unwrap();
        assert!(hit.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn different_keys_parallel() {
        let lk = Arc::new(PerKeyLock::new());
        let g1 = lk.lock("a").await;
        // 不同 key 不应被 g1 阻塞：立即拿到。
        let g2 = lk.lock("b").await;
        drop(g1);
        drop(g2);
    }
}
