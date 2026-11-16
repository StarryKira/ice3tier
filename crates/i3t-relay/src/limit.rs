//! relay 的两种限速：转发的带宽上限，以及给没有会话的来源回错误帧的频率。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// 桶里最少能攒下这么多字节。带宽上限设得很低时，也要让一批合并发送的包整批过得去。
const MIN_BURST_BYTES: f64 = 128.0 * 1024.0;
/// 桶里最多攒多长时间的额度。
const BURST: Duration = Duration::from_millis(100);

/// 负载按多长的时间片统计。
const LOAD_SLICE: Duration = Duration::from_secs(1);
/// 负载的平滑系数：每个时间片新的测量值占这么多。负载惩罚要带平滑（白皮书 §7.5），不然节点会跟着瞬时的起伏来回换路。
const LOAD_SMOOTHING: f64 = 0.3;
/// 平滑之后的占用到了上限的这个比例算过载，回落到 [`OVERLOAD_CLEAR`] 以下才算恢复。两个门槛之间不变，免得反复通知。
const OVERLOAD_ENTER: f64 = 1.0;
const OVERLOAD_CLEAR: f64 = 0.8;

/// 令牌桶：按配置的带宽放行字节，超出的丢掉。relay 不排队，所以没有"等一会儿再发"。
///
/// 它同时统计负载：要转发的流量（放行的加上被它丢掉的）占带宽上限的比例，按秒统计再做平滑。
#[derive(Debug)]
pub(crate) struct Bandwidth {
    bytes_per_sec: f64,
    capacity: f64,
    available: f64,
    refilled: Instant,
    /// 当前时间片是什么时候开始的、里面来了多少字节。
    slice_started: Instant,
    slice_bytes: f64,
    /// 平滑之后的占用比例。
    utilisation: f64,
    overloaded: bool,
}

/// relay 的负载。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Load {
    /// 要转发的流量占带宽上限的比例，平滑过的。可以超过 1。
    pub(crate) utilisation: f64,
    pub(crate) overloaded: bool,
}

impl Bandwidth {
    pub(crate) fn new(mbps: u32, now: Instant) -> Self {
        let bytes_per_sec = f64::from(mbps) * 125_000.0;
        let capacity = (bytes_per_sec * BURST.as_secs_f64()).max(MIN_BURST_BYTES);
        Self {
            bytes_per_sec,
            capacity,
            available: capacity,
            refilled: now,
            slice_started: now,
            slice_bytes: 0.0,
            utilisation: 0.0,
            overloaded: false,
        }
    }

    /// 现在的负载。
    pub(crate) fn load(&mut self, now: Instant) -> Load {
        self.roll(now);
        Load { utilisation: self.utilisation, overloaded: self.overloaded }
    }

    /// 把已经过完的时间片算进平滑值。空闲了很久之后占用早就衰减到 0 了，不用一片一片地算。
    fn roll(&mut self, now: Instant) {
        const MAX_IDLE_SLICES: u32 = 64;
        let mut slices = 0;
        while now.saturating_duration_since(self.slice_started) >= LOAD_SLICE && slices < MAX_IDLE_SLICES {
            let rate = self.slice_bytes / LOAD_SLICE.as_secs_f64();
            self.utilisation += LOAD_SMOOTHING * (rate / self.bytes_per_sec.max(1.0) - self.utilisation);
            self.slice_bytes = 0.0;
            self.slice_started += LOAD_SLICE;
            slices += 1;
        }
        if slices == MAX_IDLE_SLICES {
            self.slice_started = now;
            self.utilisation = 0.0;
        }
        if self.utilisation >= OVERLOAD_ENTER {
            self.overloaded = true;
        } else if self.utilisation < OVERLOAD_CLEAR {
            self.overloaded = false;
        }
    }

    /// 要转发 `bytes` 字节：额度够就扣掉并返回 `true`，不够返回 `false`，这批包应该丢掉。
    pub(crate) fn admit(&mut self, bytes: usize, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.available = (self.available + elapsed * self.bytes_per_sec).min(self.capacity);
        self.refilled = now;
        // 一批包最大 64KB。真有超过 4GB 的，按"无论如何都不够"算。
        let wanted = u32::try_from(bytes).map_or(f64::INFINITY, f64::from);
        self.roll(now);
        if wanted.is_finite() {
            self.slice_bytes += wanted;
        }
        if wanted > self.available {
            return false;
        }
        self.available -= wanted;
        true
    }
}

/// 同一个来源地址多久才再回一个错误帧。
const REFUSAL_INTERVAL: Duration = Duration::from_secs(1);
/// 最多记多少个来源地址。满了之后陌生地址一律不回，直到旧的过期：错误帧只是个提示，不回也不影响正确性。
const MAX_TRACKED_SOURCES: usize = 4096;

/// 错误帧的限速。没有会话的来源是没有认证过的，回给它的东西必须又小又少。
#[derive(Debug, Default)]
pub(crate) struct RefusalLimiter {
    last_sent: HashMap<SocketAddr, Instant>,
}

impl RefusalLimiter {
    /// 现在能不能给 `to` 回一个错误帧。返回 `true` 就算回过了。
    pub(crate) fn allow(&mut self, to: SocketAddr, now: Instant) -> bool {
        if let Some(last) = self.last_sent.get_mut(&to) {
            if now.saturating_duration_since(*last) < REFUSAL_INTERVAL {
                return false;
            }
            *last = now;
            return true;
        }
        if self.last_sent.len() >= MAX_TRACKED_SOURCES {
            self.sweep(now);
            if self.last_sent.len() >= MAX_TRACKED_SOURCES {
                return false;
            }
        }
        self.last_sent.insert(to, now);
        true
    }

