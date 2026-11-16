//! 路径层对外的入口：连接层通过它收发包，装配的一方通过它查状态、接信令。

use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use i3t_crypto::NodeKey;
use i3t_model::Netmap;
use i3t_model::control::{RouteEntry, Signal};
use i3t_model::netmap::Attachment;
use i3t_proto::datagram::RelayHeader;
use i3t_proto::{Datagram, NodeId};
use i3t_underlay::{BATCH_SIZE, Ecn, RecvMeta, UdpPort};
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::control::{Control, Ctrl, Inbound};
use crate::table::{Clock, Packets, PeerSlot, Shared, Table};
use crate::{LocalRelay, RouteChange, Status};

/// 收包路径交给控制任务的帧最多攒多少。满了就丢：它们都是有重试的握手和探测，而且这样陌生人灌不爆内存。
const CTRL_QUEUE: usize = 1024;
/// 不走 UDP 缓冲区直达的包（TCP 中转来的等）最多攒多少。满了之后 TCP 那边停下来等。
const INBOUND_QUEUE: usize = 1024;
/// 换路事件的订阅者最多落后多少条。
const CHANGE_BACKLOG: usize = 256;

/// 建路径层要的东西。
#[derive(Debug)]
pub struct PathConfig {
    pub key: Arc<NodeKey>,
    pub port: Arc<UdpPort>,
    pub netmap: watch::Receiver<Arc<Netmap>>,
    /// 地址查询的服务器（通常是 controller 的那个 UDP 端口）。
    pub addr_servers: Vec<SocketAddr>,
}

/// 收到的一批包是谁发来的、有多长。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketMeta {
    /// 发这些包的对端。直连时由来源地址认出来，经 relay 时取自中转头（relay 保证它是真的）。
    pub peer: NodeId,
    /// 缓冲区里有多少字节。
    pub len: usize,
    /// 每个包的长度；`len` 比它大时缓冲区里是同一个对端的多个包，每 `stride` 字节一个，最后一个可以短。
    pub stride: usize,
    pub ecn: Option<Ecn>,
}

/// 一个节点的路径层。
///
/// 它管着那个唯一的 UDP 端口上的全部收发：连接层的包走哪条路由它定，路径层自己的帧（中转会话、探测）它自己处理，
/// 归 relay 的交给 [`LocalRelay`]。连接层只说"发给哪个节点"，只看到"哪个节点发来的"，物理地址不出这一层，
/// 所以换路时连接不知道。
///
/// 丢弃这个值会停掉后台任务，关掉所有的中转会话。
#[derive(Debug)]
pub struct Paths {
    shared: Arc<Shared>,
    control: JoinHandle<()>,
}

impl Paths {
    /// 要在 tokio 运行时里调用：它会启动控制任务。
    #[must_use]
    pub fn new(config: PathConfig) -> Self {
        let PathConfig { key, port, mut netmap, addr_servers } = config;
        let (ctrl, ctrl_rx) = mpsc::channel(CTRL_QUEUE);
        let (inbound, inbound_rx) = mpsc::channel(INBOUND_QUEUE);
        let current = Arc::clone(&netmap.borrow_and_update());
        let shared = Arc::new(Shared {
            me: current.me.id,
            port,
            clock: Clock::new(),
            table: RwLock::new(Table::default()),
            ctrl,
            inbound,
            inbound_rx: Mutex::new(inbound_rx),
            local_relay: OnceLock::new(),
            signal_hook: RwLock::new(None),
            changes: broadcast::Sender::new(CHANGE_BACKLOG),
            route_changed: Notify::new(),
            status: Mutex::new(Status::default()),
        });
        let control = Control::new(Arc::clone(&shared), key, current, addr_servers);
        let control = tokio::spawn(control.run(ctrl_rx, netmap));
        Self { shared, control }
    }

    /// 本节点的 relay 角色。归 relay 处理的数据报从此交给它；对端挂在本节点自己的 relay 上时，包也经它送。
    /// 只能设一次。
    pub fn set_local_relay(&self, relay: Arc<dyn LocalRelay>) {
        if self.shared.local_relay.set(relay).is_err() {
            tracing::warn!("路径层的本地 relay 已经设过了，忽略这一次");
        }
    }

