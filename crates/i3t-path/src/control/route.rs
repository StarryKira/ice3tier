//! 一条路在控制任务里的样子，以及怎么从这一刻的状态里选出它。
//!
//! [`View`] 是选路时能看到的全部东西的只读快照；[`RouteKey`] 是选出来的结果；剩下的是把结果写成给人看的话。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use i3t_model::control::{LegTransport, Route, RouteCause, RouteHint};
use i3t_proto::NodeId;
use i3t_proto::backbone::RelayState;
use i3t_proto::datagram::{RelayHeader, RelayPath, RouteHeader};

use super::{PeerState, RelayEntry};
use crate::cost::Segment;
use crate::matrix::Matrix;
use crate::relays::Leg;
use crate::routing::{self, Candidate, Destination, DirectLink, Entry, Graph, PathId, Why};
use crate::table::{FrameHeader, Hop, Shared};
use crate::tcp::TcpLegHandle;
use crate::timing::{SETTLE, SWITCH_WINDOWS};

/// 从没有路变成有路之后多久之内，选中的 relay 还不算"在用"，见 [`relay_in_use`]。
const FIRST_ROUTE_GRACE: Duration = Duration::from_secs(1);

/// 到路径上第一个 relay 的那一段走的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Via {
    Udp(SocketAddr),
    /// TCP，值是那条 TCP 的编号：重连之后编号变了，发包用的队列也要跟着换。
    Tcp(u64),
    /// relay 就是本节点自己。
    Local,
}

/// 到一个对端现在走哪条路。和收发路径上的 [`Hop`] 一一对应，多出来的用处是能比较"变没变"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteKey {
    None,
    Direct(SocketAddr),
    Relay { path: RelayPath, via: Via },
}

impl RouteKey {
    pub(super) fn to_route(self) -> Route {
        match self {
            Self::None => Route::None,
            Self::Direct(addr) => Route::Direct { addr },
            Self::Relay { path, .. } => Route::Relay { relays: path.as_slice().to_vec() },
        }
    }
}

/// 选路时看到的、这一刻的全部状态。
pub(super) struct View<'a> {
    pub(super) shared: &'a Arc<Shared>,
    pub(super) relays: &'a HashMap<NodeId, RelayEntry>,
    pub(super) legs: &'a HashMap<NodeId, TcpLegHandle>,
    pub(super) matrix: &'a Matrix,
    pub(super) broken: &'a [(NodeId, NodeId, Instant)],
    /// 本节点现在能把包交给它的 relay（会话是通的、延迟测出来了）。
    pub(super) entries: &'a [Entry],
    pub(super) allow_direct: bool,
    pub(super) allow_relay: bool,
    pub(super) max_hops: usize,
    pub(super) now: Instant,
}

