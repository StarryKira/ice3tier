//! 控制任务：路径层里所有"慢"的事情都在这一个任务里做，所以这些状态不需要锁。
//!
//! 它处理收包路径认出来的帧（relay 的应答、探测）、信令、TCP 中转腿的事件、netmap 的更新，并且每隔一小段时间
//! 把各个状态机的定时器推进一步。每处理完一批，就重新算一遍到每个对端该走哪条路，有变化才去改收发路径上的那张表。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use i3t_crypto::{NodeKey, Signature};
use i3t_model::Netmap;
use i3t_model::control::{Route, RouteEntry, Signal};
use i3t_model::netmap::{Attachment, Reach, RoutingMode};
use i3t_proto::{NodeId, PublicKey, addr, probe, session};
use i3t_underlay::AddrQuery;
use tokio::sync::{mpsc, watch};

use crate::direct::{Action, Change, DirectPath, Policy};
use crate::relays::{self, Identity, Leg, RelayLink, Standing, Wire};
use crate::table::{self, Hop, PeerSlot, RelaySlot, Shared};
use crate::tcp::{self, TcpEvent, TcpLegHandle};
use crate::timing::TICK;
use crate::{RouteChange, Status};

/// 多久重新看一次本机的网卡地址。
const LOCAL_ADDRESS_REFRESH: Duration = Duration::from_secs(5);
/// 多久重新考虑一次挂靠哪几个 relay。
const ATTACH_REVIEW: Duration = Duration::from_secs(1);
/// 多久刷新一次对外公布的状态（路由表里的延迟等）。路变了的时候不等这个间隔。
const STATUS_REFRESH: Duration = Duration::from_millis(500);
/// relay 说某个对端没挂在它上面之后，多久之内优先经别的 relay 去找这个对端。
const UNATTACHED_HOLD: Duration = Duration::from_secs(10);
/// 陌生地址来的 PING 每秒最多验多少个签名。认识的地址不受这个限制。
const STRANGER_VERIFIES_PER_SEC: f64 = 50.0;

/// 交给控制任务处理的事情。
#[derive(Debug)]
pub(crate) enum Ctrl {
    /// UDP 上收到的、归路径层自己处理的一帧。
    Frame { from: SocketAddr, frame: Inbound },
    /// 到某个 relay 的 TCP 上发生的事。
    Tcp { relay: NodeId, conn: u64, event: TcpEvent },
    /// 对端发来的信令。
    Signal { from: NodeId, signal: Signal },
    /// 有人在等路：别等下一个定时器，现在就把状态机推进一步。
    Wake,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Inbound {
    Session(session::Frame),
    Probe(probe::Frame),
    AddrReply(addr::Reply),
}

/// 经 relay 的那一段走的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Udp(SocketAddr),
    /// TCP，值是那条 TCP 的编号：重连之后编号变了，发包用的队列也要跟着换。
    Tcp(u64),
    /// relay 就是本节点自己。
    Local,
}

/// 到一个对端现在走哪条路。和收发路径上的 [`Hop`] 一一对应，多出来的用处是能比较"变没变"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteKey {
    None,
    Direct(SocketAddr),
    Relay { relay: NodeId, via: Via },
}

impl RouteKey {
    fn to_route(self) -> Route {
        match self {
            Self::None => Route::None,
            Self::Direct(addr) => Route::Direct { addr },
            // 现在只用一个 relay；多级中转时这里按经过的顺序列出全部。
            Self::Relay { relay, .. } => Route::Relay { relays: vec![relay] },
        }
    }
}

struct PeerState {
    key: PublicKey,
    /// 对端挂靠的 relay，按它到 relay 的延迟从低到高。
    attached: Vec<Attachment>,
    dial_out_only: bool,
    direct: DirectPath,
    slot: Arc<PeerSlot>,
    route: RouteKey,
    doubt: bool,
    /// 登记在收包表里的、算作这个对端的来源地址。
    sources: Vec<SocketAddr>,
    /// 下一次换路时写进日志的原因。
    hint: Option<&'static str>,
    /// 哪些 relay 说过这个对端没挂在它上面，到什么时候为止把它们排在别的 relay 后面。
    unattached: Vec<(NodeId, Instant)>,
}

struct RelayEntry {
    link: RelayLink,
    slot: Arc<RelaySlot>,
}

/// 限制给陌生地址验签名的次数：令牌桶，每秒 [`STRANGER_VERIFIES_PER_SEC`] 个。
struct VerifyBudget {
    tokens: f64,
    refilled: Instant,
}

impl VerifyBudget {
    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = (self.tokens + elapsed * STRANGER_VERIFIES_PER_SEC).min(STRANGER_VERIFIES_PER_SEC);
        self.refilled = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

pub(crate) struct Control {
    shared: Arc<Shared>,
    key: Arc<NodeKey>,
    netmap: Arc<Netmap>,
    /// 这三项来自 netmap：能不能用直连、能不能用 relay、本节点是不是只拨出。
    allow_direct: bool,
    allow_relay: bool,
    dial_out_only: bool,
    peers: HashMap<NodeId, PeerState>,
    relays: HashMap<NodeId, RelayEntry>,
    legs: HashMap<NodeId, TcpLegHandle>,
    relay_by_addr: HashMap<SocketAddr, NodeId>,
    attached: Vec<NodeId>,
    attach_reviewed: Option<Instant>,
    addr_query: AddrQuery,
    local_candidates: Vec<SocketAddr>,
    local_refreshed: Option<Instant>,
    endpoints: Vec<SocketAddr>,
    mapping_varies: Option<bool>,
    verify_budget: VerifyBudget,
    next_conn: u64,
    status_refreshed: Option<Instant>,
}

/// 把 [`RelayLink`] 要发的东西真的发出去。
struct LinkWire<'a> {
    shared: &'a Arc<Shared>,
    slot: &'a Arc<RelaySlot>,
    legs: &'a mut HashMap<NodeId, TcpLegHandle>,
    next_conn: &'a mut u64,
}

