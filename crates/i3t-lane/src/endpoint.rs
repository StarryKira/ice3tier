//! [`Lanes`]：一个节点上所有的 lane。按对端分，开流和接受流都从这里走。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use i3t_conn::contract::{Datagram, DatagramSendError, DatagramSender, Datagrams};
use i3t_conn::{Acceptor, Connector, Incoming, OpenError};
use i3t_crypto::NodeKey;
use i3t_crypto::tls::{Identity, TlsError};
use i3t_proto::flow::Recipient;
use i3t_proto::{NodeId, PublicKey};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::{ClientConfig, ServerConfig};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;

use crate::admission::Admission;
use crate::dialler::{self, DialSide, Opening};
use crate::error::{LaneError, OpenLaneError};
use crate::frame::Purpose;
use crate::holder::{self, Holder};
use crate::lane::{self, Dial, Lane};
use crate::status::{Counters, LaneStatus, Level};
use crate::stream::{self, Gauges, LaneRecv, LaneSend};

/// 一个对端同时在用的 lane 最多这么多条（和共用连接上的并发流上限一样）。再开过来的流直接重置。
const MAX_IN_USE_PER_PEER: usize = 16384;
/// 一个对端拨来、等它自己开流的 lane（它的热 lane）最多这么多条。
const MAX_OWN_WAITING_PER_PEER: usize = 64;
/// 当场拨一条 lane 给自己开流最多等多久。超过了调用方改走共用的连接，一次 SYN 重传（1 秒）还等得起。
const OWN_DIAL_DEADLINE: Duration = Duration::from_secs(3);
/// ClientHello 要在接受连接之后多久之内到齐。
const FIRST_RECORD_DEADLINE: Duration = Duration::from_secs(3);
/// 拒绝和挤掉连接的日志最多隔这么久记一条。
const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// lane 的设置。默认值就是产品里用的；测试可以把时间调短。
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LaneConfig {
    /// 接受方：每个拨号方平时至少为本节点停着多少条空闲的 lane（本节点自己也拨它时是 0）。
    pub min_idle: usize,
    /// 拨号方：留给自己开流的空闲 lane 有几条。用完了当场拨。
    pub warm: usize,
    /// 控制 lane 上多久发一次 PING。三个周期听不到对方就判控制 lane 断了。
    pub ping_interval: Duration,
    /// 池空时取 lane 最多等多久。
    pub claim_wait: Duration,
    /// 拨进来的连接从接受起多久之内要握手完成并发来 HELLO。
    pub handshake_deadline: Duration,
}

impl Default for LaneConfig {
    fn default() -> Self {
        Self {
            min_idle: 4,
            warm: 2,
            ping_interval: Duration::from_secs(10),
            claim_wait: Duration::from_secs(2),
            handshake_deadline: Duration::from_secs(10),
        }
    }
}

/// 一个打开了独占 TCP 的对端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanePeer {
    pub id: NodeId,
    /// netmap 里它的公钥。lane 的 TLS 握手钉住它。
    pub key: PublicKey,
    /// 本节点拨它时用的地址：它的节点端口（启用了 relay 的节点在这个端口上听 TCP）。
    /// 空的表示本节点不拨它，只接受它拨来的 lane。
    pub dial: Vec<SocketAddr>,
}

/// 一个节点上所有的 lane。实现 [`Connector`] 和 [`Acceptor`]：到对端开流、接受对端开来的流。
///
/// 丢弃它时所有的空闲 lane 和控制 lane 都被关掉；正在承载流的 lane 不受影响，跑到流自己结束。
pub struct Lanes {
    inner: Arc<Inner>,
    _shutdown: watch::Sender<()>,
}

pub(crate) struct Inner {
    identity: Identity,
    server: Arc<ServerConfig>,
    pub(crate) config: LaneConfig,
    pub(crate) counters: Arc<Counters>,
    /// 认得的公钥。TLS 的校验器在握手时查它，所以和 `peers` 分开放。
    keys: Arc<Mutex<HashMap<PublicKey, NodeId>>>,
    peers: Mutex<HashMap<NodeId, Peer>>,
    incoming: mpsc::UnboundedSender<Incoming<LaneSend, LaneRecv>>,
    accepted: tokio::sync::Mutex<mpsc::UnboundedReceiver<Incoming<LaneSend, LaneRecv>>>,
    admission: Arc<Admission>,
    pub(crate) shutdown: watch::Receiver<()>,
    going_away: AtomicBool,
    random: SystemRandom,
    refusal_log: Mutex<RefusalLog>,
}