impl View<'_> {
    /// 这个 relay 通知过自己不是正常服务的状态。
    fn avoided(&self, relay: NodeId) -> bool {
        self.relays.get(&relay).is_some_and(|entry| entry.state != RelayState::Serving)
    }

    fn draining(&self, relay: NodeId) -> bool {
        self.relays.get(&relay).is_some_and(|entry| entry.state == RelayState::Draining)
    }

    /// 对端在本节点自己的 relay 上有没有会话。
    fn local_session(&self, peer: NodeId) -> bool {
        self.shared.local_relay.get().is_some_and(|local| local.has_session(peer))
    }

    /// 经 `relay` 到 `peer` 的那个会话信得过，可以把流量换过去。
    ///
    /// 会话没有断过，或者断了重连之后已经保持了 [`SETTLE`] 那么久，才信得过。`relay` 是别的节点时看的是
    /// 本节点到它的会话；是本节点自己时看的是对端在本节点的 relay 上的会话。
    ///
    /// 一条时好时坏的链路，会话断了重连之后头几秒看起来总是好的；等它保持够久再把流量换过去，
    /// 免得流量跟着它一断一连地来回换。原来的路断了、只剩它能走的时候不管这一条。
    fn settled(&self, peer: NodeId, relay: NodeId) -> bool {
        if relay == self.shared.me {
            let Some(local) = self.shared.local_relay.get() else { return false };
            return local.has_session(peer) && local.session_reopened(peer).is_none_or(|held| held >= SETTLE);
        }
        self.relays.get(&relay).is_some_and(|entry| entry.link.is_settled(self.now))
    }

    fn is_broken(&self, from: NodeId, to: NodeId) -> bool {
        // 两个编号相同的记录表示 `from` 不肯转发任何多级路径。
        self.broken.iter().any(|&(first, next, _)| first == from && (next == to || next == first))
    }

    /// 现在能不能把发给 `peer` 的包交给 `relay`。
    fn usable(&self, peer: NodeId, relay: NodeId) -> bool {
        if relay == self.shared.me {
            return self.local_session(peer);
        }
        // 对端不是 relay 时，"对端自己"这个候选在 relay 表里找不到，自然落空。
        match self.relays.get(&relay).and_then(|entry| entry.link.leg()) {
            Some(Leg::Udp(_)) => true,
            Some(Leg::Tcp) => self.legs.contains_key(&relay),
            None => false,
        }
    }

    /// 默认模式的选择：直连可用就走直连；否则经一个 relay，按 [`relay_candidates`] 的顺序取第一个现在能用的。
    pub(super) fn direct_first(&self, id: NodeId, peer: &PeerState) -> Option<PathId> {
        let direct = if self.allow_direct { peer.direct.active().map(|(addr, _)| addr) } else { None };
        if !self.allow_relay {
            return direct.map(PathId::Direct);
        }
        let current = relay_in_use(peer, self.now);
        // 通知过下线或者过载的 relay、刚说过"对端没挂在我这里"的 relay 排到后面，但不是完全不用：
        // 只有它一个可走时，对端可能只是还没来得及挂上去。
        let refused = |relay: NodeId| peer.unattached.iter().any(|(held, _)| *held == relay);
        let order = rank_relays(relay_candidates(self.shared.me, id, peer, current).collect(), current, |relay| RelayStanding {
            avoided: self.avoided(relay),
            refused: refused(relay),
            settled: self.settled(id, relay),
        });
        routing::direct_first(direct, order, |relay| self.usable(id, relay))
    }

    /// 到这个对端的全部候选路径和它们的代价，按代价从低到高。
    pub(super) fn candidates(&self, id: NodeId, peer: &PeerState) -> Vec<Candidate> {
        let me = self.shared.me;
        let direct = peer.direct.active().filter(|_| self.allow_direct).map(|(addr, rtt)| DirectLink {
            addr,
            segment: peer
                .direct
                .quality(self.now)
                .map_or_else(|| Segment::from_rtt_ms(rtt.get().as_secs_f64() * 1000.0), |quality| quality.segment()),
            lan: peer.direct.is_lan_active(),
        });
        let mut entries: Vec<Entry> = Vec::new();
        let mut exits: Vec<(NodeId, Option<Segment>)> = Vec::new();
        if self.allow_relay {
            entries.extend_from_slice(self.entries);
            let local = self.local_session(id);
            if local {
                // 对端挂在本节点自己的 relay 上：包直接交给它的会话，本节点这一头没有延迟。
                // 这条路上会断的是对端连过来的那个会话；它到本节点的延迟只有对端把本节点列进了挂靠表才知道，
                // 不知道的时候下面按最小的算，那样的数不能拿来和别的路比代价。
                let measured = peer.attached.iter().any(|attachment| attachment.relay == me);
                entries.push(Entry { relay: me, segment: Segment::from_rtt_ms(0.0), settled: measured && self.settled(id, me) });
            }
            for attachment in peer.attached.iter().filter(|attachment| attachment.relay != id && (attachment.relay != me || local)) {
                exits.push((attachment.relay, Some(Segment::from_rtt_ms(f64::from(attachment.rtt_ms)))));
            }
            if local && !exits.iter().any(|(relay, _)| *relay == me) {
                // 对端的挂靠表里（还）没有本节点，但会话确实在：延迟不知道，按最小的算。
                exits.push((me, Some(Segment::from_rtt_ms(1.0))));
            }
            if peer.is_relay {
                exits.push((id, None));
            }
        }
        let broken = |from: NodeId, to: NodeId| self.is_broken(from, to);
        let enumerate = |avoid: &dyn Fn(NodeId) -> bool| {
            let graph =
                Graph { me, max_hops: self.max_hops, entries: &entries, matrix: self.matrix, now: self.now, avoid, broken: &broken };
            routing::candidates(&graph, &Destination { id, exits: &exits, direct })
        };
        let is_relayed = |candidate: &Candidate| matches!(candidate.path, PathId::Relays(_));
        // 通知过过载的 relay 只在没有别的 relay 可走时才用，通知过即将下线的更靠后：只有它一个 relay 可走时，
        // 它退出之前（或者很快回来之后）还能送到，比没有路强。
        let mut found = enumerate(&|relay| self.avoided(relay));
        if !found.iter().any(is_relayed) {
            found = enumerate(&|relay| self.draining(relay));
        }
        if !found.iter().any(is_relayed) {
            found = enumerate(&|_| false);
        }
        // 刚被告知、或者被怀疑走不通的路径先不用，除非那样就一条经 relay 的路都没有了。
        let doubtful = |candidate: &Candidate| match candidate.path {
            PathId::Direct(_) => false,
            PathId::Relays(path) => {
                peer.suspect.iter().any(|(suspect, _)| *suspect == path) || peer.unattached.iter().any(|(relay, _)| *relay == path.last())
            }
        };
        if found.iter().any(|candidate| is_relayed(candidate) && !doubtful(candidate)) {
            found.retain(|candidate| !doubtful(candidate));
        }
        found.sort_by(|a, b| a.cost_ms.total_cmp(&b.cost_ms));
        found
    }

    /// 把一条路径落实成收发路径上的做法。此刻走不了（直连没有验证过、到第一个 relay 没有会话）时返回 `None`。
    pub(super) fn resolve(&self, id: NodeId, peer: &PeerState, path: PathId) -> Option<(RouteKey, Hop)> {
        let me = self.shared.me;
        let path = match path {
            PathId::Direct(addr) => {
                let active = self.allow_direct && peer.direct.active().is_some_and(|(active, _)| active == addr);
                return active.then_some((RouteKey::Direct(addr), Hop::Direct(addr)));
            }
            PathId::Relays(path) => path,
        };
        if !self.allow_relay {
            return None;
        }
        let first = path.first();
        if first == me {
            // 本节点自己的 relay 只用来把包交给挂在它上面的对端。
            let local = path.as_slice().len() == 1 && self.local_session(id);
            return local.then_some((RouteKey::Relay { path, via: Via::Local }, Hop::Local));
        }
        let entry = self.relays.get(&first)?;
        let header = match *path.as_slice() {
            // 只经一个 relay 时用老的 9 字节的头。
            [_] => FrameHeader::single(RelayHeader { dst: id, src: me }),
            _ => FrameHeader::routed(&RouteHeader { dst: id, src: me, relays: path }),
        };
        let relay = Arc::clone(&entry.slot);
        match entry.link.leg()? {
            Leg::Udp(addr) => Some((RouteKey::Relay { path, via: Via::Udp(addr) }, Hop::RelayUdp { relay, addr, header })),
            Leg::Tcp => {
                let leg = self.legs.get(&first)?;
                Some((RouteKey::Relay { path, via: Via::Tcp(leg.conn) }, Hop::RelayTcp { relay, sender: leg.sender.clone(), header }))
            }
        }
    }

    /// 延迟优先模式下，现在选定的路是不是还能走。不能走了要立刻重新选，不等评估窗口。
    pub(super) fn alive(&self, id: NodeId, peer: &PeerState, path: PathId) -> bool {
        if self.resolve(id, peer, path).is_none() {
            return false;
        }
        let PathId::Relays(path) = path else { return true };
        let relays = path.as_slice();
        !relays.iter().any(|relay| self.avoided(*relay))
            && !relays.windows(2).any(|pair| self.is_broken(pair[0], pair[1]))
            && !peer.suspect.iter().any(|(suspect, _)| *suspect == path)
            && !peer.unattached.iter().any(|(relay, _)| *relay == path.last())
    }
}

