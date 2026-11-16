//! 验签名的额度。路径层收到的探测帧和矩阵行都带签名，验签名不便宜，而来源地址可以伪造；
//! 所以验之前先从一个令牌桶里扣一次，扣不到就不验。

use std::time::Instant;

/// 令牌桶：每秒补 `rate` 个，最多攒 `burst` 个，开始时是满的。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    rate: f64,
    burst: f64,
    tokens: f64,
    refilled: Instant,
}

impl Budget {
    pub(crate) const fn new(rate: f64, burst: f64, now: Instant) -> Self {
        Self { rate, burst, tokens: burst, refilled: now }
    }

    /// 扣一个。扣不到返回 `false`。
    pub(crate) fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.refilled = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_budget_allows_its_burst_and_then_refills_at_its_rate() {
        let start = Instant::now();
        let mut budget = Budget::new(50.0, 50.0, start);
        let burst = (0..1000).take_while(|_| budget.take(start)).count();
        assert_eq!(burst, 50);
        assert!(!budget.take(start));
        assert!(budget.take(start + Duration::from_millis(20)), "20ms 攒出一个");
        assert!(!budget.take(start + Duration::from_millis(21)));
        let later = start + Duration::from_secs(3600);
        assert_eq!((0..1000).take_while(|_| budget.take(later)).count(), 50, "闲着不会攒得比上限多");
    }
}