    /// 忘掉已经过了限速间隔的来源。
    pub(crate) fn sweep(&mut self, now: Instant) {
        self.last_sent.retain(|_, last| now.saturating_duration_since(*last) < REFUSAL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bandwidth_admits_the_configured_rate_and_drops_the_excess() {
        let start = Instant::now();
        // 8 Mbit/s 是每秒 1MB。
        let mut bandwidth = Bandwidth::new(8, start);
        let packet = 1400;
        let burst = (0..10_000).take_while(|_| bandwidth.admit(packet, start)).count();
        assert!((90..=95).contains(&burst), "开始时桶是满的，能过 128KB 左右：{burst}");
        assert!(!bandwidth.admit(packet, start));

        // 之后的一秒里放行的量应该就是配置的带宽。
        let mut admitted = 0;
        for millis in 1..=1000 {
            let now = start + Duration::from_millis(millis);
            while bandwidth.admit(packet, now) {
                admitted += packet;
            }
        }
        assert!((990_000..=1_010_000).contains(&admitted), "{admitted}");
    }

    #[test]
    fn idle_time_does_not_accumulate_beyond_the_burst() {
        let start = Instant::now();
        let mut bandwidth = Bandwidth::new(800, start);
        assert!(bandwidth.admit(1400, start));
        let later = start + Duration::from_secs(3600);
        let burst = (0..1_000_000).take_while(|_| bandwidth.admit(1400, later)).count() * 1400;
        // 800 Mbit/s 的 100ms 是 10MB。
        assert!((9_990_000..=10_000_000).contains(&burst), "{burst}");
    }

    /// 每毫秒要转发 `bytes_per_ms` 字节，持续 `seconds` 秒，返回最后的负载。上限是 8 Mbit/s，也就是每毫秒 1000 字节。
    fn offer(bandwidth: &mut Bandwidth, start: Instant, from_ms: u64, seconds: u64, bytes_per_ms: usize) -> Load {
        for millis in from_ms..from_ms + seconds * 1000 {
            bandwidth.admit(bytes_per_ms, start + Duration::from_millis(millis));
        }
        bandwidth.load(start + Duration::from_millis(from_ms + seconds * 1000))
    }

    #[test]
    fn load_is_the_smoothed_share_of_the_cap_that_is_being_offered() {
        let start = Instant::now();
        let mut bandwidth = Bandwidth::new(8, start);
        assert_eq!(bandwidth.load(start), Load { utilisation: 0.0, overloaded: false });

        // 一半的带宽：平滑值逐步靠近 0.5，不是一下子跳过去。
        let after_one = offer(&mut bandwidth, start, 0, 1, 500);
        assert!((0.1..0.2).contains(&after_one.utilisation), "{after_one:?}");
        let settled = offer(&mut bandwidth, start, 1000, 20, 500);
        assert!((0.48..=0.52).contains(&settled.utilisation), "{settled:?}");
        assert!(!settled.overloaded);

        // 停了之后慢慢回落，很久没有流量就是 0。
        let cooling = bandwidth.load(start + Duration::from_secs(23));
        assert!(cooling.utilisation < settled.utilisation && cooling.utilisation > 0.1, "{cooling:?}");
        assert_eq!(bandwidth.load(start + Duration::from_secs(3600)).utilisation, 0.0);
    }

    #[test]
    fn overload_is_declared_at_the_cap_and_cleared_only_well_below_it() {
        let start = Instant::now();
        let mut bandwidth = Bandwidth::new(8, start);
        // 要转发的是上限的一倍半：被丢掉的也算在负载里。
        let flooded = offer(&mut bandwidth, start, 0, 20, 1500);
        assert!(flooded.overloaded && flooded.utilisation > 1.4, "{flooded:?}");
        // 回落到上限的九成：还在两个门槛之间，仍然算过载。
        let easing = offer(&mut bandwidth, start, 20_000, 20, 900);
        assert!(easing.overloaded && easing.utilisation < 1.0, "{easing:?}");
        let recovered = offer(&mut bandwidth, start, 40_000, 20, 500);
        assert!(!recovered.overloaded, "{recovered:?}");
    }

    #[test]
    fn refusals_are_limited_per_source_address() {
        let start = Instant::now();
        let mut limiter = RefusalLimiter::default();
        let (a, b): (SocketAddr, SocketAddr) = ("203.0.113.7:1".parse().unwrap(), "203.0.113.7:2".parse().unwrap());
        assert!(limiter.allow(a, start));
        assert!(!limiter.allow(a, start + Duration::from_millis(999)));
        assert!(limiter.allow(b, start), "别的来源不受影响");
        assert!(limiter.allow(a, start + REFUSAL_INTERVAL));
    }

    #[test]
    fn a_flood_of_sources_cannot_grow_the_table_without_bound() {
        let start = Instant::now();
        let mut limiter = RefusalLimiter::default();
        let source = |index: usize| SocketAddr::from(([10, 0, u8::try_from(index / 256).unwrap(), u8::try_from(index % 256).unwrap()], 9));
        let allowed = (0..MAX_TRACKED_SOURCES + 100).filter(|&index| limiter.allow(source(index), start)).count();
        assert_eq!(allowed, MAX_TRACKED_SOURCES);
        assert_eq!(limiter.last_sent.len(), MAX_TRACKED_SOURCES);
        assert!(limiter.allow(source(MAX_TRACKED_SOURCES + 1), start + REFUSAL_INTERVAL), "旧的过期之后又有位置了");
    }
}