impl Wire for LinkWire<'_> {
    fn udp(&mut self, to: SocketAddr, frame: &[u8]) {
        self.shared.port.send_datagram(frame, to);
    }

    fn tcp(&mut self, frame: &[u8]) {
        if let Some(leg) = self.legs.get(&self.slot.id) {
            // 队列满了就丢，握手和保活都有自己的重试。
            leg.sender.try_send(Bytes::copy_from_slice(frame));
        }
    }

    fn tcp_connect(&mut self, addrs: &[SocketAddr]) {
        let conn = *self.next_conn;
        *self.next_conn += 1;
        let leg = tcp::spawn(Arc::clone(self.shared), Arc::clone(self.slot), conn, addrs.to_vec());
        self.legs.insert(self.slot.id, leg);
    }

    fn tcp_close(&mut self) {
        self.legs.remove(&self.slot.id);
    }
}

impl Control {
    pub(crate) fn new(shared: Arc<Shared>, key: Arc<NodeKey>, netmap: Arc<Netmap>, addr_servers: Vec<SocketAddr>) -> Self {
        let now = Instant::now();
        let mut control = Self {
            shared,
            key,
            netmap: Arc::clone(&netmap),
            allow_direct: true,
            allow_relay: true,
            dial_out_only: false,
            peers: HashMap::new(),
            relays: HashMap::new(),
            legs: HashMap::new(),
            relay_by_addr: HashMap::new(),
            attached: Vec::new(),
            attach_reviewed: None,
            addr_query: AddrQuery::new(addr_servers),
            local_candidates: Vec::new(),
            local_refreshed: None,
            endpoints: Vec::new(),
            mapping_varies: None,
            verify_budget: VerifyBudget { tokens: STRANGER_VERIFIES_PER_SEC, refilled: now },
            next_conn: 1,
            status_refreshed: None,
        };
        control.apply_netmap(netmap, now);
        control.poll(now);
        control
    }

    pub(crate) async fn run(mut self, mut ctrl: mpsc::Receiver<Ctrl>, mut netmap: watch::Receiver<Arc<Netmap>>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut netmap_open = true;
        loop {
            tokio::select! {
                message = ctrl.recv() => {
                    let Some(message) = message else { return };
                    let now = Instant::now();
                    self.handle(message, now);
                    // 攒着的一起处理完再重新选路，免得每一帧都把所有对端过一遍。
                    while let Ok(message) = ctrl.try_recv() {
                        self.handle(message, now);
                    }
                }
                changed = netmap.changed(), if netmap_open => match changed {
                    Ok(()) => {
                        let latest = Arc::clone(&netmap.borrow_and_update());
                        self.apply_netmap(latest, Instant::now());
                    }
                    // 发 netmap 的一方没了（节点在退出）：按最后一份继续。
                    Err(_) => netmap_open = false,
                },
                _ = tick.tick() => {}
            }
            self.poll(Instant::now());
        }
    }

    fn handle(&mut self, message: Ctrl, now: Instant) {
        match message {
            Ctrl::Frame { from, frame: Inbound::Session(frame) } => self.on_relay_frame(from, &frame, now),
            Ctrl::Frame { from, frame: Inbound::Probe(probe::Frame::Ping(ping)) } => self.on_ping(from, &ping, now),
            Ctrl::Frame { from, frame: Inbound::Probe(probe::Frame::Pong(pong)) } => self.on_pong(from, &pong, now),
            Ctrl::Frame { from, frame: Inbound::AddrReply(reply) } => {
                if let Some(observation) = self.addr_query.handle_reply(from, &reply, now) {
                    tracing::debug!(observer = %from, observed = %observation.observed, "地址查询有了回答");
                }
            }
            Ctrl::Tcp { relay, conn, event } => self.on_tcp(relay, conn, &event, now),
            Ctrl::Signal { from, signal } => self.on_signal(from, &signal, now),
            Ctrl::Wake => {}
        }
    }

