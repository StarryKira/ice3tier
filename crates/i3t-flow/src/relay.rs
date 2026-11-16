//! 在一条流和一条 TCP 连接之间双向搬运。
//!
//! 两个方向各自独立，各有正常和异常两种结局，搬运要把它们如实传到另一头：
//!
//! | 这一头发生了什么 | 另一头看到什么 |
//! |---|---|
//! | TCP 读到 EOF（对方发了 FIN） | 流的发送方向正常结束 |
//! | 流读到 EOF | TCP 发 FIN（只关写的一半，另一个方向照常搬运） |
//! | TCP 读写出错（被重置、保活超时） | 流的两个方向都异常结束，带原因码 |
//! | 流被对端重置或叫停、连接断开、搬运被取消 | TCP 被重置（RST），不是 FIN |
//!
//! 被截断的数据不能看起来像正常结束。所以搬运一开始就把 TCP 设成"关闭时发 RST"，只有两个方向都正常结束了才改回去：
//! 中途不管是出错、任务被取消还是进程退出，TCP 那一头看到的都是重置。
//!
//! 没有空闲超时。一条静默的流可以一直留着，它的死活由连接层的保活和 TCP 的内核保活判断。
//!
//! 流往对端节点发数据的优先级跟着服务等级（[`priority_of`]）：同一条连接上，interactive 的流先于 bulk 的流发出。
//! `auto` 的流开始时按 interactive 对待，这一端在它上面发出的数据累计超过 [`AUTO_DEMOTE_AFTER`] 之后降到 bulk 的优先级。
//! 优先级只管本端发出去的方向，所以两端各数各的：下载时降级的是出口那一端，上传时是入口这一端。

use std::fmt;
use std::future::Future;
use std::io;
use std::time::{Duration, Instant};

use i3t_conn::contract::Priority;
use i3t_conn::{RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_proto::flow::{Class, Reason, Reply};
use socket2::SockRef;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{ReadHalf, WriteHalf};

use crate::{FlowError, header};

/// 每个方向一块这么大的缓冲区。流的数量可以上万，所以不取更大的。
const BUFFER_LEN: usize = 16 * 1024;

/// `auto` 等级的流，这一端在它上面发出多少字节之后降到 bulk 的优先级。
pub(crate) const AUTO_DEMOTE_AFTER: u64 = 1024 * 1024;

/// 一个服务等级的流刚建立时，往对端发数据用什么优先级。
pub(crate) const fn priority_of(class: Class) -> Priority {
    match class {
        Class::Interactive | Class::Auto => Priority::High,
        Class::Reliable => Priority::Normal,
        Class::Bulk => Priority::Low,
    }
}

/// TCP 那一头是谁。决定它出错时告诉对端节点的原因码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TcpRole {
    /// 入口一侧：TCP 那一头是客户端。
    Client,
    /// 出口一侧：TCP 那一头是目标。
    Target,
}

/// 一条流的两个方向，加上搬运时需要知道的事。
pub(crate) struct Parts<S, R> {
    pub send: S,
    pub recv: R,
    pub role: TcpRole,
    /// 服务等级。发送方向的优先级在建流时已经按它设好了，这里用它判断要不要中途降级。
    pub class: Class,
    /// 首字节时间从这个时刻算起。
    pub started: Instant,
    /// 流上的 REPLY 还没有读（乐观回复）。
    pub reply_pending: bool,
}

/// 一条两边都同意了的流：REPLY 已经发出（接受方）或者已经读到并且是成功（发起方），接下来只剩搬运。
pub struct Flow<S, R> {
    parts: Parts<S, R>,
}

/// 契约没有要求流实现 `Debug`，所以这里不打印它们。
impl<S, R> fmt::Debug for Flow<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flow").field("role", &self.parts.role).finish_non_exhaustive()
    }
}

impl<S: SendStream, R: RecvStream> Flow<S, R> {
    pub(crate) fn new(parts: Parts<S, R>) -> Self {
        Self { parts }
    }

    /// 在这条流和 `tcp` 之间搬运，直到两个方向都结束，或者有一头异常结束。
    ///
    /// `tcp` 是入口一侧的客户端连接，或者出口一侧到目标的连接。`cancel` 完成时搬运立刻停止：
    /// 流用它给出的原因码重置，`tcp` 被重置。不需要取消时传 [`std::future::pending`]。
    ///
    /// 返回之后流和 `tcp` 都已经关闭。
    pub async fn relay(self, tcp: TcpStream, cancel: impl Future<Output = Reason>) -> Summary {
        run(self.parts, tcp, cancel).await
    }

