//! 按对端管理连接：一对节点一条连接，按需建立，空闲回收，对端从 netmap 里消失就立刻关闭。

use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use i3t_crypto::NodeKey;
use i3t_crypto::tls;
use i3t_model::Netmap;
use i3t_model::control::Signal;
use i3t_path::Paths;
use i3t_proto::flow::{DatagramHeader, MAX_DATAGRAM_BODY_LEN, Recipient};
use i3t_proto::{NodeId, PublicKey};
use quinn::rustls::pki_types::CertificateDer;
use quinn::{ConnectError, SendDatagramError, TransportConfig, VarInt};
use tokio::sync::{Notify, mpsc, watch};

use crate::contract::{Datagram, DatagramSendError, DatagramSender, Incoming, OpenError};
use crate::quic::{self, SERVER_NAME};
use crate::socket;
use crate::stream::{self, RecvHalf, SendHalf, StreamCount};

/// 建立连接的时限（白皮书 §5.4）。没有 relay 可走时，这段时间花在等直连打通上。
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// 对端连进来时握手的时限。
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);
/// 没有流之后多久关闭连接（白皮书 §6.5）。这样 N 个节点不会留下 N² 条常驻连接和保活流量。
const IDLE_CLOSE: Duration = Duration::from_secs(300);
/// 两边同时拨号会留下两条连接。多出来的那一条没有流之后这么久就关。
const DUPLICATE_IDLE_CLOSE: Duration = Duration::from_secs(10);
/// 多久看一次连接是不是空闲。
const IDLE_CHECK: Duration = Duration::from_secs(1);
/// 连接里送来的一条信令最长多少字节、最多等多久。
const SIGNAL_MAX_BYTES: usize = 4096;
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);
/// 走单向流的数据报最多等多久读完。它是一次写进去的，正常情况下一到就是完整的。
const RELIABLE_DATAGRAM_TIMEOUT: Duration = Duration::from_secs(10);
/// 对端发来、上层还没有取走的数据报，每一侧最多排多少个。
pub(crate) const DATAGRAM_BACKLOG: usize = 1024;

/// 关闭连接时带给对端的码。
const CLOSE_IDLE: u32 = 0;
const CLOSE_SHUTDOWN: u32 = 1;
const CLOSE_REVOKED: u32 = 2;
const CLOSE_IDENTITY: u32 = 3;

type Stream = Incoming<SendHalf, RecvHalf>;

/// 对端发来的数据报排在这里等上层来取，给上下文的接受方和发起方的各一个队列。
pub(crate) struct DatagramQueues {
    pub acceptor: mpsc::Sender<Datagram>,
    pub initiator: mpsc::Sender<Datagram>,
}

impl DatagramQueues {
    fn of(&self, recipient: Recipient) -> &mpsc::Sender<Datagram> {
        match recipient {
            Recipient::Acceptor => &self.acceptor,
            Recipient::Initiator => &self.initiator,
        }
    }
}

/// 一条和对端的连接。
struct Conn {
    quic: quinn::Connection,
    /// 这条连接是谁拨的。两边同时拨号时，双方都留下编号小的那个节点拨的那一条。
    dialer: NodeId,
    /// 对端握手时用的公钥。netmap 里它的公钥变了，这条连接就不算数了。
    key: PublicKey,
    /// 越新的越大。
    serial: u64,
    streams: Arc<StreamCount>,
}

#[derive(Default)]
struct PeerEntry {
    conns: Vec<Arc<Conn>>,
    /// 同一时刻只有一个任务在拨这个对端，别的等它的结果。
    dial: Arc<tokio::sync::Mutex<()>>,
    /// 有新的连接登记进来时通知：正在拨号的人可以改用对端连进来的那一条。
    arrived: Arc<Notify>,
}

impl PeerEntry {
    /// 开新的流用哪一条：编号小的那个节点拨的；同一方拨的取最新的。
    fn preferred(&self) -> Option<Arc<Conn>> {
        let alive = self.conns.iter().filter(|conn| conn.quic.close_reason().is_none());
        alive.min_by_key(|conn| (conn.dialer, Reverse(conn.serial))).cloned()
    }
}

