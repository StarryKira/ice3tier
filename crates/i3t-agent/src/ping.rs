//! `i3t ping <节点>`（白皮书 §13）：在本节点和另一个节点之间的那条连接上量几个往返，不是 ICMP。
//!
//! 每个往返是连接上新开的一条流：写一个往返测量的 OPEN，对端读到就回一个成功的 REPLY（`i3t_flow::ping`）。
//! 对端先按它的访问规则和每个对端的流数上限筛过，所以访问规则不允许时拿到的是明确的拒绝。到对端还没有连接时，
//! 第一个往返先把连接建起来，它不算在结果里。结果里有这一段时间开始和结束时这对节点的连接走的路。
//!
//! 只能由本机问（本机状态接口要令牌），一次只做一个，最多 [`MAX_COUNT`] 个往返、[`DEADLINE`] 之内做完；
//! 只能问本节点配置里的节点，所以它不能拿来扫描别的地址。

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use i3t_access::RouteAtStart;
use i3t_conn::{StreamAborted, StreamCode};
use i3t_flow::FlowError;
use i3t_model::Netmap;
use i3t_model::control::Route;
use i3t_proto::flow::Reason;
use serde::{Deserialize, Serialize};
use tokio::time::{self, Instant};

use crate::route_detail::EndpointRoutes;
use crate::runtime::Runtime;

/// 不说要几个时量几个往返。
pub const DEFAULT_COUNT: u32 = 5;
/// 最多量几个。
pub const MAX_COUNT: u32 = 10;
/// 两个往返之间隔多久。
const INTERVAL: Duration = Duration::from_millis(200);
/// 一个往返最多等多久，超过算丢了。
const EACH: Duration = Duration::from_secs(1);
/// 第一个往返（可能要先建连接）最多等多久。
const FIRST: Duration = Duration::from_secs(3);
/// 整个过程最多多久：本机状态接口的一次问答不超过 10 秒。
pub const DEADLINE: Duration = Duration::from_secs(8);

/// 同一时间只做一个。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 本机状态接口对 `/ping` 的回答，也就是 `i3t ping` 显示的东西。字段都有默认值：问的和答的可能不是同一个版本。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PingReport {
    /// 问的是哪个节点；认不出来时是 `None`。
    pub peer: Option<PingPeer>,
    pub outcome: PingOutcome,
    /// 发出去的往返，和回来了的往返。没有回来的算丢了。
    pub sent: u32,
    pub received: u32,
    /// 回来了的往返各用了多少微秒，按发出的顺序。
    pub rtts_us: Vec<u64>,
    /// 开始和结束时这对节点的连接走的路；不知道时是 `None`。
    pub route_start: Option<Route>,
    pub route_end: Option<Route>,
    /// 中间换过路（换过又换回来的也算）。
    pub route_changed: bool,
}

/// 被问的节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingPeer {
    pub id: u32,
    pub name: String,
    pub virtual_ip: Ipv4Addr,
}

/// 量成了没有；没有的话为什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PingOutcome {
    /// 量了（丢没丢看 `sent` 和 `received`）。
    #[default]
    Done,
    /// 本节点的配置里没有这个节点：没有这个节点，或者访问规则没有把它和本节点连起来。
    UnknownNode,
    /// 是本节点自己。
    Myself,
    /// 本节点的访问规则不允许本节点访问它（只允许它访问本节点）。
    NotAllowed,
    /// 对端拒绝了：它那边的访问规则不允许本节点访问它。
    Refused,
    /// 到它开不出流：它不在线，或者没有路。
    Unreachable,
    /// 它的版本太旧，不回答往返测量。
    Unsupported,
    /// 本节点上另一个 ping 正在做。
    Busy,
    /// 比这个程序新的节点答的、这里不认识的结论。
    #[serde(other)]
    Unknown,
}

/// 在 [`RUNNING`] 上占着的名额，丢弃时还回去。
struct Running;

