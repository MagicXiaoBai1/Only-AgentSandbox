//! 时钟抽象：让 `created_at` / `started_at` / `finished_at` 在测试里确定。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// Unix 秒时钟。
pub trait Clock: Send + Sync {
    fn now_unix_secs(&self) -> i64;
}

/// 系统真实时钟（生产用）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_secs(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// 假时钟（测试用）：可手动推进。
#[derive(Debug, Clone)]
pub struct FakeClock {
    now: Arc<AtomicI64>,
}

impl FakeClock {
    pub fn new(start: i64) -> Self {
        Self {
            now: Arc::new(AtomicI64::new(start)),
        }
    }
    /// 设为指定时刻。
    pub fn advance_to(&self, t: i64) {
        self.now.store(t, Ordering::Relaxed);
    }
    /// 推进 1 秒并返回新值。
    pub fn tick(&self) -> i64 {
        let prev = self.now.fetch_add(1, Ordering::Relaxed);
        prev + 1
    }
}

impl Clock for FakeClock {
    fn now_unix_secs(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advance_and_tick() {
        let c = FakeClock::new(100);
        assert_eq!(c.now_unix_secs(), 100);
        c.advance_to(200);
        assert_eq!(c.now_unix_secs(), 200);
        assert_eq!(c.tick(), 201);
        assert_eq!(c.now_unix_secs(), 201);
    }

    #[test]
    fn system_clock_is_positive() {
        let c = SystemClock;
        assert!(c.now_unix_secs() > 1_700_000_000);
    }
}
