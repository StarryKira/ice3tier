//! relay 一侧的 TCP 中转：UDP 用不了的节点连上来，在这条 TCP 里做同样的握手、收发同样的帧。
//!
//! # 没有握手成功的连接
//!
//! 谁都可以连上来，所以还没有会话的连接要尽快打发掉，不能让人占着位子把只能走 TCP 的节点挡在外面：
//!
//! - 从接受连接时算起 [`HANDSHAKE_TIMEOUT`] 之内必须握手成功，这期间来多少帧都不延长；
//! - 握手成功之前只收 INIT 和 AUTH，来了别的就断开；
//! - 同一个来源（IPv4 地址，或者 IPv6 的 /64）同时最多有 [`MAX_PENDING_PER_SOURCE`] 条还没握手成功的连接。

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_proto::session::Frame;
use i3t_proto::{Datagram, NodeId};
use i3t_underlay::tcp_leg::{self, FrameReader};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::limit::source_key;
use crate::relay::{Inner, Source, Via};
use crate::session::SESSION_TTL;

/// 连上来之后多久之内必须握手成功。没有会话的 TCP 不值得留着。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// 同时最多接多少条 TCP。每个走 TCP 的节点只有一条，这个数对个人用的网络绰绰有余；
/// 它挡的是有人开一大堆连接又不握手。
const MAX_LEGS: usize = 1024;
/// 同一个来源同时最多有几条还没握手成功的 TCP。同一个 NAT 后面的几个节点同时连上来也够用。
const MAX_PENDING_PER_SOURCE: usize = 8;
/// 拒绝连接的日志最多隔这么久记一条，中间拒绝了多少条写在下一条里。
const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(10);
/// 还没来得及接受的连接最多排多少条。
const LISTEN_BACKLOG: i32 = 1024;

/// 在 UDP 端口的同一个地址和端口号上监听 TCP。
///
/// # Errors
///
/// 绑不上（端口被别的程序占着）。调用方稍后再试。
pub(crate) fn listen(inner: &Arc<Inner>) -> io::Result<JoinHandle<()>> {
    let addr = inner.port.local_addr();
    let listener = bind(addr)?;
    tracing::info!(%addr, "relay 开始监听 TCP 中转");
    Ok(tokio::spawn(accept(Arc::clone(inner), listener)))
}

/// UDP 端口是双栈的（绑在 `[::]` 上）时，TCP 也要 IPv4 和 IPv6 都接。有的系统上 IPv6 的 TCP 默认不接 IPv4，所以明确设一下。
fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6()
        && let Err(error) = socket.set_only_v6(false)
    {
        tracing::debug!(%error, "这个系统上 IPv6 的 TCP 监听不能同时接 IPv4 的连接");
    }
    // 和标准库在 Unix 上的做法一样：进程重启之后不用等旧连接的 TIME_WAIT 过去就能重新绑定。Windows 上这个选项的含义是
    // "允许别的进程抢同一个端口"，不能设。
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(LISTEN_BACKLOG)?;
    TcpListener::from_std(socket.into())
}

async fn accept(inner: Arc<Inner>, listener: TcpListener) {
    let slots = Arc::new(Semaphore::new(MAX_LEGS));
    let pending = Arc::new(Pending::default());
    let mut refusals = RefusalLog::default();
    let mut enabled = inner.subscribe_enabled();
    loop {
        let accepted = tokio::select! {
            // relay 被停用，或者整个 relay 被丢弃了：不再监听。
            _ = enabled.wait_for(|enabled| !enabled) => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, addr) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                // 多半是文件描述符用完了。歇一下再接，不然会空转。
                tracing::warn!(%error, "relay 接受 TCP 连接失败");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
            if let Some(more) = refusals.due(Instant::now()) {
                tracing::warn!(from = %addr, more, "TCP 中转的连接数到了上限 {MAX_LEGS}，拒绝新的连接");
            }
            continue;
        };
        let Some(handshake) = pending.admit(addr.ip()) else {
            if let Some(more) = refusals.due(Instant::now()) {
                tracing::warn!(from = %addr, more, "这个来源已经有 {MAX_PENDING_PER_SOURCE} 条 TCP 还没握手成功，拒绝新的连接");
            }
            continue;
        };
        let inner = Arc::clone(&inner);
        tokio::spawn(async move {
            let reason = serve(&inner, stream, addr, handshake).await;
            tracing::debug!(from = %addr, reason, "一条 TCP 中转结束");
            drop(slot);
        });
    }
}