    /// 信令从哪里送出去（`to` 是收信令的节点）。钩子在控制任务里被调用，不能阻塞。
    ///
    /// # Panics
    ///
    /// 之前设的钩子自己 panic 过（锁中毒）。
    pub fn set_signal_hook(&self, hook: impl Fn(NodeId, &Signal) + Send + Sync + 'static) {
        *self.shared.signal_hook.write().expect("signal hook lock poisoned") = Some(Arc::new(hook));
    }

    /// 对端 `from` 发来了一条信令。来源必须是可信的：controller 转来的，或者和 `from` 的已认证连接里收到的。
    pub fn handle_signal(&self, from: NodeId, signal: Signal) {
        if self.shared.ctrl.try_send(Ctrl::Signal { from, signal }).is_err() {
            tracing::debug!(peer = %from, "控制任务忙不过来，丢掉一条信令");
        }
    }

    /// 接下来要和 `peer` 通信了：立刻开始为它找路（建临时的中转会话、探测直连），不等第一个包。
    pub fn activate(&self, peer: NodeId) {
        if let Some(slot) = self.shared.table().peers.get(&peer) {
            slot.last_demand.touch(&self.shared.clock);
        }
        // 满了说明控制任务本来就有事要做，做完会重新选路。
        let _queued = self.shared.ctrl.try_send(Ctrl::Wake);
    }

    /// 到 `peer` 现在有没有路。
    #[must_use]
    pub fn has_route(&self, peer: NodeId) -> bool {
        self.shared.table().peers.get(&peer).is_some_and(|slot| slot.has_hop())
    }

