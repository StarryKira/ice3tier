//! 路径层：QUIC 以为对端永远在同一个地址，实际每个包走哪条路由这里决定。
//!
//! 三条路：直连 UDP、经 relay 的 UDP、经 relay 的 TCP（把包加长度前缀装进一条 TCP）。
//! 换路只是改一个原子变量，QUIC 连接不知道。

use std::fmt;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use clap::ValueEnum;
use quinn::udp::{RecvMeta, Transmit, UdpSocketState};
use quinn::{AsyncUdpSocket, UdpPoller};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter, Interest};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;

use crate::wire::{self, Frame, NodeId, RELAY_HEADER_LEN};

/// TCP 中转里收到的包在交给 QUIC 之前最多排这么多个。排满之后读 TCP 的任务停下，压力传回 TCP。
const TCP_INBOUND_QUEUE: usize = 4096;
const REGISTER_INTERVAL: Duration = Duration::from_millis(500);
pub const SOCKET_BUFFER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    Direct,
    Relay,
    RelayTcp,
}

impl Route {
    const ALL: [Self; 3] = [Self::Direct, Self::Relay, Self::RelayTcp];

    fn from_index(index: u8) -> Self {
        Self::ALL[usize::from(index) % Self::ALL.len()]
    }

    fn index(self) -> u8 {
        match self {
            Self::Direct => 0,
            Self::Relay => 1,
            Self::RelayTcp => 2,
        }
    }
}

pub struct PathConfig {
    pub local: NodeId,
    pub peer: NodeId,
    pub bind: SocketAddr,
    /// 对端的物理地址。不给的话从收到的第一个直连包里学。
    pub direct: Option<SocketAddr>,
    pub relay_udp: Option<SocketAddr>,
    pub relay_tcp: Option<SocketAddr>,
    pub initial: Route,
    /// TCP 中转的发送队列最多排多少个包，排满就丢。
    pub tcp_queue: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct PathCounters {
    pub tx_direct: u64,
    pub tx_relay: u64,
    pub tx_tcp: u64,
    pub tcp_dropped: u64,
}

pub struct PathSocket {
    io: UdpSocket,
    state: UdpSocketState,
    local: NodeId,
    peer: NodeId,
    route: AtomicU8,
    direct_peer: Mutex<Option<SocketAddr>>,
    relay_udp: Option<SocketAddr>,
    tcp: Option<TcpLeg>,
    /// 给经 relay 的包加头时用的缓冲，避免每次发送都分配。
    scratch: Mutex<Vec<u8>>,
    tx_direct: AtomicU64,
    tx_relay: AtomicU64,
    tx_tcp: AtomicU64,
}

impl PathSocket {
    pub async fn bind(config: PathConfig) -> anyhow::Result<Arc<Self>> {
        let std_sock = std::net::UdpSocket::bind(config.bind)?;
        std_sock.set_nonblocking(true)?;
        let state = UdpSocketState::new((&std_sock).into())?;
        // 默认的 200KB 左右在高速率下会在本机就溢出。内核会把它截到系统允许的上限。
        state.set_recv_buffer_size((&std_sock).into(), SOCKET_BUFFER_BYTES)?;
        state.set_send_buffer_size((&std_sock).into(), SOCKET_BUFFER_BYTES)?;
        let io = UdpSocket::from_std(std_sock)?;
        let tcp = match config.relay_tcp {
            Some(relay) => Some(TcpLeg::connect(relay, config.local, config.tcp_queue).await?),
            None => None,
        };
        let socket = Arc::new(Self {
            io,
            state,
            local: config.local,
            peer: config.peer,
            route: AtomicU8::new(config.initial.index()),
            direct_peer: Mutex::new(config.direct),
            relay_udp: config.relay_udp,
            tcp,
            scratch: Mutex::new(Vec::with_capacity(128 * 1024)),
            tx_direct: AtomicU64::new(0),
            tx_relay: AtomicU64::new(0),
            tx_tcp: AtomicU64::new(0),
        });
        if let Some(relay) = config.relay_udp {
            tokio::spawn(Arc::clone(&socket).keep_registered(relay));
        }
        Ok(socket)
    }

    pub fn route(&self) -> Route {
        Route::from_index(self.route.load(Ordering::Relaxed))
    }

    pub fn set_route(&self, route: Route) {
        self.route.store(route.index(), Ordering::Relaxed);
    }

    pub fn counters(&self) -> PathCounters {
        PathCounters {
            tx_direct: self.tx_direct.load(Ordering::Relaxed),
            tx_relay: self.tx_relay.load(Ordering::Relaxed),
            tx_tcp: self.tx_tcp.load(Ordering::Relaxed),
            tcp_dropped: self.tcp.as_ref().map_or(0, |leg| leg.dropped.load(Ordering::Relaxed)),
        }
    }