    /// 按新的 netmap 更新对端、relay 和各种限制。
    fn apply_netmap(&mut self, netmap: Arc<Netmap>, now: Instant) {
        let me = self.shared.me;
        if netmap.me.id != me {
            tracing::error!(expected = %me, got = %netmap.me.id, "netmap 里本节点的编号变了，忽略这份 netmap");
            return;
        }
        let reach = netmap.me.reach.unwrap_or(netmap.network.reach);
        self.allow_direct = reach != Reach::RelayOnly;
        self.allow_relay = reach != Reach::DirectOnly;
        self.dial_out_only = netmap.me.dial_out_only;
        if netmap.me.routing.unwrap_or(netmap.network.routing) == RoutingMode::LowestCost {
            tracing::warn!("netmap 要求延迟优先的选路模式，这个版本还没有实现，按默认模式选路（直连优先，否则经一个 relay）");
        }

        let mut table = self.shared.table_mut();
        let gone: Vec<NodeId> = self.peers.keys().copied().filter(|id| netmap.peer(*id).is_none()).collect();
        for id in gone {
            let Some(peer) = self.peers.remove(&id) else { continue };
            table.peers.remove(&id);
            for source in &peer.sources {
                table.direct.remove(source);
            }
            peer.slot.set_hop(Hop::None);
            if peer.route != RouteKey::None {
                self.announce(id, RouteKey::None, "对端不在 netmap 里了".to_owned());
            }
        }
        for info in &netmap.peers {
            if info.id == me {
                continue;
            }
            let peer = self.peers.entry(info.id).or_insert_with(|| {
                let slot = PeerSlot::new(info.id);
                table.peers.insert(info.id, Arc::clone(&slot));
                PeerState {
                    key: info.key,
                    attached: Vec::new(),
                    dial_out_only: false,
                    direct: DirectPath::new(),
                    slot,
                    route: RouteKey::None,
                    doubt: true,
                    sources: Vec::new(),
                    hint: None,
                    unattached: Vec::new(),
                }
            });
            if peer.key != info.key {
                // 换了密钥就是另一个身份：以前验证过的地址全部作废。
                tracing::info!(peer = %info.id, "对端的公钥变了，重新验证它的直连地址");
                peer.key = info.key;
                peer.direct = DirectPath::new();
            }
            peer.attached.clone_from(&info.attached);
            peer.dial_out_only = info.dial_out_only;
            peer.direct.set_reported(&info.endpoints, now);
        }

        let gone: Vec<NodeId> = self.relays.keys().copied().filter(|id| netmap.relay(*id).is_none()).collect();
        for id in gone {
            self.relays.remove(&id);
            self.legs.remove(&id);
            tracing::info!(relay = %id, "relay 不在 netmap 里了，到它的中转会话关闭");
        }
        for info in netmap.relays.iter().filter(|relay| relay.id != me) {
            let entry =
                self.relays.entry(info.id).or_insert_with(|| RelayEntry { link: RelayLink::new(info, now), slot: RelaySlot::new(info.id) });
            entry.link.update(info);
        }
        self.relay_by_addr =
            self.relays.iter().flat_map(|(&id, entry)| entry.link.udp_addrs().iter().map(move |&addr| (addr, id))).collect();
        table.relays =
            self.relays.values().flat_map(|entry| entry.link.udp_addrs().iter().map(|&addr| (addr, Arc::clone(&entry.slot)))).collect();
        drop(table);
        self.netmap = netmap;
        // 对端和 relay 都可能变了：挂靠关系马上重新考虑，对外公布的状态马上刷新。
        self.attach_reviewed = None;
        self.status_refreshed = None;
    }

    fn on_relay_frame(&mut self, from: SocketAddr, frame: &session::Frame, now: Instant) {
        let Some(&relay) = self.relay_by_addr.get(&from) else {
            tracing::trace!(%from, "中转会话帧不是从 netmap 里的 relay 地址来的，丢弃");
            return;
        };
        let Self { shared, key, relays, legs, next_conn, .. } = self;
        let Some(entry) = relays.get_mut(&relay) else { return };
        let who = Identity { me: shared.me, key };
        let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
        if let Some(peer) = entry.link.on_udp_frame(frame, now, who, &mut wire) {
            self.mark_unattached(relay, peer, now);
        }
    }

    fn on_tcp(&mut self, relay: NodeId, conn: u64, event: &TcpEvent, now: Instant) {
        // 同一个 relay 可能先后连过几次，只认现在这一条上的事件。
        if self.legs.get(&relay).is_none_or(|leg| leg.conn != conn) {
            return;
        }
        let Self { shared, key, relays, legs, next_conn, .. } = self;
        let Some(entry) = relays.get_mut(&relay) else { return };
        let who = Identity { me: shared.me, key };
        let unattached = match event {
            TcpEvent::Connected => {
                let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
                entry.link.on_tcp_connected(now, who, &mut wire);
                None
            }
            TcpEvent::Frame(frame) => {
                let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
                entry.link.on_tcp_frame(frame, now, who, &mut wire)
            }
            TcpEvent::Closed => {
                legs.remove(&relay);
                entry.link.on_tcp_closed(now);
                None
            }
        };
        if let Some(peer) = unattached {
            self.mark_unattached(relay, peer, now);
        }
    }

    /// relay 说 `peer` 没有挂在它上面：一段时间里先经对端挂靠的别的 relay 去找它。
    fn mark_unattached(&mut self, relay: NodeId, peer: NodeId, now: Instant) {
        let Some(state) = self.peers.get_mut(&peer) else { return };
        state.unattached.retain(|(id, _)| *id != relay);
        state.unattached.push((relay, now + UNATTACHED_HOLD));
        state.hint = Some("relay 说对端没有挂在它上面");
    }