    /// 等到 `peer` 有路为止，最多等 `limit`。返回等到了没有。等的时候不断把 `peer` 标记为活跃，找路不会停。
    pub async fn wait_for_route(&self, peer: NodeId, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            // 先登记再检查，这样两步之间发生的换路不会漏掉。
            let changed = self.shared.route_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            self.activate(peer);
            if self.has_route(peer) {
                return true;
            }
            // 隔一小会儿就重新标记一次：netmap 刚换、对端还没进表的时候，头一次标记会落空。
            let recheck = tokio::time::Instant::now() + Duration::from_millis(100);
            if tokio::time::timeout_at(deadline.min(recheck), changed).await.is_err() && tokio::time::Instant::now() >= deadline {
                return self.has_route(peer);
            }
        }
    }

    /// 把一批包发给 `peer`。没有路时包被悄悄丢掉，连接自己会在空闲超时之内等路恢复。
    ///
    /// # Errors
    ///
    /// 只会返回 [`io::ErrorKind::WouldBlock`]：现在发不了，等 [`writable`](Self::writable) 之后再试。
    pub fn try_send(&self, peer: NodeId, packets: &Packets<'_>) -> io::Result<()> {
        let table = self.shared.table();
        match table.peers.get(&peer) {
            Some(slot) => slot.send(&self.shared, packets),
            None => Ok(()),
        }
    }

    /// 等到可以再发。可以有多个任务同时等。
    ///
    /// # Errors
    ///
    /// socket 本身出错。
    pub async fn writable(&self) -> io::Result<()> {
        self.shared.port.writable().await
    }

    /// 收一批发给本节点的连接包。返回填了前几个缓冲区，对应的 `metas` 说明每个缓冲区里是谁的包。
    ///
    /// 那个 UDP 端口上的所有数据报都从这里过：连接包留给调用方，其余的在这里分给路径层自己和本地的 relay。
    /// 所以必须有人一直在调用它，路径层和 relay 才会工作。每个缓冲区要装得下
    /// "[`max_datagram_len`](Self::max_datagram_len) × [`max_receive_segments`](Self::max_receive_segments)" 字节。
    ///
    /// # Errors
    ///
    /// socket 本身出错。
    pub fn poll_recv(&self, cx: &mut Context<'_>, bufs: &mut [IoSliceMut<'_>], metas: &mut [PacketMeta]) -> Poll<io::Result<usize>> {
        let limit = bufs.len().min(metas.len());
        let queued = self.drain_inbound(cx, &mut bufs[..limit], metas);
        if queued > 0 {
            return Poll::Ready(Ok(queued));
        }
        let mut raw = [RecvMeta::default(); BATCH_SIZE];
        let limit = limit.min(BATCH_SIZE);
        loop {
            let count = ready!(self.shared.port.poll_recv(cx, &mut bufs[..limit], &mut raw[..limit]))?;
            let mut kept = 0;
            {
                let table = self.shared.table();
                for index in 0..count {
                    if let Some(meta) = self.demux(&table, &raw[index], &mut bufs[index]) {
                        bufs.swap(kept, index);
                        metas[kept] = meta;
                        kept += 1;
                    }
                }
            }
            if kept > 0 {
                return Poll::Ready(Ok(kept));
            }
            // 这一批里没有能直接留在缓冲区里的，但可能有放进了队列的。
            let queued = self.drain_inbound(cx, &mut bufs[..limit], metas);
            if queued > 0 {
                return Poll::Ready(Ok(queued));
            }
        }
    }

    /// 这个端口上一个数据报最长多少字节：连接包的上限加上中转头。
    #[must_use]
    pub const fn max_datagram_len() -> usize {
        i3t_proto::MAX_CONN_PACKET_LEN as usize + RelayHeader::LEN
    }

    /// 一次发送最多能带几个包。
    #[must_use]
    pub fn max_transmit_segments(&self) -> usize {
        self.shared.port.max_gso_segments()
    }

    /// 一个接收缓冲区里最多会有几个包。
    #[must_use]
    pub fn max_receive_segments(&self) -> usize {
        self.shared.port.gro_segments()
    }

    /// 发出去的包在路上会不会被 IP 层分片。
    #[must_use]
    pub fn may_fragment(&self) -> bool {
        self.shared.port.may_fragment()
    }

    /// 本地的 relay 收到了发给本节点的中转帧（从它的 TCP 中转腿上）：`packet` 是去掉中转头之后的连接包。
    pub fn deliver_local(&self, src: NodeId, packet: Bytes) {
        if self.shared.inbound.try_send((src, packet)).is_err() {
            tracing::trace!(peer = %src, "连接层来不及收，丢掉一个包");
        }
    }

    /// 到每个对端现在走哪条路，延迟多少。
    ///
    /// # Panics
    ///
    /// 控制任务 panic 过（锁中毒）。
    #[must_use]
    pub fn routes(&self) -> Vec<RouteEntry> {
        self.status().routes.clone()
    }

    /// 本节点的候选端点：网卡地址，加上 relay 和地址查询服务器看到的地址。不建立直连的节点返回空表。
    #[must_use]
    pub fn endpoints(&self) -> Vec<SocketAddr> {
        self.status().endpoints.clone()
    }

    /// 映射出来的端口是不是随对端变化。要有两个以上的观察点才知道；是的话这台机器所在的网络基本打不了洞。
    #[must_use]
    pub fn mapping_varies(&self) -> Option<bool> {
        self.status().mapping_varies
    }

    /// 本节点挂靠的 relay（会话已经建好的）和到它们的延迟，按延迟从低到高。
    #[must_use]
    pub fn attached(&self) -> Vec<Attachment> {
        self.status().attached.clone()
    }

    /// 订阅换路事件。每一次换路都有一条，带着给人看的原因。
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RouteChange> {
        self.shared.changes.subscribe()
    }

    fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.shared.status.lock().expect("path status mutex poisoned")
    }

    /// 把队列里的包拷进连接层的缓冲区，一个缓冲区一个。
    fn drain_inbound(&self, cx: &mut Context<'_>, bufs: &mut [IoSliceMut<'_>], metas: &mut [PacketMeta]) -> usize {
        let mut queue = self.shared.inbound_rx.lock().expect("inbound queue mutex poisoned");
        let mut filled = 0;
        while filled < bufs.len() {
            let Poll::Ready(Some((peer, packet))) = queue.poll_recv(cx) else { break };
            let buf = &mut bufs[filled];
            match self.shared.table().peers.get(&peer) {
                Some(slot) if packet.len() <= buf.len() => slot.last_rx.touch(&self.shared.clock),
                Some(_) | None => continue,
            }
            buf[..packet.len()].copy_from_slice(&packet);
            metas[filled] = PacketMeta { peer, len: packet.len(), stride: packet.len(), ecn: None };
            filled += 1;
        }
        filled
    }

    /// 处理一个接收缓冲区。里面可能是来自同一个地址的多个数据报，种类和去向各不相同，逐个看：
    /// 连接包就地挪到缓冲区开头留给连接层，别的当场分发。返回留下来的那一段。
    fn demux(&self, table: &Table, raw: &RecvMeta, buf: &mut [u8]) -> Option<PacketMeta> {
        let data = &mut buf[..raw.len];
        let stride = raw.stride.max(1);
        let mut kept: Option<Kept<'_>> = None;
        // 连续的、归本地 relay 处理的数据报攒成一段再交给它，这样它还能成批地转发。
        let mut for_relay: Option<Range<usize>> = None;
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + stride).min(data.len());
            let verdict = self.classify(table, raw.addr, &data[offset..end]);
            if !matches!(verdict, Verdict::Relay)
                && let Some(span) = for_relay.take()
            {
                self.to_local_relay(raw, &data[span], stride);
            }
            match verdict {
                Verdict::Packet { slot, skip } => {
                    let packet = offset + skip..end;
                    match &mut kept {
                        None => {
                            data.copy_within(packet.clone(), 0);
                            kept = Some(Kept { slot, stride: packet.len(), len: packet.len(), open: true });
                        }
                        // 同一个对端、长度也接得上的，接在后面；不然就拷出去走队列。
                        Some(run) if run.open && Arc::ptr_eq(run.slot, slot) && packet.len() <= run.stride => {
                            data.copy_within(packet.clone(), run.len);
                            run.len += packet.len();
                            run.open = packet.len() == run.stride;
                        }
                        Some(_) => {
                            if self.shared.inbound.try_send((slot.id, Bytes::copy_from_slice(&data[packet]))).is_err() {
                                tracing::trace!(peer = %slot.id, "连接层来不及收，丢掉一个包");
                            }
                        }
                    }
                }
                Verdict::Relay => for_relay = Some(for_relay.map_or(offset..end, |span| span.start..end)),
                Verdict::Control(frame) => {
                    if self.shared.ctrl.try_send(Ctrl::Frame { from: raw.addr, frame }).is_err() {
                        tracing::trace!(from = %raw.addr, "控制任务忙不过来，丢掉一帧");
                    }
                }
                Verdict::Drop => {}
            }
            offset = end;
        }
        if let Some(span) = for_relay {
            self.to_local_relay(raw, &data[span], stride);
        }
        kept.map(|run| PacketMeta { peer: run.slot.id, len: run.len, stride: run.stride, ecn: raw.ecn })
    }

    fn to_local_relay(&self, raw: &RecvMeta, data: &[u8], stride: usize) {
        if let Some(relay) = self.shared.local_relay.get() {
            relay.handle_udp(raw.addr, raw.ecn, data, stride);
        }
    }

    /// 一个数据报归谁（docs/architecture.md §5.1 的那张表）。
    fn classify<'t>(&self, table: &'t Table, from: SocketAddr, datagram: &[u8]) -> Verdict<'t> {
        let clock = &self.shared.clock;
        match Datagram::parse(datagram) {
            // 直连的连接包：来源地址不是任何对端已验证的直连地址时丢掉。
            Ok(Datagram::Conn(_)) => match table.direct.get(&from) {
                Some(slot) => {
                    slot.last_rx_direct.touch(clock);
                    slot.last_rx.touch(clock);
                    Verdict::Packet { slot, skip: 0 }
                }
                None => Verdict::Drop,
            },
            Ok(Datagram::Relayed { header, .. }) if header.dst == self.shared.me => {
                // 发给本节点的中转帧只认两种来源：netmap 里的 relay；或者挂在本节点自己的 relay 上的节点，
                // 并且帧里的来源节点就是那个会话的主人。
                let vouched = match table.relays.get(&from) {
                    Some(relay) => {
                        relay.last_rx.touch(clock);
                        true
                    }
                    None => self.shared.local_relay.get().is_some_and(|relay| relay.session_at(header.src, from)),
                };
                match table.peers.get(&header.src) {
                    Some(slot) if vouched => {
                        slot.last_rx.touch(clock);
                        Verdict::Packet { slot, skip: RelayHeader::LEN }
                    }
                    Some(_) | None => Verdict::Drop,
                }
            }
            // 发给别人的中转帧、节点发来的会话帧、地址查询：本节点是 relay 时由它处理，不是就没有人理。
            Ok(Datagram::Relayed { .. } | Datagram::AddrQuery(_)) => Verdict::Relay,
            Ok(Datagram::Session(frame)) if frame.is_request() => Verdict::Relay,
            Ok(Datagram::Session(frame)) => {
                if let Some(relay) = table.relays.get(&from) {
                    relay.last_rx.touch(clock);
                }
                Verdict::Control(Inbound::Session(frame))
            }
            Ok(Datagram::Probe(frame)) => Verdict::Control(Inbound::Probe(frame)),
            Ok(Datagram::AddrReply(reply)) => Verdict::Control(Inbound::AddrReply(reply)),
            Err(_) => Verdict::Drop,
        }
    }
}