/// 默认模式下现在走着的 relay（默认模式只经一个）。它在对端的挂靠表里排在前面，不因为刚连上往后排，免得来回换。
///
/// 刚从没有路变成有路的头 [`FIRST_ROUTE_GRACE`] 里不算：启动时到几个 relay 的会话往往前后脚建立，相差不到一毫秒，
/// 先建好的那个只是碰巧先到，不该因此一直占着、把对端排在前面的 relay 挡在后面。
///
/// 看的是选路规则选的路（[`PeerState::policy_route`]），不是主动测试暂时固定出来的路：测试结束之后要回到原来那条。
pub(super) fn relay_in_use(peer: &PeerState, now: Instant) -> Option<NodeId> {
    let just_picked = peer.first_route.is_some_and(|at| now.saturating_duration_since(at) < FIRST_ROUTE_GRACE);
    match peer.policy_route {
        RouteKey::Relay { path, .. } if !just_picked => Some(path.first()),
        RouteKey::Relay { .. } | RouteKey::Direct(_) | RouteKey::None => None,
    }
}

/// 默认模式下经哪些 relay 能到这个对端，按优先顺序。本节点会和其中是 relay 的那些保持会话。
///
/// 1. **对端自己。** 对端本身是启用了的 relay 时，在它那里建一个会话，把中转帧直接发给它，它收到目的是自己的帧就地收下。
///    这相当于它"挂靠在自己身上、延迟为 0"：包只走本节点到对端这一段，不绕第三个节点。UDP 完全用不了的节点
///    就是这样用一条 TCP 连到有公网端口的对端的（白皮书 §5.5 的情形 3）。
/// 2. **本节点自己。** 本节点是 relay、对端在它上面有会话时，经那个会话直接送过去，同样不绕路。
///    对端的挂靠表是按它自己的延迟排的，别的 relay 排在前面也不该先用。
/// 3. **对端挂靠的 relay**，按它上报的延迟从低到高。只有前两条都走不通时才用。
///    `current` 是现在走着的 relay：它还在对端的挂靠表里、延迟又没有明显高过表里最低的那个时，排在这一组的最前面。
///    对端到两个 relay 的延迟相近时，它上报的顺序会随着测量的起伏来回变，不该让路跟着变。
///
/// 前两个候选不适用时（对端不是 relay、本节点不是 relay 或者对端没挂在它上面）在选路时自然落空。
pub(super) fn relay_candidates(me: NodeId, id: NodeId, peer: &PeerState, current: Option<NodeId>) -> impl Iterator<Item = NodeId> + '_ {
    let listed = || peer.attached.iter().filter(move |attachment| attachment.relay != id && attachment.relay != me);
    let lowest = listed().map(|attachment| attachment.rtt_ms).min();
    // "明显更高"的标准和换挂靠对象时一样：高出 max(10ms, 20%)。
    let close_enough = |rtt_ms: u32| lowest.is_some_and(|lowest| rtt_ms <= lowest.saturating_add((lowest / 5).max(10)));
    let kept =
        listed().find(|attachment| Some(attachment.relay) == current && close_enough(attachment.rtt_ms)).map(|attachment| attachment.relay);
    let others = listed().map(|attachment| attachment.relay).filter(move |relay| Some(*relay) != kept);
    [id, me].into_iter().chain(kept).chain(others)
}

