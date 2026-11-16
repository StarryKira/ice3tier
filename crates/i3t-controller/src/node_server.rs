//! 节点协议的服务端：TCP 上的 TLS 1.3，双方按公钥互相认证，上面是分帧的 JSON 消息。
//!
//! 这里只管连接：握手、读写、心跳、超时。每条消息怎么处理在 [`Controller`] 里。
//!
//! 一条连接上的顺序：节点先发 `Enroll` 或 `Hello`；controller 回 `Snapshot`，不认它时回 `Rejected` 然后断开。
//! 之后节点上报、确认、发信令，controller 下发 netmap、转信令。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use i3t_crypto::tls;
use i3t_model::control::{MAX_FRAME_LEN, ToController, ToNode};
use i3t_proto::PublicKey;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use crate::controller::{Admitted, Controller};
use crate::error::{Rejection, error_chain};
use crate::frame::{FrameError, read_frame, write_frame};
use crate::server::blocking;

/// 还没报到的连接上，一帧最长这么多字节。`Enroll` 和 `Hello` 都只有几百字节。
const FIRST_FRAME_LIMIT: usize = 64 * 1024;
/// 同时最多有这么多条还没报到的连接。再多的直接关掉：正常的节点会重试，扫描和洪泛占不住资源。
const MAX_PENDING_CONNECTIONS: usize = 256;
/// 每个节点的发送队列有多少格。满了说明节点不读，controller 会断开它。
const OUTBOX_CAPACITY: usize = 64;
/// 一帧写这么久还没写完，就认为连接已经死了。
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// 关闭时最多等连接收尾这么久。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

type Reader = ReadHalf<TlsStream<TcpStream>>;
type Writer = WriteHalf<TlsStream<TcpStream>>;

/// 在 `listener` 上接受节点的连接，直到 `shutdown` 变化。`tls` 来自 [`Controller::tls_config`]。
pub async fn serve(
    controller: Arc<Controller>,
    tls: Arc<rustls::ServerConfig>,
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) {
    let acceptor = TlsAcceptor::from(tls);
    let pending = Arc::new(Semaphore::new(MAX_PENDING_CONNECTIONS));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, remote)) => {
                    let Ok(permit) = Arc::clone(&pending).try_acquire_owned() else {
                        tracing::warn!(%remote, "拒绝连接：还没报到的连接太多");
                        continue;
                    };
                    let connection = Connection { controller: Arc::clone(&controller), remote, shutdown: shutdown.clone() };
                    let acceptor = acceptor.clone();
                    connections.spawn(async move {
                        let Some(established) = connection.establish(&acceptor, stream).await else { return };
                        drop(permit);
                        connection.run(established).await;
                    });
                }
                Err(err) => {
                    // 多半是文件描述符用完了。歇一下再接，免得空转。
                    tracing::warn!("接受连接失败：{err}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = shutdown.changed() => break,
        }
    }
    drop(listener);
    // 每条连接自己也在看 shutdown：它们会把节点记为离线再退出。
    if timeout(SHUTDOWN_GRACE, async { while connections.join_next().await.is_some() {} }).await.is_err() {
        tracing::warn!("有连接没有在 {SHUTDOWN_GRACE:?} 之内结束，不再等它们");
    }
}

struct Connection {
    controller: Arc<Controller>,
    remote: SocketAddr,
    shutdown: watch::Receiver<bool>,
}

/// 一条已经报到的连接。
struct Established {
    session: Admitted,
    reader: Reader,
    writer: Writer,
    outbox: mpsc::Receiver<ToNode>,
}

impl Connection {
    /// 握手，读第一条消息，让 controller 决定认不认这个节点。不认时回 `Rejected` 并返回 `None`。
    async fn establish(&self, acceptor: &TlsAcceptor, stream: TcpStream) -> Option<Established> {
        let remote = self.remote;
        let handshake = timeout(self.controller.timing().handshake_timeout, handshake(acceptor, stream));
        let (key, first, reader, mut writer) = match handshake.await {
            Ok(Ok(parts)) => parts,
            Ok(Err(reason)) => {
                tracing::debug!(%remote, %reason, "连接没有完成报到");
                return None;
            }
            Err(_) => {
                tracing::debug!(%remote, "连接没有在限定时间内完成握手并发来第一条消息");
                return None;
            }
        };

        let (outbox, inbox) = mpsc::channel(OUTBOX_CAPACITY);
        let ip = remote.ip();
        let admitted = blocking(&self.controller, move |controller| match first {
            ToController::Enroll { token, hello } => controller.enroll(key, &token, &hello, ip, outbox),
            ToController::Hello(hello) => controller.hello(key, &hello, ip, outbox),
            ToController::Report(_) | ToController::Ack { .. } | ToController::Signal { .. } | ToController::Heartbeat => {
                tracing::warn!(key = ?key, remote = %ip, "拒绝连接：第一条消息不是 enroll 也不是 hello");
                Err(Rejection::UnexpectedFirstMessage)
            }
        })
        .await;
        match admitted {
            Ok(session) => Some(Established { session, reader, writer, outbox: inbox }),
            Err(rejection) => {
                let reply = ToNode::Rejected { reason: rejection.to_string() };
                if let Ok(Err(err)) = timeout(WRITE_TIMEOUT, write_frame(&mut writer, &reply)).await {
                    tracing::debug!(%remote, "拒绝的原因没能发给对方：{}", error_chain(&err));
                }
                close(&mut writer).await;
                None
            }
        }
    }

