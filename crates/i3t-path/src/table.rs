//! 收发每个包时要查的东西：到每个对端现在走哪条路，哪些来源地址是谁。
//!
//! 这些由控制任务（[`crate::control`]）写，由收发包的路径读。发包时只查"现在走哪条路"这一个结果，不做别的判断
//! （docs/architecture.md §5.4）。

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use bytes::{BufMut, Bytes, BytesMut};
use i3t_model::control::Signal;
use i3t_proto::NodeId;
use i3t_proto::datagram::{RelayHeader, RouteHeader};
use i3t_underlay::lan::LanDiscovery;
use i3t_underlay::tcp_leg::FrameSender;
use i3t_underlay::{Ecn, Transmit, UdpPort};
use tokio::sync::{Notify, broadcast, mpsc};

use crate::control::Ctrl;
use crate::timing::{PEER_ACTIVE, SILENCE, STASH_MAX_AGE, UNANSWERED_WINDOW};
use crate::{LocalRelay, RouteChange, Status};

/// 路可疑或者没有路的时候，留着最后几个包，路恢复时立刻补发。
const STASH_PACKETS: usize = 4;
/// 多久没有收到对端的包，就开始把发出去的包多留一份。比连接正常时两个确认之间的间隔长得多。
const STASH_WHEN_SILENT: Duration = Duration::from_millis(500);

/// 连接层交下来的一批包：一个，或者若干个等长的首尾相接（最后一个可以短）。
#[derive(Debug, Clone, Copy)]
pub struct Packets<'a> {
    pub contents: &'a [u8],
    /// 每个包的长度；只有一个包时是 `None`。
    pub segment_size: Option<usize>,
    pub ecn: Option<Ecn>,
}

impl<'a> Packets<'a> {
    fn segments(&self) -> impl Iterator<Item = &'a [u8]> + use<'a> {
        self.contents.chunks(self.segment_size.unwrap_or(self.contents.len()).max(1))
    }
}

/// 从进程启动算起的毫秒数，够用来比较"多久之前"，又能放进一个原子变量。
#[derive(Debug)]
pub(crate) struct Clock(Instant);

impl Clock {
    pub(crate) fn new() -> Self {
        Self(Instant::now())
    }

    /// 最小是 1：0 留给"从来没有过"。
    fn now(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX).saturating_add(1)
    }
}

/// 某件事最近一次发生的时刻。收发包的路径上只写一个原子变量。
#[derive(Debug, Default)]
pub(crate) struct Stamp(AtomicU64);

impl Stamp {
    pub(crate) fn touch(&self, clock: &Clock) {
        self.0.store(clock.now(), Ordering::Relaxed);
    }

    /// 距离上一次过了多久；从来没有过时返回 `None`。
    pub(crate) fn elapsed(&self, clock: &Clock) -> Option<Duration> {
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            at => Some(Duration::from_millis(clock.now().saturating_sub(at))),
        }
    }

    /// 是不是在最近 `window` 之内发生过。
    pub(crate) fn within(&self, clock: &Clock, window: Duration) -> bool {
        self.elapsed(clock).is_some_and(|elapsed| elapsed < window)
    }

    /// 这件事是不是比 `other` 晚发生（精确到毫秒；`other` 从来没有发生过也算）。
    pub(crate) fn is_after(&self, other: &Self) -> bool {
        self.0.load(Ordering::Relaxed) > other.0.load(Ordering::Relaxed)
    }
}

/// 发出去的包有没有落空：`tx` 之后再没有 `rx`，而且已经静了一阵。
///
/// 连接在传数据时对面的确认是连续不断的，所以"发了却一直没有回音"很能说明路断了。
/// 只看最近发过的：连接的重传在断路期间会退避到几秒一次，窗口要比探测间隔长才接得上。
pub(crate) fn unanswered(clock: &Clock, tx: &Stamp, rx: &Stamp) -> bool {
    tx.is_after(rx) && tx.within(clock, UNANSWERED_WINDOW) && !rx.within(clock, SILENCE)
}