    fn on_signal(&mut self, from: NodeId, signal: &Signal, now: Instant) {
        if !self.allow_direct {
            tracing::debug!(peer = %from, "本节点不建立直连，忽略打洞信令");
            return;
        }
        let Some(peer) = self.peers.get_mut(&from) else {
            tracing::debug!(peer = %from, "信令来自不在 netmap 里的节点，忽略");
            return;
        };
        let policy = direct_policy(&self.shared, self.allow_direct, self.dial_out_only, &self.endpoints, peer);
        if !policy.signal {
            tracing::debug!(peer = %from, "本节点或者对端是只拨出的，不参与打洞，忽略信令");
            return;
        }
        // 信令是经 controller 或者已经认证的连接送来的，对端确实想和本端建立直连：把它当作活跃的，后面几轮探测才会跟上。
        peer.slot.last_demand.touch(&self.shared.clock);
        let actions = peer.direct.on_signal(signal, now, &policy);
        self.perform(from, actions);
    }

    /// 对端的 PING：验签名，回 PONG，并且反过来验证它的来源地址。
    fn on_ping(&mut self, from: SocketAddr, ping: &probe::Ping, now: Instant) {
        let me = self.shared.me;
        if ping.dst != me || !self.allow_direct {
            return;
        }
        let Some(peer) = self.peers.get_mut(&ping.src) else {
            tracing::trace!(%from, src = %ping.src, "PING 自称来自不在 netmap 里的节点，丢弃");
            return;
        };
        if self.dial_out_only && !peer.slot.is_active(&self.shared.clock) {
            // 只拨出的节点不理别人主动找上来的探测，只有它自己正在连的对端除外。
            return;
        }
        if !peer.sources.contains(&from) && !self.verify_budget.take(now) {
            tracing::debug!(%from, "陌生地址来的 PING 太多，这一个不验了");
            return;
        }
        let message = probe::Ping::signed_message(ping.src, ping.dst, &ping.nonce);
        if i3t_crypto::verify(&peer.key, probe::Ping::PURPOSE, &message, &Signature::from_bytes(ping.signature)).is_err() {
            tracing::debug!(%from, src = %ping.src, "PING 的签名不对，丢弃");
            return;
        }
        if peer.direct.active().is_some_and(|(addr, _)| addr == from) {
            peer.slot.last_rx_direct.touch(&self.shared.clock);
        }
        let message = probe::Pong::signed_message(me, ping.src, &ping.nonce, from);
        let signature = *self.key.sign(probe::Pong::PURPOSE, &message).as_bytes();
        let pong = probe::Pong { src: me, dst: ping.src, nonce: ping.nonce, observed: from, signature };
        self.shared.port.send_datagram(&pong.to_bytes(), from);
        let reverse = peer.direct.on_ping(from, now);
        self.perform(ping.src, reverse);
    }

    fn on_pong(&mut self, from: SocketAddr, pong: &probe::Pong, now: Instant) {
        if pong.dst != self.shared.me || !self.allow_direct {
            return;
        }
        let Some(peer) = self.peers.get_mut(&pong.src) else { return };
        // 先看它是不是在回答本端发过的 PING，是才值得验签名。
        if !peer.direct.expects_pong(from, &pong.nonce) {
            return;
        }
        let message = probe::Pong::signed_message(pong.src, pong.dst, &pong.nonce, pong.observed);
        if i3t_crypto::verify(&peer.key, probe::Pong::PURPOSE, &message, &Signature::from_bytes(pong.signature)).is_err() {
            tracing::debug!(%from, src = %pong.src, "PONG 的签名不对，丢弃");
            return;
        }
        if peer.direct.active().is_some_and(|(addr, _)| addr == from) {
            peer.slot.last_rx_direct.touch(&self.shared.clock);
        }
        if let Some(Change::Up { addr }) = peer.direct.on_pong(from, &pong.nonce, now) {
            tracing::debug!(peer = %pong.src, %addr, "直连地址验证通过");
        }
    }

    /// 把直连状态机要做的事做掉：给 PING 签名发出去，把信令交给钩子。
    fn perform(&self, peer: NodeId, actions: impl IntoIterator<Item = Action>) {
        for action in actions {
            match action {
                Action::Ping { to, nonce } => {
                    let message = probe::Ping::signed_message(self.shared.me, peer, &nonce);
                    let signature = *self.key.sign(probe::Ping::PURPOSE, &message).as_bytes();
                    let ping = probe::Ping { src: self.shared.me, dst: peer, nonce, signature };
                    self.shared.port.send_datagram(&ping.to_bytes(), to);
                }
                Action::Signal(signal) => self.shared.send_signal(peer, &signal),
            }
        }
    }

    /// 把所有的定时器推进一步，然后重新选路。
    fn poll(&mut self, now: Instant) {
        self.refresh_endpoints(now);
        if self.allow_direct {
            self.addr_query.poll(&self.shared.port, now);
        }
        let clock = &self.shared.clock;
        let active: HashSet<NodeId> = self.peers.iter().filter(|(_, peer)| peer.slot.is_active(clock)).map(|(&id, _)| id).collect();
        self.review_attachments(now);
        self.poll_relays(now, &active);
        self.poll_directs(now);
        let changed = self.update_routes(now);
        if changed || self.status_refreshed.is_none_or(|at| now.saturating_duration_since(at) >= STATUS_REFRESH) {
            self.publish_status(now);
        }
    }