/// 默认模式下一个候选 relay 此刻的情况，决定它往后排多少。
#[derive(Debug, Clone, Copy)]
pub(super) struct RelayStanding {
    /// 它通知过即将下线或者过载。
    pub(super) avoided: bool,
    /// 它刚说过对端没挂在它上面。
    pub(super) refused: bool,
    /// 经它的那个会话已经保持了一段时间（见 [`View::settled`]）。
    pub(super) settled: bool,
}

/// 把默认模式的候选 relay 排成该先试哪个。`order` 是 [`relay_candidates`] 给的优先顺序，`current` 是现在走着的。
///
/// 有毛病的往后排，但不去掉：只剩它能走的时候还得用。先看 `avoided`，再看 `refused`，最后是会话刚连上的；
/// 现在走着的那个不因为"刚连上"往后排（已经在它上面了，没有"换过去"的问题）。排序是稳定的，同一档里顺序不变。
pub(super) fn rank_relays(mut order: Vec<NodeId>, current: Option<NodeId>, standing: impl Fn(NodeId) -> RelayStanding) -> Vec<NodeId> {
    order.sort_by_key(|&relay| {
        let RelayStanding { avoided, refused, settled } = standing(relay);
        (avoided, refused, Some(relay) != current && !settled)
    });
    order
}

/// 为什么走现在这条路：给人看的一句话（日志、旧的客户端），和它的代码（界面按自己的语言写）。两样说的是同一件事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reason {
    pub(crate) text: String,
    pub(crate) cause: RouteCause,
}

impl Reason {
    pub(crate) fn new(text: impl Into<String>, cause: RouteCause) -> Self {
        Self { text: text.into(), cause }
    }

    /// 到这个对端还没有过路。
    pub(crate) fn no_route_yet() -> Self {
        Self::new("还没有可用的路径", RouteCause::NoRouteYet)
    }
}

/// 换路的直接起因写给人看。
pub(crate) const fn hint_text(hint: RouteHint) -> &'static str {
    match hint {
        RouteHint::DirectTimeouts => "直连探测连续超时",
        RouteHint::DirectLoss => "直连探测丢失超过 20%",
        RouteHint::DirectStale => "对端不活跃，直连地址过期",
        RouteHint::DirectUnavailable => "直连不可用",
        RouteHint::RelayDraining => "relay 通知即将下线",
        RouteHint::RelayOverloaded => "relay 通知过载",
        RouteHint::RelayPathBroken => "relay 说多级路径走不下去",
        RouteHint::PeerNotAttached => "relay 说对端没有挂在它上面",
        RouteHint::PathProbeSilent => "经多个 relay 的路径上端到端的探测一直没有回音",
        RouteHint::PreferredRelayReady => "刚有路时先连上的不是对端排在前面的 relay，那个 relay 的会话随后也建好了",
        RouteHint::Shortcut => "有了不绕路的中转会话",
        RouteHint::RelayUnavailable => "原来的 relay 不可用",
        RouteHint::RelayLost => "中转会话断开，没有别的路",
        RouteHint::RouteFailed => "原来的路不能用了",
        RouteHint::NoPath => "没有任何可用的路径",
        RouteHint::Unknown => "原因不明",
    }
}

/// 代价的毫秒数换成微秒，原因的代码里用。不是数（没找到这条候选）或者是负数时没有。借 `Duration` 来换，不用 `as`。
fn micros(ms: f64) -> Option<u64> {
    let duration = std::time::Duration::try_from_secs_f64(ms / 1000.0).ok()?;
    Some(u64::try_from(duration.as_micros()).unwrap_or(u64::MAX))
}

/// 一条路径写给人看：`relay 8 → 9`。
fn relays_text(path: RelayPath) -> String {
    let hops: Vec<String> = path.as_slice().iter().map(NodeId::to_string).collect();
    format!("relay {}", hops.join(" → "))
}

/// 一条路写给人看，日志和测试都靠它认路。
pub(super) fn route_text(key: RouteKey) -> String {
    match key {
        RouteKey::None => "没有路".to_owned(),
        RouteKey::Direct(addr) => format!("直连 {addr}"),
        RouteKey::Relay { path, .. } => format!("经 {}", relays_text(path)),
    }
}