pub(crate) struct Manager {
    me: NodeId,
    key: Arc<NodeKey>,
    quic: quinn::Endpoint,
    paths: Arc<Paths>,
    netmap: watch::Receiver<Arc<Netmap>>,
    transport: Arc<TransportConfig>,
    peers: Mutex<HashMap<NodeId, PeerEntry>>,
    next_serial: AtomicU64,
    /// 对端开过来的流排在这里等上层来取。
    incoming: mpsc::Sender<Stream>,
    datagrams: DatagramQueues,
    closed: watch::Sender<bool>,
}

impl Manager {
    pub(crate) fn new(
        key: Arc<NodeKey>,
        quic: quinn::Endpoint,
        paths: Arc<Paths>,
        netmap: watch::Receiver<Arc<Netmap>>,
        incoming: mpsc::Sender<Stream>,
        datagrams: DatagramQueues,
    ) -> Arc<Self> {
        let me = netmap.borrow().me.id;
        Arc::new(Self {
            me,
            key,
            quic,
            paths,
            netmap,
            transport: quic::transport(),
            peers: Mutex::new(HashMap::new()),
            next_serial: AtomicU64::new(0),
            incoming,
            datagrams,
            closed: watch::Sender::new(false),
        })
    }

    fn peers(&self) -> std::sync::MutexGuard<'_, HashMap<NodeId, PeerEntry>> {
        self.peers.lock().expect("connection table mutex poisoned")
    }

    fn preferred(&self, peer: NodeId) -> Option<Arc<Conn>> {
        self.peers().get(&peer).and_then(PeerEntry::preferred)
    }

    pub(crate) fn subscribe_closed(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }

    /// 到 `peer` 开一条流。没有连接时先建；有对端连进来的连接就直接用。
    pub(crate) async fn open(self: &Arc<Self>, peer: NodeId) -> Result<(SendHalf, RecvHalf), OpenError> {
        let mut retried = false;
        loop {
            if *self.closed.borrow() {
                return Err(OpenError::Closed);
            }
            let conn = self.connection(peer).await?;
            match conn.quic.open_bi().await {
                Ok((send, recv)) => return Ok(stream::wrap(send, recv, &conn.streams)),
                // 查到的连接刚好断了：重建一次。再不行就报错，不无限重试。
                Err(error) if retried => return Err(OpenError::ConnectionLost { peer, reason: error.to_string() }),
                Err(_) => {
                    self.forget(peer, &conn);
                    retried = true;
                }
            }
        }
    }

    async fn connection(self: &Arc<Self>, peer: NodeId) -> Result<Arc<Conn>, OpenError> {
        let (dial, arrived) = {
            if self.netmap.borrow().peer(peer).is_none() {
                return Err(OpenError::UnknownPeer(peer));
            }
            let mut peers = self.peers();
            let entry = peers.entry(peer).or_default();
            if let Some(conn) = entry.preferred() {
                return Ok(conn);
            }
            (Arc::clone(&entry.dial), Arc::clone(&entry.arrived))
        };
        let _dialing = dial.lock().await;
        // 等锁的时候别人可能已经拨通了，或者对端连进来了。
        if let Some(conn) = self.preferred(peer) {
            return Ok(conn);
        }
        // 先登记再拨号，拨号期间对端连进来就不会漏掉。
        let arrival = arrived.notified();
        tokio::pin!(arrival);
        arrival.as_mut().enable();
        tokio::select! {
            dialed = self.dial(peer) => dialed,
            () = &mut arrival => self.preferred(peer).ok_or(OpenError::NoRoute(peer)),
        }
    }

    async fn dial(self: &Arc<Self>, peer: NodeId) -> Result<Arc<Conn>, OpenError> {
        let (peer_key, dial_out_only) = match self.netmap.borrow().peer(peer) {
            Some(info) => (info.key, info.dial_out_only),
            None => return Err(OpenError::UnknownPeer(peer)),
        };
        if dial_out_only {
            tracing::info!(%peer, "对端只拨出，不接受连进去的连接；只能等它连过来");
            return Err(OpenError::NoRoute(peer));
        }
        let deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
        if !self.paths.wait_for_route(peer, DIAL_TIMEOUT).await {
            tracing::info!(%peer, "{} 秒内没有等到任何到对端的路（直连没有打通，也没有 relay 可走）", DIAL_TIMEOUT.as_secs());
            return Err(OpenError::NoRoute(peer));
        }
        let lost = |reason: String| OpenError::ConnectionLost { peer, reason };
        let config = quic::client(&self.key, peer_key, Arc::clone(&self.transport)).map_err(|error| lost(error.to_string()))?;
        let connecting = match self.quic.connect_with(config, socket::virtual_addr(peer), SERVER_NAME) {
            Ok(connecting) => connecting,
            Err(ConnectError::EndpointStopping) => return Err(OpenError::Closed),
            Err(error) => return Err(lost(error.to_string())),
        };
        match tokio::time::timeout_at(deadline, connecting).await {
            Ok(Ok(connection)) => Ok(self.register(peer, connection, self.me, peer_key)),
            Ok(Err(error)) => {
                tracing::info!(%peer, %error, "和对端的握手失败");
                Err(lost(error.to_string()))
            }
            Err(_) => {
                tracing::info!(%peer, "和对端建立连接超时");
                Err(OpenError::Timeout(peer))
            }
        }
    }

    /// 登记一条握手完成的连接，并开始接收它上面的流。
    fn register(self: &Arc<Self>, peer: NodeId, quic: quinn::Connection, dialer: NodeId, key: PublicKey) -> Arc<Conn> {
        let serial = self.next_serial.fetch_add(1, Ordering::Relaxed);
        let conn = Arc::new(Conn { quic, dialer, key, serial, streams: StreamCount::new() });
        {
            let mut peers = self.peers();
            let entry = peers.entry(peer).or_default();
            entry.conns.push(Arc::clone(&conn));
            entry.arrived.notify_waiters();
        }
        tracing::info!(%peer, %dialer, "和对端的连接建立");
        tokio::spawn(Arc::clone(self).serve(peer, Arc::clone(&conn)));
        conn
    }

    fn forget(&self, peer: NodeId, conn: &Arc<Conn>) {
        if let Some(entry) = self.peers().get_mut(&peer) {
            entry.conns.retain(|other| !Arc::ptr_eq(other, conn));
        }
    }

    /// 一条连接的一生：接收对端开过来的流、数据报和信令，空闲够久就关掉。
    async fn serve(self: Arc<Self>, peer: NodeId, conn: Arc<Conn>) {
        let mut idle_check = tokio::time::interval(IDLE_CHECK);
        idle_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let reason = loop {
            tokio::select! {
                accepted = conn.quic.accept_bi() => match accepted {
                    Ok((send, recv)) => {
                        let (send, recv) = stream::wrap(send, recv, &conn.streams);
                        if self.incoming.send(Incoming { peer, send, recv }).await.is_err() {
                            break "本端正在关闭".to_owned();
                        }
                    }
                    Err(error) => break error.to_string(),
                },
                accepted = conn.quic.accept_uni() => match accepted {
                    Ok(recv) => drop(tokio::spawn(Arc::clone(&self).read_uni(peer, conn.quic.clone(), recv))),
                    Err(error) => break error.to_string(),
                },
                received = conn.quic.read_datagram() => match received {
                    Ok(payload) => self.deliver_datagram(peer, payload),
                    Err(error) => break error.to_string(),
                },
                _ = idle_check.tick() => {
                    if let Some(reason) = self.idle_verdict(peer, &conn) {
                        conn.quic.close(VarInt::from_u32(CLOSE_IDLE), reason.as_bytes());
                        break reason.to_owned();
                    }
                }
            }
        };
        self.forget(peer, &conn);
        tracing::info!(%peer, reason, "和对端的连接结束");
    }

    /// 把对端发来的一个不保证送达的数据报排进上层的队列。队列满了就丢：上层来不及取，留着也只会过期。
    fn deliver_datagram(&self, peer: NodeId, payload: Bytes) {
        let Some(recipient) = Recipient::peek(&payload) else {
            return tracing::trace!(%peer, len = payload.len(), "丢掉一个数据报：首字节看不出它是给哪一侧的");
        };
        if self.datagrams.of(recipient).try_send(Datagram { peer, payload }).is_err() {
            tracing::trace!(%peer, ?recipient, "丢掉一个数据报：上层的队列满了，或者本端正在关闭");
        }
    }

    /// 读对端开过来的一条单向流。首字节说明它是什么：数据报帧的首字节是标志，高位都是 0；
    /// 其余的是一条信令（一个 JSON 对象，首字节是 `{`）。
    async fn read_uni(self: Arc<Self>, peer: NodeId, connection: quinn::Connection, mut stream: quinn::RecvStream) {
        let mut first = [0; 1];
        match tokio::time::timeout(SIGNAL_TIMEOUT, stream.read_exact(&mut first)).await {
            Ok(Ok(())) => {}
            // 空的流什么都不表示；迟迟不写的流不等它。
            Ok(Err(_)) | Err(_) => return,
        }
        match Recipient::peek(&first) {
            Some(recipient) => self.read_reliable_datagram(peer, recipient, first, &connection, stream).await,
            None => read_signal(&self.paths, peer, first, stream).await,
        }
    }

    /// 读一个走单向流的数据报，排进上层的队列。
    ///
    /// 先在队列里占到位置再读流：队列满着的时候这条流就留在那里不读，对端能同时开的流数有上限，
    /// 它开不出新的流，发送就慢下来。这样本端不丢这种数据报，占的内存也有上限。
    async fn read_reliable_datagram(
        &self,
        peer: NodeId,
        recipient: Recipient,
        first: [u8; 1],
        connection: &quinn::Connection,
        mut stream: quinn::RecvStream,
    ) {
        let permit = tokio::select! {
            permit = self.datagrams.of(recipient).reserve() => match permit {
                Ok(permit) => permit,
                // 本端正在关闭。
                Err(_) => return,
            },
            // 连接没了就不用等了，不然没有人取的队列会把这些任务永远留着。
            _ = connection.closed() => return,
        };
        let limit = DatagramHeader::LEN + MAX_DATAGRAM_BODY_LEN - first.len();
        let rest = match tokio::time::timeout(RELIABLE_DATAGRAM_TIMEOUT, stream.read_to_end(limit)).await {
            Ok(Ok(rest)) => rest,
            Ok(Err(error)) => return tracing::debug!(%peer, %error, "丢掉一个走单向流的数据报：没有读完整，或者超过了长度上限"),
            Err(_) => return tracing::debug!(%peer, "丢掉一个走单向流的数据报：迟迟没有发完"),
        };
        let mut payload = BytesMut::with_capacity(first.len() + rest.len());
        payload.put_slice(&first);
        payload.put_slice(&rest);
        permit.send(Datagram { peer, payload: payload.freeze() });
    }

    /// 发数据报用的句柄。它不让连接多活：本端关掉之后，拿着它也发不出去。
    pub(crate) fn datagram_handle(self: &Arc<Self>) -> DatagramHandle {
        DatagramHandle { manager: Arc::downgrade(self) }
    }

    /// 这条连接该不该因为空闲而关掉；该关的话返回原因。
    fn idle_verdict(&self, peer: NodeId, conn: &Arc<Conn>) -> Option<&'static str> {
        let idle = conn.streams.idle_since()?.elapsed();
        let preferred = self.preferred(peer).is_some_and(|preferred| Arc::ptr_eq(&preferred, conn));
        let keep_alive = self.netmap.borrow().peer(peer).is_some_and(|info| info.keep_alive);
        let (limit, reason) = idle_limit(preferred, keep_alive)?;
        (idle >= limit).then_some(reason)
    }

    /// 接受对端连进来的连接，直到本端关闭。
    pub(crate) async fn accept_loop(self: Arc<Self>) {
        while let Some(incoming) = self.quic.accept().await {
            let claimed = socket::node_of(incoming.remote_address());
            let (known, dial_out_only) = {
                let netmap = self.netmap.borrow();
                (claimed.filter(|id| netmap.peer(*id).is_some()), netmap.me.dial_out_only)
            };
            let Some(claimed) = known else {
                tracing::debug!(from = %incoming.remote_address(), "连进来的不是 netmap 里的对端，不理");
                incoming.ignore();
                continue;
            };
            if dial_out_only {
                tracing::info!(peer = %claimed, "本节点只拨出，拒绝对端连进来的连接");
                incoming.refuse();
                continue;
            }
            tokio::spawn(Arc::clone(&self).welcome(incoming, claimed));
        }
    }

    /// 和一个连进来的对端握手，核对它是谁。
    async fn welcome(self: Arc<Self>, incoming: quinn::Incoming, claimed: NodeId) {
        let connecting = match incoming.accept() {
            Ok(connecting) => connecting,
            Err(error) => {
                tracing::debug!(peer = %claimed, %error, "对端连进来的连接没有接成");
                return;
            }
        };
        let connection = match tokio::time::timeout(WELCOME_TIMEOUT, connecting).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                // 公钥不在 netmap 里的对端在这里被拒绝。
                tracing::info!(peer = %claimed, %error, "对端连进来的连接握手失败");
                return;
            }
            Err(_) => {
                tracing::info!(peer = %claimed, "对端连进来的连接握手超时");
                return;
            }
        };
        // 对端是谁只看握手里的公钥。包是从哪个节点的路上来的（`claimed`）必须和它对得上，
        // 不然这条连接的回包会被送到另一个节点那里去。
        match self.identify(&connection) {
            Some((peer, key)) if peer == claimed => drop(self.register(peer, connection, peer, key)),
            identified => {
                tracing::warn!(peer = %claimed, identified = ?identified.map(|(peer, _)| peer), "连进来的对端的公钥和它自称的节点对不上，关闭连接");
                connection.close(VarInt::from_u32(CLOSE_IDENTITY), b"identity mismatch");
            }
        }
    }

    /// 握手时对端出示的公钥是 netmap 里哪个节点的。
    fn identify(&self, connection: &quinn::Connection) -> Option<(NodeId, PublicKey)> {
        let certificates = connection.peer_identity()?.downcast::<Vec<CertificateDer<'static>>>().ok()?;
        let key = tls::peer_key(certificates.first()?).ok()?;
        self.netmap.borrow().peers.iter().find(|peer| peer.key == key).map(|peer| (peer.id, key))
    }

    /// 跟随 netmap：对端被删除（吊销）或者换了公钥，和它的连接立刻关闭。
    pub(crate) async fn follow_netmap(self: Arc<Self>) {
        let mut netmap = self.netmap.clone();
        while netmap.changed().await.is_ok() {
            let current = Arc::clone(&netmap.borrow_and_update());
            self.peers().retain(|&peer, entry| {
                let expected = current.peer(peer).map(|info| info.key);
                entry.conns.retain(|conn| {
                    let valid = expected == Some(conn.key);
                    if !valid {
                        tracing::info!(%peer, "对端不在 netmap 里了，或者换了公钥：关闭和它的连接");
                        conn.quic.close(VarInt::from_u32(CLOSE_REVOKED), b"revoked");
                    }
                    valid
                });
                expected.is_some()
            });
        }
    }

    /// 在已有的连接里把一条信令送给对端。没有连接就不送：这时信令只能靠装配方的钩子（controller）。
    pub(crate) fn send_signal(&self, to: NodeId, signal: &Signal) {
        let Some(conn) = self.preferred(to) else { return };
        let body = match serde_json::to_vec(signal) {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(peer = %to, %error, "信令编码失败");
                return;
            }
        };
        tokio::spawn(async move {
            let sent = async {
                let mut stream = conn.quic.open_uni().await.map_err(|error| error.to_string())?;
                stream.write_all(&body).await.map_err(|error| error.to_string())?;
                stream.finish().map_err(|error| error.to_string())
            };
            if let Err(error) = sent.await {
                tracing::debug!(peer = %to, error, "信令没有在连接里送出去");
            }
        });
    }

    /// 关闭：不再接受新的流，所有连接带着关闭帧结束。
    pub(crate) async fn close(&self) {
        self.closed.send_replace(true);
        self.quic.close(VarInt::from_u32(CLOSE_SHUTDOWN), b"shutdown");
        // 给关闭帧一点时间发出去；对面收不到也没关系，它那边会空闲超时。
        let _flushed = tokio::time::timeout(Duration::from_secs(1), self.quic.wait_idle()).await;
    }

    /// 不等待的关闭，丢弃时用。
    pub(crate) fn abort(&self) {
        self.closed.send_replace(true);
        self.quic.close(VarInt::from_u32(CLOSE_SHUTDOWN), b"shutdown");
    }
}