pub(crate) struct Peer {
    key: PublicKey,
    pub(crate) holder: Holder,
    dial: Option<DialSide>,
    /// 它拨来、等它自己开流的 lane 有几条。
    own_waiting: usize,
    /// 和它之间在用的 lane。
    in_use: Level,
}

impl Peer {
    /// 本节点拨不拨这个对端。拨的话，本节点往它开流用自己拨的 lane，不需要它停 lane 过来。
    pub(crate) fn dials(&self) -> bool {
        self.dial.is_some()
    }
}

/// 拨一条 lane 要用的东西。
pub(crate) struct DialPlan {
    pub(crate) addrs: Vec<SocketAddr>,
    pub(crate) config: Arc<ClientConfig>,
}

impl Lanes {
    /// # Errors
    ///
    /// 用节点密钥签证书失败。
    pub fn new(key: &NodeKey, config: LaneConfig) -> Result<Self, TlsError> {
        let identity = Identity::new(key)?;
        let keys: Arc<Mutex<HashMap<PublicKey, NodeId>>> = Arc::default();
        let allowed = Arc::clone(&keys);
        let server = lane::server_config(&identity, move |key| allowed.lock().expect("lane keys mutex poisoned").contains_key(key))?;
        let (incoming, accepted) = mpsc::unbounded_channel();
        let (shutdown_sender, shutdown) = watch::channel(());
        let inner = Inner {
            identity,
            server,
            config,
            counters: Arc::default(),
            keys,
            peers: Mutex::default(),
            incoming,
            accepted: tokio::sync::Mutex::new(accepted),
            admission: Arc::default(),
            shutdown,
            going_away: AtomicBool::new(false),
            random: SystemRandom::new(),
            refusal_log: Mutex::default(),
        };
        Ok(Self { inner: Arc::new(inner), _shutdown: shutdown_sender })
    }

    /// 换成新的一组打开了独占 TCP 的对端。netmap 每次变化时整组交进来。
    ///
    /// 不在里面的对端：它的空闲 lane 和控制 lane 关掉，它新拨来的 lane 在握手时被拒绝；正在承载流的 lane 不受影响。
    /// 公钥或者拨号地址变了的对端按新的重来一遍。要在 tokio 运行时里调用（拨号的任务在这里起）。
    ///
    /// # Errors
    ///
    /// 给某个对端做 TLS 配置失败。这时什么都没有改。
    pub fn set_peers(&self, peers: impl IntoIterator<Item = LanePeer>) -> Result<(), TlsError> {
        let inner = &self.inner;
        let wanted: Vec<LanePeer> = peers.into_iter().collect();
        let configs = wanted
            .iter()
            .filter(|peer| !peer.dial.is_empty())
            .map(|peer| Ok((peer.id, lane::client_config(&inner.identity, peer.key)?)))
            .collect::<Result<HashMap<NodeId, Arc<ClientConfig>>, TlsError>>()?;
        *inner.keys.lock().expect("lane keys mutex poisoned") = wanted.iter().map(|peer| (peer.key, peer.id)).collect();
        let mut peers = inner.lock_peers();
        peers.retain(|id, entry| {
            let keep = wanted.iter().any(|peer| peer.id == *id && peer.key == entry.key);
            if !keep {
                entry.holder.close();
                tracing::info!(peer = %id, "不再为这个节点开独占 TCP");
            }
            keep
        });
        for peer in wanted {
            let entry = peers.entry(peer.id).or_insert_with(|| Peer {
                key: peer.key,
                holder: Holder::default(),
                dial: None,
                own_waiting: 0,
                in_use: Level::default(),
            });
            let same_dial = entry.dial.as_ref().is_some_and(|dial| dial.addrs == peer.dial);
            if same_dial || (peer.dial.is_empty() && entry.dial.is_none()) {
                continue;
            }
            entry.dial = configs.get(&peer.id).map(|config| {
                let wake = Arc::new(Notify::new());
                let task = tokio::spawn(dialler::run(Arc::clone(inner), peer.id, Arc::clone(&wake))).abort_handle();
                tracing::info!(peer = %peer.id, addrs = ?peer.dial, "开始为这个节点拨独占 TCP");
                DialSide {
                    addrs: peer.dial.clone(),
                    config: Arc::clone(config),
                    warm: Default::default(),
                    peer_going_away: false,
                    control_up: false,
                    parked: 0,
                    wake,
                    task,
                    last_good: None,
                }
            });
        }
        Ok(())
    }

