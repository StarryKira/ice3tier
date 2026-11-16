//! 控制任务：路径层里所有"慢"的事情都在这一个任务里做，所以这些状态不需要锁。
//!
//! 它处理收包路径认出来的帧（relay 的应答、探测、骨干矩阵的行）、信令、局域网里看到的信标、TCP 中转腿的事件、
//! netmap 的更新，并且每隔一小段时间把各个状态机的定时器推进一步。每处理完一批，就重新看一遍到每个对端该走哪条路，
//! 有变化才去改收发路径上的那张表。
//!
//! 选路有两种模式（白皮书 §7.1），按 netmap 里本节点的设置（没有就用网络的默认值）：
//!
//! - **默认模式**：直连可用就走直连；否则只经一个 relay，按 [`relay_candidates`] 的顺序取第一个能用的。每一轮都重新看，
//!   所以直连一验证通过、一个 relay 一断开，路马上跟着变。
//! - **延迟优先**：候选路径（直连、经 1 到最大中转级数个 relay）按代价比较，规则见 [`crate::routing`]。
//!   代价每个评估窗口（5 秒）算一次；窗口之间只看现在的路还在不在，不在了立刻换。
//!
//! 两种模式下都会定期把候选路径和代价算出来放进对外公布的状态，给界面解释"为什么走这条路"。

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use i3t_crypto::{NodeKey, Signature};
use i3t_model::Netmap;
use i3t_model::control::{Route, RouteEntry, Signal};
use i3t_model::netmap::{Attachment, Reach, RoutingMode};
use i3t_proto::backbone::{Link, LinkQuality, RelayState, Row};
use i3t_proto::datagram::{MAX_RELAY_HOPS, RelayPath};
use i3t_proto::session::Nonce;
use i3t_proto::{NodeId, PublicKey, addr, probe, session};
use i3t_underlay::lan::Sighting;
use i3t_underlay::portmap::{GatewaySource, PortMapConfig, PortMapper};
use i3t_underlay::{AddrQuery, ifaddr};
use tokio::sync::{mpsc, oneshot, watch};

use self::route::{RouteKey, Via, View, describe, explain, relay_candidates, route_text};
use crate::cost::Segment;
use crate::direct::{Action, Change, DirectPath, Policy};
use crate::matrix::Matrix;
use crate::relays::{self, Complaint, Identity, Leg, RelayLink, Standing, Wire};
use crate::report::{BackboneLink, BackboneRow, CandidateReport, LegReport, PathReport};
use crate::routing::{Candidate, PathId, Selector};
use crate::table::{self, Hop, PeerSlot, RelaySlot, Shared};
use crate::tcp::{self, TcpEvent, TcpLegHandle};
use crate::timing::{BACKBONE_PROBE_INTERVAL, ENTRY_RELAYS, EVAL_WINDOW, MAX_TIMEOUTS, PROBE_INTERVAL, ROW_INTERVAL, Rtt, TICK};
use crate::{RouteChange, Status};

mod route;

/// 多久重新看一次本机的网卡地址。
const LOCAL_ADDRESS_REFRESH: Duration = Duration::from_secs(5);
/// 多久重新考虑一次挂靠哪几个 relay。
const ATTACH_REVIEW: Duration = Duration::from_secs(1);
/// 多久刷新一次对外公布的状态（路由表里的延迟等）。路变了的时候不等这个间隔。
const STATUS_REFRESH: Duration = Duration::from_millis(500);
/// relay 说某个对端没挂在它上面、或者某条骨干连接走不通之后，多久之内不走那条路。
const UNATTACHED_HOLD: Duration = Duration::from_secs(10);
/// 陌生地址来的 PING 每秒最多验多少个签名。认识的地址不受这个限制。
const STRANGER_VERIFIES_PER_SEC: f64 = 50.0;
/// 经多个 relay 的路径上后面的 relay 没法把"送不到"告诉来源节点，所以本节点自己经这条路径给对端发端到端的探测
/// （对端从它自己的路回答）。有流量时每隔这么久探测一次，没有流量时按普通的探测间隔。
const PATH_PROBE_BUSY: Duration = Duration::from_secs(1);
/// 一个路径探测等多久没有回答算丢了。连续丢 [`MAX_TIMEOUTS`] 个就认为这条路径中间断了。
const PATH_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// 同一个对端的路径探测最多这么频繁地回答：验签名不便宜，对端正常情况下一秒才发一个。
const PATH_PING_MIN_GAP: Duration = Duration::from_millis(200);
/// 被这样怀疑的路径多久之内不再用（除非没有别的路）。
const SUSPECT_HOLD: Duration = Duration::from_secs(20);
/// relay 宣布下线之后，多久之内把它排在别的 relay 后面。比通知本身的有效期短得多：它要是升级完很快回来了，
/// 不该因为一行旧的通知一直被冷落；它回来之后发的新的一行到了，这个标记也就提前解除。
/// 这段时间里到它的会话照常保持、断了照常重连（带退避和随机抖动）：只有它一个 relay 可走的节点还得靠它。
const DRAIN_HOLD: Duration = Duration::from_secs(30);
/// relay 通知过载之后，至少这么久不回去用它，另外每个节点再加上 0 到这么久的一段随机时间。
const OVERLOAD_HOLD: Duration = Duration::from_secs(30);
/// 有新的会话时，至少隔这么久才为它们额外发一轮矩阵。
const NEWCOMER_BATCH: Duration = Duration::from_millis(500);
/// 一个对端的候选路径最多报多少条给界面。
const MAX_REPORTED_CANDIDATES: usize = 16;

