//! relay 一侧的 TCP 中转：UDP 用不了的节点连上来，在这条 TCP 里做同样的握手、收发同样的帧。

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use i3t_proto::NodeId;
use i3t_underlay::tcp_leg::{self, FrameReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::relay::{Inner, Source, Via};
use crate::session::SESSION_TTL;

/// 连上来之后多久之内必须握手成功。没有会话的 TCP 不值得留着。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// 同时最多接多少条 TCP。每个走 TCP 的节点只有一条，这个数对个人用的网络绰绰有余；
/// 它挡的是有人开一大堆连接又不握手。
const MAX_LEGS: usize = 1024;

/// 在 UDP 端口的同一个地址和端口号上监听 TCP。
///
/// # Errors
///
/// 绑不上（端口被别的程序占着）。调用方稍后再试。
pub(crate) async fn listen(inner: &Arc<Inner>) -> io::Result<JoinHandle<()>> {
    let addr = inner.port.local_addr();
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "relay 开始监听 TCP 中转");
    Ok(tokio::spawn(accept(Arc::clone(inner), listener)))
}

async fn accept(inner: Arc<Inner>, listener: TcpListener) {
    let slots = Arc::new(Semaphore::new(MAX_LEGS));
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
            tracing::warn!(from = %addr, "TCP 中转的连接数到了上限 {MAX_LEGS}，拒绝新的连接");
            continue;
        };
        let inner = Arc::clone(&inner);
        tokio::spawn(async move {
            let reason = serve(&inner, stream, addr).await;
            tracing::debug!(from = %addr, reason, "一条 TCP 中转结束");
            drop(slot);
        });
    }
}

/// 服务一条 TCP，直到它断开、超时或者 relay 被停用。返回结束的原因，给日志用。
async fn serve(inner: &Inner, stream: TcpStream, addr: SocketAddr) -> &'static str {
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

    let reason = loop {
        // 有会话时按会话的有效期等：节点的 PING 也从这条 TCP 来，这么久没有任何帧说明它不在了。
        let has_session = owner.is_some_and(|node| inner.tcp_session_alive(node, conn));
        let patience = if has_session { SESSION_TTL } else { HANDSHAKE_TIMEOUT };
        tokio::select! {
            _ = enabled.wait_for(|enabled| !enabled) => break "relay 停用",
            frame = tokio::time::timeout(patience, reader.next()) => match frame {
                Ok(Ok(Some(frame))) => {
                    if let Some(node) = inner.handle_tcp_frame(&source, &frame) {
                        owner = Some(node);
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