    /// 本节点的候选端点：网卡地址，加上 relay 和地址查询服务器看到的地址。
    fn refresh_endpoints(&mut self, now: Instant) {
        if self.local_refreshed.is_none_or(|at| now.saturating_duration_since(at) >= LOCAL_ADDRESS_REFRESH) {
            self.local_candidates = self.shared.port.local_candidates();
            self.local_refreshed = Some(now);
        }
        // 只有公网上的观察点才拿来判断"映射出来的端口是否随对端变化"。
        let observed: Vec<SocketAddr> = self
            .relays
            .values()
            .filter_map(|entry| entry.link.observed(now))
            .chain(self.addr_query.observations(now).map(|observation| observation.observed))
            .collect();
        self.mapping_varies = mapping_varies(&observed);

        // 不建立直连的节点不向别人暴露自己的地址（白皮书 §7.1 的 relay-only）。
        let mut endpoints = if self.allow_direct { [self.local_candidates.as_slice(), observed.as_slice()].concat() } else { Vec::new() };
        endpoints.sort_unstable();
        endpoints.dedup();
        if endpoints != self.endpoints {
            tracing::debug!(?endpoints, "本节点的候选端点变了");
            self.endpoints = endpoints;
            for peer in self.peers.values_mut() {
                peer.direct.local_changed();
            }
        }
    }

    /// 重新考虑挂靠哪几个 relay。
    fn review_attachments(&mut self, now: Instant) {
        if self.attach_reviewed.is_some_and(|at| now.saturating_duration_since(at) < ATTACH_REVIEW) {
            return;
        }
        self.attach_reviewed = Some(now);
        // 按 netmap 里的顺序列出来：延迟还没测出来的时候就按这个顺序用。
        let standings: Vec<Standing> = self
            .netmap
            .relays
            .iter()
            .filter_map(|relay| Some((relay.id, self.relays.get(&relay.id)?)))
            .map(|(id, entry)| Standing {
                id,
                rtt: entry.link.estimate(now),
                failing: self.attached.contains(&id) && entry.link.is_failing(now),
            })
            .collect();
        let chosen =
            if self.allow_relay { relays::choose_attached(&self.attached, &standings, &self.netmap.me.pinned_relays) } else { Vec::new() };
        if chosen != self.attached {
            let reason = if !self.allow_relay {
                "本节点不使用 relay"
            } else if !self.netmap.me.pinned_relays.is_empty() {
                "管理员固定了挂靠的 relay"
            } else {
                "按到各个 relay 的延迟选择"
            };
            tracing::info!(from = ?self.attached, to = ?chosen, reason, "挂靠的 relay 变了");
            self.attached = chosen;
        }
    }

    /// 决定和哪些 relay 保持会话，并推进到每个 relay 的状态机。
    fn poll_relays(&mut self, now: Instant, active: &HashSet<NodeId>) {
        let mut wanted: HashSet<NodeId> = self.attached.iter().copied().collect();
        if self.allow_relay {
            // 能通到活跃对端的 relay 都连着（对端自己是 relay 的话也包括它）：走 relay 时要经它们，走直连时它们是随时能退回去的备用路。
            for (&id, peer) in self.peers.iter().filter(|(id, _)| active.contains(id)) {
                wanted.extend(relay_candidates(self.shared.me, id, peer));
            }
        }
        let survey = self.allow_relay && self.netmap.me.pinned_relays.is_empty();
        let Self { shared, key, relays, legs, next_conn, .. } = self;
        let who = Identity { me: shared.me, key };
        for (id, entry) in relays.iter_mut() {
            let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
            let want = wanted.contains(id);
            if want != entry.link.is_wanted() {
                tracing::debug!(relay = %id, want, "是否和这个 relay 保持中转会话变了");
                entry.link.set_wanted(want, now, &mut wire);
            }
            let urgent = table::unanswered(&shared.clock, &entry.slot.last_tx, &entry.slot.last_rx);
            entry.link.poll(now, who, urgent, survey, &mut wire);
        }
    }

    fn poll_directs(&mut self, now: Instant) {
        let mut todo = Vec::new();
        for (&id, peer) in &mut self.peers {
            let policy = direct_policy(&self.shared, self.allow_direct, self.dial_out_only, &self.endpoints, peer);
            let step = peer.direct.poll(now, &policy);
            if let Some(Change::Down { reason }) = step.change {
                peer.hint = Some(reason.describe());
            }
            if !step.actions.is_empty() {
                todo.push((id, step.actions));
            }
        }
        for (id, actions) in todo {
            self.perform(id, actions);
        }
    }

    /// 重新算一遍到每个对端走哪条路，把变化写进收发路径上的表。返回有没有变化。
    fn update_routes(&mut self, now: Instant) -> bool {
        let Self { shared, relays, legs, peers, allow_direct, allow_relay, .. } = self;
        let mut announcements = Vec::new();
        for (&id, peer) in peers.iter_mut() {
            let sources: Vec<SocketAddr> = if *allow_direct { peer.direct.sources(now).collect() } else { Vec::new() };
            if sources != peer.sources {
                let mut table = shared.table_mut();
                for source in &peer.sources {
                    table.direct.remove(source);
                }
                for &source in &sources {
                    table.direct.insert(source, Arc::clone(&peer.slot));
                }
                peer.sources = sources;
            }
            peer.unattached.retain(|(_, until)| now < *until);

            let (key, hop) = choose(shared, relays, legs, *allow_direct, *allow_relay, id, peer);
            let doubt = match key {
                RouteKey::None => true,
                RouteKey::Direct(_) => peer.direct.in_doubt(),
                RouteKey::Relay { via: Via::Local, .. } => false,
                RouteKey::Relay { relay, via: Via::Udp(_) | Via::Tcp(_) } => relays.get(&relay).is_some_and(|entry| entry.link.in_doubt()),
            };
            let changed = key != peer.route;
            // 可疑的路又通了，或者换了一条路（包括从没有路到有路）：把攒着的包补发出去。
            let restored = key != RouteKey::None && (changed || (peer.doubt && !doubt));
            if changed {
                peer.slot.set_hop(hop);
                announcements.push((id, key, describe(id, peer.route, key, peer.hint.take())));
                peer.route = key;
            }
            if doubt != peer.doubt {
                peer.slot.set_doubt(doubt);
                peer.doubt = doubt;
            }
            if restored {
                peer.slot.flush(shared, now);
            }
        }
        let changed = !announcements.is_empty();
        for (id, key, reason) in announcements {
            self.announce(id, key, reason);
        }
        changed
    }