    /// 接过一条已经被接受的 TCP 连接：它是拨来的 lane（开头是 TLS 记录，见 [`is_lane`]）。
    ///
    /// 给节点端口上的 TCP 监听用：那个监听属于 relay，它按开头的字节分辨出 lane 之后交到这里。
    /// `already_read` 是它为了分辨已经从连接上读走的字节（用 `peek` 的话传空）。
    ///
    /// 不等待：握手在自己的任务里做，从接受起有绝对的时限；同一来源、以及全部来源还没握手完的连接数有上限，
    /// 满了挤掉最老的。
    pub fn accept_tcp(&self, stream: TcpStream, from: SocketAddr, already_read: &[u8]) {
        let inner = &self.inner;
        let (ticket, evicted) = inner.admission.admit(from.ip());
        if evicted > 0 {
            inner.counters.handshake_busy.add(evicted);
            inner.log_refusal(format_args!("还没握手完的 lane 太多，挤掉了 {evicted} 条最老的（来自 {from} 的新连接进来了）"));
        }
        let id = ticket.id();
        let started = Instant::now();
        let task = tokio::spawn({
            let inner = Arc::clone(inner);
            let prefix = BytesMut::from(already_read);
            async move {
                let deadline = started + inner.config.handshake_deadline;
                let first_record = started + FIRST_RECORD_DEADLINE;
                let resolve = |key: &PublicKey| inner.keys.lock().expect("lane keys mutex poisoned").get(key).copied();
                let accepted = lane::accept(stream, prefix, Arc::clone(&inner.server), resolve, first_record, deadline).await;
                drop(ticket);
                match accepted {
                    Ok(lane) => inner.dispatch(lane),
                    Err(error) => inner.handshake_failed(from, &error),
                }
            }
        });
        inner.admission.attach(id, task.abort_handle());
    }

