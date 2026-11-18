//! 入口上的用量：按监听（代理和端口转发）、按出口分别记（白皮书 §9.3、§13）。
//!
//! 都是这次运行以来的累计，随搬随记（[`IngressMeter`] 过每一段数据），不等连接结束。装配层定期取一份快照上报给 controller：
//! 出口的那一份拿去和出口自己报的交叉核对。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use i3t_flow::{Direction, Meter};
use i3t_proto::NodeId;
use i3t_proto::flow::{Attribution, Reason, Status};

/// 两个方向的字节数。入口这一侧，上行是客户端发出的，下行是发回给客户端的。
#[derive(Debug, Default)]
pub struct Tally {
    up: AtomicU64,
    down: AtomicU64,
}

impl Tally {
    #[must_use]
    pub fn up(&self) -> u64 {
        self.up.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn down(&self) -> u64 {
        self.down.load(Ordering::Relaxed)
    }

    pub(crate) fn add(&self, upward: bool, len: usize) {
        let counter = if upward { &self.up } else { &self.down };
        // usize 不会比 u64 宽，这里不会截断。
        counter.fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// 发起这一侧看到的、经一个出口出去的流。
#[derive(Debug, Default)]
pub(crate) struct ExitTally {
    flows: AtomicU64,
    traffic: Tally,
    failed_by_target: AtomicU64,
    refused: AtomicU64,
    failed_by_exit: AtomicU64,
    unreachable: AtomicU64,
}

impl ExitTally {
    /// 又发起了一条经它出去的流（或者一个数据报上下文）。
    pub(crate) fn started(&self) {
        self.flows.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn traffic(&self) -> &Tally {
        &self.traffic
    }

    /// 出口回了一个失败的应答。
    pub(crate) fn refused_with(&self, status: Status) {
        let counter = match status.attribution() {
            Some(Attribution::Target) => &self.failed_by_target,
            Some(Attribution::Refused) => &self.refused,
            // 拒绝里写着"成功"是对端的错，算在它头上。
            Some(Attribution::Exit) | None => &self.failed_by_exit,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// 到出口开流就失败了。
    pub(crate) fn unreachable(&self) {
        self.unreachable.fetch_add(1, Ordering::Relaxed);
    }
}

/// 经一个出口出去的流的快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitSnapshot {
    pub exit: NodeId,
    pub flows: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub failed_by_target: u64,
    pub refused: u64,
    pub failed_by_exit: u64,
    pub unreachable: u64,
}

/// 入口上的全部用量。
#[derive(Debug, Default)]
pub(crate) struct Usage {
    exits: Mutex<HashMap<NodeId, Arc<ExitTally>>>,
}

impl Usage {
    /// 经 `exit` 出去的流记在哪里。第一次用到这个出口时建一份。
    pub(crate) fn exit(&self, exit: NodeId) -> Arc<ExitTally> {
        Arc::clone(self.exits.lock().expect("exit usage mutex poisoned").entry(exit).or_default())
    }

    pub(crate) fn exits(&self) -> Vec<ExitSnapshot> {
        let exits = self.exits.lock().expect("exit usage mutex poisoned");
        let mut snapshots: Vec<ExitSnapshot> = exits
            .iter()
            .map(|(&exit, tally)| ExitSnapshot {
                exit,
                flows: tally.flows.load(Ordering::Relaxed),
                bytes_up: tally.traffic.up(),
                bytes_down: tally.traffic.down(),
                failed_by_target: tally.failed_by_target.load(Ordering::Relaxed),
                refused: tally.refused.load(Ordering::Relaxed),
                failed_by_exit: tally.failed_by_exit.load(Ordering::Relaxed),
                unreachable: tally.unreachable.load(Ordering::Relaxed),
            })
            .collect();
        snapshots.sort_by_key(|snapshot| snapshot.exit);
        snapshots
    }
}

/// 入口搬运时的关卡：把每一段数据记到这条连接的监听和出口上。
#[derive(Debug, Clone, Copy)]
pub(crate) struct IngressMeter<'a> {
    pub(crate) listen: &'a Tally,
    pub(crate) exit: Option<&'a Tally>,
}

impl IngressMeter<'_> {
    /// 客户端那一侧的方向：上行是客户端发出的。
    pub(crate) fn record(&self, upward: bool, len: usize) {
        for tally in [Some(self.listen), self.exit].into_iter().flatten() {
            tally.add(upward, len);
        }
    }
}

impl Meter for IngressMeter<'_> {
    async fn pass(&self, direction: Direction, len: usize) -> Result<(), Reason> {
        // 入口一侧 TCP 那一头是客户端：从它读出来的就是上行。
        self.record(direction == Direction::FromTcp, len);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_count_flows_bytes_and_failures_by_attribution() {
        let usage = Usage::default();
        let gw = NodeId::new(2).unwrap();
        let tally = usage.exit(gw);
        tally.started();
        tally.traffic.add(true, 10);
        let again = usage.exit(gw);
        again.started();
        again.refused_with(Status::ConnectionRefused);
        again.refused_with(Status::QuotaExhausted);
        again.refused_with(Status::InternalError);
        again.unreachable();
        assert_eq!(
            usage.exits(),
            [ExitSnapshot {
                exit: gw,
                flows: 2,
                bytes_up: 10,
                bytes_down: 0,
                failed_by_target: 1,
                refused: 1,
                failed_by_exit: 1,
                unreachable: 1
            }]
        );
    }
}