    /// relay 靠这个知道本节点的 UDP 地址。丢了也没关系，下一次再报。
    async fn keep_registered(self: Arc<Self>, relay: SocketAddr) {
        let frame = wire::register_frame(self.local);
        loop {
            let _sent = self.io.send_to(&frame, relay).await;
            tokio::time::sleep(REGISTER_INTERVAL).await;
        }
    }

    fn send_udp(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        self.io.try_io(Interest::WRITABLE, || self.state.send((&self.io).into(), transmit))
    }

    fn send_direct(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let known = *self.direct_peer.lock().expect("direct_peer mutex poisoned");
        // 还不知道对端的物理地址时只能丢，QUIC 会重传。
        let Some(destination) = known else { return Ok(()) };
        self.tx_direct.fetch_add(1, Ordering::Relaxed);
        self.send_udp(&Transmit { destination, src_ip: None, ..transmit.clone() })
    }

    fn send_via_relay(&self, relay: SocketAddr, transmit: &Transmit<'_>) -> io::Result<()> {
        let mut framed = self.scratch.lock().expect("scratch mutex poisoned");
        framed.clear();
        for segment in segments(transmit) {
            wire::push_data_header(&mut framed, self.peer, self.local);
            framed.extend_from_slice(segment);
        }
        self.tx_relay.fetch_add(1, Ordering::Relaxed);
        // 每段都多了同样长的头，所以仍然是等长分段，内核的 UDP 分段卸载照样能用。
        self.send_udp(&Transmit {
            destination: relay,
            ecn: transmit.ecn,
            contents: &framed,
            segment_size: transmit.segment_size.map(|size| size + RELAY_HEADER_LEN),
            src_ip: None,
        })
    }

    fn send_via_tcp(&self, leg: &TcpLeg, transmit: &Transmit<'_>) {
        for segment in segments(transmit) {
            let mut frame = Vec::with_capacity(RELAY_HEADER_LEN + segment.len());
            wire::push_data_header(&mut frame, self.peer, self.local);
            frame.extend_from_slice(segment);
            leg.push(frame.into());
            self.tx_tcp.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 把刚收到的一批数据报改写成 QUIC 期望的样子：来源改成对端的固定地址，去掉中转头。
    fn rewrite_received(&self, meta: &mut RecvMeta, buf: &mut [u8]) {
        let data = &mut buf[..meta.len];
        let Some(&first) = data.first() else { return };
        let physical = meta.addr;
        meta.addr = self.peer.virtual_addr();
        if matches!(Frame::parse(&[first]), Some(Frame::Quic)) {
            let mut known = self.direct_peer.lock().expect("direct_peer mutex poisoned");
            if *known != Some(physical) {
                *known = Some(physical);
            }
            return;
        }

        let stride = if meta.stride == 0 { meta.len } else { meta.stride };
        let mut kept = 0;
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + stride).min(data.len());
            let ours = matches!(
                Frame::parse(&data[offset..end]),
                Some(Frame::Data { dst, src, .. }) if dst == self.local && src == self.peer
            );
            if ours {
                data.copy_within(offset + RELAY_HEADER_LEN..end, kept);
                kept += end - offset - RELAY_HEADER_LEN;
            }
            offset = end;
        }
        meta.len = kept;
        meta.stride = stride.saturating_sub(RELAY_HEADER_LEN).max(1);
    }
}

/// 一次发送里的各个数据报。开了分段卸载时 `contents` 里是若干个等长的包拼在一起。
fn segments<'a>(transmit: &Transmit<'a>) -> impl Iterator<Item = &'a [u8]> {
    let size = transmit.segment_size.unwrap_or(transmit.contents.len()).max(1);
    transmit.contents.chunks(size)
}

impl fmt::Debug for PathSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathSocket").field("local", &self.local).field("route", &self.route()).finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for PathSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(WritablePoller { socket: self, pending: None })
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        match (self.route(), self.relay_udp, &self.tcp) {
            (Route::Direct, _, _) => self.send_direct(transmit),
            (Route::Relay, Some(relay), _) => self.send_via_relay(relay, transmit),
            (Route::RelayTcp, _, Some(leg)) => {
                self.send_via_tcp(leg, transmit);
                Ok(())
            }
            (Route::Relay, None, _) | (Route::RelayTcp, _, None) => {
                Err(io::Error::new(io::ErrorKind::NotConnected, "选中的路径没有配置 relay 地址"))
            }
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if let Some(leg) = &self.tcp
            && let Poll::Ready(count) = leg.poll_recv(cx, bufs, meta, self.peer)
        {
            return Poll::Ready(Ok(count));
        }
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            let received = self.io.try_io(Interest::READABLE, || self.state.recv((&self.io).into(), bufs, meta));
            // try_io 返回 WouldBlock 时已经清掉了就绪标记，回到循环顶部重新登记唤醒。
            if let Ok(count) = received {
                for (one_meta, buf) in meta.iter_mut().zip(bufs.iter_mut()).take(count) {
                    self.rewrite_received(one_meta, buf);
                }
                return Poll::Ready(Ok(count));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.state.max_gso_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.state.gro_segments()
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
    }
}

type WritableFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>;

/// quinn 在 `try_send` 返回 WouldBlock 之后用它等 UDP socket 重新可写。
struct WritablePoller {
    socket: Arc<PathSocket>,
    pending: Option<WritableFuture>,
}

impl fmt::Debug for WritablePoller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WritablePoller").finish_non_exhaustive()
    }
}

impl UdpPoller for WritablePoller {
    fn poll_writable(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let pending = this.pending.get_or_insert_with(|| {
            let socket = Arc::clone(&this.socket);
            Box::pin(async move { socket.io.writable().await })
        });
        let result = ready!(pending.as_mut().poll(cx));
        this.pending = None;
        Poll::Ready(result)
    }
}

/// 到 relay 的那一条 TCP。包加 2 字节长度前缀后顺序写进去。
struct TcpLeg {
    outbound: mpsc::Sender<Bytes>,
    inbound: Mutex<mpsc::Receiver<Bytes>>,
    dropped: AtomicU64,
}

impl TcpLeg {
    async fn connect(relay: SocketAddr, local: NodeId, queue: usize) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(relay).await?;
        stream.set_nodelay(true)?;
        let (reader, writer) = stream.into_split();
        let (outbound, outbound_rx) = mpsc::channel(queue.max(1));
        let (inbound_tx, inbound) = mpsc::channel(TCP_INBOUND_QUEUE);
        tokio::spawn(async move {
            if let Err(err) = write_frames(writer, outbound_rx, Some(local)).await {
                eprintln!("TCP 中转写入结束：{err}");
            }
        });
        tokio::spawn(async move {
            if let Err(err) = read_data_frames(reader, inbound_tx, local).await {
                eprintln!("TCP 中转读取结束：{err}");
            }
        });
        Ok(Self { outbound, inbound: Mutex::new(inbound), dropped: AtomicU64::new(0) })
    }

    fn push(&self, frame: Bytes) {
        // 队列满了就丢：让 QUIC 看到丢包而不是越积越多的延迟。
        if self.outbound.try_send(frame).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
        peer: NodeId,
    ) -> Poll<usize> {
        let mut inbound = self.inbound.lock().expect("tcp inbound mutex poisoned");
        let mut count = 0;
        for (buf, one_meta) in bufs.iter_mut().zip(meta.iter_mut()) {
            let Poll::Ready(Some(packet)) = inbound.poll_recv(cx) else { break };
            let len = packet.len().min(buf.len());
            buf[..len].copy_from_slice(&packet[..len]);
            *one_meta = RecvMeta { addr: peer.virtual_addr(), len, stride: len, ecn: None, dst_ip: None };
            count += 1;
        }
        if count == 0 { Poll::Pending } else { Poll::Ready(count) }
    }
}

/// 把队列里的帧写进 TCP。队列空了才 flush，这样一批包合成一次写。
///
/// `register` 给出时先写一帧登记，relay 靠它知道这条 TCP 属于哪个节点。
pub async fn write_frames(
    writer: OwnedWriteHalf,
    mut queue: mpsc::Receiver<Bytes>,
    register: Option<NodeId>,
) -> io::Result<()> {
    let mut writer = BufWriter::with_capacity(64 * 1024, writer);
    if let Some(id) = register {
        write_frame(&mut writer, &wire::register_frame(id)).await?;
        writer.flush().await?;
    }
    while let Some(frame) = queue.recv().await {
        write_frame(&mut writer, &frame).await?;
        while let Ok(frame) = queue.try_recv() {
            write_frame(&mut writer, &frame).await?;
        }
        writer.flush().await?;
    }
    Ok(())
}

async fn write_frame(writer: &mut BufWriter<OwnedWriteHalf>, frame: &[u8]) -> io::Result<()> {
    let len = u16::try_from(frame.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "帧超过 64KB"))?;
    writer.write_u16(len).await?;
    writer.write_all(frame).await
}

/// 从 TCP 里读出一帧。连接正常关闭时返回 `None`。
pub async fn read_frame(reader: &mut OwnedReadHalf) -> io::Result<Option<Bytes>> {
    let len = match reader.read_u16().await {
        Ok(len) => usize::from(len),
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut frame = vec![0; len];
    reader.read_exact(&mut frame).await?;
    Ok(Some(frame.into()))
}

async fn read_data_frames(mut reader: OwnedReadHalf, inbound: mpsc::Sender<Bytes>, local: NodeId) -> io::Result<()> {
    while let Some(frame) = read_frame(&mut reader).await? {
        let payload_at = match Frame::parse(&frame) {
            Some(Frame::Data { dst, .. }) if dst == local => RELAY_HEADER_LEN,
            Some(Frame::Data { .. } | Frame::Register { .. } | Frame::Quic) | None => continue,
        };
        if inbound.send(frame.slice(payload_at..)).await.is_err() {
            break;
        }
    }
    Ok(())
}