/// 延迟优先模式换路的原因。`candidates` 是做决定时的候选，用来写出代价。
pub(super) fn explain(why: Why, to: RouteKey, from: RouteKey, candidates: &[Candidate], hint: Option<RouteHint>) -> Reason {
    let chosen = match to {
        RouteKey::None => None,
        RouteKey::Direct(addr) => Some(PathId::Direct(addr)),
        RouteKey::Relay { path, .. } => Some(PathId::Relays(path)),
    };
    let cost = candidates.iter().find(|candidate| Some(candidate.path) == chosen).map_or(f64::NAN, |candidate| candidate.cost_ms);
    let count = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    let (to_text, from_text) = (route_text(to), route_text(from));
    match why {
        Why::Lan => Reason::new(format!("对端在同一个局域网里，局域网直连无条件优先，改走{to_text}"), RouteCause::Lan),
        Why::First => Reason::new(
            format!("延迟优先：{} 条候选里{to_text}的代价最低（{cost:.0}ms）", candidates.len()),
            RouteCause::LowestCost { candidates: count, cost_us: micros(cost) },
        ),
        Why::Failover => {
            let hint = hint.unwrap_or(RouteHint::RouteFailed);
            Reason::new(
                format!("{}，不等迟滞，立刻换到剩下的 {} 条候选里代价最低的{to_text}（{cost:.0}ms）", hint_text(hint), candidates.len()),
                RouteCause::Failover { hint, candidates: count, cost_us: micros(cost) },
            )
        }
        Why::Cheaper { from_ms, to_ms } => Reason::new(
            format!(
                "延迟优先：{to_text}的代价（{to_ms:.0}ms）连续 {SWITCH_WINDOWS} 个评估窗口都比原来的{from_text}（{from_ms:.0}ms）低至少 max(5ms, 10%)，切换"
            ),
            RouteCause::Cheaper {
                previous: from.to_route(),
                previous_cost_us: micros(from_ms).unwrap_or_default(),
                cost_us: micros(to_ms).unwrap_or_default(),
                windows: u32::from(SWITCH_WINDOWS),
            },
        ),
        Why::NoPath => {
            let hint = hint.unwrap_or(RouteHint::NoPath);
            Reason::new(hint_text(hint), RouteCause::Unreachable { hint })
        }
    }
}

