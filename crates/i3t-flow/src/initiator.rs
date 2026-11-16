//! 发起方：开流、发 OPEN、等 REPLY。

use std::fmt;
use std::future::{self, Future};
use std::time::Instant;

use i3t_conn::{Connector, RecvStream, SendStream, StreamCode};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Open, Reason, Reply};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::meter::{Meter, Unmetered};
use crate::relay::{self, Flow, Parts, Summary, TcpRole};
use crate::{FlowError, header};

/// 到 `peer` 开一条流，把 OPEN 和调用者的首批数据一次写出去。
///
/// 和 `peer` 之间已经有连接时不加任何往返：OPEN 和首批数据在同一次写里。
/// 这一端往对端发数据的优先级按 OPEN 里的服务等级来，首批数据也是。
///
/// 失败之后不会自己重试，调用者也不应该原样重放：OPEN 一旦发出，连接断了就无从知道对端有没有把首批数据交给目标，
/// 重放可能让一个非幂等的请求被执行两次。
///
/// # Errors
///
/// 开流失败（对端不可达、连接断开）返回 [`FlowError::Connect`]，头没有写出去返回 [`FlowError::Write`]。
pub async fn open<C: Connector>(
    connector: &C,
    peer: NodeId,
    request: &Open,
    first_data: &[u8],
) -> Result<Pending<C::Send, C::Recv>, FlowError> {
    let started = Instant::now();
    let (mut send, recv) = connector.open(peer).await?;
    send.set_priority(relay::priority_of(request.class));
    let mut bytes = Vec::with_capacity(Open::MAX_LEN + first_data.len());
    request.encode(&mut bytes);
    bytes.extend_from_slice(first_data);
    send.write_all(&bytes).await.map_err(FlowError::Write)?;
    Ok(Pending { send, recv, class: request.class, started })
}

/// 已经发出 OPEN、还没有读 REPLY 的流。
///
/// 接下来二选一：[`established`](Self::established) 等对端说连上了再往下走，客户端能拿到准确的错误；
/// [`relay`](Self::relay) 不等，省一个全程往返，代价是失败只能表现为连接被重置（乐观回复）。
pub struct Pending<S, R> {
    send: S,
    recv: R,
    class: Class,
    started: Instant,
}

/// 契约没有要求流实现 `Debug`，所以这里不打印它们。
impl<S, R> fmt::Debug for Pending<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending").field("started", &self.started).finish_non_exhaustive()
    }
}

impl<S: SendStream, R: RecvStream> Pending<S, R> {
    /// 等对端的 REPLY。
    ///
    /// 这里不设时限：对端连接目标有它自己的时限，连接断开时流会出错。
    ///
    /// # Errors
    ///
    /// 对端拒绝时返回 [`FlowError::Refused`]，里面是它给的状态和说明；REPLY 读不到或者不合法时返回对应的错误。
    /// 出错时这条流已经被放弃。
    pub async fn established(mut self) -> Result<Flow<S, R>, FlowError> {
        let failure = match header::read(&mut self.recv, Reply::decode).await {
            Ok(reply) if reply.status.is_ok() => return Ok(Flow::new(self.parts(false))),
            Ok(reply) => FlowError::Refused(reply),
            Err(error) => error,
        };
        let reason = match failure {
            FlowError::Malformed(_) => Reason::ProtocolError,
            _ => Reason::Cancelled,
        };
        self.send.reset(StreamCode(reason.to_wire()));
        self.recv.stop(StreamCode(reason.to_wire()));
        Err(failure)
    }

    /// 不等 REPLY，直接在这条流和客户端的连接之间搬运。
    ///
    /// REPLY 在往客户端搬运的那个方向上读：对端拒绝了，或者流出了别的问题，客户端的连接被重置（RST），
    /// 结果在 [`Summary::end`] 里。
    pub async fn relay(self, client: TcpStream) -> Summary {
        relay::run(self.parts(true), client, future::pending(), &Unmetered).await
    }

    /// 和 [`relay`](Self::relay) 一样，只是可以取消（`cancel` 完成时用它给的原因码结束），每一段数据写出去之前先过 `meter`。
    pub async fn relay_metered(self, client: TcpStream, cancel: impl Future<Output = Reason>, meter: &impl Meter) -> Summary {
        relay::run(self.parts(true), client, cancel, meter).await
    }

    fn parts(self, reply_pending: bool) -> Parts<S, R> {
        Parts { send: self.send, recv: self.recv, role: TcpRole::Client, class: self.class, started: self.started, reply_pending }
    }
}