impl Drop for Paths {
    fn drop(&mut self) {
        self.control.abort();
    }
}

/// 一个数据报的去向。
enum Verdict<'t> {
    /// 连接包，是 `slot` 这个对端发来的；前 `skip` 个字节是中转头，要去掉。
    Packet {
        slot: &'t Arc<PeerSlot>,
        skip: usize,
    },
    /// 归本地的 relay。
    Relay,
    /// 归控制任务。
    Control(Inbound),
    Drop,
}

/// 一个接收缓冲区里留给连接层的那一段：都来自同一个对端，除了最后一个都一样长。
struct Kept<'t> {
    slot: &'t Arc<PeerSlot>,
    stride: usize,
    len: usize,
    /// 还能不能往后接：接上一个短的之后就不能了。
    open: bool,
}

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use i3t_proto::session::{Init, Pong};

    use super::*;

    const ME: u32 = 1;
    const RELAY_ADDR: &str = "198.51.100.9:20330";

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// 一个路径层：对端是 2 和 3，netmap 里有一个 relay（编号 9）。没有人去读它的 UDP 端口，测试直接把缓冲区交给 `demux`。
    fn paths() -> (Paths, watch::Sender<Arc<Netmap>>) {
        let key = |seed: u8| i3t_proto::PublicKey::from_bytes([seed; 32]);
        let peer = |id: u32| serde_json::json!({ "id": id, "key": key(2), "name": format!("n{id}"), "virtual_ip": format!("10.66.0.{id}"), "inbound": true, "outbound": true });
        let netmap = serde_json::json!({
            "version": 1,
            "network": { "name": "test", "domain": "test.i3t", "cidr": "10.66.0.0/16" },
            "me": { "id": ME, "name": "me", "virtual_ip": "10.66.0.1" },
            "peers": [peer(2), peer(3)],
            "relays": [{ "id": 9, "key": key(9), "name": "relay", "udp": [RELAY_ADDR] }],
        });
        let (sender, netmap) = watch::channel(Arc::new(serde_json::from_value(netmap).unwrap()));
        let port = Arc::new(UdpPort::bind(addr("127.0.0.1:0")).unwrap());
        let key = Arc::new(NodeKey::generate().unwrap());
        (Paths::new(PathConfig { key, port, netmap, addr_servers: Vec::new() }), sender)
    }

    fn relayed(dst: u32, src: u32, packet: &[u8]) -> Vec<u8> {
        let mut datagram = RelayHeader { dst: node(dst), src: node(src) }.to_bytes().to_vec();
        datagram.extend_from_slice(packet);
        datagram
    }

    /// 把一批数据报当作内核合并接收交上来的一个缓冲区，过一遍 `demux`。返回留给连接层的那一段。
    fn demux(paths: &Paths, from: &str, datagrams: &[Vec<u8>]) -> Option<(PacketMeta, Vec<u8>)> {
        let stride = datagrams[0].len();
        assert!(datagrams[..datagrams.len() - 1].iter().all(|datagram| datagram.len() == stride), "除了最后一个都要一样长");
        let mut buf = datagrams.concat();
        let raw = RecvMeta { addr: addr(from), len: buf.len(), stride, ecn: Some(Ecn::Ect0) };
        let table = paths.shared.table();
        let meta = paths.demux(&table, &raw, &mut buf)?;
        Some((meta, buf[..meta.len].to_vec()))
    }

    /// 走队列的那些包。
    fn queued(paths: &Paths) -> Vec<(NodeId, Vec<u8>)> {
        let mut storage = vec![0u8; 8 * 2048];
        let mut bufs: Vec<IoSliceMut<'_>> = storage.chunks_mut(2048).map(IoSliceMut::new).collect();
        let mut metas = [PacketMeta { peer: node(ME), len: 0, stride: 0, ecn: None }; 8];
        let filled = paths.drain_inbound(&mut Context::from_waker(Waker::noop()), &mut bufs, &mut metas);
        metas[..filled].iter().zip(&bufs).map(|(meta, buf)| (meta.peer, buf[..meta.len].to_vec())).collect()
    }

    #[derive(Default)]
    struct RecordingRelay {
        handled: Mutex<Vec<(SocketAddr, usize, Vec<u8>)>>,
        clients: Vec<(NodeId, SocketAddr)>,
    }

    impl LocalRelay for RecordingRelay {
        fn handle_udp(&self, from: SocketAddr, _ecn: Option<Ecn>, data: &[u8], stride: usize) {
            self.handled.lock().unwrap().push((from, stride, data.to_vec()));
        }

        fn has_session(&self, client: NodeId) -> bool {
            self.clients.iter().any(|(id, _)| *id == client)
        }

        fn session_at(&self, client: NodeId, from: SocketAddr) -> bool {
            self.clients.contains(&(client, from))
        }

        fn send_to_client(&self, _client: NodeId, _contents: &[u8], _segment_size: Option<usize>, _ecn: Option<Ecn>) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn relayed_packets_from_one_peer_stay_in_the_buffer_with_their_headers_removed() {
        let (paths, _netmap) = paths();
        let batch = [relayed(ME, 2, &[0xc1; 1200]), relayed(ME, 2, &[0xc2; 1200]), relayed(ME, 2, &[0xc3; 700])];
        let (meta, kept) = demux(&paths, RELAY_ADDR, &batch).expect("这些包都是给连接层的");
        assert_eq!(meta, PacketMeta { peer: node(2), len: 3100, stride: 1200, ecn: Some(Ecn::Ect0) });
        assert_eq!(kept, [vec![0xc1; 1200], vec![0xc2; 1200], vec![0xc3; 700]].concat());
        assert!(queued(&paths).is_empty());
    }

    #[tokio::test]
    async fn a_batch_mixing_peers_keeps_one_peer_in_place_and_queues_the_rest_in_order() {
        let (paths, _netmap) = paths();
        // 同一个 relay 转来的、两个对端的包交错在一批里；最后一个短一些。
        let batch = [
            relayed(ME, 2, &[0xc1; 1000]),
            relayed(ME, 3, &[0xd1; 1000]),
            relayed(ME, 2, &[0xc2; 1000]),
            relayed(ME, 3, &[0xd2; 1000]),
            relayed(ME, 2, &[0xc3; 400]),
        ];
        let (meta, kept) = demux(&paths, RELAY_ADDR, &batch).expect("对端 2 的包留在缓冲区里");
        assert_eq!(meta, PacketMeta { peer: node(2), len: 2400, stride: 1000, ecn: Some(Ecn::Ect0) });
        assert_eq!(kept, [vec![0xc1; 1000], vec![0xc2; 1000], vec![0xc3; 400]].concat());
        assert_eq!(queued(&paths), [(node(3), vec![0xd1; 1000]), (node(3), vec![0xd2; 1000])], "另一个对端的包走队列，顺序不变");

        // 留在缓冲区里的那一段只有最后一个可以短。短的接上之后，同一个对端后面的包也只能走队列。
        let batch = [relayed(ME, 3, &[0xd1; 600]), relayed(ME, 2, &[0xc1; 600]), relayed(ME, 3, &[0xd2; 100])];
        let (meta, _kept) = demux(&paths, RELAY_ADDR, &batch).unwrap();
        assert_eq!(meta, PacketMeta { peer: node(3), len: 700, stride: 600, ecn: Some(Ecn::Ect0) });
        assert_eq!(queued(&paths), [(node(2), vec![0xc1; 600])]);
    }

    #[tokio::test]
    async fn only_packets_vouched_for_by_a_relay_or_a_verified_address_reach_the_connection_layer() {
        let (paths, _netmap) = paths();
        let quic = vec![0xc0u8; 1200];

        // 中转帧：来源不是 relay 的地址；或者帧里的来源节点不是对端。
        assert!(demux(&paths, "203.0.113.50:1", &[relayed(ME, 2, &quic)]).is_none());
        assert!(demux(&paths, RELAY_ADDR, &[relayed(ME, 77, &quic)]).is_none());
        assert!(demux(&paths, RELAY_ADDR, &[relayed(ME, 2, &quic)]).is_some());

        // 直连的连接包：来源地址没有验证过时丢掉；验证过之后算作那个对端的。
        let direct = "203.0.113.7:4000";
        assert!(demux(&paths, direct, std::slice::from_ref(&quic)).is_none());
        {
            let mut table = paths.shared.table_mut();
            let slot = Arc::clone(&table.peers[&node(3)]);
            table.direct.insert(addr(direct), slot);
        }
        let (meta, kept) = demux(&paths, direct, &[quic.clone(), quic.clone()]).unwrap();
        assert_eq!(meta, PacketMeta { peer: node(3), len: 2400, stride: 1200, ecn: Some(Ecn::Ect0) });
        assert_eq!(kept.len(), 2400);

        // 不认识的、残缺的：丢掉。
        assert!(demux(&paths, direct, &[vec![0x3f; 100]]).is_none());
        assert!(demux(&paths, RELAY_ADDR, &[vec![0x01, 0, 0]]).is_none());
        assert!(queued(&paths).is_empty());
    }

    #[tokio::test]
    async fn frames_for_the_local_relay_are_handed_over_in_contiguous_runs() {
        let (paths, _netmap) = paths();
        let client = addr("203.0.113.60:5000");
        let relay = Arc::new(RecordingRelay { handled: Mutex::default(), clients: vec![(node(2), client)] });
        paths.set_local_relay(Arc::clone(&relay) as Arc<dyn LocalRelay>);

        // 同一个来源的一批：两个要转发给别人的、一个发给本节点的、又两个要转发的。
        let stride = RelayHeader::LEN + 500;
        let batch = [
            relayed(3, 2, &[1; 500]),
            relayed(8, 2, &[2; 500]),
            relayed(ME, 2, &[0xc3; 500]),
            relayed(3, 2, &[4; 500]),
            relayed(3, 2, &[5; 100]),
        ];
        let (meta, kept) = demux(&paths, "203.0.113.60:5000", &batch).expect("中间那个是给本节点的");
        // 发给本节点的那个：来源节点在本地 relay 上的会话就绑定在这个地址上，所以收下。
        assert_eq!(meta, PacketMeta { peer: node(2), len: 500, stride: 500, ecn: Some(Ecn::Ect0) });
        assert_eq!(kept, vec![0xc3; 500]);
        let handled = relay.handled.lock().unwrap().clone();
        assert_eq!(handled.len(), 2, "被发给本节点的那个隔开，成了两段");
        assert_eq!(handled[0], (client, stride, [batch[0].clone(), batch[1].clone()].concat()));
        assert_eq!(handled[1], (client, stride, [batch[3].clone(), batch[4].clone()].concat()), "原样交过去，一个字节都不改");

        // 节点发给 relay 的会话帧也交给它；relay 的应答不交。
        relay.handled.lock().unwrap().clear();
        let init = Init { node: node(2), nonce: [1; 8] }.to_bytes().to_vec();
        assert!(demux(&paths, "203.0.113.60:5000", std::slice::from_ref(&init)).is_none());
        assert!(demux(&paths, RELAY_ADDR, &[Pong { nonce: [1; 8], observed: client }.to_bytes()]).is_none());
        assert_eq!(*relay.handled.lock().unwrap(), [(client, init.len(), init)]);

        // 别的地址冒充那个节点发给本节点的帧：本地 relay 不认，丢掉。
        assert!(demux(&paths, "203.0.113.61:5000", &[relayed(ME, 2, &[0xc3; 500])]).is_none());
    }

    #[tokio::test]
    async fn packets_for_unknown_peers_are_dropped_and_sending_to_them_is_a_silent_no_op() {
        let (paths, netmap) = paths();
        paths.deliver_local(node(2), Bytes::from_static(b"from a tcp leg"));
        paths.deliver_local(node(77), Bytes::from_static(b"not a peer"));
        assert_eq!(queued(&paths), [(node(2), b"from a tcp leg".to_vec())]);

        let packets = Packets { contents: &[0xc0; 1200], segment_size: None, ecn: None };
        assert!(paths.try_send(node(77), &packets).is_ok());
        // 没有路的对端：包被悄悄丢掉。
        assert!(!paths.has_route(node(2)));
        assert!(paths.try_send(node(2), &packets).is_ok());
        assert!(!paths.wait_for_route(node(2), Duration::from_millis(100)).await);
        assert_eq!(paths.routes().len(), 2);
        drop(netmap);
    }
}