    /// 在 `listener` 上接受拨来的 lane，直到本节点关闭。节点用的是 relay 的监听（见 [`accept_tcp`](Self::accept_tcp)）；
    /// 这个给测试和单独跑 lane 的场合用。
    pub async fn serve(&self, listener: TcpListener) {
        let mut shutdown = self.inner.shutdown.clone();
        loop {
            let accepted = tokio::select! {
                _ = shutdown.changed() => return,
                accepted = listener.accept() => accepted,
            };
            match accepted {
                Ok((stream, from)) => self.accept_tcp(stream, from, &[]),
                Err(error) => {
                    tracing::warn!(%error, "接受 lane 的连接失败");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// 在 lane 上到 `peer` 开一条流。
    ///
    /// 本节点拨 `peer` 时用自己的热 lane，没有了就当场拨一条（最多等 3 秒）；否则从 `peer` 停在本节点的 lane 里取，
    /// 池空时等它补拨，最多等 [`LaneConfig::claim_wait`]。
    ///
    /// 返回的流还没有发出任何东西：头写完时调 [`LaneSend::end_header`]（见那里）。
    ///
    /// # Errors
    ///
    /// 取不到 lane。这时开流请求还没有发出去，调用方可以放心地改走共用的连接。
    pub async fn open_stream(&self, peer: NodeId) -> Result<(LaneSend, LaneRecv), OpenLaneError> {
        let inner = &self.inner;
        if inner.is_going_away() {
            return Err(OpenLaneError::GoingAway(peer));
        }
        let gauges = inner.gauges(peer).ok_or(OpenLaneError::TooMany(peer))?;
        let lane = match inner.take_warm(peer)? {
            Some(Ok(lane)) => lane,
            Some(Err(plan)) => inner.dial_now(peer, plan).await?,
            None => holder::claim(inner, peer).await?,
        };
        Ok(stream::initiate(lane, &inner.counters, gauges))
    }

    /// 现在能不能在 lane 上到 `peer` 开流（不保证池里马上有）。不能的话调用方直接走共用的连接，不用试。
    #[must_use]
    pub fn can_open(&self, peer: NodeId) -> bool {
        let inner = &self.inner;
        if inner.is_going_away() {
            return false;
        }
        inner.lock_peers().get(&peer).is_some_and(|entry| match &entry.dial {
            Some(dial) => dial.control_up && !dial.peer_going_away,
            None => entry.holder.available(inner.config.ping_interval),
        })
    }

    /// 本节点要走了（排空）：告诉所有对端，不再拨新的 lane；对端随之不再在和本节点之间的 lane 上开新的流。
    /// 手上的流照常结束。
    pub fn go_away(&self) {
        let inner = &self.inner;
        inner.going_away.store(true, Ordering::Relaxed);
        let mut peers = inner.lock_peers();
        for entry in peers.values_mut() {
            entry.holder.go_away();
            if let Some(dial) = &mut entry.dial {
                dial.warm.clear();
                dial.wake.notify_one();
            }
        }
        drop(peers);
        tracing::info!("独占 TCP 进入排空：通知对端，不再拨新的 lane");
    }

    /// 本节点的地址变了（家宽重拨等）：拨出去的 lane 多半已经死了而且不会收到重置。全部换一个新的 epoch 重拨。
    pub fn address_changed(&self) {
        let inner = &self.inner;
        let mut peers = inner.lock_peers();
        for (id, entry) in peers.iter_mut() {
            if let Some(dial) = &mut entry.dial {
                dial.task.abort();
                dial.warm.clear();
                dial.last_good = None;
                dial.task = tokio::spawn(dialler::run(Arc::clone(inner), *id, Arc::clone(&dial.wake))).abort_handle();
            }
        }
        drop(peers);
        tracing::info!("本节点的地址变了，拨出去的 lane 换新的 epoch 重拨");
    }

    #[must_use]
    pub fn status(&self) -> LaneStatus {
        let inner = &self.inner;
        let mut status = inner.counters.snapshot();
        let peers = inner.lock_peers();
        for entry in peers.values() {
            status.idle += u64::try_from(entry.holder.idle()).unwrap_or(u64::MAX);
            if let Some(dial) = &entry.dial {
                status.warm += u64::try_from(dial.warm.len()).unwrap_or(u64::MAX);
                status.parked_at_peers += u64::try_from(dial.parked).unwrap_or(u64::MAX);
            }
        }
        status
    }
}

impl Inner {
    pub(crate) fn lock_peers(&self) -> MutexGuard<'_, HashMap<NodeId, Peer>> {
        self.peers.lock().expect("lane peers mutex poisoned")
    }

    pub(crate) fn random_u64(&self) -> u64 {
        let mut bytes = [0; 8];
        self.random.fill(&mut bytes).expect("系统的随机数源不可用");
        u64::from_be_bytes(bytes)
    }

    pub(crate) fn is_going_away(&self) -> bool {
        self.going_away.load(Ordering::Relaxed)
    }

    /// 握手完成的一条 lane，按它的用途交出去。
    fn dispatch(self: &Arc<Self>, lane: Lane) {
        match lane.purpose {
            Purpose::Control => holder::control_arrived(self, lane),
            Purpose::Parked => holder::parked_arrived(self, lane),
            Purpose::Own => self.own_arrived(lane),
        }
    }

    /// 对端拨来一条给它自己开流用的 lane：守着它，等对端开流。
    fn own_arrived(self: &Arc<Self>, mut lane: Lane) {
        let peer = lane.peer;
        {
            let mut peers = self.lock_peers();
            let Some(entry) = peers.get_mut(&peer) else {
                drop(peers);
                return self.refuse(peer, "没有为这个节点打开独占 TCP");
            };
            if entry.own_waiting >= MAX_OWN_WAITING_PER_PEER {
                drop(peers);
                return self.refuse(peer, "它等着开流的 lane 到了上限");
            }
            entry.own_waiting += 1;
        }
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let mut shutdown = inner.shutdown.clone();
            let opening = tokio::select! {
                _ = shutdown.changed() => Ok(None),
                opening = dialler::await_opening(&mut lane) => opening,
            };
            if let Some(entry) = inner.lock_peers().get_mut(&peer) {
                entry.own_waiting -= 1;
            }
            match opening {
                Ok(Some(opening)) => inner.deliver(lane, &opening),
                Ok(None) => {}
                Err(error) => tracing::debug!(%peer, %error, "对方的一条 lane 没有开流就结束了"),
            }
        });
    }

    /// 对方在一条 lane 上开了流：交给接受队列。
    pub(crate) fn deliver(&self, lane: Lane, opening: &Opening) {
        let peer = lane.peer;
        let Some(gauges) = self.gauges(peer) else { return self.refuse(peer, "和它之间在用的 lane 到了上限") };
        let incoming = dialler::incoming(lane, opening, &self.counters, gauges);
        // 接受队列没有人在收（本节点在关闭）：流被丢弃，对方看到重置。
        let _ = self.incoming.send(incoming);
    }

    /// 一条 lane 开始承载流时占的数。和这个对端之间在用的到了上限时返回 `None`。
    fn gauges(&self, peer: NodeId) -> Option<Gauges> {
        let peers = self.lock_peers();
        let entry = peers.get(&peer)?;
        let in_use = usize::try_from(entry.in_use.get()).unwrap_or(usize::MAX);
        (in_use < MAX_IN_USE_PER_PEER).then(|| [self.counters.in_use.enter(), entry.in_use.enter()])
    }

    /// 握手完成之后因为用途或者数量不对关掉一条 lane。对方是认证过的节点，但它可以一直这样做，所以只记 debug。
    pub(crate) fn refuse(&self, peer: NodeId, reason: &str) {
        self.counters.refused.bump();
        tracing::debug!(%peer, reason, "关掉了对方拨来的一条 lane");
    }

    fn handshake_failed(&self, from: SocketAddr, error: &LaneError) {
        let counter = match error {
            LaneError::Deadline(_) => &self.counters.handshake_deadline,
            _ if error.is_unknown_key() => &self.counters.handshake_unknown_key,
            LaneError::Tls(_) => &self.counters.handshake_tls,
            _ => &self.counters.handshake_protocol,
        };
        counter.bump();
        // 外人能造成的失败，每次都记 info 就能刷屏。
        tracing::debug!(%from, %error, "拨来的 lane 没有握手成功");
    }

    fn log_refusal(&self, message: std::fmt::Arguments<'_>) {
        let mut log = self.refusal_log.lock().expect("refusal log mutex poisoned");
        if let Some(skipped) = log.due(Instant::now()) {
            tracing::warn!(skipped, "{message}");
        }
    }

    pub(crate) fn log_dry_pool(&self, peer: NodeId, waited: Duration) {
        let mut log = self.refusal_log.lock().expect("refusal log mutex poisoned");
        if let Some(skipped) = log.due(Instant::now()) {
            tracing::info!(%peer, ?waited, skipped, "对方停在这里的 lane 用完了，等不到补拨，新的流改走共用的连接");
        }
    }

    /// 取一条热 lane。`Ok(None)`：本节点不拨这个对端（去它停过来的 lane 里取）；`Ok(Some(Err(_)))`：热的没有了，要当场拨。
    fn take_warm(&self, peer: NodeId) -> Result<Option<Result<Lane, DialPlan>>, OpenLaneError> {
        let mut peers = self.lock_peers();
        let entry = peers.get_mut(&peer).ok_or(OpenLaneError::NotEnabled(peer))?;
        let Some(dial) = &mut entry.dial else { return Ok(None) };
        if dial.peer_going_away {
            return Err(OpenLaneError::GoingAway(peer));
        }
        // 对方还没认本节点（或者已经不认了）：当场拨的 lane 会在开流的帧发出去之后才被拒绝，那时已经不能改走别的路。
        if !dial.control_up {
            return Err(OpenLaneError::NoControl(peer));
        }
        dial.wake.notify_one();
        while let Some(lane) = dial.warm.pop_back() {
            if lane.is_usable() {
                return Ok(Some(Ok(lane)));
            }
        }
        Ok(Some(Err(DialPlan { addrs: dial.addrs_in_order(), config: Arc::clone(&dial.config) })))
    }

    /// 当场拨一条 lane 给自己开流。HELLO 留在待发的密文里，和开流的帧一起走。
    async fn dial_now(&self, peer: NodeId, plan: DialPlan) -> Result<Lane, OpenLaneError> {
        let dial = Dial {
            addrs: &plan.addrs,
            config: plan.config,
            peer,
            purpose: Purpose::Own,
            epoch: 0,
            deadline: Instant::now() + OWN_DIAL_DEADLINE,
        };
        let dialled = {
            let _dialling = self.counters.dialling.enter();
            lane::dial(dial).await
        };
        match dialled {
            Ok((lane, addr)) => {
                self.remember_good(peer, addr);
                Ok(lane)
            }
            Err(source) => {
                self.counters.dial_failures.bump();
                Err(OpenLaneError::Dial { peer, source })
            }
        }
    }

    pub(crate) fn dial_plan(&self, peer: NodeId) -> Option<DialPlan> {
        let peers = self.lock_peers();
        let dial = peers.get(&peer)?.dial.as_ref()?;
        Some(DialPlan { addrs: dial.addrs_in_order(), config: Arc::clone(&dial.config) })
    }

    pub(crate) fn remember_good(&self, peer: NodeId, addr: SocketAddr) {
        if let Some(dial) = self.lock_peers().get_mut(&peer).and_then(|entry| entry.dial.as_mut()) {
            dial.last_good = Some(addr);
        }
    }

    /// 热 lane 的条数，和对方是不是说了要走。本节点不拨它时返回 `None`。
    pub(crate) fn warm_state(&self, peer: NodeId) -> Option<(usize, bool)> {
        let peers = self.lock_peers();
        let dial = peers.get(&peer)?.dial.as_ref()?;
        Some((dial.warm.len(), dial.peer_going_away))
    }

    pub(crate) fn add_warm(&self, peer: NodeId, lane: Lane) {
        if let Some(dial) = self.lock_peers().get_mut(&peer).and_then(|entry| entry.dial.as_mut()) {
            dial.warm.push_back(lane);
        }
    }

    pub(crate) fn set_parked(&self, peer: NodeId, parked: usize) {
        if let Some(dial) = self.lock_peers().get_mut(&peer).and_then(|entry| entry.dial.as_mut()) {
            dial.parked = parked;
        }
    }

    pub(crate) fn set_control_up(&self, peer: NodeId, up: bool) {
        if let Some(dial) = self.lock_peers().get_mut(&peer).and_then(|entry| entry.dial.as_mut()) {
            dial.control_up = up;
            if !up {
                dial.warm.clear();
            }
        }
    }

    pub(crate) fn peer_going_away(&self, peer: NodeId) {
        if let Some(dial) = self.lock_peers().get_mut(&peer).and_then(|entry| entry.dial.as_mut()) {
            dial.peer_going_away = true;
            dial.warm.clear();
        }
        tracing::info!(%peer, "对方说它要走了，不再拨新的 lane，也不再在 lane 上往它开流");
    }
}

/// 一条 TCP 连接开头的字节像不像 lane：lane 以 TLS 的握手记录开头（`0x16 0x03`）。
///
/// relay 的 TCP 中转以 2 字节的帧长度开头，第一个帧是很短的会话握手，长度的高字节远小于 `0x16`，两者不会混淆。
/// 只有第一个字节时按第一个字节判断。
#[must_use]
pub fn is_lane(prefix: &[u8]) -> bool {
    matches!(prefix, [0x16] | [0x16, 0x03, ..])
}

/// 拒绝和挤掉连接的日志的限速：有人一直往上连的时候不刷屏。
#[derive(Debug, Default)]
struct RefusalLog {
    logged: Option<Instant>,
    skipped: u64,
}

impl RefusalLog {
    fn due(&mut self, now: Instant) -> Option<u64> {
        if self.logged.is_some_and(|at| now.saturating_duration_since(at) < REFUSAL_LOG_INTERVAL) {
            self.skipped += 1;
            return None;
        }
        self.logged = Some(now);
        Some(std::mem::take(&mut self.skipped))
    }
}

impl Connector for Lanes {
    type Send = LaneSend;
    type Recv = LaneRecv;