    /// 每一次换路都记一条带原因的日志，并且通知订阅的人。
    fn announce(&self, peer: NodeId, key: RouteKey, reason: String) {
        let route = key.to_route();
        tracing::info!(%peer, ?route, reason, "到对端的路变了");
        // 没有人订阅时发送会失败，那就是没有人要听。
        let _subscribers = self.shared.changes.send(RouteChange { peer, route, reason });
        self.shared.route_changed.notify_waiters();
    }

    fn publish_status(&mut self, now: Instant) {
        self.status_refreshed = Some(now);
        let mut routes: Vec<RouteEntry> = self
            .peers
            .iter()
            .map(|(&id, peer)| {
                let rtt_ms = match peer.route {
                    RouteKey::None => None,
                    RouteKey::Direct(_) => peer.direct.active().map(|(_, rtt)| rtt.millis()),
                    // 经 relay 的延迟是两段之和：本节点到 relay 是自己测的，relay 到对端是对端上报的。
                    RouteKey::Relay { relay, .. } => {
                        let near = self.relays.get(&relay).and_then(|entry| entry.link.rtt()).map(|rtt| rtt.millis());
                        let far = peer.attached.iter().find(|attachment| attachment.relay == relay).map(|attachment| attachment.rtt_ms);
                        match (near, far) {
                            // relay 就是对端自己：只有一段。
                            (near, None) if relay == id => near,
                            (near, Some(far)) => Some(near.unwrap_or(0).saturating_add(far)),
                            (_, None) => None,
                        }
                    }
                };
                RouteEntry { peer: id, route: peer.route.to_route(), rtt_ms }
            })
            .collect();
        routes.sort_unstable_by_key(|entry| entry.peer);
        let mut attached: Vec<Attachment> = self
            .attached
            .iter()
            .filter_map(|id| Some(Attachment { relay: *id, rtt_ms: self.relays.get(id)?.link.rtt()?.millis() }))
            .collect();
        attached.sort_by_key(|attachment| attachment.rtt_ms);
        let status = Status { routes, endpoints: self.endpoints.clone(), mapping_varies: self.mapping_varies, attached };
        *self.shared.status.lock().expect("path status mutex poisoned") = status;
    }
}

/// 这个对端此刻适用的直连限制。
fn direct_policy<'a>(
    shared: &Shared,
    allow_direct: bool,
    node_dial_out_only: bool,
    endpoints: &'a [SocketAddr],
    peer: &PeerState,
) -> Policy<'a> {
    let clock = &shared.clock;
    Policy {
        wanted: allow_direct && peer.slot.is_active(clock),
        initiate: !peer.dial_out_only,
        signal: !node_dial_out_only && !peer.dial_out_only,
        silent: matches!(peer.route, RouteKey::Direct(_)) && table::unanswered(clock, &peer.slot.last_tx, &peer.slot.last_rx_direct),
        local: endpoints,
    }
}

/// 默认的选路模式：直连可用就走直连；否则经一个 relay，按 [`relay_candidates`] 的顺序取第一个现在能用的。
fn choose(
    shared: &Shared,
    relays: &HashMap<NodeId, RelayEntry>,
    legs: &HashMap<NodeId, TcpLegHandle>,
    allow_direct: bool,
    allow_relay: bool,
    id: NodeId,
    peer: &PeerState,
) -> (RouteKey, Hop) {
    if allow_direct && let Some((addr, _)) = peer.direct.active() {
        return (RouteKey::Direct(addr), Hop::Direct(addr));
    }
    if !allow_relay {
        return (RouteKey::None, Hop::None);
    }
    let candidates = || relay_candidates(shared.me, id, peer);
    let refused = |relay: &NodeId| peer.unattached.iter().any(|(held, _)| held == relay);
    // 刚说过"对端没挂在我这里"的 relay 排到后面，但不是完全不用：只有它一个可走时，对端可能只是还没来得及挂上去。
    let in_order = candidates().filter(|relay| !refused(relay)).chain(candidates().filter(refused));
    for relay in in_order {
        if relay == shared.me {
            if shared.local_relay.get().is_some_and(|local| local.has_session(id)) {
                return (RouteKey::Relay { relay, via: Via::Local }, Hop::Local);
            }
            continue;
        }
        // 对端不是 relay 时，"对端自己"这个候选在 relay 表里找不到，自然跳过。
        let Some(entry) = relays.get(&relay) else { continue };
        match entry.link.leg() {
            Some(Leg::Udp(addr)) => {
                return (RouteKey::Relay { relay, via: Via::Udp(addr) }, Hop::RelayUdp { relay: Arc::clone(&entry.slot), addr });
            }
            Some(Leg::Tcp) => {
                if let Some(leg) = legs.get(&relay) {
                    return (
                        RouteKey::Relay { relay, via: Via::Tcp(leg.conn) },
                        Hop::RelayTcp { relay: Arc::clone(&entry.slot), sender: leg.sender.clone() },
                    );
                }
            }
            None => {}
        }
    }
    (RouteKey::None, Hop::None)
}

