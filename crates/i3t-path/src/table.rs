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
use i3t_proto::datagram::RelayHeader;
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

/// 发给一个对端的包此刻怎么送。
#[derive(Debug)]
pub(crate) enum Hop {
    /// 没有路，包被丢掉。
    None,
    Direct(SocketAddr),
    RelayUdp {
        relay: Arc<RelaySlot>,
        addr: SocketAddr,
    },
    RelayTcp {
        relay: Arc<RelaySlot>,
        sender: FrameSender,
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
            Hop::RelayUdp { relay, addr } => {
                relay.last_tx.touch(&shared.clock);
                let transmit = self.framed(shared, scratch, packets, *addr);
                shared.port.try_send(&transmit)
            }
            Hop::RelayTcp { relay, sender } => {
                relay.last_tx.touch(&shared.clock);
                let header = self.header(shared);
                for packet in packets.segments() {
                    // 队列满了就丢：让连接看到丢包，而不是越积越多的延迟。
                    sender.try_send(frame(&header, packet));
                }
                Ok(())
            }
            Hop::Local => {
                let transmit = self.framed(shared, scratch, packets, shared.port.local_addr());
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
            if now.saturating_duration_since(stashed) > STASH_MAX_AGE {
                continue;
            }
            let packets = Packets { contents: &packet, segment_size: None, ecn: None };
            match hop {
                Hop::None => {}
                // 补发是尽力而为的：这几个分支里发不出去就算了，返回值不看。
                Hop::Direct(addr) => {
                    let _sent =
                        shared.port.send_or_drop(&Transmit { destination: *addr, contents: &packet, segment_size: None, ecn: None });
                }
                Hop::RelayUdp { addr, .. } => {
                    let _sent = shared.port.send_or_drop(&self.framed(shared, scratch, &packets, *addr));
                }
                Hop::RelayTcp { sender, .. } => {
                    let _queued = sender.try_send(frame(&self.header(shared), &packet));
                }
                Hop::Local => {
                    let transmit = self.framed(shared, scratch, &packets, shared.port.local_addr());
                    if let Some(relay) = shared.local_relay.get() {
                        relay.send_to_client(self.id, transmit.contents, transmit.segment_size, transmit.ecn);
                    }
                }
            }
        }
    }

    fn header(&self, shared: &Shared) -> [u8; RelayHeader::LEN] {
        RelayHeader { dst: self.id, src: shared.me }.to_bytes()
    }

    /// 给每个包加上中转头，放进 `scratch`。每个包都多了同样长的头，所以仍然是等长的，分段卸载照样能用。
    fn framed<'a>(&self, shared: &Shared, scratch: &'a mut Vec<u8>, packets: &Packets<'_>, destination: SocketAddr) -> Transmit<'a> {
        let header = self.header(shared);
        scratch.clear();
        for packet in packets.segments() {
            scratch.extend_from_slice(&header);
            scratch.extend_from_slice(packet);
        }
        Transmit {
            destination,
            contents: scratch,
            segment_size: packets.segment_size.map(|size| size + RelayHeader::LEN),
            ecn: packets.ecn,
        }
    }
}

fn frame(header: &[u8; RelayHeader::LEN], packet: &[u8]) -> Bytes {
    let mut datagram = BytesMut::with_capacity(RelayHeader::LEN + packet.len());
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