/// 默认模式换路的原因。`peer` 是这条路通向的对端。延迟优先模式下路径没变、只是到 relay 的会话换了传输时也用它。
pub(super) fn describe(peer: NodeId, from: RouteKey, to: RouteKey, hint: Option<RouteHint>) -> Reason {
    let transport = |via: Via| match via {
        Via::Udp(_) => "UDP",
        Via::Tcp(_) => "TCP",
        Via::Local => "本节点的 relay",
    };
    // 为什么是这个 relay。
    let relay_choice = |path: RelayPath, via: Via| match (path.as_slice(), via) {
        (_, Via::Local) => "本节点自己的 relay（对端在它上面有会话，不用绕别的 relay）".to_owned(),
        (&[relay], Via::Udp(_) | Via::Tcp(_)) if relay == peer => {
            format!("对端自己的 relay（它本身就是 relay，会话走 {}，不用绕别的 relay）", transport(via))
        }
        (&[relay], Via::Udp(_) | Via::Tcp(_)) => format!("对端挂靠的 relay {relay}（会话走 {}）", transport(via)),
        (_, Via::Udp(_) | Via::Tcp(_)) => format!("{}（到第一个 relay 的会话走 {}）", relays_text(path), transport(via)),
    };
    // 新的路是不是两个节点之间直接的中转会话。
    let is_shortcut = |path: RelayPath, via: Via| via == Via::Local || *path.as_slice() == [peer];
    match (from, to) {
        (_, RouteKey::Direct(addr)) => Reason::new(format!("直连地址 {addr} 验证通过，改走直连"), RouteCause::DirectVerified),
        (RouteKey::Direct(_), RouteKey::Relay { path, via }) => {
            let hint = hint.unwrap_or(RouteHint::DirectUnavailable);
            Reason::new(format!("{}，退回{}", hint_text(hint), relay_choice(path, via)), RouteCause::DirectLost { hint })
        }
        (RouteKey::None, RouteKey::Relay { path, via }) => {
            Reason::new(format!("中转会话可用，经{}", relay_choice(path, via)), RouteCause::RelayReady)
        }
        (RouteKey::Relay { path: old, .. }, RouteKey::Relay { path, via }) if old == path => {
            let leg = match via {
                Via::Udp(_) => Some(LegTransport::Udp),
                Via::Tcp(_) => Some(LegTransport::Tcp),
                Via::Local => None,
            };
            Reason::new(format!("到 relay {} 的会话改走 {}", path.first(), transport(via)), RouteCause::LegChanged { transport: leg })
        }
        (RouteKey::Relay { path: old, .. }, RouteKey::Relay { path, via }) => {
            let hint = hint.unwrap_or(if is_shortcut(path, via) { RouteHint::Shortcut } else { RouteHint::RelayUnavailable });
            Reason::new(
                format!("{}，从 {} 换到{}", hint_text(hint), relays_text(old), relay_choice(path, via)),
                RouteCause::RelayChanged { hint, previous: old.as_slice().to_vec() },
            )
        }
        (RouteKey::Direct(_), RouteKey::None) => {
            let hint = hint.unwrap_or(RouteHint::DirectUnavailable);
            Reason::new(format!("{}，也没有 relay 可走", hint_text(hint)), RouteCause::Unreachable { hint })
        }
        (_, RouteKey::None) => {
            let hint = hint.unwrap_or(RouteHint::RelayLost);
            Reason::new(hint_text(hint), RouteCause::Unreachable { hint })
        }
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::Attachment;
    use i3t_proto::PublicKey;

    use super::*;
    use crate::direct::DirectPath;
    use crate::routing::Selector;
    use crate::table::PeerSlot;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn path(relays: &[u32]) -> RelayPath {
        RelayPath::new(&relays.iter().map(|&relay| node(relay)).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn every_switch_is_described_with_its_reason() {
        let peer = node(2);
        let direct = RouteKey::Direct(addr("203.0.113.7:4000"));
        let via_udp = RouteKey::Relay { path: path(&[9]), via: Via::Udp(addr("198.51.100.9:20330")) };
        let via_tcp = RouteKey::Relay { path: path(&[9]), via: Via::Tcp(1) };
        let other = RouteKey::Relay { path: path(&[8]), via: Via::Udp(addr("198.51.100.8:20330")) };
        let itself = RouteKey::Relay { path: path(&[2]), via: Via::Tcp(2) };
        let local = RouteKey::Relay { path: path(&[1]), via: Via::Local };

        assert_eq!(describe(peer, via_udp, direct, None).text, "直连地址 203.0.113.7:4000 验证通过，改走直连");
        assert_eq!(
            describe(peer, direct, via_udp, Some(RouteHint::DirectTimeouts)).text,
            "直连探测连续超时，退回对端挂靠的 relay 9（会话走 UDP）"
        );
        assert_eq!(describe(peer, RouteKey::None, via_tcp, None).text, "中转会话可用，经对端挂靠的 relay 9（会话走 TCP）");
        assert_eq!(describe(peer, via_tcp, via_udp, None).text, "到 relay 9 的会话改走 UDP");
        assert_eq!(
            describe(peer, via_udp, other, Some(RouteHint::PeerNotAttached)).text,
            "relay 说对端没有挂在它上面，从 relay 9 换到对端挂靠的 relay 8（会话走 UDP）"
        );
        assert_eq!(describe(peer, via_udp, other, None).text, "原来的 relay 不可用，从 relay 9 换到对端挂靠的 relay 8（会话走 UDP）");
        assert_eq!(describe(peer, direct, RouteKey::None, None).text, "直连不可用，也没有 relay 可走");
        assert_eq!(describe(peer, via_udp, RouteKey::None, None).text, "中转会话断开，没有别的路");

        // 两个节点之间直接的中转会话：说明为什么不走对端挂靠表里排在前面的 relay。
        assert_eq!(
            describe(peer, RouteKey::None, itself, None).text,
            "中转会话可用，经对端自己的 relay（它本身就是 relay，会话走 TCP，不用绕别的 relay）"
        );
        assert_eq!(
            describe(peer, via_udp, itself, None).text,
            "有了不绕路的中转会话，从 relay 9 换到对端自己的 relay（它本身就是 relay，会话走 TCP，不用绕别的 relay）"
        );
        assert_eq!(
            describe(peer, via_udp, local, None).text,
            "有了不绕路的中转会话，从 relay 9 换到本节点自己的 relay（对端在它上面有会话，不用绕别的 relay）"
        );

        // 从延迟优先换回默认模式时，原来的路可能是多级的。
        let two_hops = RouteKey::Relay { path: path(&[8, 9]), via: Via::Udp(addr("198.51.100.8:20330")) };
        assert_eq!(
            describe(peer, two_hops, via_udp, None).text,
            "原来的 relay 不可用，从 relay 8 → 9 换到对端挂靠的 relay 9（会话走 UDP）"
        );
        let two_hops_tcp = RouteKey::Relay { path: path(&[8, 9]), via: Via::Tcp(4) };
        assert_eq!(describe(peer, two_hops, two_hops_tcp, None).text, "到 relay 8 的会话改走 TCP");

        assert_eq!(direct.to_route(), Route::Direct { addr: addr("203.0.113.7:4000") });
        assert_eq!(via_udp.to_route(), Route::Relay { relays: vec![node(9)] });
        assert_eq!(two_hops.to_route(), Route::Relay { relays: vec![node(8), node(9)] });
        assert_eq!(RouteKey::None.to_route(), Route::None);
        assert_eq!(
            (route_text(direct), route_text(two_hops), route_text(RouteKey::None)),
            ("直连 203.0.113.7:4000".to_owned(), "经 relay 8 → 9".to_owned(), "没有路".to_owned())
        );
    }

    #[test]
    fn every_reason_carries_a_code_with_the_values_it_names() {
        let peer = node(2);
        let direct = RouteKey::Direct(addr("203.0.113.7:4000"));
        let via_udp = RouteKey::Relay { path: path(&[9]), via: Via::Udp(addr("198.51.100.9:20330")) };
        let via_tcp = RouteKey::Relay { path: path(&[9]), via: Via::Tcp(1) };
        let other = RouteKey::Relay { path: path(&[8]), via: Via::Udp(addr("198.51.100.8:20330")) };
        let itself = RouteKey::Relay { path: path(&[2]), via: Via::Tcp(2) };
        let cause = |from, to, hint| describe(peer, from, to, hint).cause;

        assert_eq!(cause(via_udp, direct, None), RouteCause::DirectVerified);
        assert_eq!(cause(direct, via_udp, Some(RouteHint::DirectLoss)), RouteCause::DirectLost { hint: RouteHint::DirectLoss });
        assert_eq!(cause(direct, via_udp, None), RouteCause::DirectLost { hint: RouteHint::DirectUnavailable }, "说不出起因时是默认的");
        assert_eq!(cause(RouteKey::None, via_tcp, None), RouteCause::RelayReady);
        assert_eq!(cause(via_udp, via_tcp, None), RouteCause::LegChanged { transport: Some(LegTransport::Tcp) });
        assert_eq!(
            cause(other, via_udp, Some(RouteHint::RelayDraining)),
            RouteCause::RelayChanged { hint: RouteHint::RelayDraining, previous: vec![node(8)] }
        );
        assert_eq!(cause(via_udp, itself, None), RouteCause::RelayChanged { hint: RouteHint::Shortcut, previous: vec![node(9)] });
        assert_eq!(cause(direct, RouteKey::None, None), RouteCause::Unreachable { hint: RouteHint::DirectUnavailable });
        assert_eq!(cause(via_udp, RouteKey::None, None), RouteCause::Unreachable { hint: RouteHint::RelayLost });

        let candidate =
            |path: PathId, cost_ms: f64| Candidate { path, legs: Vec::new(), cost_ms, load_percent: 0, lan: false, settled: true };
        let candidates = [candidate(PathId::Direct(addr("203.0.113.7:4000")), 12.5)];
        assert_eq!(
            explain(Why::First, direct, RouteKey::None, &candidates, None).cause,
            RouteCause::LowestCost { candidates: 1, cost_us: Some(12_500) }
        );
        assert_eq!(
            explain(Why::Cheaper { from_ms: 20.0, to_ms: 12.5 }, direct, via_udp, &candidates, None).cause,
            RouteCause::Cheaper { previous: via_udp.to_route(), previous_cost_us: 20_000, cost_us: 12_500, windows: 3 }
        );
        assert_eq!(
            explain(Why::Failover, via_udp, direct, &[], Some(RouteHint::DirectStale)).cause,
            RouteCause::Failover { hint: RouteHint::DirectStale, candidates: 0, cost_us: None },
            "候选里没有它时说不出代价"
        );
        assert_eq!(explain(Why::Lan, direct, via_udp, &candidates, None).cause, RouteCause::Lan);
        assert_eq!(explain(Why::NoPath, RouteKey::None, direct, &[], None).cause, RouteCause::Unreachable { hint: RouteHint::NoPath });
    }

    #[test]
    fn every_cost_based_decision_is_explained_with_the_numbers_behind_it() {
        let direct = RouteKey::Direct(addr("203.0.113.7:4000"));
        let two_hops = RouteKey::Relay { path: path(&[8, 9]), via: Via::Udp(addr("198.51.100.8:20330")) };
        let candidate =
            |path: PathId, cost_ms: f64| Candidate { path, legs: Vec::new(), cost_ms, load_percent: 0, lan: false, settled: true };
        let candidates = [candidate(PathId::Relays(path(&[8, 9])), 185.8), candidate(PathId::Direct(addr("203.0.113.7:4000")), 201.4)];

        assert_eq!(
            explain(Why::First, two_hops, RouteKey::None, &candidates, None).text,
            "延迟优先：2 条候选里经 relay 8 → 9的代价最低（186ms）"
        );
        assert_eq!(
            explain(Why::Cheaper { from_ms: 201.4, to_ms: 185.8 }, two_hops, direct, &candidates, None).text,
            "延迟优先：经 relay 8 → 9的代价（186ms）连续 3 个评估窗口都比原来的直连 203.0.113.7:4000（201ms）低至少 max(5ms, 10%)，切换"
        );
        assert_eq!(
            explain(Why::Failover, direct, two_hops, &candidates[1..], Some(RouteHint::RelayPathBroken)).text,
            "relay 说多级路径走不下去，不等迟滞，立刻换到剩下的 1 条候选里代价最低的直连 203.0.113.7:4000（201ms）"
        );
        assert_eq!(
            explain(Why::Failover, direct, two_hops, &candidates[1..], None).text,
            "原来的路不能用了，不等迟滞，立刻换到剩下的 1 条候选里代价最低的直连 203.0.113.7:4000（201ms）"
        );
        assert_eq!(
            explain(Why::Lan, direct, two_hops, &candidates, None).text,
            "对端在同一个局域网里，局域网直连无条件优先，改走直连 203.0.113.7:4000"
        );
        assert_eq!(explain(Why::NoPath, RouteKey::None, direct, &[], None).text, "没有任何可用的路径");
        assert_eq!(explain(Why::NoPath, RouteKey::None, direct, &[], Some(RouteHint::DirectTimeouts)).text, "直连探测连续超时");
    }

    fn peer_attached_to(relays: &[u32]) -> PeerState {
        PeerState {
            key: PublicKey::from_bytes([2; 32]),
            attached: relays.iter().zip(1..).map(|(&relay, rtt_ms)| Attachment { relay: node(relay), rtt_ms, shared: false }).collect(),
            endpoints: Vec::new(),
            is_relay: false,
            dial_out_only: false,
            direct: DirectPath::new(),
            slot: PeerSlot::new(node(2)),
            route: RouteKey::None,
            doubt: true,
            sources: Vec::new(),
            active_source: None,
            stranger_budget: crate::budget::Budget::new(1.0, 1.0, Instant::now()),
            hint: None,
            unattached: Vec::new(),
            suspect: Vec::new(),
            probe: crate::control::PathProbe::default(),
            selector: Selector::default(),
            candidates: Vec::new(),
            reason: Reason::no_route_yet(),
            switched: None,
            first_route: None,
            pin: None,
            pinned: false,
            policy_route: RouteKey::None,
            other_mode: None,
        }
    }

    #[test]
    fn a_relay_picked_the_moment_a_route_first_appeared_does_not_count_as_in_use_yet() {
        let now = Instant::now();
        let mut peer = peer_attached_to(&[8, 9]);
        peer.route = RouteKey::Relay { path: path(&[9]), via: Via::Udp(addr("198.51.100.9:20330")) };
        peer.policy_route = peer.route;
        peer.first_route = Some(now);
        assert_eq!(relay_in_use(&peer, now + Duration::from_millis(1)), None, "到 8 的会话晚到一点点，照样按对端的顺序排");
        assert_eq!(relay_in_use(&peer, now + FIRST_ROUTE_GRACE), Some(node(9)), "之后就是在用的 relay");
        peer.first_route = None;
        assert_eq!(relay_in_use(&peer, now), Some(node(9)));
        peer.policy_route = RouteKey::Direct(addr("203.0.113.7:4000"));
        assert_eq!(relay_in_use(&peer, now), None);
        // 主动测试把路固定在 relay 9 上时，"现在走着的 relay"仍然按选路规则选的那条算。
        peer.route = RouteKey::Relay { path: path(&[9]), via: Via::Udp(addr("198.51.100.9:20330")) };
        assert_eq!(relay_in_use(&peer, now), None);
    }

    #[test]
    fn a_relay_whose_session_just_connected_is_tried_after_the_ones_that_have_held() {
        let (itself, mine, listed) = (node(4), node(1), node(8));
        let order = vec![itself, mine, listed];
        let healthy = RelayStanding { avoided: false, refused: false, settled: true };
        let fresh = RelayStanding { settled: false, ..healthy };
        let ranked = |current: Option<NodeId>, fresh_ones: &[NodeId]| {
            rank_relays(order.clone(), current, |relay| if fresh_ones.contains(&relay) { fresh } else { healthy })
        };

        // 到对端自己（它是 relay）的会话刚重连上：先不换回去，留在对端挂靠的 relay 上。
        assert_eq!(ranked(Some(listed), &[itself]), [mine, listed, itself]);
        // 它保持够久之后回到最前面。
        assert_eq!(ranked(Some(listed), &[]), [itself, mine, listed]);
        // 已经走着的那个不因为刚连上往后排：刚启动时所有会话都是新的，选中的第一个不该马上又被换掉。
        assert_eq!(ranked(Some(itself), &[itself, mine, listed]), [itself, mine, listed]);
        assert_eq!(ranked(None, &[itself, mine, listed]), [itself, mine, listed], "全都是新的：按原来的优先顺序");
        // 通知过下线的排得比刚连上的还靠后。
        let standing = |relay| {
            if relay == itself {
                RelayStanding { avoided: true, ..healthy }
            } else if relay == mine {
                fresh
            } else {
                healthy
            }
        };
        assert_eq!(rank_relays(order.clone(), None, standing), [listed, mine, itself]);
    }

    #[test]
    fn the_relay_in_use_stays_first_among_the_peers_relays_unless_it_became_clearly_slower() {
        let (me, peer) = (node(1), node(2));
        let listed = |rtts: &[(u32, u32)], current: u32| {
            let mut state = peer_attached_to(&[]);
            state.attached = rtts.iter().map(|&(relay, rtt_ms)| Attachment { relay: node(relay), rtt_ms, shared: false }).collect();
            relay_candidates(me, peer, &state, Some(node(current))).skip(2).collect::<Vec<_>>()
        };
        // 对端到两个 relay 的延迟差不多，这一次上报把顺序换了：现在走着的 9 还是排在前面。
        assert_eq!(listed(&[(8, 340), (9, 350)], 9), [node(9), node(8)]);
        assert_eq!(listed(&[(8, 3), (9, 12)], 9), [node(9), node(8)], "差不到 10ms 也不算明显");
        // 9 明显变慢了（高出 20% 以上）：按对端上报的顺序来。
        assert_eq!(listed(&[(8, 340), (9, 440)], 9), [node(8), node(9)]);
        // 现在走着的 relay 已经不在对端的挂靠表里了：同样按上报的顺序。
        assert_eq!(listed(&[(8, 340), (7, 350)], 9), [node(8), node(7)]);
    }

    #[test]
    fn the_peer_itself_and_this_nodes_own_relay_come_before_the_relays_the_peer_lists() {
        let (me, peer) = (node(1), node(2));
        let order = |attached: &[u32]| relay_candidates(me, peer, &peer_attached_to(attached), None).collect::<Vec<_>>();
        // 对端只列了别的 relay：它自己和本节点仍然排在前面。
        assert_eq!(order(&[8, 9]), [peer, me, node(8), node(9)]);
        // 对端把本节点（或者它自己）列在后面也一样，而且不重复出现。
        assert_eq!(order(&[8, 1]), [peer, me, node(8)]);
        assert_eq!(order(&[8, 2, 1, 9]), [peer, me, node(8), node(9)]);
        assert_eq!(order(&[]), [peer, me]);
    }
}