impl Running {
    fn enter() -> Option<Self> {
        (!RUNNING.swap(true, Ordering::AcqRel)).then_some(Self)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// 在到 `target`（节点名、全名或者虚拟 IP）的连接上量 `count` 个往返（1 到 [`MAX_COUNT`]）。
pub(crate) async fn ping(runtime: &Runtime, target: &str, count: u32) -> PingReport {
    let netmap = Arc::clone(&runtime.netmap.borrow());
    let peer = match find(&netmap, target) {
        Ok(peer) => peer,
        Err(outcome) => return PingReport { outcome, ..PingReport::default() },
    };
    let mut report = PingReport {
        peer: Some(PingPeer { id: peer.id.get(), name: peer.name.clone(), virtual_ip: peer.virtual_ip }),
        ..PingReport::default()
    };
    if !peer.outbound {
        report.outcome = PingOutcome::NotAllowed;
        return report;
    }
    let Some(_running) = Running::enter() else {
        report.outcome = PingOutcome::Busy;
        return report;
    };
    let (id, endpoint) = (peer.id, &*runtime.endpoint);
    let routes = EndpointRoutes(Arc::clone(&runtime.endpoint));
    let mut route = RouteAtStart::now(&routes, id);
    let deadline = Instant::now() + DEADLINE;

    // 第一个往返把连接建起来，不算数；开始时的路从它之后算，不然第一次联系时总是"不可达 → 直连"。
    let first = time::timeout(FIRST, i3t_flow::ping(endpoint, id)).await;
    if let Some(outcome) = match first {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(outcome_of(&error)),
        Err(_) => Some(PingOutcome::Unreachable),
    } {
        tracing::info!(peer = %id, ?outcome, "ping：第一个往返就没有成");
        report.outcome = outcome;
    } else {
        route = RouteAtStart::now(&routes, id);
        for index in 0..count.clamp(1, MAX_COUNT) {
            if index > 0 {
                time::sleep(INTERVAL).await;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()) else { break };
            report.sent += 1;
            if let Ok(Ok(rtt)) = time::timeout(EACH.min(left), i3t_flow::ping(endpoint, id)).await {
                report.received += 1;
                report.rtts_us.push(u64::try_from(rtt.as_micros()).unwrap_or(u64::MAX));
            }
        }
    }
    let span = route.ended(&routes);
    (report.route_start, report.route_end, report.route_changed) = (span.start, span.end, span.changed);
    tracing::info!(peer = %id, outcome = ?report.outcome, sent = report.sent, received = report.received, "ping 做完了");
    report
}

/// 按名字（短名或者全名）或者虚拟 IP 在配置里找节点。
fn find<'a>(netmap: &'a Netmap, target: &str) -> Result<&'a i3t_model::netmap::Peer, PingOutcome> {
    let target = target.trim();
    let mine = match target.parse::<Ipv4Addr>() {
        Ok(ip) => ip == netmap.me.virtual_ip,
        Err(_) => {
            let full = target.strip_suffix('.').unwrap_or(target);
            netmap.network.short_name(full).eq_ignore_ascii_case(&netmap.me.name)
        }
    };
    if mine {
        return Err(PingOutcome::Myself);
    }
    let found = match target.parse::<Ipv4Addr>() {
        Ok(ip) => netmap.peer_by_ip(ip),
        Err(_) => netmap.peer_by_name(target),
    };
    found.ok_or(PingOutcome::UnknownNode)
}

/// 一个往返没有成，是哪一种。
fn outcome_of(error: &FlowError) -> PingOutcome {
    match error {
        FlowError::Refused(_) => PingOutcome::Refused,
        FlowError::Read(error) if StreamAborted::of(error) == Some(StreamAborted::Reset(StreamCode(Reason::ProtocolError.to_wire()))) => {
            PingOutcome::Unsupported
        }
        _ => PingOutcome::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::{LaneMode, LocalNode, Network, Peer};
    use i3t_proto::{NodeId, PublicKey};

    use super::*;

    fn netmap() -> Netmap {
        let peer = |raw: u32, name: &str, outbound: bool| Peer {
            id: NodeId::new(raw).unwrap(),
            key: PublicKey::from_bytes([u8::try_from(raw).unwrap(); 32]),
            name: name.into(),
            virtual_ip: Ipv4Addr::new(10, 66, 0, u8::try_from(raw).unwrap()),
            inbound: true,
            outbound,
            endpoints: Vec::new(),
            attached: Vec::new(),
            exit: None,
            dial_out_only: false,
            keep_alive: false,
            lane: LaneMode::Off,
        };
        Netmap {
            version: 1,
            network: Network {
                name: "home".into(),
                domain: "home.i3t".into(),
                cidr: "10.66.0.0/16".parse().unwrap(),
                routing: Default::default(),
                max_relay_hops: 2,
                reach: Default::default(),
                class: None,
            },
            me: LocalNode {
                id: NodeId::new(1).unwrap(),
                name: "laptop".into(),
                virtual_ip: Ipv4Addr::new(10, 66, 0, 1),
                routing: None,
                reach: None,
                dial_out_only: false,
                relay: None,
                exit: None,
                listeners: Vec::new(),
                forwards: Vec::new(),
                forward_targets: Vec::new(),
                pinned_relays: Vec::new(),
                profile: None,
            },
            peers: vec![peer(2, "nas", true), peer(3, "phone", false)],
            relays: Vec::new(),
            relay_clients: Vec::new(),
            credentials: Vec::new(),
        }
    }

    #[test]
    fn a_peer_is_found_by_name_full_name_or_virtual_ip_and_never_ourselves() {
        let netmap = netmap();
        for target in ["nas", "NAS", "nas.home.i3t", "10.66.0.2"] {
            assert_eq!(find(&netmap, target).map(|peer| peer.id.get()), Ok(2), "{target}");
        }
        for target in ["laptop", "laptop.home.i3t", "10.66.0.1"] {
            assert_eq!(find(&netmap, target).map(|peer| peer.id.get()), Err(PingOutcome::Myself), "{target}");
        }
        assert_eq!(find(&netmap, "printer").map(|peer| peer.id.get()), Err(PingOutcome::UnknownNode));
        assert!(!find(&netmap, "phone").unwrap().outbound, "只能被它访问的节点找得到，由调用方说不允许");
    }

    #[test]
    fn failures_are_told_apart() {
        let refused = FlowError::Refused(i3t_proto::flow::Reply::new(i3t_proto::flow::Status::AccessDenied, ""));
        assert_eq!(outcome_of(&refused), PingOutcome::Refused);
        let old = FlowError::Read(StreamAborted::Reset(StreamCode(Reason::ProtocolError.to_wire())).into());
        assert_eq!(outcome_of(&old), PingOutcome::Unsupported);
        assert_eq!(outcome_of(&FlowError::Connect(i3t_conn::OpenError::NoRoute(NodeId::new(2).unwrap()))), PingOutcome::Unreachable);
        let newer: PingReport = serde_json::from_value(serde_json::json!({ "outcome": "later", "extra": 1 })).unwrap();
        assert_eq!(newer.outcome, PingOutcome::Unknown);
    }
}