/// 交给控制任务处理的事情。
#[derive(Debug)]
pub(crate) enum Ctrl {
    /// UDP 上收到的、归路径层自己处理的一帧。
    Frame { from: SocketAddr, frame: Inbound },
    /// 到某个 relay 的 TCP 上发生的事。
    Tcp { relay: NodeId, conn: u64, event: TcpEvent },
    /// 对端发来的信令。
    Signal { from: NodeId, signal: Signal },
    /// 在本地网段上看到了一个对端的信标。
    Lan(Sighting),
    /// 局域网发现刚启动：把要认的对端告诉它。
    LanStarted,
    /// 按这份参数（重新）启动端口映射。
    PortMap(PortMapConfig),
    /// 节点要停了：删掉端口映射，然后结束控制任务。
    Shutdown(oneshot::Sender<()>),
    /// 有人在等路：别等下一个定时器，现在就把状态机推进一步。
    Wake,
}

#[derive(Debug, Clone)]
pub(crate) enum Inbound {
    Session(session::Frame),
    Probe(probe::Frame),
    AddrReply(addr::Reply),
    /// 骨干矩阵的一行，整帧。
    Row(Bytes),
    /// 对端经 relay 送来的端到端路径探测，或者对本端探测的回答。
    PathProbe {
        peer: NodeId,
        frame: probe::Frame,
    },
}

struct PeerState {
    key: PublicKey,
    /// 对端挂靠的 relay，按它到 relay 的延迟从低到高。
    attached: Vec<Attachment>,
    /// 对端上报的候选端点。用来推断它和本节点是不是在同一个 NAT 后面。
    endpoints: Vec<SocketAddr>,
    /// 对端自己是不是网络里的 relay。
    is_relay: bool,
    dial_out_only: bool,
    direct: DirectPath,
    slot: Arc<PeerSlot>,
    route: RouteKey,
    doubt: bool,
    /// 登记在收包表里的、算作这个对端的来源地址。
    sources: Vec<SocketAddr>,
    /// 下一次换路时写进日志的原因。
    hint: Option<&'static str>,
    /// 哪些 relay 说过这个对端没挂在它上面，到什么时候为止不把它们当作最后一个 relay。
    unattached: Vec<(NodeId, Instant)>,
    /// 端到端的探测没有回音的多级路径，到什么时候为止不用。
    suspect: Vec<(RelayPath, Instant)>,
    probe: PathProbe,
    /// 延迟优先模式下的选路状态。
    selector: Selector,
    /// 最近一次算出来的候选路径，按代价从低到高。
    candidates: Vec<Candidate>,
    /// 现在这条路是为什么选的，以及是什么时候换上的。
    reason: String,
    switched: Option<Instant>,
}

/// 经现在这条多级路径做的端到端探测。
#[derive(Debug, Default)]
struct PathProbe {
    /// 在等回答的那个探测。
    pending: Option<(Nonce, Instant)>,
    last_sent: Option<Instant>,
    /// 连续几个没有回答。
    missed: u8,
    /// 最近一次回答对端的探测，用来限速。
    answered: Option<Instant>,
}

struct RelayEntry {
    link: RelayLink,
    slot: Arc<RelaySlot>,
    /// 这个 relay 最近通知的状态。不是正常服务时，路径和挂靠都尽量避开它。
    state: RelayState,
    /// 它通知过过载：到这个时刻之前，哪怕它说自己恢复了也先不回去。
    cooling_until: Option<Instant>,
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
    /// 本节点的选路模式，和一条路径最多经过几个 relay。
    mode: RoutingMode,
    max_hops: usize,
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
    /// 给骨干矩阵的行验签名的额度。
    row_budget: VerifyBudget,
    next_conn: u64,
    status_refreshed: Option<Instant>,
    /// 手上的骨干矩阵。
    matrix: Matrix,
    /// relay 说走不通的骨干连接（从前一个到后一个），到什么时候为止不用。两个编号相同表示那个 relay 不肯转发多级路径。
    broken: Vec<(NodeId, NodeId, Instant)>,
    /// 上一个评估窗口是什么时候开始的。
    evaluated: Option<Instant>,
    /// 本节点是 relay 时：上一次把矩阵发给会话的时刻，和当时自己的状态、握手成功过的次数。
    rows_sent: Option<Instant>,
    own_state: Option<RelayState>,
    sessions_seen: u64,
    portmap: Option<PortMapper>,
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
        // 端口绑在未指定地址上的才是真的节点（测试里的节点绑在回环的具体地址上）：只有它们自动去找网关要端口映射。
        let local = shared.port.local_addr();
        let portmap =
            local.ip().is_unspecified().then(|| PortMapper::spawn(PortMapConfig { port: local.port(), gateway: GatewaySource::System }));
        let mut control = Self {
            shared,
            key,
            netmap: Arc::clone(&netmap),
            allow_direct: true,
            allow_relay: true,
            dial_out_only: false,
            mode: RoutingMode::DirectFirst,
            max_hops: 1,
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
            row_budget: VerifyBudget { tokens: STRANGER_VERIFIES_PER_SEC, refilled: now },
            next_conn: 1,
            status_refreshed: None,
            matrix: Matrix::default(),
            broken: Vec::new(),
            evaluated: None,
            rows_sent: None,
            own_state: None,
            sessions_seen: 0,
            portmap,
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
                    // 攒着的一起处理完再重新选路，免得每一帧都把所有对端过一遍。
                    let mut next = Some(message);
                    while let Some(message) = next.take().or_else(|| ctrl.try_recv().ok()) {
                        if let Ctrl::Shutdown(done) = message {
                            self.shutdown().await;
                            // 等结果的一方不等了也没有关系。
                            let _waiting = done.send(());
                            return;
                        }
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
            Ctrl::Frame { from, frame: Inbound::Row(frame) } => {
                if let Some(&relay) = self.relay_by_addr.get(&from) {
                    self.on_row(relay, &frame, now);
                }
            }
            Ctrl::Frame { frame: Inbound::PathProbe { peer, frame: probe::Frame::Ping(ping) }, .. } => self.on_path_ping(peer, &ping, now),
            Ctrl::Frame { frame: Inbound::PathProbe { peer, frame: probe::Frame::Pong(pong) }, .. } => self.on_path_pong(peer, &pong),
            Ctrl::Tcp { relay, conn, event } => self.on_tcp(relay, conn, &event, now),
            Ctrl::Signal { from, signal } => self.on_signal(from, &signal, now),
            Ctrl::Lan(sighting) => self.on_lan(sighting, now),
            Ctrl::LanStarted => self.sync_lan(),
            Ctrl::PortMap(config) => {
                let mapper = PortMapper::spawn(config);
                mapper.set_tcp(self.netmap.me.relay.is_some());
                self.portmap = Some(mapper);
            }
            // 在 `run` 里已经处理掉了；这里只是让匹配保持完整。
            Ctrl::Shutdown(_) | Ctrl::Wake => {}
        }
    }

