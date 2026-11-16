//! 接受方：读 OPEN，交给处理者，回 REPLY。

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use i3t_conn::{Acceptor, Incoming, RecvStream, SendStream, StreamCode};
use i3t_proto::NodeId;
use i3t_proto::flow::{Open, Reason, Reply};
use tokio::io::AsyncWriteExt;
use tokio::time;

use crate::relay::{Flow, Parts, TcpRole};
use crate::{FlowError, header};

/// [`serve`] 等一条流的 OPEN 等多久。OPEN 是和流一起到的，正常情况下不用等；这个时限只是不让空流一直占着。
pub const OPEN_DEADLINE: Duration = Duration::from_secs(10);

/// 一条已经读出 OPEN、还没有应答的流。
///
/// 处理者要么 [`accept`](Self::accept)，要么 [`refuse`](Self::refuse)。什么都不做就丢弃，对端看到的是流被重置。
pub struct Request<S, R> {
    peer: NodeId,
    open: Open,
    send: S,
    recv: R,
    received: Instant,
}

/// 契约没有要求流实现 `Debug`，所以这里不打印它们。
impl<S, R> fmt::Debug for Request<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request").field("peer", &self.peer).field("open", &self.open).finish_non_exhaustive()
    }
}

impl<S: SendStream, R: RecvStream> Request<S, R> {
    /// 从对端开过来的流上读 OPEN，最多等 `deadline`。
    ///
    /// # Errors
    ///
    /// OPEN 没有按时到、不合法，或者流先出了错。这时流已经被重置，原因码是 [`Reason::ProtocolError`]。
    pub async fn read(incoming: Incoming<S, R>, deadline: Duration) -> Result<Self, FlowError> {
        let Incoming { peer, mut send, mut recv } = incoming;
        let outcome = time::timeout(deadline, header::read(&mut recv, Open::decode)).await;
        match outcome.unwrap_or(Err(FlowError::Deadline(deadline))) {
            Ok(open) => Ok(Self { peer, open, send, recv, received: Instant::now() }),
            Err(error) => {
                let code = StreamCode(Reason::ProtocolError.to_wire());
                send.reset(code);
                recv.stop(code);
                Err(error)
            }
        }
    }

    /// 发起这条流的节点。它来自连接的握手，伪造不了。
    #[must_use]
    pub fn peer(&self) -> NodeId {
        self.peer
    }

    #[must_use]
    pub fn open(&self) -> &Open {
        &self.open
    }

    /// OPEN 读完的时刻。[`Flow::relay`] 报告的首字节时间从这里算起，所以包含了连接目标花的时间。
    #[must_use]
    pub fn received(&self) -> Instant {
        self.received
    }

    /// 目标已经连上：回一个成功的 REPLY，之后可以搬运。
    ///
    /// # Errors
    ///
    /// REPLY 没有写出去（发起方已经放弃了这条流，或者连接断了）。
    pub async fn accept(mut self) -> Result<Flow<S, R>, FlowError> {
        let mut bytes = Vec::with_capacity(Reply::MAX_LEN);
        Reply::ok().encode(&mut bytes);
        self.send.write_all(&bytes).await.map_err(FlowError::Write)?;
        Ok(Flow::new(Parts { send: self.send, recv: self.recv, role: TcpRole::Target, started: self.received, reply_pending: false }))
    }

    /// 拒绝：把 `reply` 发回去，然后结束这条流。
    ///
    /// 发送方向是正常结束的，这样 REPLY 一定送得到；接收方向叫停，让发起方别再发数据。
    /// 发起方已经不在了的话，REPLY 送不到也没有人需要它，所以这里不报错。
    pub async fn refuse(mut self, reply: &Reply) {
        debug_assert!(!reply.status.is_ok(), "拒绝时不能回成功的状态");
        let mut bytes = Vec::with_capacity(Reply::MAX_LEN);
        reply.encode(&mut bytes);
        let sent = async {
            self.send.write_all(&bytes).await?;
            self.send.shutdown().await
        };
        if let Err(error) = sent.await {
            tracing::debug!(peer = %self.peer, %error, "拒绝的 REPLY 没有送出去");
        }
        self.recv.stop(StreamCode(Reason::Cancelled.to_wire()));
    }
}

/// 处理对端开过来的流。出口模块实现它。
pub trait Handler: Send + Sync + 'static {
    /// 处理一条流直到它结束。每条流在自己的任务里调用，互不阻塞。
    fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) -> impl Future<Output = ()> + Send;
}

/// 接受对端开过来的流，读出 OPEN 之后交给 `handler`，直到 `acceptor` 关闭。
///
/// 每条流一个任务。这个函数返回之后，已经在处理的流不受影响，继续跑到自己结束。
pub async fn serve<A: Acceptor, H: Handler>(acceptor: &A, handler: Arc<H>) {
    while let Some(incoming) = acceptor.accept().await {
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            let peer = incoming.peer;
            match Request::read(incoming, OPEN_DEADLINE).await {
                Ok(request) => handler.handle(request).await,
                Err(error) => tracing::info!(%peer, ?error, "重置了一条流：没有读到合法的 OPEN"),
            }
        });
    }
}
