//! 给拥塞控制器套一层计数：发了多少、送达多少、丢了多少、窗口和往返时间是多少，每秒在日志里记一行。
//!
//! QUIC 库的统计只能从连接对象上取，连接层现在没有把它们往外报。对比两种拥塞控制、排查某条连接为什么慢时，
//! 需要的正是这几个数，所以在这一层自己数。计数只是几次加法；日志在 debug 级别，这一秒里没有收到确认就不记。

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use i3t_model::netmap::Congestion;
use quinn::congestion::{Controller, ControllerFactory, ControllerMetrics};
use quinn_proto::RttEstimator;

use super::Algorithm;

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// 日志里用来区分各条连接的编号，进程内递增。
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 套着计数的控制器工厂。
pub(super) struct Metered {
    algorithm: Algorithm,
    inner: Arc<dyn ControllerFactory + Send + Sync>,
}

impl Metered {
    pub(super) fn new(algorithm: Algorithm, inner: Arc<dyn ControllerFactory + Send + Sync>) -> Self {
        Self { algorithm, inner }
    }
}

impl ControllerFactory for Metered {
    fn build(self: Arc<Self>, now: Instant, current_mtu: u16) -> Box<dyn Controller> {
        Box::new(Meter {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            algorithm: self.algorithm,
            inner: Arc::clone(&self.inner).build(now, current_mtu),
            totals: Totals::default(),
            next_report: now + REPORT_INTERVAL,
            rtt_max: Duration::ZERO,
            is_dirty: false,
        })
    }
}

/// 从连接建立起累计的字节数。
#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    sent: u64,
    acked: u64,
    lost: u64,
}

struct Meter {
    id: u64,
    algorithm: Algorithm,
    inner: Box<dyn Controller>,
    totals: Totals,
    next_report: Instant,
    /// 上一次记日志之后见过的最大的平滑往返时间。
    rtt_max: Duration,
    /// 上一次记日志之后有没有新的确认。
    is_dirty: bool,
}

impl Controller for Meter {
    fn on_sent(&mut self, now: Instant, bytes: u64, last_packet_number: u64) {
        self.totals.sent += bytes;
        self.inner.on_sent(now, bytes, last_packet_number);
    }

    fn on_ack(&mut self, now: Instant, sent: Instant, bytes: u64, app_limited: bool, rtt: &RttEstimator) {
        self.totals.acked += bytes;
        self.is_dirty = true;
        self.inner.on_ack(now, sent, bytes, app_limited, rtt);
        self.rtt_max = self.rtt_max.max(rtt.get());
        if now >= self.next_report {
            self.next_report = now + REPORT_INTERVAL;
            self.is_dirty = false;
            tracing::debug!(
                conn = self.id,
                algorithm = %Congestion::from(self.algorithm),
                sent = self.totals.sent,
                acked = self.totals.acked,
                lost = self.totals.lost,
                window = self.inner.window(),
                rtt_us = rtt.get().as_micros(),
                rtt_max_us = self.rtt_max.as_micros(),
                min_rtt_us = rtt.min().as_micros(),
                "拥塞控制统计"
            );
            self.rtt_max = Duration::ZERO;
        }
    }

    fn on_end_acks(&mut self, now: Instant, in_flight: u64, app_limited: bool, largest_packet_num_acked: Option<u64>) {
        self.inner.on_end_acks(now, in_flight, app_limited, largest_packet_num_acked);
    }

    fn on_congestion_event(&mut self, now: Instant, sent: Instant, is_persistent_congestion: bool, lost_bytes: u64) {
        self.totals.lost += lost_bytes;
        self.inner.on_congestion_event(now, sent, is_persistent_congestion, lost_bytes);
    }

    fn on_mtu_update(&mut self, new_mtu: u16) {
        self.inner.on_mtu_update(new_mtu);
    }

    fn window(&self) -> u64 {
        self.inner.window()
    }

    fn metrics(&self) -> ControllerMetrics {
        self.inner.metrics()
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            algorithm: self.algorithm,
            inner: self.inner.clone_box(),
            totals: self.totals,
            next_report: self.next_report,
            rtt_max: self.rtt_max,
            is_dirty: self.is_dirty,
        })
    }

    fn initial_window(&self) -> u64 {
        self.inner.initial_window()
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        if self.is_dirty {
            tracing::debug!(
                conn = self.id,
                algorithm = %Congestion::from(self.algorithm),
                sent = self.totals.sent,
                acked = self.totals.acked,
                lost = self.totals.lost,
                "拥塞控制统计（连接结束）"
            );
        }
    }
}