/// 经哪些 relay 能到这个对端，按优先顺序。本节点会和其中是 relay 的那些保持会话。
///
/// 1. **对端自己。** 对端本身是启用了的 relay 时，在它那里建一个会话，把中转帧直接发给它，它收到目的是自己的帧就地收下。
///    这相当于它"挂靠在自己身上、延迟为 0"：包只走本节点到对端这一段，不绕第三个节点。UDP 完全用不了的节点
///    就是这样用一条 TCP 连到有公网端口的对端的（白皮书 §5.5 的情形 3）。
/// 2. **本节点自己。** 本节点是 relay、对端在它上面有会话时，经那个会话直接送过去，同样不绕路。
///    对端的挂靠表是按它自己的延迟排的，别的 relay 排在前面也不该先用。
/// 3. **对端挂靠的 relay**，按它上报的延迟从低到高。只有前两条都走不通时才用。
///
/// 前两个候选不适用时（对端不是 relay、本节点不是 relay 或者对端没挂在它上面）在选路时自然落空。
fn relay_candidates(me: NodeId, id: NodeId, peer: &PeerState) -> impl Iterator<Item = NodeId> + '_ {
    let listed = peer.attached.iter().map(|attachment| attachment.relay).filter(move |relay| *relay != id && *relay != me);
    [id, me].into_iter().chain(listed)
}

/// 换路的原因，给人看的。`peer` 是这条路通向的对端。
fn describe(peer: NodeId, from: RouteKey, to: RouteKey, hint: Option<&'static str>) -> String {
    let transport = |via: Via| match via {
        Via::Udp(_) => "UDP",
        Via::Tcp(_) => "TCP",
        Via::Local => "本节点的 relay",
    };
    // 为什么是这个 relay。
    let relay_choice = |relay: NodeId, via: Via| match via {
        Via::Local => "本节点自己的 relay（对端在它上面有会话，不用绕别的 relay）".to_owned(),
        Via::Udp(_) | Via::Tcp(_) if relay == peer => {
            format!("对端自己的 relay（它本身就是 relay，会话走 {}，不用绕别的 relay）", transport(via))
        }
        Via::Udp(_) | Via::Tcp(_) => format!("对端挂靠的 relay {relay}（会话走 {}）", transport(via)),
    };
    // 新的路是不是两个节点之间直接的中转会话。
    let is_shortcut = |relay: NodeId, via: Via| via == Via::Local || relay == peer;
    match (from, to) {
        (_, RouteKey::Direct(addr)) => format!("直连地址 {addr} 验证通过，改走直连"),
        (RouteKey::Direct(_), RouteKey::Relay { relay, via }) => {
            format!("{}，退回{}", hint.unwrap_or("直连不可用"), relay_choice(relay, via))
        }
        (RouteKey::None, RouteKey::Relay { relay, via }) => format!("中转会话可用，经{}", relay_choice(relay, via)),
        (RouteKey::Relay { relay: old, .. }, RouteKey::Relay { relay, via }) if old == relay => {
            format!("到 relay {relay} 的会话改走 {}", transport(via))
        }
        (RouteKey::Relay { relay: old, .. }, RouteKey::Relay { relay, via }) => {
            let cause =
                hint.unwrap_or(if is_shortcut(relay, via) { "有了不绕路的中转会话" } else { "原来的 relay 不可用" });
            format!("{cause}，从 relay {old} 换到{}", relay_choice(relay, via))
        }
        (RouteKey::Direct(_), RouteKey::None) => format!("{}，也没有 relay 可走", hint.unwrap_or("直连不可用")),
        (_, RouteKey::None) => hint.unwrap_or("中转会话断开，没有别的路").to_owned(),
    }
}

