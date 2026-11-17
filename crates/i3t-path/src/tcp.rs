//! 节点一侧到 relay 的那条 TCP（UDP 用不了时的退路）。握手的规则在 [`crate::relays`]，这里只管连接和搬运。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use i3t_proto::Datagram;
use i3t_proto::datagram::RelayHeader;
use i3t_proto::session::{Echo, Frame, Refused};
use i3t_underlay::tcp_leg::{self, FrameQueue, FrameReader, FrameSender};
use i3t_underlay::tcp_recovery::Recovery;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use crate::control::Ctrl;
use crate::table::{RelaySlot, Shared};
use crate::timing::TCP_LEG_GIVE_UP;

/// 每个地址连多久算连不上。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

/// 这条 TCP 上发生的事，报给控制任务。
#[derive(Debug)]
pub(crate) enum TcpEvent {
    Connected,
    /// relay 发来的会话帧，错误帧除外。
    Frame(Frame),
    /// relay 发来的错误帧和它回显的字节。
    Refusal(Refused, Echo),
    /// relay 发来的骨干矩阵的一行，整帧。
    Row(Bytes),
    /// 没连上，或者断了。
    Closed,
}

/// 一条到 relay 的 TCP。丢弃它就关掉连接。
#[derive(Debug)]
pub(crate) struct TcpLegHandle {
    /// 控制任务给这条 TCP 编的号。同一个 relay 先后连过几次时，靠它认出迟到的事件属于哪一次。
    pub(crate) conn: u64,
    pub(crate) sender: FrameSender,
    task: JoinHandle<()>,
}

impl Drop for TcpLegHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 去连 `addrs` 里的地址（按顺序，连上一个为止），之后在这条 TCP 上收发帧。
pub(crate) fn spawn(shared: Arc<Shared>, relay: Arc<RelaySlot>, conn: u64, addrs: Vec<SocketAddr>) -> TcpLegHandle {
    let (sender, queue) = tcp_leg::frame_queue();
    let task = tokio::spawn(async move {
        let reason = run(&shared, &relay, conn, &addrs, queue).await;
        tracing::debug!(relay = %relay.id, conn, reason, "到 relay 的 TCP 结束");
        // 控制任务已经没了的话，这个事件也没有人要了。
        let _closed = shared.ctrl.send(Ctrl::Tcp { relay: relay.id, conn, event: TcpEvent::Closed }).await;
    });
    TcpLegHandle { conn, sender, task }
}

async fn run(shared: &Shared, relay: &RelaySlot, conn: u64, addrs: &[SocketAddr], queue: FrameQueue) -> &'static str {
    let Some(stream) = connect(addrs).await else { return "连不上" };
    if tcp_leg::configure(&stream).is_err() {
        return "设置不了 socket 选项";
    }
    if !Recovery::giving_up_after(TCP_LEG_GIVE_UP).apply(&stream) {
        tracing::debug!(relay = %relay.id, "到 relay 的 TCP 没有设重传上限（内核不支持），断网之后恢复得慢一些");
    }
    if shared.ctrl.send(Ctrl::Tcp { relay: relay.id, conn, event: TcpEvent::Connected }).await.is_err() {
        return "路径层已经停了";
    }
    let (read_half, write_half) = stream.into_split();
    tokio::select! {
        _ = tcp_leg::write_frames(write_half, queue) => "写不进去了",
        reason = read(shared, relay, conn, FrameReader::new(read_half)) => reason,
    }
}

async fn connect(addrs: &[SocketAddr]) -> Option<TcpStream> {
    for &addr in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => return Some(stream),
            Ok(Err(error)) => tracing::debug!(%addr, %error, "连 relay 的 TCP 失败"),
            Err(_) => tracing::debug!(%addr, "连 relay 的 TCP 超时"),
        }
    }
    None
}

/// 读这条 TCP：发给本节点的中转帧去掉头交给连接层，会话帧交给控制任务，别的丢掉。
async fn read(shared: &Shared, relay: &RelaySlot, conn: u64, mut reader: FrameReader) -> &'static str {
    loop {
        let frame = match reader.next().await {
            Ok(Some(frame)) => frame,
            Ok(None) => return "relay 关闭了连接",
            Err(_) => return "读出错",
        };
        relay.last_rx.touch(&shared.clock);
        match Datagram::parse(&frame) {
            Ok(Datagram::Relayed { header, .. }) if header.dst == shared.me => {
                // 连接层来不及收的时候在这里等，压力传回这条 TCP，relay 那头的队列会开始丢包。
                if shared.inbound.send((header.src, frame.slice(RelayHeader::LEN..))).await.is_err() {
                    return "路径层已经停了";
                }
            }
            // 多级中转的包：这条 TCP 通向的 relay 必须就是路径上的最后一个。
            Ok(Datagram::Routed { header, .. }) if header.dst == shared.me && header.relays.last() == relay.id => {
                if shared.inbound.send((header.src, frame.slice(header.encoded_len()..))).await.is_err() {
                    return "路径层已经停了";
                }
            }
            Ok(Datagram::Session(session)) if !session.is_request() => {
                let event = match (session, Refused::echo(&frame)) {
                    (Frame::Error(refused), Some(echo)) => TcpEvent::Refusal(refused, echo),
                    (session, _) => TcpEvent::Frame(session),
                };
                if shared.ctrl.send(Ctrl::Tcp { relay: relay.id, conn, event }).await.is_err() {
                    return "路径层已经停了";
                }
            }
            Ok(Datagram::Row(_)) => {
                if shared.ctrl.send(Ctrl::Tcp { relay: relay.id, conn, event: TcpEvent::Row(frame) }).await.is_err() {
                    return "路径层已经停了";
                }
            }
            Ok(_) | Err(_) => {}
        }
    }
}
