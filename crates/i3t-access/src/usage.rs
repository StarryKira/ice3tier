//! 入口上的用量：按凭据、按监听（代理监听器和端口转发）、按出口分别记（白皮书 §9.2、§9.3、§13）。
//!
//! 都是这次运行以来的累计，随搬随记（[`IngressMeter`] 过每一段数据），不等连接结束。装配层定期取一份快照上报给 controller：
//! 凭据的用量由 controller 汇总各个入口算流量上限；出口的那一份拿去和出口自己报的交叉核对。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use i3t_flow::{Direction, Meter};
use i3t_model::netmap::{Credential, TokenHash};
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

/// 一个凭据在这个入口上的用量。
#[derive(Debug, Default)]
pub(crate) struct CredentialUsage {
    /// controller 上的编号，按最近一次见到的 netmap 记。旧版本的 controller 不给编号，是 0。
    id: AtomicU32,
    name: Mutex<String>,
    traffic: Tally,
    connections: AtomicU32,
    last_used: Mutex<Option<Instant>>,
}

/// 一个凭据用量的快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSnapshot {
    pub id: u32,
    pub name: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub connections: u32,
    /// 最近一次有连接用它是多久之前。
    pub last_used: Option<Duration>,
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
    /// 按令牌的哈希分：它不会变，编号可能是旧版本 controller 给的 0。
    credentials: Mutex<HashMap<TokenHash, Arc<CredentialUsage>>>,
    exits: Mutex<HashMap<NodeId, Arc<ExitTally>>>,
}

impl Usage {
    /// 给 `credential` 占一个连接名额；已经到了它的并发上限时返回 `None`。名额在 [`Slot`] 被丢弃时归还。
    ///
    /// 上限按当时的 netmap 取：上限调低之后，已经在用的连接不受影响，新的连接要等用量降下来。
    pub(crate) fn acquire(&self, credential: &Credential) -> Option<Slot> {
        let mut credentials = self.credentials.lock().expect("credential usage mutex poisoned");
        let usage = Arc::clone(credentials.entry(credential.token).or_default());
        drop(credentials);
        usage.id.store(credential.id, Ordering::Relaxed);
        credential.name.clone_into(&mut usage.name.lock().expect("credential name mutex poisoned"));
        // 比较和加一要是一步：用 CAS，两条连接同时来时只有一条能占到最后一个名额。
        let admitted = usage.connections.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            credential.max_concurrent.is_none_or(|max| count < max).then_some(count + 1)
        });
        admitted.ok()?;
        *usage.last_used.lock().expect("credential last-used mutex poisoned") = Some(Instant::now());
        Some(Slot { usage })
    }

    /// 经 `exit` 出去的流记在哪里。第一次用到这个出口时建一份。
    pub(crate) fn exit(&self, exit: NodeId) -> Arc<ExitTally> {
        Arc::clone(self.exits.lock().expect("exit usage mutex poisoned").entry(exit).or_default())
    }

    pub(crate) fn credentials(&self) -> Vec<CredentialSnapshot> {
        let credentials = self.credentials.lock().expect("credential usage mutex poisoned");
        let mut snapshots: Vec<CredentialSnapshot> = credentials
            .values()
            .map(|usage| CredentialSnapshot {
                id: usage.id.load(Ordering::Relaxed),
                name: usage.name.lock().expect("credential name mutex poisoned").clone(),
                bytes_up: usage.traffic.up(),
                bytes_down: usage.traffic.down(),
                connections: usage.connections.load(Ordering::Relaxed),
                last_used: usage.last_used.lock().expect("credential last-used mutex poisoned").map(|at| at.elapsed()),
            })
            .collect();
        snapshots.sort_by(|a, b| (a.id, &a.name).cmp(&(b.id, &b.name)));
        snapshots
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

/// 一条连接占着的凭据名额。
#[derive(Debug)]
pub(crate) struct Slot {
    usage: Arc<CredentialUsage>,
}

impl Slot {
    pub(crate) fn traffic(&self) -> &Tally {
        &self.usage.traffic
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.usage.connections.fetch_sub(1, Ordering::Relaxed);
        *self.usage.last_used.lock().expect("credential last-used mutex poisoned") = Some(Instant::now());
    }
}

/// 入口搬运时的关卡：把每一段数据记到这条连接的监听、凭据、出口上。
#[derive(Debug, Clone, Copy)]
pub(crate) struct IngressMeter<'a> {
    pub(crate) listen: &'a Tally,
    pub(crate) credential: Option<&'a Tally>,
    pub(crate) exit: Option<&'a Tally>,
}

impl IngressMeter<'_> {
    /// 客户端那一侧的方向：上行是客户端发出的。
    pub(crate) fn record(&self, upward: bool, len: usize) {
        for tally in [Some(self.listen), self.credential, self.exit].into_iter().flatten() {
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
    use ring::digest;

    use super::*;

    fn credential(id: u32, token: &str, max_concurrent: Option<u32>) -> Credential {
        let hash = digest::digest(&digest::SHA256, token.as_bytes());
        Credential {
            name: format!("c{id}"),
            token: TokenHash::from_bytes(hash.as_ref().try_into().unwrap()),
            max_concurrent,
            id,
            exhausted: false,
        }
    }

    #[test]
    fn concurrency_is_limited_per_credential_and_slots_come_back() {
        let usage = Usage::default();
        let limited = credential(1, "token-a", Some(2));
        let other = credential(2, "token-b", Some(1));

        let first = usage.acquire(&limited).unwrap();
        let _second = usage.acquire(&limited).unwrap();
        assert!(usage.acquire(&limited).is_none(), "第三条连接超过了上限 2");
        assert!(usage.acquire(&other).is_some(), "别的凭据不受影响");

        drop(first);
        assert!(usage.acquire(&limited).is_some(), "名额归还之后可以再用");
        assert!(usage.acquire(&credential(3, "token-c", Some(0))).is_none(), "上限是 0 就一条都不行");
    }

    #[test]
    fn bytes_connections_and_last_use_are_reported_per_credential() {
        let usage = Usage::default();
        let phone = credential(7, "token-a", None);
        assert!(usage.credentials().is_empty(), "没有用过的凭据不报");
        let slot = usage.acquire(&phone).unwrap();
        let listen = Tally::default();
        let meter = IngressMeter { listen: &listen, credential: Some(slot.traffic()), exit: None };
        meter.record(true, 100);
        meter.record(false, 4000);
        let [snapshot] = usage.credentials().try_into().unwrap();
        assert_eq!(
            (snapshot.id, snapshot.name.as_str(), snapshot.bytes_up, snapshot.bytes_down, snapshot.connections),
            (7, "c7", 100, 4000, 1)
        );
        assert!(snapshot.last_used.is_some_and(|ago| ago < Duration::from_secs(5)));
        assert_eq!((listen.up(), listen.down()), (100, 4000), "监听器上也记了");

        drop(slot);
        let [snapshot] = usage.credentials().try_into().unwrap();
        assert_eq!((snapshot.connections, snapshot.bytes_down), (0, 4000), "连接结束之后用量还在");
    }

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