/// 没有流的连接最多留多久、到时候关闭的原因；不回收时返回 `None`。
///
/// `preferred`：到这个对端开新的流用的就是这一条。不是的话它是两边同时拨号多出来的那一条，很快就关。
/// `keep_alive`：netmap 把这个对端设成了常驻。它可能连不进去，关掉之后只能等它再连过来，所以不回收。
fn idle_limit(preferred: bool, keep_alive: bool) -> Option<(Duration, &'static str)> {
    match (preferred, keep_alive) {
        (false, _) => Some((DUPLICATE_IDLE_CLOSE, "到同一个对端有两条连接，多出来的这一条空闲了")),
        (true, true) => None,
        (true, false) => Some((IDLE_CLOSE, "没有流超过 5 分钟")),
    }
}

/// 读对端在连接里送来的一条信令（一条单向流，内容是一个 JSON 对象），交给路径层。`first` 是已经读走的首字节。
async fn read_signal(paths: &Paths, peer: NodeId, first: [u8; 1], mut stream: quinn::RecvStream) {
    let rest = match tokio::time::timeout(SIGNAL_TIMEOUT, stream.read_to_end(SIGNAL_MAX_BYTES - first.len())).await {
        Ok(Ok(rest)) => rest,
        Ok(Err(error)) => {
            tracing::debug!(%peer, %error, "读连接里的信令失败");
            return;
        }
        Err(_) => {
            tracing::debug!(%peer, "连接里的信令迟迟没有发完");
            return;
        }
    };
    let body = [&first[..], &rest].concat();
    match serde_json::from_slice::<Signal>(&body) {
        Ok(signal) => paths.handle_signal(peer, signal),
        Err(error) => tracing::debug!(%peer, %error, "连接里的信令解析不了"),
    }
}