    /// 不搬运了：两个方向都异常结束，带上 `reason`。
    pub fn abort(mut self, reason: Reason) {
        let code = StreamCode(reason.to_wire());
        self.parts.send.reset(code);
        self.parts.recv.stop(code);
    }
}

/// 一个方向上搬了多少。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Traffic {
    pub bytes: u64,
    /// 从流建立到这个方向上读到第一个字节过了多久；一个字节都没有时是 `None`。
    pub first_byte: Option<Duration>,
}

impl Traffic {
    fn arrived(&mut self, started: Instant) {
        self.first_byte.get_or_insert_with(|| started.elapsed());
    }

    fn delivered(&mut self, len: usize) {
        // usize 不会比 u64 宽，这里不会截断。
        self.bytes += len as u64;
    }
}

/// 一次搬运的结果。
#[derive(Debug)]
pub struct Summary {
    /// 从 TCP 读出来、写进流的。出口一侧这是目标发回来的数据，它的 `first_byte` 就是目标的首字节时间。
    pub from_tcp: Traffic,
    /// 从流读出来、写进 TCP 的。
    pub from_flow: Traffic,
    pub end: End,
}

/// 搬运是怎么结束的。
#[derive(Debug)]
#[non_exhaustive]
pub enum End {
    /// 两个方向都正常结束。
    Clean,
    /// TCP 那一头出错（被重置、保活超时等）。
    Tcp(io::Error),
    /// 对端节点重置或者叫停了这条流，里面有它给的原因码。
    Aborted(StreamAborted),
    /// 流出了别的错：和对端节点的连接断了。
    Lost(io::Error),
    /// 乐观回复时，对端的 REPLY 是拒绝。
    Refused(Reply),
    /// 乐观回复时，对端的 REPLY 读不出来。
    BadReply(FlowError),
    /// 调用者取消了搬运。
    Cancelled(Reason),
}

impl End {
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

/// 一个方向停下来的原因。
enum Abort {
    Tcp(io::Error),
    Flow(io::Error),
    Refused(Reply),
    BadReply(FlowError),
    Cancelled(Reason),
}

impl Abort {
    /// 流的发送方向被对端叫停了。
    fn is_stop(&self) -> bool {
        matches!(self, Self::Flow(error) if matches!(StreamAborted::of(error), Some(StreamAborted::Stopped(_))))
    }