    async fn run(mut self, established: Established) {
        let Established { session, reader, mut writer, mut outbox } = established;
        let timing = self.controller.timing();
        let ip = self.remote.ip();
        let (mut inbound, reader_task) = spawn_reader(reader);
        let mut last_heard = Instant::now();
        let mut last_sent = Instant::now();

        let reason = loop {
            tokio::select! {
                received = inbound.recv() => match received {
                    Some(Ok(message)) => {
                        last_heard = Instant::now();
                        let handled = blocking(&self.controller, move |controller| controller.handle(session, ip, message)).await;
                        if let Err(err) = handled {
                            tracing::error!(node = %session.node, "处理节点的消息时出错：{}", error_chain(&err));
                        }
                    }
                    // 多半是更新版本的节点发来的、这个版本不认识的消息。跳过它，连接接着用。
                    Some(Err(FrameError::Json(err))) => {
                        last_heard = Instant::now();
                        tracing::debug!(node = %session.node, "忽略一条解析不了的消息：{err}");
                    }
                    Some(Err(err)) => break format!("读取失败：{}", error_chain(&err)),
                    None => break "节点关闭了连接".to_owned(),
                },
                outgoing = outbox.recv() => match outgoing {
                    Some(message) => match timeout(WRITE_TIMEOUT, write_frame(&mut writer, &message)).await {
                        Ok(Ok(())) => last_sent = Instant::now(),
                        Ok(Err(err)) => break format!("写入失败：{}", error_chain(&err)),
                        Err(_) => break format!("一条消息 {WRITE_TIMEOUT:?} 没写完"),
                    },
                    // 发送队列的另一头没了：这条连接被同一个节点的新连接顶替，或者节点被删除（Revoked 已经在上面发出去了）。
                    None => break "controller 结束了这条连接".to_owned(),
                },
                () = sleep_until(last_sent + timing.heartbeat_idle) => {
                    match timeout(WRITE_TIMEOUT, write_frame(&mut writer, &ToNode::Heartbeat)).await {
                        Ok(Ok(())) => last_sent = Instant::now(),
                        _ => break "心跳发不出去".to_owned(),
                    }
                }
                () = sleep_until(last_heard + timing.liveness_timeout) => {
                    break format!("{:?} 没有收到节点的任何消息", timing.liveness_timeout);
                }
                _ = self.shutdown.changed() => break "controller 正在关闭".to_owned(),
            }
        };

        tracing::info!(node = %session.node, remote = %self.remote, %reason, "和节点的连接结束");
        reader_task.abort();
        blocking(&self.controller, move |controller| controller.disconnect(session)).await;
        close(&mut writer).await;
    }
}

/// TLS 握手，取出对端的公钥，读第一条消息。失败时返回给日志看的原因。
async fn handshake(acceptor: &TlsAcceptor, stream: TcpStream) -> Result<(PublicKey, ToController, Reader, Writer), String> {
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!("设置 TCP_NODELAY 失败：{err}");
    }
    let stream = acceptor.accept(stream).await.map_err(|err| format!("TLS 握手失败：{err}"))?;
    let key = {
        let (_, session) = stream.get_ref();
        let certificate = session.peer_certificates().and_then(|chain| chain.first()).ok_or("对端没有出示证书")?;
        tls::peer_key(certificate).map_err(|err| format!("对端的证书不能用：{err}"))?
    };
    let (mut reader, writer) = tokio::io::split(stream);
    match read_frame(&mut reader, FIRST_FRAME_LIMIT).await {
        Ok(Some(first)) => Ok((key, first, reader, writer)),
        Ok(None) => Err("对端没有发消息就关闭了连接".to_owned()),
        Err(err) => Err(format!("第一条消息读不出来：{}", error_chain(&err))),
    }
}

/// 读取任务：把收到的帧一条条放进队列。读一帧不能被打断（长度和内容分两次读），所以不能直接放在 `select!` 里。
/// 连接断开或者出了解析之外的错，放进最后一个结果然后退出。
fn spawn_reader(mut reader: Reader) -> (mpsc::Receiver<Result<ToController, FrameError>>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(16);
    let task = tokio::spawn(async move {
        loop {
            let frame = match read_frame(&mut reader, MAX_FRAME_LEN).await {
                Ok(Some(message)) => Ok(message),
                Ok(None) => return,
                Err(err) => Err(err),
            };
            let fatal = matches!(frame, Err(FrameError::Io(_) | FrameError::TooLong { .. }));
            if tx.send(frame).await.is_err() || fatal {
                return;
            }
        }
    });
    (rx, task)
}

/// 尽量体面地关掉连接。对方不配合也不多等。
async fn close(writer: &mut Writer) {
    // 关不关得干净不影响任何状态：连接反正要丢弃了。
    let _ = timeout(Duration::from_secs(1), writer.shutdown()).await;
}
