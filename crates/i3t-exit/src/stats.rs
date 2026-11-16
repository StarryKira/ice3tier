//! 出口的计数。

use std::sync::atomic::{AtomicU64, Ordering};

use i3t_flow::Summary;
use i3t_proto::flow::{Attribution, Status};

/// 出口到现在为止的计数。流的字节数在每条流结束时才加进来，数据报的随到随加。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExitStats {
    /// 正在处理的流和数据报上下文：从读到 OPEN 到结束。
    pub active: u64,
    /// 收到过的建流请求，包括建立数据报上下文的。
    pub total: u64,
    /// 从发起方搬到目标的字节，流的数据和数据报的载荷都算。
    pub bytes_to_target: u64,
    /// 从目标搬回发起方的字节。
    pub bytes_from_target: u64,
    /// 替发起方发往目标的 UDP 报文。
    pub datagrams_to_target: u64,
    /// 从目标送回发起方的 UDP 报文。
    pub datagrams_from_target: u64,
    /// 因为目标没有通过检查（访问规则、出站策略等）而丢掉的数据报。每个这样的目标另外在下面按原因计一次。
    pub datagrams_refused: u64,
    /// 没有建成的请求里，归因于目标的：域名不存在、拒绝连接、不可达、超时。
    pub failed_by_target: u64,
    /// 没有建成的请求里，被访问规则或者出站策略拒绝的。
    pub refused: u64,
    /// 没有建成的请求里，出口自己出问题的。只有这一项说明出口不健康。
    pub failed_by_exit: u64,
    /// 建成之后没有正常结束的流和数据报上下文：某一头异常断开、连接断了，或者访问权限被收回。
    pub aborted: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Counters {
    active: AtomicU64,
    total: AtomicU64,
    bytes_to_target: AtomicU64,
    bytes_from_target: AtomicU64,
    datagrams_to_target: AtomicU64,
    datagrams_from_target: AtomicU64,
    datagrams_refused: AtomicU64,
    failed_by_target: AtomicU64,
    refused: AtomicU64,
    failed_by_exit: AtomicU64,
    aborted: AtomicU64,
}

impl Counters {
    /// 收到了一个建流请求。返回的守卫被丢弃时，这条流不再算在"正在处理"里。
    pub(crate) fn begin(&self) -> ActiveFlow<'_> {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.active.fetch_add(1, Ordering::Relaxed);
        ActiveFlow { counters: self }
    }

    /// 一个请求没有建成，`status` 是回给发起方的状态。
    pub(crate) fn failed(&self, status: Status) {
        let counter = match status.attribution() {
            Some(Attribution::Target) => &self.failed_by_target,
            Some(Attribution::Refused) => &self.refused,
            Some(Attribution::Exit) => &self.failed_by_exit,
            None => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// 一条建成的流搬运结束了。
    pub(crate) fn finished(&self, summary: &Summary) {
        self.bytes_to_target.fetch_add(summary.from_flow.bytes, Ordering::Relaxed);
        self.bytes_from_target.fetch_add(summary.from_tcp.bytes, Ordering::Relaxed);
        if !summary.end.is_clean() {
            self.aborted.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 一个建成的数据报上下文没有正常结束。
    pub(crate) fn aborted(&self) {
        self.aborted.fetch_add(1, Ordering::Relaxed);
    }

    /// 替发起方往目标发了一个 `len` 字节的 UDP 报文。
    pub(crate) fn datagram_to_target(&self, len: usize) {
        self.datagrams_to_target.fetch_add(1, Ordering::Relaxed);
        // usize 不会比 u64 宽，这里不会截断。
        self.bytes_to_target.fetch_add(len as u64, Ordering::Relaxed);
    }

    /// 把目标发回来的一个 `len` 字节的 UDP 报文送回了发起方。
    pub(crate) fn datagram_from_target(&self, len: usize) {
        self.datagrams_from_target.fetch_add(1, Ordering::Relaxed);
        self.bytes_from_target.fetch_add(len as u64, Ordering::Relaxed);
    }

    /// 丢掉了一个目标没有通过检查的数据报。
    pub(crate) fn datagram_refused(&self) {
        self.datagrams_refused.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> ExitStats {
        ExitStats {
            active: self.active.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            bytes_to_target: self.bytes_to_target.load(Ordering::Relaxed),
            bytes_from_target: self.bytes_from_target.load(Ordering::Relaxed),
            datagrams_to_target: self.datagrams_to_target.load(Ordering::Relaxed),
            datagrams_from_target: self.datagrams_from_target.load(Ordering::Relaxed),
            datagrams_refused: self.datagrams_refused.load(Ordering::Relaxed),
            failed_by_target: self.failed_by_target.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            failed_by_exit: self.failed_by_exit.load(Ordering::Relaxed),
            aborted: self.aborted.load(Ordering::Relaxed),
        }
    }
}

/// 一条正在处理的流占着的计数。
#[derive(Debug)]
pub(crate) struct ActiveFlow<'a> {
    counters: &'a Counters,
}

impl Drop for ActiveFlow<'_> {
    fn drop(&mut self) {
        self.counters.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_counted_by_attribution() {
        let counters = Counters::default();
        for status in [Status::ConnectionRefused, Status::NameNotFound, Status::TimedOut] {
            counters.failed(status);
        }
        for status in [Status::AccessDenied, Status::PolicyDenied] {
            counters.failed(status);
        }
        counters.failed(Status::ResolverFailure);
        counters.failed(Status::Ok);
        let stats = counters.snapshot();
        assert_eq!((stats.failed_by_target, stats.refused, stats.failed_by_exit), (3, 2, 1));
    }

    #[test]
    fn a_flow_is_active_until_its_guard_is_dropped() {
        let counters = Counters::default();
        let first = counters.begin();
        let second = counters.begin();
        assert_eq!((counters.snapshot().active, counters.snapshot().total), (2, 2));
        drop(first);
        assert_eq!((counters.snapshot().active, counters.snapshot().total), (1, 2));
        drop(second);
        assert_eq!((counters.snapshot().active, counters.snapshot().total), (0, 2));
    }
}
