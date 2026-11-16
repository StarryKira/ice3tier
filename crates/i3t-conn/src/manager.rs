//! 按对端管理连接：一对节点一条连接，按需建立，空闲回收，对端从 netmap 里消失就立刻关闭。

use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_crypto::NodeKey;
use i3t_crypto::tls;
use i3t_model::Netmap;
use i3t_model::control::Signal;
use i3t_path::Paths;
use i3t_proto::{NodeId, PublicKey};
use quinn::rustls::pki_types::CertificateDer;
use quinn::{ConnectError, TransportConfig, VarInt};
use tokio::sync::{Notify, mpsc, watch};

use crate::contract::{Incoming, OpenError};
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

/// 关闭连接时带给对端的码。
const CLOSE_IDLE: u32 = 0;
const CLOSE_SHUTDOWN: u32 = 1;
const CLOSE_REVOKED: u32 = 2;
const CLOSE_IDENTITY: u32 = 3;

type Stream = Incoming<SendHalf, RecvHalf>;

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
    closed: watch::Sender<bool>,
}

impl Manager {
    pub(crate) fn new(
        key: Arc<NodeKey>,
        quic: quinn::Endpoint,
        paths: Arc<Paths>,
        netmap: watch::Receiver<Arc<Netmap>>,
        incoming: mpsc::Sender<Stream>,
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

    /// 一条连接的一生：接收对端开过来的流和信令，空闲够久就关掉。
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
                    Ok(recv) => drop(tokio::spawn(read_signal(Arc::clone(&self.paths), peer, recv))),
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

/// 读对端在连接里送来的一条信令（一条单向流，内容是一个 JSON 对象），交给路径层。
async fn read_signal(paths: Arc<Paths>, peer: NodeId, mut stream: quinn::RecvStream) {
    let body = match tokio::time::timeout(SIGNAL_TIMEOUT, stream.read_to_end(SIGNAL_MAX_BYTES)).await {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => {
            tracing::debug!(%peer, %error, "读连接里的信令失败");
            return;
        }
        Err(_) => {
            tracing::debug!(%peer, "连接里的信令迟迟没有发完");
            return;
        }
    };
    // 空的流什么都不表示。
    if body.is_empty() {
        return;
    }
    match serde_json::from_slice::<Signal>(&body) {
        Ok(signal) => paths.handle_signal(peer, signal),
        Err(error) => tracing::debug!(%peer, %error, "连接里的信令解析不了"),
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
