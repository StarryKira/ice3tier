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
//!
//! # 不可信的输入
//!
//! 收包路径交来的帧都没有经过认证，来源地址也可以伪造。这里的规矩是：
//!
//! - 验签名之前先扣额度（[`crate::budget`]），额度按对端、按候选地址、按矩阵行的主人分开算，一个来源灌包花不掉别人的额度；
//! - relay 的错误帧只在回显对得上时才算数（[`crate::attach`]），说多级路径走不通的还要是本节点正在用的路径；
//! - 一个直连地址被几个对端认领时，收包表归正在把它当直连用的那个（[`rebuild_direct`]）；
//! - 对端回答直连探测时报的"看到的本节点地址"必须像是本节点的（[`observed_is_mine`]），
//!   不然可能是有人在中间来回转发，把自己插进了"直连"里。
//! - 主动测试的探测只在测试打开着的时候、只对测试指定的对端回答，个数有上限，签名在这些检查之后才验（[`crate::diag`]）。

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use i3t_crypto::{NodeKey, Signature};
use i3t_model::Netmap;
use i3t_model::control::{Route, RouteCause, RouteEntry, RouteHint, Signal};
use i3t_model::netmap::{Attachment, Reach, RoutingMode};
use i3t_proto::backbone::{Link, LinkQuality, RelayState, Row};
use i3t_proto::datagram::{MAX_RELAY_HOPS, RelayPath};
use i3t_proto::probe::TestId;
use i3t_proto::session::{Echo, Nonce, Refused};
use i3t_proto::{NodeId, PublicKey, addr, probe, session};
use i3t_underlay::lan::Sighting;
use i3t_underlay::portmap::{GatewaySource, PortMapConfig, PortMapper};
use i3t_underlay::{AddrQuery, ifaddr};
use tokio::sync::{mpsc, oneshot, watch};

use self::route::{Reason, RouteKey, Via, View, describe, explain, relay_candidates, relay_in_use, route_text};
use crate::budget::Budget;
use crate::diag::{
    DirectAttempt, HopReport, Pin, RouteDecision, Side, TestPath, TestSpec, TrainError, TrainReport, TrainRun, TrainSpec, Window,
};
use crate::direct::{Action, Change, DirectPath, Policy};
use crate::matrix::Matrix;
use crate::relays::{self, Complaint, Identity, Leg, RelayLink, Standing, Wire};
use crate::report::{BackboneLink, BackboneRow, CandidateReport, LegReport, PathReport, RelayLeg};
use crate::routing::{Candidate, Entry, PathId, Selector};
use crate::table::{self, Hop, PeerSlot, RelaySlot, Shared, Table};
use crate::tcp::{self, TcpEvent, TcpLegHandle};
use crate::timing::{BACKBONE_PROBE_INTERVAL, ENTRY_RELAYS, EVAL_WINDOW, MAX_TIMEOUTS, PROBE_INTERVAL, ROW_INTERVAL, Rtt, TICK};
use crate::{RouteChange, Status};

mod active;
mod route;

/// 多久重新看一次本机的网卡地址。
const LOCAL_ADDRESS_REFRESH: Duration = Duration::from_secs(5);
/// 多久重新考虑一次挂靠哪几个 relay。
const ATTACH_REVIEW: Duration = Duration::from_secs(1);
/// 多久刷新一次对外公布的状态（路由表里的延迟等）。路变了的时候不等这个间隔。
const STATUS_REFRESH: Duration = Duration::from_millis(500);
/// relay 说某个对端没挂在它上面、或者某条骨干连接走不通之后，多久之内不走那条路。
const UNATTACHED_HOLD: Duration = Duration::from_secs(10);
/// 陌生地址（不是对端的候选地址）来的 PING，全体加起来每秒最多验多少个签名。对端候选地址来的各有各的额度（见 `direct`）。
const STRANGER_VERIFIES_PER_SEC: f64 = 50.0;
/// 陌生地址来的、自称是同一个对端的 PING，每秒最多验多少个，最多攒多少个。伪造某一个对端的 PING 只花得掉这一份，
/// 别的对端从新地址发来的真 PING 还能用上面那个总的额度。
const STRANGER_VERIFIES_PER_PEER: f64 = 5.0;
const STRANGER_VERIFY_BURST_PER_PEER: f64 = 10.0;
/// 每个 relay 的矩阵行每秒最多验多少个签名，最多攒多少个。正常情况下一个 relay 几秒才有一行新的。
const ROW_VERIFIES_PER_SEC: f64 = 2.0;
const ROW_VERIFY_BURST: f64 = 5.0;
/// 状态变了的行不等下一轮就转给本节点 relay 上的会话，但同一个 relay 的行最多这么频繁地转：
/// 状态来回翻的行再多，经本节点放大出去的也有限，其余的跟着下一轮一起发。
const ROW_FORWARD_GAP: Duration = Duration::from_secs(1);
/// 控制任务每一轮从每个队列最多处理多少条，免得一个队列被灌满时别的等太久。
const BATCH: usize = 256;
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
    /// 打开一个主动测试的时间窗。
    OpenTest(TestSpec),
    /// 关掉一个主动测试的时间窗（测试做完了、被取消了，或者等它的一方不在了）。
    CloseTest(TestId),
    /// 在一条路径上发一列探测。
    Train { spec: TrainSpec, reply: oneshot::Sender<Result<TrainReport, TrainError>> },
    /// 把到对端的路暂时固定在一条路径上。
    Pin { peer: NodeId, pin: Pin },
    /// 解除固定。`id` 对不上（已经被新的固定替换了）时不做什么。
    Unpin { peer: NodeId, id: u64 },
    /// 现在就试一次直连（带头），或者等对端来试（跟随），最多等 `limit`。
    AttemptDirect { peer: NodeId, side: Side, limit: Duration, reply: oneshot::Sender<DirectAttempt> },
    /// 两种选路模式此刻各会选哪条路。
    Decide { peer: NodeId, reply: oneshot::Sender<Option<RouteDecision>> },
    /// 一条路径上各段的测量值。
    Hops { peer: NodeId, path: TestPath, reply: oneshot::Sender<Vec<HopReport>> },
}

#[derive(Debug, Clone)]
pub(crate) enum Inbound {
    /// relay 发来的会话帧，错误帧除外。
    Session(session::Frame),
    /// relay 发来的错误帧和它回显的字节。
    Refusal(Refused, Echo),
    Probe(probe::Frame),
    AddrReply(addr::Reply),
    /// 骨干矩阵的一行，整帧。
    Row(Bytes),
    /// 对端经 relay 送来的端到端路径探测，或者对本端探测的回答。
    PathProbe {
        peer: NodeId,
        frame: probe::Frame,
    },
    /// 主动测试的探测或者回答（`frame` 是 [`probe::Frame::TestPing`] 或 [`probe::Frame::TestPong`]）。
    /// `at` 是收包路径看到它的时刻，往返时间从这里算，不含它在队列里等的时间；`relayed` 说它是经 relay 来的还是直连来的。
    Test {
        frame: probe::Frame,
        at: Instant,
        relayed: bool,
    },
}