/// 一个 relay 在收发路径上的记录。
#[derive(Debug)]
pub(crate) struct RelaySlot {
    pub(crate) id: NodeId,
    /// 最近一次从它收到任何东西。
    pub(crate) last_rx: Stamp,
    /// 最近一次经它发包。
    pub(crate) last_tx: Stamp,
}

impl RelaySlot {
    pub(crate) fn new(id: NodeId) -> Arc<Self> {
        Arc::new(Self { id, last_rx: Stamp::default(), last_tx: Stamp::default() })
    }
}

/// 经 relay 的包前面要加的头，已经编码好。只经一个 relay 时是 9 字节的那一种，经多个时带着整条路径。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameHeader {
    len: usize,
    bytes: [u8; RouteHeader::MAX_LEN],
}

impl FrameHeader {
    /// 经一个 relay：老的格式，relay 不用升级也认识。
    pub(crate) fn single(header: RelayHeader) -> Self {
        let mut bytes = [0; RouteHeader::MAX_LEN];
        bytes[..RelayHeader::LEN].copy_from_slice(&header.to_bytes());
        Self { len: RelayHeader::LEN, bytes }
    }

    /// 经多个 relay。
    pub(crate) fn routed(header: &RouteHeader) -> Self {
        let encoded = header.to_bytes();
        let mut bytes = [0; RouteHeader::MAX_LEN];
        bytes[..encoded.len()].copy_from_slice(&encoded);
        Self { len: encoded.len(), bytes }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// 发给一个对端的包此刻怎么送。
#[derive(Debug)]
pub(crate) enum Hop {
    /// 没有路，包被丢掉。
    None,
    Direct(SocketAddr),
    /// 加上 `header` 发给路径上第一个 relay 的 UDP 地址。
    RelayUdp {
        relay: Arc<RelaySlot>,
        addr: SocketAddr,
        header: FrameHeader,
    },
    /// 加上 `header` 写进到第一个 relay 的那条 TCP。
    RelayTcp {
        relay: Arc<RelaySlot>,
        sender: FrameSender,
        header: FrameHeader,
    },
    /// 对端挂在本节点自己的 relay 上，直接交给它的会话。
    Local,
}

#[derive(Debug)]
struct SlotInner {
    hop: Hop,
    /// 给经 relay 的包加头时用的缓冲，免得每次发送都分配。
    scratch: Vec<u8>,
    stash: VecDeque<(Instant, Vec<u8>)>,
}

/// 一个对端在收发路径上的记录。
#[derive(Debug)]
pub(crate) struct PeerSlot {
    pub(crate) id: NodeId,
    /// 连接层最近一次有包要发给它（不管有没有路）。
    pub(crate) last_tx: Stamp,
    /// 最近一次有人明确表示要和它通信：上层准备拨号，或者它发来了打洞信令。
    pub(crate) last_demand: Stamp,
    /// 最近一次从它的直连地址收到东西。
    pub(crate) last_rx_direct: Stamp,
    /// 最近一次收到它的连接包，不管从哪条路来的。
    pub(crate) last_rx: Stamp,
    /// 当前的路可疑（探测没有回答），或者根本没有路。这时发的包多留一份，见 [`PeerSlot::flush`]。
    doubt: AtomicBool,
    inner: Mutex<SlotInner>,
}

impl PeerSlot {
    pub(crate) fn new(id: NodeId) -> Arc<Self> {
        Arc::new(Self {
            id,
            last_tx: Stamp::default(),
            last_demand: Stamp::default(),
            last_rx_direct: Stamp::default(),
            last_rx: Stamp::default(),
            doubt: AtomicBool::new(true),
            inner: Mutex::new(SlotInner { hop: Hop::None, scratch: Vec::new(), stash: VecDeque::new() }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlotInner> {
        self.inner.lock().expect("peer slot mutex poisoned")
    }

    pub(crate) fn set_hop(&self, hop: Hop) {
        self.lock().hop = hop;
    }

    /// 这个对端是不是活跃的：最近有包要发给它，或者有人要和它通信。
    /// 只有活跃的对端才值得探测直连、为它保持临时的中转会话。
    pub(crate) fn is_active(&self, clock: &Clock) -> bool {
        self.last_tx.within(clock, PEER_ACTIVE) || self.last_demand.within(clock, PEER_ACTIVE)
    }

    /// 收到了它的一个连接包（不管从哪条路来的）。
    pub(crate) fn received(&self, clock: &Clock) {
        self.last_rx.touch(clock);
    }

    /// 现在有没有路。
    pub(crate) fn has_hop(&self) -> bool {
        !matches!(self.lock().hop, Hop::None)
    }

    pub(crate) fn set_doubt(&self, doubt: bool) {
        self.doubt.store(doubt, Ordering::Relaxed);
    }

    /// 连接层的发送。没有路时包被悄悄丢掉。
    ///
    /// # Errors
    ///
    /// 只会返回 [`io::ErrorKind::WouldBlock`]：UDP 的发送缓冲区满了。
    pub(crate) fn send(&self, shared: &Shared, packets: &Packets<'_>) -> io::Result<()> {
        self.last_tx.touch(&shared.clock);
        let mut inner = self.lock();
        let SlotInner { hop, scratch, stash } = &mut *inner;
        // 路可疑、没有路，或者有一阵没有收到对端的任何包了：这个包多留一份。连接正常传数据时不会走到这里，
        // 因为对面的确认是连续不断的。
        let suspicious = self.doubt.load(Ordering::Relaxed) || !self.last_rx.within(&shared.clock, STASH_WHEN_SILENT);
        if suspicious && let Some(last) = packets.segments().last() {
            if stash.len() == STASH_PACKETS {
                stash.pop_front();
            }
            stash.push_back((Instant::now(), last.to_vec()));
        }
        match hop {
            Hop::None => Ok(()),
            Hop::Direct(addr) => shared.port.try_send(&Transmit {
                destination: *addr,
                contents: packets.contents,
                segment_size: packets.segment_size,
                ecn: packets.ecn,
            }),
            Hop::RelayUdp { relay, addr, header } => {
                relay.last_tx.touch(&shared.clock);
                let transmit = framed(header.as_slice(), scratch, packets, *addr);
                shared.port.try_send(&transmit)
            }
            Hop::RelayTcp { relay, sender, header } => {
                relay.last_tx.touch(&shared.clock);
                for packet in packets.segments() {
                    // 队列满了就丢：让连接看到丢包，而不是越积越多的延迟。
                    sender.try_send(frame(header.as_slice(), packet));
                }
                Ok(())
            }
            Hop::Local => {
                let transmit = framed(&self.header(shared), scratch, packets, shared.port.local_addr());
                if let Some(relay) = shared.local_relay.get() {
                    relay.send_to_client(self.id, transmit.contents, transmit.segment_size, transmit.ecn);
                }
                Ok(())
            }
        }
    }

    /// 路恢复了（可疑的路又通了，或者换了一条路，包括从没有路到有路）：把留着的包从现在这条路补发出去。
    ///
    /// 连接在路断着的时候重传定时器会退避到几秒一次（实现前测试：断 3 秒，数据到第 6 秒才恢复）。连接库没有
    /// "现在就发一个包"的接口，而且拥塞窗口被断路之前发出去的包占着，新的包也发不出去。补发断路期间最后的几个包
    /// 就够了：对面收到之后回确认，这边的丢包检测和重传立刻恢复。补发的包对面要是已经收到过，会被当作重复的丢掉，没有害处。
    pub(crate) fn flush(&self, shared: &Shared, now: Instant) {
        let mut inner = self.lock();
        let SlotInner { hop, scratch, stash } = &mut *inner;
        for (stashed, packet) in stash.drain(..) {
            if now.saturating_duration_since(stashed) <= STASH_MAX_AGE {
                self.emit(shared, hop, scratch, &packet);
            }
        }
    }

    /// 把路径层自己的一个帧从现在这条路送给对端：经 relay 时和连接包一样套上中转头，直连时原样发。
    /// 端到端的路径探测靠它走和连接包完全相同的路。它不算连接层的流量：不动"最近发过包"的记录，也不留底。
    /// 没有路时什么都不做。
    pub(crate) fn send_frame(&self, shared: &Shared, frame: &[u8]) {
        let mut inner = self.lock();
        let SlotInner { hop, scratch, .. } = &mut *inner;
        self.emit(shared, hop, scratch, frame);
    }

    /// 从 `hop` 这条路发一个包。尽力而为：发不出去就算了。
    fn emit(&self, shared: &Shared, hop: &Hop, scratch: &mut Vec<u8>, packet: &[u8]) {
        let packets = Packets { contents: packet, segment_size: None, ecn: None };
        match hop {
            Hop::None => {}
            Hop::Direct(addr) => {
                let _sent = shared.port.send_or_drop(&Transmit { destination: *addr, contents: packet, segment_size: None, ecn: None });
            }
            Hop::RelayUdp { addr, header, .. } => {
                let _sent = shared.port.send_or_drop(&framed(header.as_slice(), scratch, &packets, *addr));
            }
            Hop::RelayTcp { sender, header, .. } => {
                let _queued = sender.try_send(frame(header.as_slice(), packet));
            }
            Hop::Local => {
                let transmit = framed(&self.header(shared), scratch, &packets, shared.port.local_addr());
                if let Some(relay) = shared.local_relay.get() {
                    relay.send_to_client(self.id, transmit.contents, transmit.segment_size, transmit.ecn);
                }
            }
        }
    }

    fn header(&self, shared: &Shared) -> [u8; RelayHeader::LEN] {
        RelayHeader { dst: self.id, src: shared.me }.to_bytes()
    }
}

/// 给每个包加上中转头，放进 `scratch`。每个包都多了同样长的头，所以仍然是等长的，分段卸载照样能用。
fn framed<'a>(header: &[u8], scratch: &'a mut Vec<u8>, packets: &Packets<'_>, destination: SocketAddr) -> Transmit<'a> {
    scratch.clear();
    for packet in packets.segments() {
        scratch.extend_from_slice(header);
        scratch.extend_from_slice(packet);
    }
    Transmit { destination, contents: scratch, segment_size: packets.segment_size.map(|size| size + header.len()), ecn: packets.ecn }
}

fn frame(header: &[u8], packet: &[u8]) -> Bytes {
    let mut datagram = BytesMut::with_capacity(header.len() + packet.len());
    datagram.put_slice(header);
    datagram.put_slice(packet);
    datagram.freeze()
}

/// 收包时按来源地址查的表。
#[derive(Debug, Default)]
pub(crate) struct Table {
    pub(crate) peers: HashMap<NodeId, Arc<PeerSlot>>,
    /// 这些地址来的连接包算作这个对端的。只有收到过对端签了名的探测帧的地址才在这里。
    pub(crate) direct: HashMap<SocketAddr, Arc<PeerSlot>>,
    /// netmap 里各个 relay 的 UDP 地址。只有这些地址来的中转帧才收。
    pub(crate) relays: HashMap<SocketAddr, Arc<RelaySlot>>,
}

/// 把信令送出去的钩子，由装配的一方提供。
pub(crate) type SignalHook = Arc<dyn Fn(NodeId, &Signal) + Send + Sync>;

/// 收发路径和控制任务共用的全部状态。
pub(crate) struct Shared {
    pub(crate) me: NodeId,
    pub(crate) port: Arc<UdpPort>,
    pub(crate) clock: Clock,
    pub(crate) table: RwLock<Table>,
    /// 收包路径认出来的、要控制任务处理的帧。
    pub(crate) ctrl: mpsc::Sender<Ctrl>,
    /// 不是直接从 UDP 端口读进连接层缓冲区的包：TCP 中转来的、本节点的 relay 交过来的、一批里放不下的。
    pub(crate) inbound: mpsc::Sender<(NodeId, Bytes)>,
    pub(crate) inbound_rx: Mutex<mpsc::Receiver<(NodeId, Bytes)>>,
    pub(crate) local_relay: OnceLock<Arc<dyn LocalRelay>>,
    pub(crate) signal_hook: RwLock<Option<SignalHook>>,
    pub(crate) changes: broadcast::Sender<RouteChange>,
    /// 任何一个对端的路变了就通知一次，等路的人靠它醒来。
    pub(crate) route_changed: Notify,
    pub(crate) status: Mutex<Status>,
    /// 局域网发现。绑在具体地址上的端口（测试）不自动开，要调用方明确要求。
    pub(crate) lan: Mutex<Option<LanDiscovery>>,
}

impl Shared {
    pub(crate) fn table(&self) -> std::sync::RwLockReadGuard<'_, Table> {
        self.table.read().expect("path table lock poisoned")
    }

    pub(crate) fn table_mut(&self) -> std::sync::RwLockWriteGuard<'_, Table> {
        self.table.write().expect("path table lock poisoned")
    }

    pub(crate) fn send_signal(&self, to: NodeId, signal: &Signal) {
        let hook = self.signal_hook.read().expect("signal hook lock poisoned").clone();
        if let Some(hook) = hook {
            hook(to, signal);
        }
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").field("me", &self.me).field("port", &self.port.local_addr()).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        let (ctrl, _ctrl_rx) = mpsc::channel(1);
        let (inbound, inbound_rx) = mpsc::channel(1);
        Arc::new(Shared {
            me: NodeId::new(1).unwrap(),
            port: Arc::new(UdpPort::bind("127.0.0.1:0".parse().unwrap()).unwrap()),
            clock: Clock::new(),
            table: RwLock::new(Table::default()),
            ctrl,
            inbound,
            inbound_rx: Mutex::new(inbound_rx),
            local_relay: OnceLock::new(),
            signal_hook: RwLock::new(None),
            changes: broadcast::Sender::new(1),
            route_changed: Notify::new(),
            status: Mutex::new(Status::default()),
            lan: Mutex::new(None),
        })
    }

    #[tokio::test]
    async fn a_path_layer_frame_takes_the_same_route_as_connection_packets_without_counting_as_traffic() {
        let shared = shared();
        let receiver = UdpPort::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let slot = PeerSlot::new(NodeId::new(2).unwrap());
        // 没有路：什么都不发。
        slot.send_frame(&shared, b"probe");

        // 经 relay：套上和连接包一样的中转头。
        let header = FrameHeader::single(RelayHeader { dst: slot.id, src: shared.me });
        slot.set_hop(Hop::RelayUdp { relay: RelaySlot::new(NodeId::new(9).unwrap()), addr: receiver.local_addr(), header });
        slot.send_frame(&shared, b"probe");
        let mut storage = vec![0u8; 2048 * receiver.gro_segments()];
        let mut metas = [i3t_underlay::RecvMeta::default(); 1];
        let mut bufs = [std::io::IoSliceMut::new(&mut storage)];
        let received = tokio::time::timeout(Duration::from_secs(5), receiver.recv(&mut bufs, &mut metas)).await.unwrap().unwrap();
        assert_eq!(received, 1);
        let expected = [&RelayHeader { dst: slot.id, src: shared.me }.to_bytes()[..], b"probe"].concat();
        assert_eq!(&bufs[0][..metas[0].len], expected);
        assert_eq!(slot.last_tx.elapsed(&shared.clock), None, "探测不算连接层发过包：不活跃的对端不会因为它一直被当成活跃的");
    }

    #[test]
    fn a_frame_header_is_the_single_relay_header_or_the_whole_route() {
        let (me, peer) = (NodeId::new(1).unwrap(), NodeId::new(2).unwrap());
        let single = FrameHeader::single(RelayHeader { dst: peer, src: me });
        assert_eq!(single.as_slice(), RelayHeader { dst: peer, src: me }.to_bytes());
        let relays = i3t_proto::datagram::RelayPath::new(&[NodeId::new(8).unwrap(), NodeId::new(9).unwrap()]).unwrap();
        let header = RouteHeader { dst: peer, src: me, relays };
        assert_eq!(FrameHeader::routed(&header).as_slice(), header.to_bytes());
        assert_eq!(frame(single.as_slice(), b"quic").len(), RelayHeader::LEN + 4);
    }
}