/// 还没握手成功的 TCP 有几条，按来源算。
#[derive(Debug, Default)]
struct Pending(Mutex<HashMap<IpAddr, usize>>);

impl Pending {
    /// 来自 `from` 的一条新连接能不能接。能的话返回它占着的名额，握手成功或者连接结束时放掉。
    fn admit(self: &Arc<Self>, from: IpAddr) -> Option<Handshaking> {
        let source = source_key(from);
        let mut by_source = self.0.lock().expect("pending TCP legs mutex poisoned");
        let count = by_source.entry(source).or_default();
        if *count >= MAX_PENDING_PER_SOURCE {
            return None;
        }
        *count += 1;
        Some(Handshaking { pending: Arc::clone(self), source })
    }
}

/// 一条还没握手成功的 TCP 占着的名额。
#[derive(Debug)]
struct Handshaking {
    pending: Arc<Pending>,
    source: IpAddr,
}

impl Drop for Handshaking {
    fn drop(&mut self) {
        let mut by_source = self.pending.0.lock().expect("pending TCP legs mutex poisoned");
        if let Some(count) = by_source.get_mut(&self.source) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                by_source.remove(&self.source);
            }
        }
    }
}

/// 拒绝连接的日志的限速：有人一直往上连的时候不刷屏。
#[derive(Debug, Default)]
struct RefusalLog {
    logged: Option<Instant>,
    skipped: u64,
}

impl RefusalLog {
    /// 这一次拒绝要不要记日志。要的话返回上一条之后又悄悄拒绝了多少条。
    fn due(&mut self, now: Instant) -> Option<u64> {
        if self.logged.is_some_and(|at| now.saturating_duration_since(at) < REFUSAL_LOG_INTERVAL) {
            self.skipped += 1;
            return None;
        }
        self.logged = Some(now);
        Some(std::mem::take(&mut self.skipped))
    }
}

/// 握手成功之前只收这两种帧。
fn is_handshake(frame: &[u8]) -> bool {
    matches!(Datagram::parse(frame), Ok(Datagram::Session(Frame::Init(_) | Frame::Auth(_))))
}

/// 服务一条 TCP，直到它断开、超时或者 relay 被停用。返回结束的原因，给日志用。
async fn serve(inner: &Inner, stream: TcpStream, addr: SocketAddr, handshake: Handshaking) -> &'static str {
    if let Err(error) = tcp_leg::configure(&stream) {
        tracing::debug!(from = %addr, %error, "给 TCP 中转设置选项失败");
        return "设置不了 socket 选项";
    }
    let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port());
    let conn = inner.next_conn();
    let (read_half, write_half) = stream.into_split();
    let (sender, queue) = tcp_leg::frame_queue();
    let writer = tokio::spawn(tcp_leg::write_frames(write_half, queue));
    let mut reader = FrameReader::new(read_half);
    let mut enabled = inner.subscribe_enabled();
    let source = Source { addr, via: Via::Tcp { conn, sender: &sender } };
    let mut owner: Option<NodeId> = None;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut handshake = Some(handshake);

    let reason = loop {
        // 握手成功之前：从接受连接时算起的截止时间，来多少帧都不延长。之后按会话的有效期等，每一帧重新算：
        // 节点的 PING 也从这条 TCP 来，这么久没有任何帧说明它不在了。
        let has_session = owner.is_some_and(|node| inner.tcp_session_alive(node, conn));
        let until = match owner {
            None => deadline,
            Some(_) => Instant::now() + if has_session { SESSION_TTL } else { HANDSHAKE_TIMEOUT },
        };
        tokio::select! {
            _ = enabled.wait_for(|enabled| !enabled) => break "relay 停用",
            frame = tokio::time::timeout_at(until, reader.next()) => match frame {
                Ok(Ok(Some(frame))) => {
                    if owner.is_none() && !is_handshake(&frame) {
                        break "握手成功之前发来了别的帧";
                    }
                    if let Some(node) = inner.handle_tcp_frame(&source, &frame) {
                        owner = Some(node);
                        // 握手成功了：不再占着"还没握手成功"的名额。
                        drop(handshake.take());
                    }
                }
                Ok(Ok(None)) => break "对方关闭",
                Ok(Err(_)) => break "读出错",
                Err(_) if has_session => break "会话过期",
                Err(_) => break "迟迟没有握手成功",
            },
        }
    };
    inner.close_tcp(conn);
    writer.abort();
    reason
}