/// 控制任务要读的三个队列，见 [`Shared`] 里对应的发送端。
#[derive(Debug)]
pub(crate) struct Queues {
    pub(crate) ctrl: mpsc::Receiver<Ctrl>,
    pub(crate) from_relays: mpsc::Receiver<Ctrl>,
    pub(crate) from_peers: mpsc::Receiver<Ctrl>,
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
    /// 算作这个对端的来源地址，和其中正在当作直连用的那个。收包表按所有对端的这两项建（[`rebuild_direct`]）。
    sources: Vec<SocketAddr>,
    active_source: Option<SocketAddr>,
    /// 陌生地址来的、自称是这个对端的 PING 的验签额度。
    stranger_budget: Budget,
    /// 下一次换路的直接起因。
    hint: Option<RouteHint>,
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
    reason: Reason,
    switched: Option<Instant>,
    /// 上一次从没有路变成有路的时刻。见 [`route::relay_in_use`]。
    first_route: Option<Instant>,
    /// 主动测试把路暂时固定在哪里（见 [`crate::diag`]）。
    pin: Option<Pin>,
    /// 现在的路是不是固定出来的。
    pinned: bool,
    /// 选路规则此刻选的路。没有固定时就是 `route`；固定期间选路规则照样每一轮都选，"现在走着的 relay"这类判断看的是它，
    /// 所以测试结束之后回到的是没有这次测试时会走的那条路。
    policy_route: RouteKey,
}

/// 一次正在进行的直连尝试（主动测试）。
#[derive(Debug)]
struct Attempt {
    peer: NodeId,
    side: Side,
    started: Instant,
    deadline: Instant,
    reply: oneshot::Sender<DirectAttempt>,
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

/// 一个 relay 的矩阵行：验签名的额度，和上一次不等下一轮就把它的行转给本节点 relay 上的会话的时刻。
struct RowGate {
    budget: Budget,
    forwarded: Option<Instant>,
}

impl RowGate {
    const fn new(now: Instant) -> Self {
        Self { budget: Budget::new(ROW_VERIFIES_PER_SEC, ROW_VERIFY_BURST, now), forwarded: None }
    }

    /// 现在能不能把这个 relay 状态变了的行立刻转出去。返回 `true` 就算转过了。
    fn forward(&mut self, now: Instant) -> bool {
        if self.forwarded.is_some_and(|at| now.saturating_duration_since(at) < ROW_FORWARD_GAP) {
            return false;
        }
        self.forwarded = Some(now);
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
    /// 别人看到的本节点地址（见 [`crate::Paths::public_addresses`]），不管用不用直连。
    public_addresses: Vec<SocketAddr>,
    mapping_varies: Option<bool>,
    /// 陌生地址来的 PING 全体的验签额度。
    verify_budget: Budget,
    /// 各个 relay 的矩阵行的验签额度和转发的节奏。只有 netmap 里的 relay 在这里。
    row_gates: HashMap<NodeId, RowGate>,
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
    /// 打开着的主动测试。
    windows: HashMap<TestId, Window>,
    /// 正在发的探测包列。
    trains: Vec<TrainRun>,
    /// 正在进行的直连尝试。
    attempts: Vec<Attempt>,
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
            public_addresses: Vec::new(),
            mapping_varies: None,
            verify_budget: Budget::new(STRANGER_VERIFIES_PER_SEC, STRANGER_VERIFIES_PER_SEC, now),
            row_gates: HashMap::new(),
            next_conn: 1,
            status_refreshed: None,
            matrix: Matrix::default(),
            broken: Vec::new(),
            evaluated: None,
            rows_sent: None,
            own_state: None,
            sessions_seen: 0,
            portmap,
            windows: HashMap::new(),
            trains: Vec::new(),
            attempts: Vec::new(),
        };
        control.apply_netmap(netmap, now);
        control.poll(now);
        control
    }