/// 往对端发数据报的句柄，[`crate::Endpoint`] 给出来的。
///
/// 数据报走开新的流会用的那条连接。两边同时拨号留下两条连接时，两端可能各用各的那一条发，
/// 这没有关系：收的一方不分数据报是从哪一条连接上来的，只认对端是谁。
#[derive(Clone)]
pub struct DatagramHandle {
    manager: Weak<Manager>,
}

impl std::fmt::Debug for DatagramHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatagramHandle").finish_non_exhaustive()
    }
}

impl DatagramHandle {
    fn connection(&self, peer: NodeId) -> Result<Arc<Conn>, DatagramSendError> {
        self.manager.upgrade().and_then(|manager| manager.preferred(peer)).ok_or(DatagramSendError::NotConnected(peer))
    }
}

impl DatagramSender for DatagramHandle {
    fn max_datagram_len(&self, peer: NodeId) -> Option<usize> {
        self.connection(peer).ok()?.quic.max_datagram_size()
    }

    fn send_datagram(&self, peer: NodeId, datagram: Bytes) -> Result<(), DatagramSendError> {
        let conn = self.connection(peer)?;
        let len = datagram.len();
        conn.quic.send_datagram(datagram).map_err(|error| match error {
            SendDatagramError::UnsupportedByPeer | SendDatagramError::Disabled => DatagramSendError::Unsupported(peer),
            SendDatagramError::TooLarge => DatagramSendError::TooLarge { len, max: conn.quic.max_datagram_size().unwrap_or(0) },
            SendDatagramError::ConnectionLost(reason) => DatagramSendError::ConnectionLost { peer, reason: reason.to_string() },
        })
    }

    async fn send_reliable(&self, peer: NodeId, datagram: Bytes) -> Result<(), DatagramSendError> {
        let conn = self.connection(peer)?;
        let lost = |reason: String| DatagramSendError::ConnectionLost { peer, reason };
        // 对端同时接受的单向流有上限，到了上限就在这里等它读完前面的。
        let mut stream = conn.quic.open_uni().await.map_err(|error| lost(error.to_string()))?;
        stream.write_all(&datagram).await.map_err(|error| lost(error.to_string()))?;
        stream.finish().map_err(|error| lost(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_connections_are_closed_after_five_minutes_unless_the_peer_is_kept_alive() {
        assert_eq!(idle_limit(true, false).map(|(limit, _)| limit), Some(Duration::from_secs(300)));
        assert_eq!(idle_limit(true, true), None, "常驻的对端不回收");
        // 多出来的那一条连接很快就关，常驻也一样：留着的是另一条。
        assert_eq!(idle_limit(false, false).map(|(limit, _)| limit), Some(DUPLICATE_IDLE_CLOSE));
        assert_eq!(idle_limit(false, true).map(|(limit, _)| limit), Some(DUPLICATE_IDLE_CLOSE));
    }
}