    async fn open(&self, peer: NodeId) -> Result<(LaneSend, LaneRecv), OpenError> {
        self.open_stream(peer).await.map_err(|error| error.to_open_error())
    }
}

impl Acceptor for Lanes {
    type Send = LaneSend;
    type Recv = LaneRecv;

    async fn accept(&self) -> Option<Incoming<LaneSend, LaneRecv>> {
        self.inner.accepted.lock().await.recv().await
    }
}

/// lane 不传数据报：UDP 应用的数据始终走共用的连接。实现它只是为了让 `i3t_flow::serve` 能直接跑在 lane 上；
/// 收数据报的一方立刻结束，`serve` 照常只接流。
impl Datagrams for Lanes {
    type Sender = NoDatagrams;

    fn datagram_sender(&self) -> NoDatagrams {
        NoDatagrams
    }

    async fn recv_datagram(&self, _recipient: Recipient) -> Option<Datagram> {
        None
    }
}

/// lane 上发不了数据报。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDatagrams;

impl DatagramSender for NoDatagrams {
    fn max_datagram_len(&self, _peer: NodeId) -> Option<usize> {
        None
    }

    fn send_datagram(&self, peer: NodeId, _datagram: Bytes) -> Result<(), DatagramSendError> {
        Err(DatagramSendError::Unsupported(peer))
    }

    async fn send_reliable(&self, peer: NodeId, _datagram: Bytes) -> Result<(), DatagramSendError> {
        Err(DatagramSendError::Unsupported(peer))
    }
}