    pub(crate) async fn run(mut self, queues: Queues, mut netmap: watch::Receiver<Arc<Netmap>>) {
        let Queues { mut ctrl, mut from_relays, mut from_peers } = queues;
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut netmap_open = true;
        loop {
            // 探测包列的间隔比定时器细：有列在发的时候按它们自己的时刻醒，只推进它们。
            let train_wake = self.trains.iter().map(TrainRun::wake_at).min();
            let train_sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(train_wake.unwrap_or_else(|| Instant::now() + TICK)));
            let first = tokio::select! {
                () = train_sleep, if train_wake.is_some() => {
                    self.poll_trains(Instant::now());
                    continue;
                }
                message = ctrl.recv() => {
                    let Some(message) = message else { return };
                    Some(message)
                }
                Some(message) = from_relays.recv() => Some(message),
                Some(message) = from_peers.recv() => Some(message),
                changed = netmap.changed(), if netmap_open => match changed {
                    Ok(()) => {
                        let latest = Arc::clone(&netmap.borrow_and_update());
                        self.apply_netmap(latest, Instant::now());
                        None
                    }
                    // 发 netmap 的一方没了（节点在退出）：按最后一份继续。
                    Err(_) => {
                        netmap_open = false;
                        None
                    }
                },
                _ = tick.tick() => None,
            };
            // 攒着的一起处理完再重新选路，免得每一帧都把所有对端过一遍。每个队列每轮最多拿 [`BATCH`] 条。
            let now = Instant::now();
            let batch = first
                .into_iter()
                .chain(std::iter::from_fn(|| ctrl.try_recv().ok()).take(BATCH))
                .chain(std::iter::from_fn(|| from_relays.try_recv().ok()).take(BATCH))
                .chain(std::iter::from_fn(|| from_peers.try_recv().ok()).take(BATCH));
            for message in batch {
                if let Ctrl::Shutdown(done) = message {
                    self.shutdown().await;
                    // 等结果的一方不等了也没有关系。
                    let _waiting = done.send(());
                    return;
                }
                self.handle(message, now);
            }
            self.poll(Instant::now());
        }
    }

    fn handle(&mut self, message: Ctrl, now: Instant) {
        match message {
            Ctrl::Frame { from, frame: Inbound::Session(frame) } => self.on_relay_frame(from, &frame, now),
            Ctrl::Frame { from, frame: Inbound::Refusal(refused, echo) } => self.on_relay_refusal(from, &refused, &echo, now),
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
            Ctrl::Frame { from, frame: Inbound::Test { frame: probe::Frame::TestPing(ping), at, relayed } } => {
                self.on_test_ping(from, &ping, at, relayed, now);
            }
            Ctrl::Frame { frame: Inbound::Test { frame: probe::Frame::TestPong(pong), at, .. }, .. } => self.on_test_pong(&pong, at),
            // 收包路径只把测试的帧放进 `Test`，也不会把测试的帧当作普通的探测：这几种组合不会出现。
            Ctrl::Frame {
                frame:
                    Inbound::Test { frame: probe::Frame::Ping(_) | probe::Frame::Pong(_), .. }
                    | Inbound::Probe(probe::Frame::TestPing(_) | probe::Frame::TestPong(_))
                    | Inbound::PathProbe { frame: probe::Frame::TestPing(_) | probe::Frame::TestPong(_), .. },
                ..
            } => {}
            Ctrl::OpenTest(spec) => self.open_test(&spec, now),
            Ctrl::CloseTest(test) => self.close_test(test, "测试结束"),
            Ctrl::Train { spec, reply } => self.start_train(spec, reply, now),
            Ctrl::Pin { peer, pin } => self.pin(peer, pin, now),
            Ctrl::Unpin { peer, id } => self.unpin(peer, id),
            Ctrl::AttemptDirect { peer, side, limit, reply } => self.attempt_direct(peer, side, limit, reply, now),
            Ctrl::Decide { peer, reply } => {
                // 问的人不等了也没有关系。
                let _waiting = reply.send(self.decide(peer, now));
            }
            Ctrl::Hops { peer, path, reply } => {
                let _waiting = reply.send(self.hops(peer, path, now));
            }
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
        for &id in &gone {
            let Some(peer) = self.peers.remove(&id) else { continue };
            table.peers.remove(&id);
            peer.slot.set_hop(Hop::None);
            if peer.route != RouteKey::None {
                self.announce(id, RouteKey::None, Reason::new("对端不在 netmap 里了", RouteCause::PeerRemoved));
            }
        }
        if !gone.is_empty() {
            rebuild_direct(&mut table, &self.peers);
        }
        drop(table);
        for test in self.windows.iter().filter(|(_, window)| gone.contains(&window.peer)).map(|(test, _)| *test).collect::<Vec<_>>() {
            self.close_test(test, "对端不在 netmap 里了");
        }
        let mut table = self.shared.table_mut();
        let local = self.shared.port.local_addr();
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
                    direct: DirectPath::for_port(local),
                    slot,
                    route: RouteKey::None,
                    doubt: true,
                    sources: Vec::new(),
                    active_source: None,
                    stranger_budget: Budget::new(STRANGER_VERIFIES_PER_PEER, STRANGER_VERIFY_BURST_PER_PEER, now),
                    hint: None,
                    unattached: Vec::new(),
                    suspect: Vec::new(),
                    probe: PathProbe::default(),
                    selector: Selector::default(),
                    candidates: Vec::new(),
                    reason: Reason::no_route_yet(),
                    switched: None,
                    first_route: None,
                    pin: None,
                    pinned: false,
                    policy_route: RouteKey::None,
                }
            });
            if peer.key != info.key {
                // 换了密钥就是另一个身份：以前验证过的地址全部作废。
                tracing::info!(peer = %info.id, "对端的公钥变了，重新验证它的直连地址");
                peer.key = info.key;
                peer.direct = DirectPath::for_port(local);
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
        self.row_gates.retain(|relay, _| netmap.relay(*relay).is_some());
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
        entry.link.on_udp_frame(frame, now, who, &mut wire);
    }

    fn on_relay_refusal(&mut self, from: SocketAddr, refused: &Refused, echo: &Echo, now: Instant) {
        let Some(&relay) = self.relay_by_addr.get(&from) else { return };
        let Self { shared, key, relays, legs, next_conn, .. } = self;
        let Some(entry) = relays.get_mut(&relay) else { return };
        let who = Identity { me: shared.me, key };
        let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
        if let Some(complaint) = entry.link.on_udp_refusal(refused, echo, now, who, &mut wire) {
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
                entry.link.on_tcp_frame(frame, now, who, &mut wire);
                None
            }
            TcpEvent::Refusal(refused, echo) => {
                let mut wire = LinkWire { shared, slot: &entry.slot, legs, next_conn };
                entry.link.on_tcp_refusal(refused, echo, now, who, &mut wire)
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
                // 只认本节点正在用的多级路径：第一段就是 `relay`，下一个是 `next`（`next` 是它自己时表示它不肯转发多级路径）。
                // 所以这张表的大小有上限，也就是 relay 两两组合的数目。
                let on_route = |peer: &PeerState| {
                    matches!(peer.route, RouteKey::Relay { path, .. }
                        if path.first() == relay && path.as_slice().len() > 1 && (next == relay || path.as_slice().get(1) == Some(&next)))
                };
                if !self.peers.values().any(on_route) {
                    tracing::debug!(%relay, %next, "relay 说多级路径走不下去，但本节点没有在走这一段，不理");
                    return;
                }
                let until = now + UNATTACHED_HOLD;
                if let Some(held) = self.broken.iter_mut().find(|(from, to, _)| (*from, *to) == (relay, next)) {
                    held.2 = until;
                    return;
                }
                tracing::info!(%relay, %next, "relay 说多级路径走不下去，这一段先不用");
                self.broken.push((relay, next, until));
                for peer in self.peers.values_mut().filter(|peer| on_route(peer)) {
                    peer.hint = Some(RouteHint::RelayPathBroken);
                }
            }
        }
    }

    /// relay 说 `peer` 没有挂在它上面：一段时间里先经对端挂靠的别的 relay 去找它。
    fn mark_unattached(&mut self, relay: NodeId, peer: NodeId, now: Instant) {
        let Some(state) = self.peers.get_mut(&peer) else { return };
        state.unattached.retain(|(id, _)| *id != relay);
        state.unattached.push((relay, now + UNATTACHED_HOLD));
        state.hint = Some(RouteHint::PeerNotAttached);
    }

    /// 骨干矩阵的一行，是从 `via` 这个 relay 的会话上收到的（它自己的，或者它转来的别的 relay 的）。
    fn on_row(&mut self, via: NodeId, frame: &Bytes, now: Instant) {
        // 只认现在有会话的 relay 发来的：没有会话就没有核对过它的地址。
        if self.relays.get(&via).is_none_or(|entry| entry.link.leg().is_none()) {
            return;
        }
        let Some((origin, seq)) = Row::peek(frame) else { return };
        let Some(origin_key) = self.netmap.relay(origin).map(|relay| relay.key).filter(|_| origin != self.shared.me) else { return };
        // 不比手上的新就不用往下看了：验签名是这里唯一花钱的事。来源地址是可以伪造的，所以验的次数也有上限，
        // 按行的主人分开算：一个 relay 不停地发新的行，花掉的只是它自己的额度。正常情况下每个 relay 几秒才有一行新的。
        if !self.matrix.is_news(origin, seq, now) {
            return;
        }
        let gate = self.row_gates.entry(origin).or_insert_with(|| RowGate::new(now));
        if !gate.budget.take(now) {
            tracing::debug!(%via, %origin, "这个 relay 的矩阵行来得太多，这一行不验了");
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
            // 同一个 relay 的行转得太勤就等下一轮一起发，状态来回翻的 relay 借不了本节点放大。
            if let Some(local) = self.shared.local_relay.get()
                && gate.forward(now)
            {
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
        // 对端的候选地址来的用那个地址自己的额度；陌生地址来的先用这个对端的、再用全体的。不管签名对不对都要扣：
        // 不验就不知道对不对。
        let budgeted = match peer.direct.charge(from, now) {
            Some(granted) => granted,
            None => peer.stranger_budget.take(now) && self.verify_budget.take(now),
        };
        if !budgeted {
            tracing::debug!(%from, src = %ping.src, "这个地址来的 PING 太多，这一个不验了");
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
        let local_network = peer.direct.is_local_network(from);
        if !observed_is_mine(pong.observed, local_network, &self.local_candidates, &self.endpoints) {
            tracing::debug!(%from, src = %pong.src, observed = %pong.observed, "对端看到的本节点地址不是本节点的，可能有人在中间来回转发探测，不认");
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
        // 验签名之前就记下：这个间隔限的是验签名的次数，签名不对的也算一次。
        peer.probe.answered = Some(now);
        let message = probe::Ping::signed_message(ping.src, ping.dst, &ping.nonce);
        if i3t_crypto::verify(&peer.key, probe::Ping::PURPOSE, &message, &Signature::from_bytes(ping.signature)).is_err() {
            tracing::debug!(peer = %from, "路径探测的签名不对，丢弃");
            return;
        }
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
        self.poll_tests(now);
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
            let local = outside_overlay(self.shared.port.local_candidates(), &self.netmap);
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
        let mapping = self.portmap.as_ref().map(PortMapper::mapped).unwrap_or_default();
        let mapped = mapping.udp;
        let mut public: Vec<SocketAddr> = observed.iter().chain(&mapping.udp).chain(&mapping.tcp).copied().collect();
        public.sort_unstable();
        public.dedup();
        if public != self.public_addresses {
            tracing::debug!(addresses = ?public, "别人看到的本节点地址变了");
            self.public_addresses = public;
            self.status_refreshed = None;
        }

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
                    Some(RouteHint::RelayDraining)
                }
                RelayState::Overloaded => {
                    tracing::info!(relay = %id, "relay 通知过载：有别的 relay 可走的话先改走别的，等它恢复");
                    Some(RouteHint::RelayOverloaded)
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
                wanted.extend(relay_candidates(self.shared.me, id, peer, None));
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
        if self.allow_relay {
            // 主动测试指定的路径上，本节点这一头的第一个 relay：测试期间保持会话，平时可能不连它。
            wanted.extend(self.windows.values().filter(|window| window.is_open(now)).flat_map(|window| window.relays.iter().copied()));
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
                peer.hint = Some(reason.hint());
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
        let entries = self.entries(now);
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
        let mut claims_changed = false;
        for peer in peers.values_mut() {
            let (sources, active): (Vec<SocketAddr>, _) = if *allow_direct {
                (peer.direct.sources(now).collect(), peer.direct.active().map(|(addr, _)| addr))
            } else {
                (Vec::new(), None)
            };
            if (&sources, active) != (&peer.sources, peer.active_source) {
                peer.sources = sources;
                peer.active_source = active;
                claims_changed = true;
            }
        }
        if claims_changed {
            rebuild_direct(&mut shared.table_mut(), peers);
        }
        let mut announcements = Vec::new();
        for (&id, peer) in peers.iter_mut() {
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
                        peer.hint = Some(RouteHint::PathProbeSilent);
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
            let (policy_key, policy_hop) = path.and_then(|path| view.resolve(id, peer, path)).unwrap_or((RouteKey::None, Hop::None));
            peer.policy_route = policy_key;
            if peer.pin.is_some_and(|pin| now >= pin.until) {
                tracing::info!(peer = %id, "主动测试固定的路到期了，解除");
                peer.pin = None;
            }
            // 固定的路此刻走不了（直连没了、到第一个 relay 的会话断了）时照常按选路规则走：有路总比没有强。
            let pinned = peer.pin.and_then(|pin| {
                let path = match pin.path {
                    TestPath::Direct => PathId::Direct(peer.direct.active()?.0),
                    TestPath::Relays(relays) => PathId::Relays(relays),
                };
                view.resolve(id, peer, path)
            });
            let was_pinned = peer.pinned;
            peer.pinned = pinned.is_some();
            let (key, hop) = pinned.unwrap_or((policy_key, policy_hop));
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
                if matches!((peer.route, key), (RouteKey::Relay { .. }, RouteKey::Relay { .. }))
                    && *mode == RoutingMode::DirectFirst
                    && relay_in_use(peer, now).is_none()
                    && peer.hint.is_none()
                {
                    peer.hint = Some(RouteHint::PreferredRelayReady);
                }
                let hint = peer.hint.take();
                let reason = if peer.pinned {
                    Reason::new(format!("主动测试暂时把路固定在{}", route_text(key)), RouteCause::TestPinned)
                } else if was_pinned {
                    Reason::new(format!("主动测试结束，回到选路规则选的{}", route_text(key)), RouteCause::TestEnded)
                } else {
                    match why {
                        Some(why) => explain(why, key, peer.route, &peer.candidates, hint),
                        None => describe(id, peer.route, key, hint),
                    }
                };
                peer.slot.set_hop(hop);
                // 新的路要从头探测。
                peer.probe = PathProbe { answered: peer.probe.answered, ..PathProbe::default() };
                peer.reason.clone_from(&reason);
                peer.switched = Some(now);
                if peer.route == RouteKey::None {
                    peer.first_route = Some(now);
                }
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

    /// 本节点现在能把包交给它的 relay：会话是通的、延迟测出来了。
    fn entries(&self, now: Instant) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self
            .relays
            .iter()
            .filter(|(_, entry)| entry.link.leg().is_some())
            .filter_map(|(&relay, entry)| {
                Some(Entry { relay, segment: entry.link.quality(now)?.segment(), settled: entry.link.is_settled(now) })
            })
            .collect();
        entries.sort_unstable_by_key(|entry| entry.relay);
        entries
    }

    /// 每一次换路都记一条带原因的日志，并且通知订阅的人。
    fn announce(&self, peer: NodeId, key: RouteKey, reason: Reason) {
        let route = key.to_route();
        tracing::info!(%peer, path = %route_text(key), reason = reason.text.as_str(), "到对端的路变了");
        // 没有人订阅时发送会失败，那就是没有人要听。
        let _subscribers = self.shared.changes.send(RouteChange { peer, route, reason: reason.text, cause: reason.cause });
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
        let status = Status {
            routes,
            endpoints: self.endpoints.clone(),
            public_addresses: self.public_addresses.clone(),
            mapping_varies: self.mapping_varies,
            attached,
            reports,
            backbone,
        };
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
                relay_leg: match candidate.path {
                    PathId::Direct(_) => None,
                    PathId::Relays(path) => self.relays.get(&path.first()).and_then(|entry| entry.link.leg()).map(|leg| match leg {
                        Leg::Udp(_) => RelayLeg::Udp,
                        Leg::Tcp => RelayLeg::Tcp,
                    }),
                },
            })
            .collect();
        PathReport {
            peer: id,
            mode: self.mode,
            route: peer.route.to_route(),
            direct: matches!(peer.route, RouteKey::Direct(_)).then(|| peer.direct.kind()).flatten(),
            relay_leg: match peer.route {
                RouteKey::Relay { via: Via::Udp(_), .. } => Some(RelayLeg::Udp),
                RouteKey::Relay { via: Via::Tcp(_), .. } => Some(RelayLeg::Tcp),
                RouteKey::Relay { via: Via::Local, .. } | RouteKey::Direct(_) | RouteKey::None => None,
            },
            reason: peer.reason.text.clone(),
            cause: Some(peer.reason.cause.clone()),
            since_switch_ms: peer.switched.map(|at| u64::try_from(now.saturating_duration_since(at).as_millis()).unwrap_or(u64::MAX)),
            candidates,
        }
    }
}

/// 按每个对端认领的来源地址重建收包表里的直连部分。
///
/// 同一个地址可能被几个对端认领：重放的 PING、网络里的恶意成员自己签的 PING，都能让一个对端认领别的对端在用的地址。
/// 所以不能每个对端各改各的条目（那样后来的会覆盖先来的，过期时又会把别人的条目删掉）。按这个顺序归属：
///
/// 1. 正在把它当作直连地址用的对端（它的保活一直在证明这个地址）；
/// 2. 原来就登记着、现在还认领它的对端；
/// 3. 编号最小的那个。
fn rebuild_direct(table: &mut Table, peers: &HashMap<NodeId, PeerState>) {
    let previous = std::mem::take(&mut table.direct);
    let mut ordered: Vec<(&NodeId, &PeerState)> = peers.iter().collect();
    ordered.sort_unstable_by_key(|(id, _)| **id);
    let claims = |id: NodeId, addr: &SocketAddr| peers.get(&id).is_some_and(|peer| peer.sources.contains(addr));
    for (_, peer) in &ordered {
        if let Some(addr) = peer.active_source.filter(|addr| peer.sources.contains(addr)) {
            table.direct.entry(addr).or_insert_with(|| Arc::clone(&peer.slot));
        }
    }
    for &(&id, peer) in &ordered {
        for addr in &peer.sources {
            if let Some(owner) = table.direct.get(addr) {
                if owner.id != id {
                    tracing::debug!(%addr, owner = %owner.id, claimant = %id, "两个对端认领同一个直连地址，只算其中一个的");
                }
                continue;
            }
            let owner = previous.get(addr).filter(|owner| claims(owner.id, addr)).unwrap_or(&peer.slot);
            table.direct.insert(*addr, Arc::clone(owner));
        }
    }
}

/// 对端回答直连探测时报的、它看到的本节点地址 `observed` 像不像真的是本节点的。
///
/// 有人在两个节点之间来回转发探测（经过的 relay、局域网里恰好占着某个候选地址的主机），两边就会把它的地址验证成对方的直连地址，
/// 流量改走它那里。这时对端看到的是转发者的地址，不是本节点的，所以看一眼就分得出来：
///
/// - `local_network`：探测发往的地址不经过地址转换（私有地址、局域网信标里来的）。对端看到的必须是本节点网卡上的地址。
/// - 其余的：必须是本节点已知的地址之一（网卡上的，relay 和地址查询服务器看到的，路由器映射出来的）。
///   本节点还不知道自己这个地址族的任何公网地址时没法判断，只好接受。
///
/// 读不到网卡地址时同样没法判断，接受。
fn observed_is_mine(observed: SocketAddr, local_network: bool, interfaces: &[SocketAddr], known: &[SocketAddr]) -> bool {
    let ip = observed.ip().to_canonical();
    let same = |addr: &SocketAddr| addr.ip().to_canonical() == ip;
    if local_network {
        return interfaces.is_empty() || interfaces.iter().any(same);
    }
    let public_known =
        known.iter().map(|addr| addr.ip().to_canonical()).any(|own| own.is_ipv4() == ip.is_ipv4() && !ifaddr::is_private(own));
    !public_known || known.iter().chain(interfaces).any(same)
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
/// 去掉覆盖网的地址：透明访问打开时本节点的虚拟 IP 在一块本机的网卡上（`i3t-intercept`），它只在组网里有意义，
/// 别的节点连不上它，也不该去试。
fn outside_overlay(candidates: Vec<SocketAddr>, netmap: &Netmap) -> Vec<SocketAddr> {
    candidates.into_iter().filter(|addr| !i3t_model::netmap::is_overlay_address(&netmap.network, addr.ip().to_canonical())).collect()
}

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
    use i3t_proto::probe::{TestPing, TestPong};

    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn overlay_addresses_on_local_interfaces_are_never_candidates() {
        let netmap: Netmap = serde_json::from_value(serde_json::json!({
            "version": 1,
            "network": { "name": "test", "domain": "test.i3t", "cidr": "10.66.0.0/16" },
            "me": { "id": 1, "name": "me", "virtual_ip": "10.66.0.1" }
        }))
        .unwrap();
        let local = vec![addr("192.168.1.5:20330"), addr("10.66.0.1:20330"), addr("[2001:db8::5]:20330"), addr("10.67.0.1:20330")];
        assert_eq!(
            outside_overlay(local, &netmap),
            [addr("192.168.1.5:20330"), addr("[2001:db8::5]:20330"), addr("10.67.0.1:20330")],
            "透明访问的网卡上的虚拟 IP 不公布给别人"
        );
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

    /// 一个只有控制任务、没有收包路径的节点：测试直接调用它处理帧的方法，看它发出了什么、表里记了什么。
    struct Harness {
        control: Control,
        /// 各个对端的密钥，测试拿它们签探测帧。
        keys: HashMap<NodeId, NodeKey>,
        _queues: [mpsc::Receiver<Ctrl>; 3],
    }

    const ME: u32 = 1;

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    impl Harness {
        /// 本节点是 1。`peers` 是对端的编号和它在 netmap 里上报的端点。
        fn new(peers: &[(u32, &[SocketAddr])]) -> Self {
            Self::with_me(peers, &serde_json::json!({}))
        }

        /// 同 [`new`](Self::new)，`me` 里的字段加进 netmap 里本节点的那一项（例如路径限制）。
        fn with_me(peers: &[(u32, &[SocketAddr])], me: &serde_json::Value) -> Self {
            let keys: HashMap<NodeId, NodeKey> = peers.iter().map(|&(id, _)| (node(id), NodeKey::generate().unwrap())).collect();
            let entries: Vec<serde_json::Value> = peers
                .iter()
                .map(|&(id, endpoints)| {
                    serde_json::json!({
                        "id": id, "key": keys[&node(id)].public(), "name": format!("n{id}"), "virtual_ip": format!("10.66.0.{id}"),
                        "inbound": true, "outbound": true, "endpoints": endpoints,
                    })
                })
                .collect();
            let mut local = serde_json::json!({ "id": ME, "name": "me", "virtual_ip": "10.66.0.1" });
            local.as_object_mut().unwrap().extend(me.as_object().cloned().unwrap_or_default());
            let netmap = serde_json::json!({
                "version": 1,
                "network": { "name": "test", "domain": "test.i3t", "cidr": "10.66.0.0/16" },
                "me": local,
                "peers": entries,
            });
            let port = Arc::new(i3t_underlay::UdpPort::bind(addr("127.0.0.1:0")).unwrap());
            let (shared, queues) = Shared::for_tests(node(ME), port, 16);
            let control =
                Control::new(shared, Arc::new(NodeKey::generate().unwrap()), Arc::new(serde_json::from_value(netmap).unwrap()), Vec::new());
            Self { control, keys, _queues: queues }
        }

        fn me(&self) -> SocketAddr {
            self.control.shared.port.local_addr()
        }

        fn ping(&self, from: u32, nonce: Nonce) -> probe::Ping {
            let (src, dst) = (node(from), node(ME));
            let signature = *self.keys[&src].sign(probe::Ping::PURPOSE, &probe::Ping::signed_message(src, dst, &nonce)).as_bytes();
            probe::Ping { src, dst, nonce, signature }
        }

        fn pong(&self, from: u32, nonce: Nonce, observed: SocketAddr) -> probe::Pong {
            let (src, dst) = (node(from), node(ME));
            let message = probe::Pong::signed_message(src, dst, &nonce, observed);
            probe::Pong { src, dst, nonce, observed, signature: *self.keys[&src].sign(probe::Pong::PURPOSE, &message).as_bytes() }
        }

        fn direct_owner(&self, addr: SocketAddr) -> Option<NodeId> {
            self.control.shared.table().direct.get(&addr).map(|slot| slot.id)
        }
    }

    /// 一个扮演对端地址的 UDP socket。
    struct Socket(tokio::net::UdpSocket);

    impl Socket {
        async fn new() -> Self {
            Self(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap())
        }

        fn addr(&self) -> SocketAddr {
            self.0.local_addr().unwrap()
        }

        /// 收到的下一个探测帧；等一会儿还没有就是 `None`。
        async fn probe(&self) -> Option<probe::Frame> {
            let mut buf = [0u8; 2048];
            let (len, _) = tokio::time::timeout(Duration::from_millis(300), self.0.recv_from(&mut buf)).await.ok()?.ok()?;
            probe::Frame::parse(&buf[..len])
        }

        /// 收完眼下到了的测试探测的回答。
        async fn test_pongs(&self) -> Vec<TestPong> {
            let mut pongs = Vec::new();
            while let Some(frame) = self.probe().await {
                if let probe::Frame::TestPong(pong) = frame {
                    pongs.push(pong);
                }
            }
            pongs
        }

        /// 收完眼下到了的探测帧，返回其中的 PONG 和 PING。
        async fn drain(&self) -> (Vec<probe::Pong>, Vec<probe::Ping>) {
            let (mut pongs, mut pings) = (Vec::new(), Vec::new());
            while let Some(frame) = self.probe().await {
                match frame {
                    probe::Frame::Pong(pong) => pongs.push(pong),
                    probe::Frame::Ping(ping) => pings.push(ping),
                    probe::Frame::TestPing(_) | probe::Frame::TestPong(_) => {}
                }
            }
            (pongs, pings)
        }
    }

    /// 对端 `peer` 从 `socket` 的地址验证通过、成为本节点在用的直连地址。
    async fn validate(harness: &mut Harness, peer: u32, socket: &Socket, now: Instant) {
        let ping = harness.ping(peer, [peer.to_be_bytes()[3]; 8]);
        harness.control.on_ping(socket.addr(), &ping, now);
        let (_, reverse) = socket.drain().await;
        let pong = harness.pong(peer, reverse[0].nonce, harness.me());
        harness.control.on_pong(socket.addr(), &pong, now);
        assert_eq!(harness.control.peers[&node(peer)].direct.active().map(|(addr, _)| addr), Some(socket.addr()));
    }

    #[tokio::test]
    async fn a_replayed_ping_can_neither_take_over_nor_later_remove_another_peers_direct_address() {
        let c = Socket::new().await;
        let mut harness = Harness::new(&[(2, &[]), (3, &[])]);
        let start = Instant::now();
        validate(&mut harness, 3, &c, start).await;
        harness.control.update_routes(start, false);
        assert_eq!(harness.direct_owner(c.addr()), Some(node(3)));

        // 对端 2 的一个签名真实的 PING，以 3 的直连地址为来源重放过来（或者 2 自己签了一个这样发）。
        let replayed = harness.ping(2, [0x22; 8]);
        harness.control.on_ping(c.addr(), &replayed, start + Duration::from_secs(1));
        harness.control.update_routes(start + Duration::from_secs(1), false);
        assert_eq!(harness.direct_owner(c.addr()), Some(node(3)), "3 正在用这个地址，它的包不能被当成 2 的");

        // 2 对这个地址的认领过期了：3 的条目不能跟着被删掉。
        harness.control.update_routes(start + Duration::from_secs(20), false);
        assert_eq!(harness.direct_owner(c.addr()), Some(node(3)));
    }

    #[tokio::test]
    async fn forged_pings_cannot_use_up_the_budget_of_real_ones() {
        let (a_known, stranger, c_new) = (Socket::new().await, Socket::new().await, Socket::new().await);
        let mut harness = Harness::new(&[(2, &[a_known.addr()]), (3, &[])]);
        let now = Instant::now();
        // 不认识的地址灌进来两百个自称是 2 的 PING，签名是瞎编的。
        for index in 0..200u8 {
            let forged = probe::Ping { signature: [index; 64], ..harness.ping(2, [index; 8]) };
            harness.control.on_ping(stranger.addr(), &forged, now);
        }
        // 2 从它上报过的地址发来的真 PING 照样有回答。
        harness.control.on_ping(a_known.addr(), &harness.ping(2, [1; 8]), now);
        assert_eq!(a_known.drain().await.0.len(), 1, "上报过的地址有自己的额度");
        // 3 从一个新地址发来的真 PING 也有回答：伪造的只花掉了"自称是 2 的"那一份。
        harness.control.on_ping(c_new.addr(), &harness.ping(3, [2; 8]), now);
        assert_eq!(c_new.drain().await.0.len(), 1, "别的对端的陌生地址还有额度");
        assert!(stranger.drain().await.0.is_empty());
    }

    #[tokio::test]
    async fn a_forged_path_probe_costs_its_peer_the_answering_slot_before_the_signature_is_checked() {
        let mut harness = Harness::new(&[(2, &[])]);
        let now = Instant::now();
        let forged = probe::Ping { signature: [0; 64], ..harness.ping(2, [1; 8]) };
        harness.control.on_path_ping(node(2), &forged, now);
        assert_eq!(harness.control.peers[&node(2)].probe.answered, Some(now), "签名不对的也算一次，验签的次数才有上限");
    }

    #[tokio::test]
    async fn a_pong_reporting_an_address_that_is_not_this_nodes_does_not_validate_the_candidate() {
        let a = Socket::new().await;
        let mut harness = Harness::new(&[(2, &[a.addr()])]);
        let now = Instant::now();
        harness.control.on_ping(a.addr(), &harness.ping(2, [1; 8]), now);
        let (_, reverse) = a.drain().await;
        // 有人在中间来回转发：对端看到的探测是从转发者的地址来的。
        let relayed = harness.pong(2, reverse[0].nonce, addr("198.51.100.77:4000"));
        harness.control.on_pong(a.addr(), &relayed, now);
        assert_eq!(harness.control.peers[&node(2)].direct.active(), None);
        let genuine = harness.pong(2, reverse[0].nonce, harness.me());
        harness.control.on_pong(a.addr(), &genuine, now);
        assert!(harness.control.peers[&node(2)].direct.active().is_some());
    }

    #[test]
    fn an_observed_address_has_to_be_one_this_node_is_known_by() {
        let interfaces = [addr("192.168.1.5:20330"), addr("[2001:db8::5]:20330")];
        let known = [interfaces[0], interfaces[1], addr("203.0.113.7:41000")];
        // 局域网里的地址：对端看到的必须是本节点网卡上的地址。
        assert!(observed_is_mine(addr("192.168.1.5:20330"), true, &interfaces, &known));
        assert!(!observed_is_mine(addr("192.168.1.66:20330"), true, &interfaces, &known));
        assert!(!observed_is_mine(addr("203.0.113.7:41000"), true, &interfaces, &known));
        // 公网上的：relay 看到的地址也算，端口不算。
        assert!(observed_is_mine(addr("203.0.113.7:5555"), false, &interfaces, &known));
        assert!(observed_is_mine(addr("[2001:db8::5]:1"), false, &interfaces, &known));
        assert!(!observed_is_mine(addr("198.51.100.77:4000"), false, &interfaces, &known));
        // 还不知道自己的公网地址（没有 relay，也没有地址查询服务器）：没法判断，只好接受。
        assert!(observed_is_mine(addr("198.51.100.77:4000"), false, &interfaces, &interfaces));
        assert!(observed_is_mine(addr("192.168.1.66:20330"), true, &[], &[]), "读不到网卡地址");
    }

    #[tokio::test]
    async fn a_complaint_about_a_path_this_node_is_not_using_is_ignored_and_repeats_do_not_pile_up() {
        let mut harness = Harness::new(&[(2, &[])]);
        let now = Instant::now();
        harness.control.on_complaint(node(9), Complaint::RouteUnavailable(node(8)), now);
        assert!(harness.control.broken.is_empty(), "本节点没有在走 9 → 8");

        let path = RelayPath::new(&[node(9), node(8)]).unwrap();
        harness.control.peers.get_mut(&node(2)).unwrap().route = RouteKey::Relay { path, via: Via::Udp(addr("198.51.100.9:20330")) };
        for next in 1..=1000u32 {
            harness.control.on_complaint(node(9), Complaint::RouteUnavailable(node(next)), now);
        }
        harness.control.on_complaint(node(9), Complaint::RouteUnavailable(node(8)), now);
        let broken: Vec<(NodeId, NodeId)> = harness.control.broken.iter().map(|&(from, to, _)| (from, to)).collect();
        assert_eq!(broken, [(node(9), node(8)), (node(9), node(9))], "只记正在走的那一段，和 9 自己不肯转发多级路径");
    }

    #[test]
    fn rows_of_one_relay_cannot_use_up_the_budget_of_another() {
        let now = Instant::now();
        let (mut noisy, mut quiet) = (RowGate::new(now), RowGate::new(now));
        assert_eq!((0..100).filter(|_| noisy.budget.take(now)).count(), 5);
        assert!(quiet.budget.take(now));
        assert!(noisy.budget.take(now + Duration::from_millis(500)), "之后每秒 2 个");
        assert!(noisy.forward(now) && !noisy.forward(now + Duration::from_millis(999)), "状态变了的行每秒最多立刻转一次");
        assert!(noisy.forward(now + ROW_FORWARD_GAP));
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

    /// 本节点自己的 relay，只说哪些对端在它上面有会话。
    struct LocalSessions(Vec<NodeId>);

    impl crate::LocalRelay for LocalSessions {
        fn handle_udp(&self, _from: SocketAddr, _ecn: Option<i3t_underlay::Ecn>, _data: &[u8], _stride: usize) {}

        fn has_session(&self, client: NodeId) -> bool {
            self.0.contains(&client)
        }

        fn session_reopened(&self, _client: NodeId) -> Option<Duration> {
            None
        }

        fn session_at(&self, _client: NodeId, _from: SocketAddr) -> bool {
            false
        }

        fn send_to_client(&self, _client: NodeId, _contents: &[u8], _segment_size: Option<usize>, _ecn: Option<i3t_underlay::Ecn>) -> bool {
            true
        }

        fn has_udp_session(&self, client: NodeId) -> bool {
            self.has_session(client)
        }

        fn status(&self) -> Option<crate::RelayStatus> {
            None
        }

        fn publish_row(&self, _links: &[Link]) {}

        fn broadcast(&self, _frame: &[u8]) {}
    }

    #[tokio::test]
    async fn a_pinned_route_overrides_the_policy_until_it_is_released_or_runs_out() {
        let socket = Socket::new().await;
        let mut harness = Harness::new(&[(2, &[])]);
        let start = Instant::now();
        assert!(harness.control.shared.local_relay.set(Arc::new(LocalSessions(vec![node(2)]))).is_ok());
        validate(&mut harness, 2, &socket, start).await;
        harness.control.update_routes(start, true);
        let direct = RouteKey::Direct(socket.addr());
        let route = |harness: &Harness| harness.control.peers[&node(2)].route;
        assert_eq!(route(&harness), direct, "默认模式：直连可用就走直连");

        // 固定在本节点自己的 relay 上（对端在它上面有会话）：选路规则照样选直连，走的却是被固定的路。
        let own = RelayPath::new(&[node(ME)]).unwrap();
        let pinned = RouteKey::Relay { path: own, via: Via::Local };
        let mut changes = harness.control.shared.changes.subscribe();
        harness.control.pin(node(2), Pin { id: 1, path: TestPath::Relays(own), until: start + Duration::from_secs(10) }, start);
        harness.control.update_routes(start, false);
        assert_eq!(route(&harness), pinned);
        assert_eq!(harness.control.peers[&node(2)].policy_route, direct, "选路规则选的还是直连");
        let decision = harness.control.decide(node(2), start).unwrap();
        assert_eq!(decision.current, direct.to_route(), "测试的结果里写的是规则选的路，不是测试自己固定的");
        let change = changes.try_recv().unwrap();
        assert!(change.reason.contains("主动测试暂时把路固定在"));
        assert_eq!(change.cause, RouteCause::TestPinned);
        let peer = &harness.control.peers[&node(2)];
        assert_eq!(harness.control.report(node(2), peer, start).cause, Some(RouteCause::TestPinned), "现问时说的也是同一个原因");

        // 别的固定的解除不算数；这一次的解除之后回到直连。
        harness.control.unpin(node(2), 7);
        harness.control.update_routes(start, false);
        assert_eq!(route(&harness), pinned);
        harness.control.unpin(node(2), 1);
        harness.control.update_routes(start, false);
        assert_eq!(route(&harness), direct);
        let change = changes.try_recv().unwrap();
        assert!(change.reason.contains("主动测试结束"));
        assert_eq!(change.cause, RouteCause::TestEnded);

        // 到期的固定由控制任务自己解除，拿着它的一方什么都不用做。
        harness.control.pin(node(2), Pin { id: 2, path: TestPath::Relays(own), until: start + Duration::from_secs(1) }, start);
        harness.control.update_routes(start, false);
        assert_eq!(route(&harness), pinned);
        harness.control.update_routes(start + Duration::from_secs(2), false);
        assert_eq!((route(&harness), harness.control.peers[&node(2)].pin), (direct, None));

        // 不管要多久，最多固定 MAX_PIN。
        harness.control.pin(node(2), Pin { id: 3, path: TestPath::Direct, until: start + Duration::from_secs(3600) }, start);
        assert_eq!(harness.control.peers[&node(2)].pin.map(|pin| pin.until), Some(start + crate::diag::MAX_PIN));
    }

    fn test_ping(harness: &Harness, from: u32, test: TestId, seq: u32, path: Option<RelayPath>) -> TestPing {
        let (src, dst) = (node(from), node(ME));
        let message = TestPing::signed_message(src, dst, test, seq, path);
        TestPing { src, dst, test, seq, path, signature: *harness.keys[&src].sign(TestPing::PURPOSE, &message).as_bytes() }
    }

    fn window(peer: u32, test: TestId, max_probes: u32) -> TestSpec {
        TestSpec { test, peer: node(peer), side: Side::Lead, window: Duration::from_secs(10), max_probes, relays: Vec::new() }
    }

    #[tokio::test]
    async fn test_probes_are_answered_only_inside_an_open_test_from_its_peer_once_each_up_to_the_limit() {
        let a = Socket::new().await;
        let mut harness = Harness::new(&[(2, &[]), (3, &[])]);
        let now = Instant::now();
        let me = harness.control.key.public();

        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 7, 0, None), now, false, now);
        assert!(a.test_pongs().await.is_empty(), "没有打开的测试不回答：对端自己开不了测试");
        assert!(!harness.control.shared.expects_test(7, node(2)), "收包路径也不会把它交上来");

        harness.control.open_test(&window(2, 7, 3), now);
        assert!(harness.control.shared.expects_test(7, node(2)));
        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 7, 0, None), now, false, now);
        let pongs = a.test_pongs().await;
        assert_eq!(pongs.len(), 1, "直连的探测回到它的来源地址");
        let pong = pongs[0];
        assert_eq!((pong.seq, pong.received, pong.same_path, pong.dst), (0, 1, true, node(2)));
        assert!(i3t_crypto::verify(&me, TestPong::PURPOSE, &pong.signed_message(), &Signature::from_bytes(pong.signature)).is_ok());

        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 7, 0, None), now, false, now);
        assert!(a.test_pongs().await.is_empty(), "重放的探测不回答第二次");
        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 3, 7, 1, None), now, false, now);
        assert!(a.test_pongs().await.is_empty(), "别的对端拿着这个编号也不行");
        let forged = TestPing { signature: [0; 64], ..test_ping(&harness, 2, 7, 1, None) };
        harness.control.on_test_ping(a.addr(), &forged, now, false, now);
        assert!(a.test_pongs().await.is_empty(), "签名不对");
        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 7, 1, None), now, true, now);
        assert!(a.test_pongs().await.is_empty(), "说是直连、却是经 relay 来的");

        for seq in 1..5 {
            harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 7, seq, None), now, false, now);
        }
        let answered: Vec<u32> = a.test_pongs().await.iter().map(|pong| pong.seq).collect();
        assert_eq!(answered, [1, 2], "这个测试最多回答 3 个");

        harness.control.open_test(&window(2, 8, 10), now);
        let later = now + Duration::from_secs(11);
        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 8, 0, None), later, false, later);
        assert!(a.test_pongs().await.is_empty(), "时间窗过了");
        harness.control.poll_tests(later);
        assert!(!harness.control.shared.expects_test(8, node(2)), "过了时间窗的测试自己关掉");
        harness.control.close_test(7, "测试结束");
        assert!(!harness.control.shared.expects_test(7, node(2)));
    }

    #[tokio::test]
    async fn a_relay_only_node_answers_no_direct_test_probe_and_refuses_to_try_a_direct_path() {
        let a = Socket::new().await;
        let mut harness = Harness::with_me(&[(2, &[])], &serde_json::json!({ "reach": "relay_only" }));
        let now = Instant::now();
        harness.control.open_test(&window(2, 7, 10), now);
        harness.control.on_test_ping(a.addr(), &test_ping(&harness, 2, 7, 0, None), now, false, now);
        assert!(a.test_pongs().await.is_empty(), "回答直连的探测就把地址告诉了对端");

        let (reply, answer) = oneshot::channel();
        harness.control.attempt_direct(node(2), Side::Lead, Duration::from_secs(1), reply, now);
        let attempt = answer.await.unwrap();
        assert_eq!((attempt.ok, attempt.failure), (false, Some(crate::diag::DirectFailure::RelayOnly)));
        assert!(harness.control.attempts.is_empty(), "不建立直连的节点连试都不试");
    }

    #[tokio::test]
    async fn a_direct_attempt_reports_an_existing_direct_path_at_once_and_otherwise_gives_up_with_a_reason() {
        let socket = Socket::new().await;
        let mut harness = Harness::new(&[(2, &[]), (3, &[])]);
        let now = Instant::now();
        validate(&mut harness, 2, &socket, now).await;
        let (reply, answer) = oneshot::channel();
        harness.control.attempt_direct(node(2), Side::Lead, Duration::from_secs(1), reply, now);
        let attempt = answer.await.unwrap();
        assert!(attempt.ok && attempt.already && attempt.elapsed.is_none());

        // 对端 3 没有任何候选地址：到时间也没有结果，说得出原因。
        let (reply, answer) = oneshot::channel();
        harness.control.attempt_direct(node(3), Side::Lead, Duration::from_secs(1), reply, now);
        harness.control.poll_tests(now);
        assert_eq!(harness.control.attempts.len(), 1, "还在等");
        harness.control.poll_tests(now + Duration::from_secs(1));
        let attempt = answer.await.unwrap();
        assert_eq!((attempt.ok, attempt.failure, attempt.candidates), (false, Some(crate::diag::DirectFailure::NoCandidates), 0));
    }
}