/// 映射出来的端口是不是随对端变化。要有两个以上的观察点看到同一个地址族的地址才知道。
fn mapping_varies(observed: &[SocketAddr]) -> Option<bool> {
    let verdict = |family: fn(&SocketAddr) -> bool| {
        let mut seen = observed.iter().filter(|addr| family(addr));
        let first = seen.next()?;
        let mut rest = seen.peekable();
        rest.peek()?;
        Some(rest.any(|addr| addr != first))
    };
    // IPv4 上才有地址转换的问题，先看它；没有 IPv4 的观察时再看 IPv6（有的网络也给 IPv6 做转换）。
    verdict(SocketAddr::is_ipv4).or_else(|| verdict(SocketAddr::is_ipv6))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    #[test]
    fn mapping_variation_needs_two_observers_of_the_same_family() {
        assert_eq!(mapping_varies(&[]), None);
        assert_eq!(mapping_varies(&[addr("203.0.113.7:4000")]), None);
        assert_eq!(mapping_varies(&[addr("203.0.113.7:4000"), addr("[2001:db8::1]:4000")]), None, "两个地址族各一个，比不了");
        assert_eq!(mapping_varies(&[addr("203.0.113.7:4000"), addr("203.0.113.7:4000")]), Some(false));
        assert_eq!(mapping_varies(&[addr("203.0.113.7:4000"), addr("203.0.113.7:4001")]), Some(true), "端口随对端变化");
        assert_eq!(mapping_varies(&[addr("203.0.113.7:4000"), addr("203.0.113.8:4000")]), Some(true));
        let v6 = [addr("[2001:db8::1]:4000"), addr("[2001:db8::1]:4000")];
        assert_eq!(mapping_varies(&v6), Some(false));
    }

    #[test]
    fn the_verify_budget_refills_over_time() {
        let start = Instant::now();
        let mut budget = VerifyBudget { tokens: STRANGER_VERIFIES_PER_SEC, refilled: start };
        let burst = (0..1000).take_while(|_| budget.take(start)).count();
        assert_eq!(burst, 50);
        assert!(!budget.take(start));
        assert!(budget.take(start + Duration::from_millis(20)), "20ms 攒出一个");
        assert!(!budget.take(start + Duration::from_millis(21)));
    }

    #[test]
    fn every_switch_is_described_with_its_reason() {
        let peer = node(2);
        let direct = RouteKey::Direct(addr("203.0.113.7:4000"));
        let via_udp = RouteKey::Relay { relay: node(9), via: Via::Udp(addr("198.51.100.9:20330")) };
        let via_tcp = RouteKey::Relay { relay: node(9), via: Via::Tcp(1) };
        let other = RouteKey::Relay { relay: node(8), via: Via::Udp(addr("198.51.100.8:20330")) };
        let itself = RouteKey::Relay { relay: peer, via: Via::Tcp(2) };
        let local = RouteKey::Relay { relay: node(1), via: Via::Local };

        assert_eq!(describe(peer, via_udp, direct, None), "直连地址 203.0.113.7:4000 验证通过，改走直连");
        assert_eq!(describe(peer, direct, via_udp, Some("直连探测连续超时")), "直连探测连续超时，退回对端挂靠的 relay 9（会话走 UDP）");
        assert_eq!(describe(peer, RouteKey::None, via_tcp, None), "中转会话可用，经对端挂靠的 relay 9（会话走 TCP）");
        assert_eq!(describe(peer, via_tcp, via_udp, None), "到 relay 9 的会话改走 UDP");
        assert_eq!(
            describe(peer, via_udp, other, Some("relay 说对端没有挂在它上面")),
            "relay 说对端没有挂在它上面，从 relay 9 换到对端挂靠的 relay 8（会话走 UDP）"
        );
        assert_eq!(describe(peer, via_udp, other, None), "原来的 relay 不可用，从 relay 9 换到对端挂靠的 relay 8（会话走 UDP）");
        assert_eq!(describe(peer, direct, RouteKey::None, None), "直连不可用，也没有 relay 可走");
        assert_eq!(describe(peer, via_udp, RouteKey::None, None), "中转会话断开，没有别的路");

        // 两个节点之间直接的中转会话：说明为什么不走对端挂靠表里排在前面的 relay。
        assert_eq!(
            describe(peer, RouteKey::None, itself, None),
            "中转会话可用，经对端自己的 relay（它本身就是 relay，会话走 TCP，不用绕别的 relay）"
        );
        assert_eq!(
            describe(peer, via_udp, itself, None),
            "有了不绕路的中转会话，从 relay 9 换到对端自己的 relay（它本身就是 relay，会话走 TCP，不用绕别的 relay）"
        );
        assert_eq!(
            describe(peer, via_udp, local, None),
            "有了不绕路的中转会话，从 relay 9 换到本节点自己的 relay（对端在它上面有会话，不用绕别的 relay）"
        );

        assert_eq!(direct.to_route(), Route::Direct { addr: addr("203.0.113.7:4000") });
        assert_eq!(via_udp.to_route(), Route::Relay { relays: vec![node(9)] });
        assert_eq!(RouteKey::None.to_route(), Route::None);
    }

    fn peer_attached_to(relays: &[u32]) -> PeerState {
        PeerState {
            key: PublicKey::from_bytes([2; 32]),
            attached: relays.iter().zip(1..).map(|(&relay, rtt_ms)| Attachment { relay: node(relay), rtt_ms }).collect(),
            dial_out_only: false,
            direct: DirectPath::new(),
            slot: PeerSlot::new(node(2)),
            route: RouteKey::None,
            doubt: true,
            sources: Vec::new(),
            hint: None,
            unattached: Vec::new(),
        }
    }

    #[test]
    fn the_peer_itself_and_this_nodes_own_relay_come_before_the_relays_the_peer_lists() {
        let (me, peer) = (node(1), node(2));
        let order = |attached: &[u32]| relay_candidates(me, peer, &peer_attached_to(attached)).collect::<Vec<_>>();
        // 对端只列了别的 relay：它自己和本节点仍然排在前面。
        assert_eq!(order(&[8, 9]), [peer, me, node(8), node(9)]);
        // 对端把本节点（或者它自己）列在后面也一样，而且不重复出现。
        assert_eq!(order(&[8, 1]), [peer, me, node(8)]);
        assert_eq!(order(&[8, 2, 1, 9]), [peer, me, node(8), node(9)]);
        assert_eq!(order(&[]), [peer, me]);
    }
}
