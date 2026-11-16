//! 计数和水位：节点以后上报、在 WebUI 上显示的东西。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 只增不减的计数。
#[derive(Debug, Default)]
pub(crate) struct Counter(AtomicU64);

impl Counter {
    pub(crate) fn add(&self, n: usize) {
        // usize 不会比 u64 宽，这里不会截断。
        self.0.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub(crate) fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// 有进有出的水位，用 [`Gauge`] 守卫进出。
#[derive(Debug, Default)]
pub(crate) struct Level(Arc<AtomicU64>);

impl Level {
    pub(crate) fn enter(&self) -> Gauge {
        self.0.fetch_add(1, Ordering::Relaxed);
        Gauge(Arc::clone(&self.0))
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// 占着水位里的一个数，丢弃时减回去。
#[derive(Debug)]
pub(crate) struct Gauge(Arc<AtomicU64>);

impl Drop for Gauge {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) in_use: Level,
    pub(crate) dialling: Level,
    pub(crate) sealed_bytes: Counter,
    pub(crate) raw_bytes: Counter,
    pub(crate) claims: Counter,
    pub(crate) claims_waited: Counter,
    pub(crate) claim_wait_micros: Counter,
    pub(crate) claims_failed: Counter,
    pub(crate) dial_failures: Counter,
    pub(crate) handshake_deadline: Counter,
    pub(crate) handshake_unknown_key: Counter,
    pub(crate) handshake_tls: Counter,
    pub(crate) handshake_protocol: Counter,
    pub(crate) handshake_busy: Counter,
    pub(crate) refused: Counter,
}

/// lane 的状况，[`crate::Lanes::status`] 给出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LaneStatus {
    /// 对端拨来、停在本节点等本节点开流时取用的空闲 lane。
    pub idle: u64,
    /// 本节点拨好、留给自己开流的空闲 lane。
    pub warm: u64,
    /// 本节点拨出去、停在对端那里等对端取用的 lane（按本节点所知；对端刚取走的要过半个往返才知道）。
    pub parked_at_peers: u64,
    /// 正在承载流的 lane，两个方向都算。
    pub in_use: u64,
    /// 正在拨号（连接或握手）的 lane。
    pub dialling: u64,
    /// 从对端停在这里的 lane 里取用的次数。
    pub claims: u64,
    /// 其中池是空的、要等补拨的次数，和一共等了多久。
    pub claims_waited: u64,
    pub claim_wait: Duration,
    /// 取不到 lane（池空等满了、控制 lane 不在、对端要走了）、调用方只能改走共用连接的次数。
    pub claims_failed: u64,
    /// 本节点拨号失败的次数。
    pub dial_failures: u64,
    /// 别人拨进来、没有成功的握手，按原因分。
    pub handshake_failures: HandshakeFailures,
    /// 握手成功、但 lane 因为用途或者数量不对被关掉的次数（旧的 epoch、超过上限、没有为对端打开）。
    pub refused: u64,
    /// 经 lane 的 TLS 收发的流数据字节数（两个方向合计）。
    pub sealed_bytes: u64,
    /// passthrough 时经 [`crate::LaneSend`] / [`crate::LaneRecv`] 在用户态搬的原始字节数。用 splice 搬的另外在 [`crate::splice::Splicer`] 里数。
    pub passthrough_bytes: u64,
}

/// 拨进来的握手失败的原因。这些都是外人能造成的，所以只计数，每一次只记 debug 日志。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HandshakeFailures {
    /// 没有在时限之内完成（包括连上来什么都不发的）。
    pub deadline: u64,
    /// 对端的公钥不认识或者已经吊销。
    pub unknown_key: u64,
    /// 别的 TLS 错误。
    pub tls: u64,
    /// 握手之后的 HELLO 不对，或者根本不是 TLS。
    pub protocol: u64,
    /// 还没握手完的连接太多（同一来源或者全部），被挤掉或者拒绝的。
    pub busy: u64,
}

impl Counters {
    pub(crate) fn snapshot(&self) -> LaneStatus {
        LaneStatus {
            in_use: self.in_use.get(),
            dialling: self.dialling.get(),
            claims: self.claims.get(),
            claims_waited: self.claims_waited.get(),
            claim_wait: Duration::from_micros(self.claim_wait_micros.get()),
            claims_failed: self.claims_failed.get(),
            dial_failures: self.dial_failures.get(),
            handshake_failures: HandshakeFailures {
                deadline: self.handshake_deadline.get(),
                unknown_key: self.handshake_unknown_key.get(),
                tls: self.handshake_tls.get(),
                protocol: self.handshake_protocol.get(),
                busy: self.handshake_busy.get(),
            },
            refused: self.refused.get(),
            sealed_bytes: self.sealed_bytes.get(),
            passthrough_bytes: self.raw_bytes.get(),
            ..LaneStatus::default()
        }
    }
}
