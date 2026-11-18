//! 节点一侧到 relay 的那条 TCP（UDP 用不了时的退路）。握手的规则在 [`crate::relays`]，这里只管连接和搬运。

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use i3t_proto::Datagram;
use i3t_proto::datagram::RelayHeader;
use i3t_proto::session::{Echo, Frame, Refused};
use i3t_underlay::tcp_leg::{self, FrameQueue, FrameReader, FrameSender};
use i3t_underlay::tcp_mss;
use i3t_underlay::tcp_recovery::Recovery;
use tokio::net::TcpStream;
use tokio::task::{JoinHandle, JoinSet};

use crate::control::Ctrl;
use crate::table::{RelaySlot, Shared};
use crate::timing::TCP_LEG_GIVE_UP;

/// 每个地址连多久算连不上。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// 一个 relay 有几个地址时，前一个还没连上，隔这么久就同时去连下一个（前一个失败了就马上连下一个）。
/// 连不上的地址多半是根本不回答，挨个等满 [`CONNECT_TIMEOUT`] 的话，排在后面的好地址要等好几秒。
const CONNECT_STAGGER: Duration = Duration::from_millis(250);

/// 这条 TCP 上发生的事，报给控制任务。
#[derive(Debug)]
pub(crate) enum TcpEvent {
    /// 连上了这个地址。
    Connected(SocketAddr),
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

/// 去连 `addrs` 里的地址（按顺序错开一点时间一个个开始，先连上的那个为准），之后在这条 TCP 上收发帧。
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
    let Ok((stream, addr)) = connect_first(addrs).await else { return "连不上" };
    if tcp_leg::configure(&stream).is_err() {
        return "设置不了 socket 选项";
    }
    if !Recovery::giving_up_after(TCP_LEG_GIVE_UP).apply(&stream) {
        tracing::debug!(relay = %relay.id, "到 relay 的 TCP 没有设重传上限（内核不支持），断网之后恢复得慢一些");
    }
    if shared.ctrl.send(Ctrl::Tcp { relay: relay.id, conn, event: TcpEvent::Connected(addr) }).await.is_err() {
        return "路径层已经停了";
    }
    let (read_half, write_half) = stream.into_split();
    tokio::select! {
        _ = tcp_leg::write_frames(write_half, queue) => "写不进去了",
        reason = read(shared, relay, conn, FrameReader::new(read_half)) => reason,
    }
}

/// 按顺序去连 `addrs`：每隔 250 毫秒（或者前一个失败时马上）开始连下一个，几个同时在连，先连上的那个为准，
/// 别的放下。每个地址最多等 4 秒。所以排在前面的地址连不上（不回答）时，后面的好地址不用等它超时。
/// 握手里通告压低的 MSS（[`tcp_mss`]）。
///
/// relay 的几个地址（controller 找出来的，白皮书 §7.2）不一定都连得上：到 relay 的 TCP 中转和独占 TCP（`i3t-lane`）
/// 都这样连。调用方按"上一次连上的排最前"排好 `addrs`。
///
/// # Errors
///
/// 全都连不上：最后一个失败的原因；全都超时（或者 `addrs` 是空的）时是 [`io::ErrorKind::TimedOut`]。
pub async fn connect_first(addrs: &[SocketAddr]) -> io::Result<(TcpStream, SocketAddr)> {
    let mut pending = addrs.iter().copied();
    let mut attempts = JoinSet::new();
    let mut next_start = tokio::time::Instant::now();
    let mut last_error = io::Error::new(io::ErrorKind::TimedOut, "没有连得上的地址");
    loop {
        let more = pending.len() > 0;
        tokio::select! {
            () = tokio::time::sleep_until(next_start), if more => {
                if let Some(addr) = pending.next() {
                    attempts.spawn(async move { (addr, tokio::time::timeout(CONNECT_TIMEOUT, tcp_mss::connect(addr)).await) });
                }
                next_start = tokio::time::Instant::now() + CONNECT_STAGGER;
            }
            Some(done) = attempts.join_next() => match done {
                Ok((addr, Ok(Ok(stream)))) => return Ok((stream, addr)),
                Ok((addr, Ok(Err(error)))) => {
                    tracing::debug!(%addr, %error, "连 TCP 失败");
                    last_error = error;
                    next_start = tokio::time::Instant::now();
                }
                Ok((addr, Err(_))) => tracing::debug!(%addr, "连 TCP 超时"),
                // 连接的任务不会 panic，也只在这里被放下。
                Err(_) => {}
            },
            else => return Err(last_error),
        }
    }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// 一个不回答握手的地址：监听着、却从不接受连接，等着接受的队列已经满了，再来的 SYN 内核直接丢掉。
    /// 返回那个地址和占着队列的东西（丢掉它们地址就不"死"了）。
    async fn silent_address() -> (SocketAddr, tokio::net::TcpListener, TcpStream) {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(0).unwrap();
        let addr = listener.local_addr().unwrap();
        let filler = TcpStream::connect(addr).await.unwrap();
        (addr, listener, filler)
    }

    #[tokio::test]
    async fn an_address_that_never_answers_does_not_hold_up_the_next_one() {
        // 排在前面的地址不回答：错开一点时间就去连下一个，不等它超时。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = listener.local_addr().unwrap();
        let (dead, _queue, _filler) = silent_address().await;
        assert!(tokio::time::timeout(Duration::from_millis(300), TcpStream::connect(dead)).await.is_err(), "这个地址应该不回答");
        let started = tokio::time::Instant::now();
        let (stream, addr) = connect_first(&[dead, live]).await.unwrap();
        assert_eq!(addr, live);
        assert!(started.elapsed() < Duration::from_secs(1), "等了 {:?}", started.elapsed());

        // 连上的那一条通告压低的 MSS：对端（这里是一个什么都不设的监听者）发过来的分段不超过它。回环上不压的话是六万多。
        #[cfg(unix)]
        {
            let accepted = listener.accept().await.unwrap().0;
            let clamped = u32::from(tcp_mss::DEFAULT);
            assert!(socket2::SockRef::from(&stream).tcp_mss().unwrap() <= clamped);
            assert!(socket2::SockRef::from(&accepted).tcp_mss().unwrap() <= clamped, "对端按本端通告的 MSS 发");
        }
        drop(stream);

        // 一个也连不上：说出最后一个的原因。
        drop(listener);
        assert!(connect_first(&[live]).await.is_err());
        assert_eq!(connect_first(&[]).await.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