    /// 节点要停了：把在路由器上申请的端口映射删掉。
    async fn shutdown(&mut self) {
        if let Some(mapper) = self.portmap.take() {
            mapper.shutdown().await;
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
        let mode = netmap.me.routing.unwrap_or(netmap.network.routing);
        let max_hops = usize::from(netmap.network.max_relay_hops).clamp(1, MAX_RELAY_HOPS);
        if (mode, max_hops) != (self.mode, self.max_hops) {
            let name = match mode {
                RoutingMode::DirectFirst => "默认模式（直连优先，否则经一个 relay）",
                RoutingMode::LowestCost => "延迟优先（按代价比较直连和各条 relay 路径）",
            };
            tracing::info!(mode = name, max_relay_hops = max_hops, "本节点的选路模式");
            self.mode = mode;
            self.max_hops = max_hops;
            // 换了规则：不等下一个窗口，马上按新的规则看一遍。
            self.evaluated = None;
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
                    endpoints: Vec::new(),
                    is_relay: false,
                    dial_out_only: false,
                    direct: DirectPath::new(),
                    slot,
                    route: RouteKey::None,
                    doubt: true,
                    sources: Vec::new(),
                    hint: None,
                    unattached: Vec::new(),
                    suspect: Vec::new(),
                    probe: PathProbe::default(),
                    selector: Selector::default(),
                    candidates: Vec::new(),
                    reason: "还没有可用的路径".to_owned(),
                    switched: None,
                }
            });
            if peer.key != info.key {
                // 换了密钥就是另一个身份：以前验证过的地址全部作废。
                tracing::info!(peer = %info.id, "对端的公钥变了，重新验证它的直连地址");
                peer.key = info.key;
                peer.direct = DirectPath::new();
            }
            peer.attached.clone_from(&info.attached);
            peer.endpoints.clone_from(&info.endpoints);
            peer.is_relay = netmap.relay(info.id).is_some();
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
            let entry = self.relays.entry(info.id).or_insert_with(|| RelayEntry {
                link: RelayLink::new(info, now),
                slot: RelaySlot::new(info.id),
                state: RelayState::Serving,
                cooling_until: None,
            });
            entry.link.update(info);
        }
        self.relay_by_addr =
            self.relays.iter().flat_map(|(&id, entry)| entry.link.udp_addrs().iter().map(move |&addr| (addr, id))).collect();
        table.relays =
            self.relays.values().flat_map(|entry| entry.link.udp_addrs().iter().map(|&addr| (addr, Arc::clone(&entry.slot)))).collect();
        drop(table);
        self.matrix.retain(now, |relay| netmap.relay(relay).is_some());
        if let Some(mapper) = &self.portmap {
            // relay 在同一个端口号上还监听 TCP，那个端口也要映射出去。
            mapper.set_tcp(netmap.me.relay.is_some());
        }
        self.netmap = netmap;
        self.sync_lan();
        // 对端和 relay 都可能变了：挂靠关系马上重新考虑，对外公布的状态马上刷新。
        self.attach_reviewed = None;
        self.status_refreshed = None;
    }

    /// 把要认的对端和"发不发信标"告诉局域网发现。
    fn sync_lan(&self) {
        let lan = self.shared.lan.lock().expect("lan discovery mutex poisoned");
        let Some(lan) = lan.as_ref() else { return };
        // 不建立直连的节点既不找人也不让人找；只拨出的节点照样听，但自己不出声。
        let keys: Vec<PublicKey> = if self.allow_direct { self.peers.values().map(|peer| peer.key).collect() } else { Vec::new() };
        lan.set_peers(&keys);
        lan.set_announcing(self.allow_direct && !self.dial_out_only);
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
        if let Some(complaint) = entry.link.on_udp_frame(frame, now, who, &mut wire) {
            self.on_complaint(relay, complaint, now);
        }
    }

    fn on_tcp(&mut self, relay: NodeId, conn: u64, event: &TcpEvent, now: Instant) {
        // 同一个 relay 可能先后连过几次，只认现在这一条上的事件。
        if self.legs.get(&relay).is_none_or(|leg| leg.conn != conn) {
            return;
        }
        if let TcpEvent::Row(frame) = event {
            self.on_row(relay, frame, now);
            return;
        }
        let Self { shared, key, relays, legs, next_conn, .. } = self;
        let Some(entry) = relays.get_mut(&relay) else { return };
        let who = Identity { me: shared.me, key };
        let complaint = match event {
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
            TcpEvent::Row(_) => None,
        };
        if let Some(complaint) = complaint {
            self.on_complaint(relay, complaint, now);
        }
    }

    fn on_complaint(&mut self, relay: NodeId, complaint: Complaint, now: Instant) {
        match complaint {
            Complaint::PeerUnattached(peer) => self.mark_unattached(relay, peer, now),
            Complaint::RouteUnavailable(next) => {
                tracing::info!(%relay, %next, "relay 说多级路径走不下去，这一段先不用");
                self.broken.retain(|&(from, to, _)| (from, to) != (relay, next));
                self.broken.push((relay, next, now + UNATTACHED_HOLD));
                for peer in self.peers.values_mut() {
                    if matches!(peer.route, RouteKey::Relay { path, .. } if path.first() == relay && path.as_slice().len() > 1) {
                        peer.hint = Some("relay 说多级路径走不下去");
                    }
                }
            }
        }
    }

    /// relay 说 `peer` 没有挂在它上面：一段时间里先经对端挂靠的别的 relay 去找它。
    fn mark_unattached(&mut self, relay: NodeId, peer: NodeId, now: Instant) {
        let Some(state) = self.peers.get_mut(&peer) else { return };
        state.unattached.retain(|(id, _)| *id != relay);
        state.unattached.push((relay, now + UNATTACHED_HOLD));
        state.hint = Some("relay 说对端没有挂在它上面");
    }

    /// 骨干矩阵的一行，是从 `via` 这个 relay 的会话上收到的（它自己的，或者它转来的别的 relay 的）。
    fn on_row(&mut self, via: NodeId, frame: &Bytes, now: Instant) {
        // 只认现在有会话的 relay 发来的：没有会话就没有核对过它的地址。
        if self.relays.get(&via).is_none_or(|entry| entry.link.leg().is_none()) {
            return;
        }
        let Some((origin, seq)) = Row::peek(frame) else { return };
        let Some(origin_key) = self.netmap.relay(origin).map(|relay| relay.key).filter(|_| origin != self.shared.me) else { return };
        // 不比手上的新就不用往下看了：验签名是这里唯一花钱的事。来源地址是可以伪造的，所以验的次数也有上限；
        // 正常情况下每个 relay 几秒才有一行新的，远到不了这个数。
        if !self.matrix.is_news(origin, seq, now) {
            return;
        }
        if !self.row_budget.take(now) {
            tracing::debug!(%via, %origin, "骨干矩阵的行来得太多，这一行不验了");
            return;
        }
        let (Some((row, signature)), Some(signed)) = (Row::parse(frame), Row::signed_part(frame)) else { return };
        if i3t_crypto::verify(&origin_key, Row::PURPOSE, signed, &Signature::from_bytes(signature)).is_err() {
            tracing::debug!(%via, %origin, "骨干矩阵的一行签名不对，丢弃");
            return;
        }
        let state = row.state;
        if self.matrix.insert(row, frame.clone(), now) {
            tracing::info!(relay = %origin, ?state, %via, "relay 的状态变了");
            // 状态变了的行不等下一轮，立刻传给本节点 relay 上的会话：经它中转的节点也要尽快知道。
            if let Some(local) = self.shared.local_relay.get() {
                local.broadcast(frame);
            }
        }
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
        let policy = direct_policy(&self.shared, self.allow_direct, self.dial_out_only, self.mode, &self.endpoints, peer);
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
        // 对端从直连回答了本端经 relay 发的路径探测（它那个方向走的是直连）。
        if peer.probe.pending.is_some_and(|(nonce, _)| nonce == pong.nonce) {
            self.on_path_pong(pong.src, pong);
            return;
        }
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

    /// 对端经 relay 发来的端到端路径探测：验签名，然后从本节点到它的那条路回答。
    ///
    /// 回答走的是本节点自己选的路，不是探测来的那条路（每个节点只为自己发出的包选路），所以回答到了只说明对端的路是通的。
    fn on_path_ping(&mut self, from: NodeId, ping: &probe::Ping, now: Instant) {
        let me = self.shared.me;
        let Some(peer) = self.peers.get_mut(&from) else { return };
        if ping.src != from || ping.dst != me {
            return;
        }
        if peer.probe.answered.is_some_and(|at| now.saturating_duration_since(at) < PATH_PING_MIN_GAP) {
            return;
        }
        let message = probe::Ping::signed_message(ping.src, ping.dst, &ping.nonce);
        if i3t_crypto::verify(&peer.key, probe::Ping::PURPOSE, &message, &Signature::from_bytes(ping.signature)).is_err() {
            tracing::debug!(peer = %from, "路径探测的签名不对，丢弃");
            return;
        }
        peer.probe.answered = Some(now);
        // 探测是经 relay 来的，没有"看到的来源地址"可报。
        let observed = SocketAddr::from(([0, 0, 0, 0], 0));
        let message = probe::Pong::signed_message(me, from, &ping.nonce, observed);
        let signature = *self.key.sign(probe::Pong::PURPOSE, &message).as_bytes();
        let pong = probe::Pong { src: me, dst: from, nonce: ping.nonce, observed, signature };
        peer.slot.send_frame(&self.shared, &pong.to_bytes());
    }

    /// 对端回答了本端的路径探测：现在这条路径确实送得到。
    fn on_path_pong(&mut self, from: NodeId, pong: &probe::Pong) {
        let Some(peer) = self.peers.get_mut(&from) else { return };
        if pong.src != from || pong.dst != self.shared.me || peer.probe.pending.is_none_or(|(nonce, _)| nonce != pong.nonce) {
            return;
        }
        let message = probe::Pong::signed_message(pong.src, pong.dst, &pong.nonce, pong.observed);
        if i3t_crypto::verify(&peer.key, probe::Pong::PURPOSE, &message, &Signature::from_bytes(pong.signature)).is_err() {
            tracing::debug!(peer = %from, "路径探测的回答签名不对，丢弃");
            return;
        }
        peer.probe.pending = None;
        peer.probe.missed = 0;
    }

    /// 在本地网段上看到了一个对端的信标：它的来源地址成为一个直连候选，照样要用带签名的探测验证过才用。
    fn on_lan(&mut self, sighting: Sighting, now: Instant) {
        if !self.allow_direct {
            return;
        }
        let Some((&id, peer)) = self.peers.iter_mut().find(|(_, peer)| peer.key == sighting.key) else { return };
        tracing::debug!(peer = %id, addr = %sighting.addr, "在局域网里看到了对端，把这个地址加进直连的候选");
        peer.direct.on_lan_sighting(sighting.addr, now);
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
        self.sync_standing(now);
        self.review_attachments(now);
        self.poll_relays(now, &active);
        self.poll_directs(now);
        self.publish_rows(now);
        let window = self.evaluated.is_none_or(|at| now.saturating_duration_since(at) >= EVAL_WINDOW);
        if window {
            self.evaluated = Some(now);
        }
        let changed = self.update_routes(now, window);
        if changed || self.status_refreshed.is_none_or(|at| now.saturating_duration_since(at) >= STATUS_REFRESH) {
            self.publish_status(now);
        }
    }

    /// 本节点的候选端点：网卡地址，relay 和地址查询服务器看到的地址，路由器上映射出来的地址。
    fn refresh_endpoints(&mut self, now: Instant) {
        if self.local_refreshed.is_none_or(|at| now.saturating_duration_since(at) >= LOCAL_ADDRESS_REFRESH) {
            let local = self.shared.port.local_candidates();
            if local != self.local_candidates {
                // 网卡地址变了多半是换了网络：网关可能也换了，端口映射要重新申请。头一次读到地址不算变化。
                if let (Some(mapper), true) = (&self.portmap, self.local_refreshed.is_some()) {
                    mapper.network_changed();
                }
                self.local_candidates = local;
            }
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
        let mapped = self.portmap.as_ref().and_then(|mapper| mapper.mapped().udp);

        // 两边的公网地址相同，说明在同一个 NAT 后面：对端的私有地址多半就在本节点的局域网里（白皮书 §5.3）。
        let public: Vec<IpAddr> = observed.iter().chain(&mapped).map(SocketAddr::ip).filter(|ip| !ifaddr::is_private(*ip)).collect();
        for peer in self.peers.values_mut() {
            peer.direct.set_same_nat(shares_public_address(&public, &peer.endpoints));
        }

        // 不建立直连的节点不向别人暴露自己的地址（白皮书 §7.1 的 relay-only）。
        let mut endpoints: Vec<SocketAddr> =
            if self.allow_direct { self.local_candidates.iter().chain(&observed).chain(&mapped).copied().collect() } else { Vec::new() };
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

    /// 按矩阵里各个 relay 最新的通知更新它们的状态。
    fn sync_standing(&mut self, now: Instant) {
        let Self { relays, matrix, attach_reviewed, peers, .. } = self;
        for (&id, entry) in relays.iter_mut() {
            let state = match matrix.state(id, now) {
                // 下线的通知只管一小段时间，见 [`DRAIN_HOLD`]。
                RelayState::Draining if matrix.age(id, now).is_some_and(|age| age >= DRAIN_HOLD) => RelayState::Serving,
                RelayState::Overloaded => {
                    // 每个节点各自多等一段随机的时间再回去。大家一走它就不过载了；要是一看到"恢复"又一起回去，
                    // 它马上又过载，这样来回就是群体震荡（白皮书 §7.5）。
                    if entry.state != RelayState::Overloaded {
                        entry.cooling_until = Some(now + OVERLOAD_HOLD + relays::jitter(OVERLOAD_HOLD));
                    }
                    RelayState::Overloaded
                }
                RelayState::Serving if entry.cooling_until.is_some_and(|until| now < until) => RelayState::Overloaded,
                state => state,
            };
            if state == entry.state {
                continue;
            }
            let notice = match state {
                RelayState::Draining => {
                    tracing::info!(relay = %id, "relay 通知即将下线：有别的路可走的话，经它的路径和到它的挂靠马上换到别处");
                    Some("relay 通知即将下线")
                }
                RelayState::Overloaded => {
                    tracing::info!(relay = %id, "relay 通知过载：有别的 relay 可走的话先改走别的，等它恢复");
                    Some("relay 通知过载")
                }
                RelayState::Serving => {
                    tracing::info!(relay = %id, "relay 可以正常使用了");
                    None
                }
            };
            // 正经过它的路马上要换：把原因记下来，换路的日志里说得清是为什么。
            for peer in peers.values_mut() {
                if notice.is_some() && matches!(peer.route, RouteKey::Relay { path, .. } if path.as_slice().contains(&id)) {
                    peer.hint = notice;
                }
            }
            entry.state = state;
            *attach_reviewed = None;
        }
    }

    /// 重新考虑挂靠哪几个 relay。
    fn review_attachments(&mut self, now: Instant) {
        if self.attach_reviewed.is_some_and(|at| now.saturating_duration_since(at) < ATTACH_REVIEW) {
            return;
        }
        self.attach_reviewed = Some(now);
        // 按 netmap 里的顺序列出来：延迟还没测出来的时候就按这个顺序用。通知过即将下线或者过载的 relay 当作"连不上"：
        // 有别的可换就换掉，没有就留着（只有它一个的时候，等它回来比没有 relay 强）。
        let standings: Vec<Standing> = self
            .netmap
            .relays
            .iter()
            .filter_map(|relay| Some((relay.id, self.relays.get(&relay.id)?)))
            .map(|(id, entry)| Standing {
                id,
                rtt: entry.link.estimate(now),
                failing: entry.state != RelayState::Serving || (self.attached.contains(&id) && entry.link.is_failing(now)),
            })
            .collect();
        let chosen =
            if self.allow_relay { relays::choose_attached(&self.attached, &standings, &self.netmap.me.pinned_relays) } else { Vec::new() };
        if chosen != self.attached {
            let avoided = |id: &NodeId| self.relays.get(id).is_some_and(|entry| entry.state != RelayState::Serving);
            let reason = if !self.allow_relay {
                "本节点不使用 relay"
            } else if !self.netmap.me.pinned_relays.is_empty() {
                "管理员固定了挂靠的 relay"
            } else if self.attached.iter().any(avoided) {
                "原来挂靠的 relay 通知即将下线或者过载"
            } else {
                "按到各个 relay 的延迟选择"
            };
            tracing::info!(from = ?self.attached, to = ?chosen, reason, "挂靠的 relay 变了");
            self.attached = chosen;
        }
    }

    /// 决定和哪些 relay 保持会话，并推进到每个 relay 的状态机。
    fn poll_relays(&mut self, now: Instant, active: &HashSet<NodeId>) {
        let serving = self.shared.local_relay.get().is_some_and(|local| local.status().is_some());
        let lowest_cost = self.mode == RoutingMode::LowestCost && self.allow_relay && !active.is_empty();
        let mut wanted: HashSet<NodeId> = self.attached.iter().copied().collect();
        if self.allow_relay {
            // 能通到活跃对端的 relay 都连着（对端自己是 relay 的话也包括它）：走 relay 时要经它们，走直连时它们是随时能退回去的备用路。
            for (&id, peer) in self.peers.iter().filter(|(id, _)| active.contains(id)) {
                wanted.extend(relay_candidates(self.shared.me, id, peer));
            }
        }
        if lowest_cost {
            // 延迟优先：多级路径从离本节点最近的几个 relay 进。只连这几个，探测量才有上限。
            let mut nearest: Vec<(Duration, NodeId)> =
                self.relays.iter().filter_map(|(&id, entry)| Some((entry.link.estimate(now)?, id))).collect();
            nearest.sort_unstable();
            wanted.extend(nearest.into_iter().take(ENTRY_RELAYS).map(|(_, id)| id));
        }
        if serving {
            // 本节点是 relay：和别的 relay 连得上的都保持会话，这就是骨干。
            wanted.extend(self.relays.keys().copied());
        }
        let survey = self.allow_relay && self.netmap.me.pinned_relays.is_empty();
        let Self { shared, key, relays, legs, next_conn, .. } = self;
        let who = Identity { me: shared.me, key };
        for (id, entry) in relays.iter_mut() {
            let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
            let want = wanted.contains(id);
            entry.link.set_probe_interval(if serving { BACKBONE_PROBE_INTERVAL } else { PROBE_INTERVAL });
            // 延迟优先模式下，会话在用的 relay 都可能是候选路径的第一段：定期发探测包列，丢包率才估得出来。
            entry.link.set_training(lowest_cost && want);
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
            let policy = direct_policy(&self.shared, self.allow_direct, self.dial_out_only, self.mode, &self.endpoints, peer);
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

    /// 本节点是 relay 时：把自己测到的骨干链路和手上别的 relay 的行发给会话。定期发；自己的状态变了立刻发。
    fn publish_rows(&mut self, now: Instant) {
        let Some(local) = self.shared.local_relay.get() else { return };
        let Some(status) = local.status() else {
            self.own_state = None;
            return;
        };
        let state_changed = self.own_state != Some(status.state);
        let since_sent = self.rows_sent.map(|at| now.saturating_duration_since(at));
        let due = since_sent.is_none_or(|since| since >= ROW_INTERVAL);
        // 有新的会话：让它马上拿到矩阵。一批节点同时挂上来时合成一次发，不是每来一个发一轮。
        let newcomers = self.sessions_seen != status.sessions_opened && since_sent.is_none_or(|since| since >= NEWCOMER_BATCH);
        if !due && !state_changed && !newcomers {
            return;
        }
        self.own_state = Some(status.state);
        self.sessions_seen = status.sessions_opened;
        self.rows_sent = Some(now);
        let me = self.shared.me;
        let links: Vec<Link> = self
            .netmap
            .relays
            .iter()
            .filter(|relay| relay.id != me)
            .map(|relay| {
                // 一条骨干连接算通，要本节点在对方那里有 UDP 的会话（包从那里送过去），对方在本节点这里也有（它据此认本节点发的包）。
                let quality = self
                    .relays
                    .get(&relay.id)
                    .filter(|entry| matches!(entry.link.leg(), Some(Leg::Udp(_))) && local.has_udp_session(relay.id))
                    .and_then(|entry| entry.link.quality(now))
                    .map(|quality| LinkQuality::from_measured(quality.rtt, quality.jitter, quality.lost, quality.probes));
                Link { relay: relay.id, quality }
            })
            .collect();
        local.publish_row(&links);
        if due || newcomers {
            for frame in self.matrix.frames(now) {
                local.broadcast(frame);
            }
        }
    }

    /// 重新看一遍到每个对端走哪条路，把变化写进收发路径上的表。`window`：这一轮是不是一个评估窗口的开始。
    /// 返回有没有变化。
    fn update_routes(&mut self, now: Instant, window: bool) -> bool {
        self.broken.retain(|(_, _, until)| now < *until);
        let mut entries: Vec<(NodeId, Segment)> = self
            .relays
            .iter()
            .filter(|(_, entry)| entry.link.leg().is_some())
            .filter_map(|(&id, entry)| Some((id, entry.link.quality(now)?.segment())))
            .collect();
        entries.sort_unstable_by_key(|(id, _)| *id);
        let Self { shared, key, relays, legs, peers, matrix, broken, allow_direct, allow_relay, mode, max_hops, .. } = self;
        let view = View {
            shared,
            relays,
            legs,
            matrix,
            broken,
            entries: &entries,
            allow_direct: *allow_direct,
            allow_relay: *allow_relay,
            max_hops: *max_hops,
            now,
        };
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
            peer.suspect.retain(|(_, until)| now < *until);
            match peer.route {
                RouteKey::Relay { path, .. } if path.as_slice().len() > 1 && peer.slot.is_active(&shared.clock) => {
                    if peer.probe.pending.is_some_and(|(_, sent)| now.saturating_duration_since(sent) >= PATH_PROBE_TIMEOUT) {
                        peer.probe.pending = None;
                        peer.probe.missed += 1;
                    }
                    if peer.probe.missed >= MAX_TIMEOUTS {
                        peer.probe = PathProbe { answered: peer.probe.answered, ..PathProbe::default() };
                        peer.suspect.retain(|(suspect, _)| *suspect != path);
                        peer.suspect.push((path, now + SUSPECT_HOLD));
                        peer.hint = Some("经多个 relay 的路径上端到端的探测一直没有回音");
                    } else if peer.probe.pending.is_none() {
                        // 有流量、或者已经丢过探测时探测得勤一些：断了要在几秒之内发现。
                        let busy = peer.probe.missed > 0 || peer.slot.last_tx.within(&shared.clock, PATH_PROBE_BUSY);
                        let interval = if busy { PATH_PROBE_BUSY } else { PROBE_INTERVAL };
                        if peer.probe.last_sent.is_none_or(|at| now.saturating_duration_since(at) >= interval) {
                            let nonce: Nonce = i3t_underlay::random_bytes();
                            let message = probe::Ping::signed_message(shared.me, id, &nonce);
                            let signature = *key.sign(probe::Ping::PURPOSE, &message).as_bytes();
                            peer.slot.send_frame(shared, &probe::Ping { src: shared.me, dst: id, nonce, signature }.to_bytes());
                            peer.probe.pending = Some((nonce, now));
                            peer.probe.last_sent = Some(now);
                        }
                    }
                }
                RouteKey::None | RouteKey::Direct(_) | RouteKey::Relay { .. } => {
                    peer.probe = PathProbe { answered: peer.probe.answered, ..PathProbe::default() };
                }
            }

            let mut why = None;
            let path = match mode {
                RoutingMode::DirectFirst => {
                    if window {
                        // 默认模式不拿代价做选择，算出来只是给界面看的。
                        peer.candidates = view.candidates(id, peer);
                    }
                    view.direct_first(id, peer)
                }
                RoutingMode::LowestCost => {
                    let current = peer.selector.current();
                    let gone = current.is_none_or(|path| !view.alive(id, peer, path));
                    let lan_waiting = *allow_direct && peer.direct.is_lan_active() && !matches!(current, Some(PathId::Direct(_)));
                    if window || gone || lan_waiting {
                        let mut found = view.candidates(id, peer);
                        found.retain(|candidate| view.resolve(id, peer, candidate.path).is_some());
                        peer.candidates = found;
                        why = peer.selector.choose(now, &peer.candidates, window, &mut random_below);
                    }
                    peer.selector.current()
                }
            };
            let (key, hop) = path.and_then(|path| view.resolve(id, peer, path)).unwrap_or((RouteKey::None, Hop::None));
            let doubt = match key {
                RouteKey::None => true,
                RouteKey::Direct(_) => peer.direct.in_doubt(),
                RouteKey::Relay { via: Via::Local, .. } => false,
                RouteKey::Relay { path, via: Via::Udp(_) | Via::Tcp(_) } => {
                    relays.get(&path.first()).is_some_and(|entry| entry.link.in_doubt())
                }
            };
            let changed = key != peer.route;
            // 可疑的路又通了，或者换了一条路（包括从没有路到有路）：把攒着的包补发出去。
            let restored = key != RouteKey::None && (changed || (peer.doubt && !doubt));
            if changed {
                let hint = peer.hint.take();
                let reason = match why {
                    Some(why) => explain(why, key, peer.route, &peer.candidates, hint),
                    None => describe(id, peer.route, key, hint),
                };
                peer.slot.set_hop(hop);
                // 新的路要从头探测。
                peer.probe = PathProbe { answered: peer.probe.answered, ..PathProbe::default() };
                peer.reason.clone_from(&reason);
                peer.switched = Some(now);
                announcements.push((id, key, reason));
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
        tracing::info!(%peer, path = %route_text(key), reason, "到对端的路变了");
        // 没有人订阅时发送会失败，那就是没有人要听。
        let _subscribers = self.shared.changes.send(RouteChange { peer, route, reason });
        self.shared.route_changed.notify_waiters();
    }

    fn publish_status(&mut self, now: Instant) {
        self.status_refreshed = Some(now);
        let mut routes: Vec<RouteEntry> = self
            .peers
            .iter()
            .map(|(&id, peer)| RouteEntry { peer: id, route: peer.route.to_route(), rtt_ms: self.route_rtt_ms(id, peer) })
            .collect();
        routes.sort_unstable_by_key(|entry| entry.peer);
        let mut attached: Vec<Attachment> = self
            .attached
            .iter()
            .filter_map(|id| Some(Attachment { relay: *id, rtt_ms: self.relays.get(id)?.link.rtt()?.millis() }))
            .collect();
        attached.sort_by_key(|attachment| attachment.rtt_ms);

        let mut reports: Vec<PathReport> = self.peers.iter().map(|(&id, peer)| self.report(id, peer, now)).collect();
        reports.sort_unstable_by_key(|report| report.peer);
        let mut backbone: Vec<BackboneRow> = self
            .matrix
            .rows(now)
            .map(|(row, age)| BackboneRow {
                relay: row.origin,
                state: row.state,
                load_percent: row.load_percent,
                age_ms: u64::try_from(age.as_millis()).unwrap_or(u64::MAX),
                links: row
                    .links
                    .iter()
                    .map(|link| BackboneLink {
                        to: link.relay,
                        rtt_ms: link.quality.map(|quality| quality.rtt_ms),
                        jitter_ms: link.quality.map_or(0, |quality| quality.jitter_ms),
                        loss: link.quality.map_or(0.0, LinkQuality::loss),
                    })
                    .collect(),
            })
            .collect();
        backbone.sort_unstable_by_key(|row| row.relay);
        let status = Status { routes, endpoints: self.endpoints.clone(), mapping_varies: self.mapping_varies, attached, reports, backbone };
        *self.shared.status.lock().expect("path status mutex poisoned") = status;
    }

    /// 现在这条路上的往返延迟。
    fn route_rtt_ms(&self, id: NodeId, peer: &PeerState) -> Option<u32> {
        match peer.route {
            RouteKey::None => None,
            RouteKey::Direct(_) => peer.direct.active().map(|(_, rtt)| rtt.millis()),
            RouteKey::Relay { path, .. } => match *path.as_slice() {
                // 经一个 relay 的延迟是两段之和：本节点到 relay 是自己测的，relay 到对端是对端上报的。
                [relay] => {
                    let near = self.relays.get(&relay).and_then(|entry| entry.link.rtt()).map(|rtt| rtt.millis());
                    let far = peer.attached.iter().find(|attachment| attachment.relay == relay).map(|attachment| attachment.rtt_ms);
                    match (near, far) {
                        // relay 就是对端自己：只有一段。
                        (near, None) if relay == id => near,
                        (near, Some(far)) => Some(near.unwrap_or(0).saturating_add(far)),
                        (_, None) => None,
                    }
                }
                // 多级路径：各段相加，取最近一次算候选时的测量值。
                _ => {
                    let chosen = peer.candidates.iter().find(|candidate| candidate.path == PathId::Relays(path))?;
                    let total: f64 = chosen.legs.iter().map(|leg| leg.segment.rtt_ms).sum();
                    Some(Rtt::new(Duration::try_from_secs_f64(total / 1000.0).ok()?).millis())
                }
            },
        }
    }

    /// 给界面看的：到这个对端考虑过哪些路、现在走哪条、为什么。
    fn report(&self, id: NodeId, peer: &PeerState, now: Instant) -> PathReport {
        let current = match peer.route {
            RouteKey::None => None,
            RouteKey::Direct(addr) => Some(PathId::Direct(addr)),
            RouteKey::Relay { path, .. } => Some(PathId::Relays(path)),
        };
        let candidates = peer
            .candidates
            .iter()
            .take(MAX_REPORTED_CANDIDATES)
            .map(|candidate| CandidateReport {
                route: match candidate.path {
                    PathId::Direct(addr) => Route::Direct { addr },
                    PathId::Relays(path) => Route::Relay { relays: path.as_slice().to_vec() },
                },
                cost_ms: candidate.cost_ms,
                legs: candidate
                    .legs
                    .iter()
                    .map(|leg| LegReport {
                        from: leg.from,
                        to: leg.to,
                        rtt_ms: leg.segment.rtt_ms,
                        jitter_ms: leg.segment.jitter_ms,
                        loss: leg.segment.loss,
                    })
                    .collect(),
                load_percent: candidate.load_percent,
                current: Some(candidate.path) == current,
            })
            .collect();
        PathReport {
            peer: id,
            mode: self.mode,
            route: peer.route.to_route(),
            direct: matches!(peer.route, RouteKey::Direct(_)).then(|| peer.direct.kind()).flatten(),
            reason: peer.reason.clone(),
            since_switch_ms: peer.switched.map(|at| u64::try_from(now.saturating_duration_since(at).as_millis()).unwrap_or(u64::MAX)),
            candidates,
        }
    }
}

/// 这个对端此刻适用的直连限制。
fn direct_policy<'a>(
    shared: &Shared,
    allow_direct: bool,
    node_dial_out_only: bool,
    mode: RoutingMode,
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
        // 延迟优先模式下直连是要和 relay 路径比代价的候选，丢包率得测出来。
        train: mode == RoutingMode::LowestCost,
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

/// 对端上报的端点里有没有和本节点相同的公网地址（端口不算）。有的话两个节点在同一个 NAT 后面。
/// `public` 是别人看到的本节点的公网地址。
fn shares_public_address(public: &[IpAddr], peer_endpoints: &[SocketAddr]) -> bool {
    peer_endpoints.iter().map(SocketAddr::ip).any(|ip| !ifaddr::is_private(ip) && public.contains(&ip))
}

/// 0 到 `bound - 1` 之间的一个随机数。
fn random_below(bound: usize) -> usize {
    let raw = u32::from_be_bytes(i3t_underlay::random_bytes());
    usize::try_from(raw).unwrap_or(0) % bound.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
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
    fn peers_behind_the_same_nat_are_recognised_by_a_shared_public_address() {
        let public: Vec<IpAddr> = vec!["203.0.113.7".parse().unwrap()];
        // 对端上报的反射地址和本节点的是同一个 IP（端口不同）：在同一个 NAT 后面。
        assert!(shares_public_address(&public, &[addr("192.168.1.9:20330"), addr("203.0.113.7:41000")]));
        assert!(!shares_public_address(&public, &[addr("192.168.1.9:20330"), addr("203.0.113.8:41000")]));
        // 两边都只有私有地址说明不了什么：不相干的两个网络可以用同一个私有网段。
        assert!(!shares_public_address(&["192.168.1.5".parse().unwrap()], &[addr("192.168.1.5:20330")]));
        assert!(!shares_public_address(&[], &[addr("203.0.113.7:41000")]));
    }

    #[test]
    fn the_random_pick_stays_within_bounds() {
        assert!((0..1000).all(|_| random_below(3) < 3));
        assert_eq!(random_below(1), 0);
        assert_eq!(random_below(0), 0);
    }
}