    /// 这一端结束流时告诉对端节点的原因。
    fn reason(&self, role: TcpRole) -> Reason {
        match self {
            Self::Tcp(error) => match role {
                TcpRole::Client => Reason::ClientReset,
                TcpRole::Target if error.kind() == io::ErrorKind::ConnectionReset => Reason::TargetReset,
                TcpRole::Target => Reason::TargetError,
            },
            Self::BadReply(_) => Reason::ProtocolError,
            // 是对端先结束的，这里只是跟着放弃另一个方向。
            Self::Flow(_) | Self::Refused(_) => Reason::Cancelled,
            Self::Cancelled(reason) => *reason,
        }
    }
}

impl From<Abort> for End {
    fn from(abort: Abort) -> Self {
        match abort {
            Abort::Tcp(error) => Self::Tcp(error),
            Abort::Flow(error) => match StreamAborted::of(&error) {
                Some(aborted) => Self::Aborted(aborted),
                None => Self::Lost(error),
            },
            Abort::Refused(reply) => Self::Refused(reply),
            Abort::BadReply(error) => Self::BadReply(error),
            Abort::Cancelled(reason) => Self::Cancelled(reason),
        }
    }
}

pub(crate) async fn run<S: SendStream, R: RecvStream>(
    parts: Parts<S, R>,
    mut tcp: TcpStream,
    cancel: impl Future<Output = Reason>,
) -> Summary {
    let Parts { mut send, mut recv, role, class, started, reply_pending } = parts;
    // 先设成"关闭时发 RST"。这个选项只管 close，中途用 shutdown 发的 FIN 不受影响。
    if let Err(error) = tcp.set_zero_linger() {
        tracing::warn!(%error, "设置不了 SO_LINGER，搬运中断时 TCP 一侧可能看到正常结束");
    }

    let mut from_tcp = Traffic::default();
    let mut from_flow = Traffic::default();
    let outcome = {
        let (tcp_read, tcp_write) = tcp.split();
        let upstream = tcp_to_flow(tcp_read, &mut send, &mut from_tcp, started, class);
        let downstream = flow_to_tcp(&mut recv, tcp_write, &mut from_flow, started, reply_pending);
        drive(upstream, downstream, cancel).await
    };

    let end = match outcome {
        Ok(()) => {
            if let Err(error) = SockRef::from(&tcp).set_linger(None) {
                tracing::warn!(%error, "恢复不了 SO_LINGER，正常结束的连接会以 RST 关闭");
            }
            End::Clean
        }
        Err(abort) => {
            let code = StreamCode(abort.reason(role).to_wire());
            send.reset(code);
            recv.stop(code);
            End::from(abort)
        }
    };
    drop(tcp);
    Summary { from_tcp, from_flow, end }
}

/// 同时跑两个方向，决定整次搬运的结局。
///
/// 一个方向正常结束时，另一个方向继续（半关闭）。一个方向异常结束时，整次搬运立刻结束。
/// 例外是 `upstream` 被对端叫停：这只说明对端不再读了，原因在另一个方向上（对端拒绝时那里是 REPLY，
/// 对端异常结束时那里是重置），所以等 `downstream` 给出结局；它要是正常结束，才把"被叫停"当作结局。
async fn drive(
    upstream: impl Future<Output = Result<(), Abort>>,
    downstream: impl Future<Output = Result<(), Abort>>,
    cancel: impl Future<Output = Reason>,
) -> Result<(), Abort> {
    enum Upstream {
        Running,
        Finished,
        Stopped(Abort),
    }

    tokio::pin!(upstream, downstream, cancel);
    let mut up = Upstream::Running;
    let mut down_finished = false;
    loop {
        tokio::select! {
            biased;
            reason = &mut cancel => return Err(Abort::Cancelled(reason)),
            end = &mut downstream, if !down_finished => {
                end?;
                down_finished = true;
            }
            end = &mut upstream, if matches!(up, Upstream::Running) => {
                up = match end {
                    Ok(()) => Upstream::Finished,
                    Err(abort) if abort.is_stop() && !down_finished => Upstream::Stopped(abort),
                    Err(abort) => return Err(abort),
                };
            }
        }
        if down_finished {
            match up {
                Upstream::Running => {}
                Upstream::Finished => return Ok(()),
                Upstream::Stopped(abort) => return Err(abort),
            }
        }
    }
}

async fn tcp_to_flow<S: SendStream>(
    mut tcp: ReadHalf<'_>,
    send: &mut S,
    traffic: &mut Traffic,
    started: Instant,
    class: Class,
) -> Result<(), Abort> {
    let mut buf = vec![0; BUFFER_LEN];
    let mut demotable = class == Class::Auto;
    loop {
        let len = tcp.read(&mut buf).await.map_err(Abort::Tcp)?;
        if len == 0 {
            return send.shutdown().await.map_err(Abort::Flow);
        }
        traffic.arrived(started);
        send.write_all(&buf[..len]).await.map_err(Abort::Flow)?;
        traffic.delivered(len);
        if demotable && traffic.bytes > AUTO_DEMOTE_AFTER {
            // 这条流原来是在传大块的数据：往后让交互的流先走。
            send.set_priority(priority_of(Class::Bulk));
            demotable = false;
        }
    }
}

async fn flow_to_tcp<R: RecvStream>(
    recv: &mut R,
    mut tcp: WriteHalf<'_>,
    traffic: &mut Traffic,
    started: Instant,
    reply_pending: bool,
) -> Result<(), Abort> {
    if reply_pending {
        match header::read(recv, Reply::decode).await {
            Ok(reply) if reply.status.is_ok() => {}
            Ok(reply) => return Err(Abort::Refused(reply)),
            // 流本身出错（被重置、连接断开）和 REPLY 的内容不对是两回事。
            Err(FlowError::Read(error)) => return Err(Abort::Flow(error)),
            Err(error) => return Err(Abort::BadReply(error)),
        }
    }
    let mut buf = vec![0; BUFFER_LEN];
    loop {
        let len = recv.read(&mut buf).await.map_err(Abort::Flow)?;
        if len == 0 {
            return tcp.shutdown().await.map_err(Abort::Tcp);
        }
        traffic.arrived(started);
        tcp.write_all(&buf[..len]).await.map_err(Abort::Tcp)?;
        traffic.delivered(len);
    }
}
