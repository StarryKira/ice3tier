//! 接受方：读 OPEN，交给处理者，回 REPLY。

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use i3t_conn::contract::{DatagramSender, Datagrams};
use i3t_conn::{Acceptor, Incoming, RecvStream, SendStream, StreamCode};
use i3t_proto::NodeId;
use i3t_proto::flow::{Open, OpenDatagrams, Opening, Reason, Recipient, Reply, Status};
use tokio::io::AsyncWriteExt;
use tokio::time;

use crate::datagram::{self, Mux};
use crate::relay::{self, Flow, Parts, TcpRole};
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

/// 对端开过来的一条流上读出来的 OPEN：它要一条流，还是要一个数据报上下文。
pub(crate) enum Arrival<S, R> {
    Stream(Request<S, R>),
    Datagrams(DatagramArrival<S, R>),
}

/// 一条读出了数据报上下文的 OPEN 的流。
pub(crate) struct DatagramArrival<S, R> {
    pub peer: NodeId,
    pub open: OpenDatagrams,
    pub send: S,
    pub recv: R,
}

impl<S: SendStream, R: RecvStream> Arrival<S, R> {
    /// 从对端开过来的流上读 OPEN，最多等 `deadline`。出错时流已经被重置，原因码是 [`Reason::ProtocolError`]。
    pub(crate) async fn read(incoming: Incoming<S, R>, deadline: Duration) -> Result<Self, FlowError> {
        let Incoming { peer, mut send, mut recv } = incoming;
        let outcome = time::timeout(deadline, header::read(&mut recv, Opening::decode)).await;
        match outcome.unwrap_or(Err(FlowError::Deadline(deadline))) {
            Ok(Opening::Stream(open)) => Ok(Self::Stream(Request { peer, open, send, recv, received: Instant::now() })),
            Ok(Opening::Datagrams(open)) => Ok(Self::Datagrams(DatagramArrival { peer, open, send, recv })),
            Err(error) => {
                let code = StreamCode(Reason::ProtocolError.to_wire());
                send.reset(code);
                recv.stop(code);
                Err(error)
            }
        }
    }
}

impl<S: SendStream, R: RecvStream> Request<S, R> {
    /// 从对端开过来的流上读 OPEN，最多等 `deadline`。
    ///
    /// # Errors
    ///
    /// OPEN 没有按时到、不合法，或者流先出了错；对端要的是数据报上下文时是 [`FlowError::NotAStream`]
    /// （要两种都接受的话用 [`serve`]）。这时流已经被重置，原因码是 [`Reason::ProtocolError`]。
    pub async fn read(incoming: Incoming<S, R>, deadline: Duration) -> Result<Self, FlowError> {
        match Arrival::read(incoming, deadline).await? {
            Arrival::Stream(request) => Ok(request),
            Arrival::Datagrams(mut arrival) => {
                let code = StreamCode(Reason::ProtocolError.to_wire());
                arrival.send.reset(code);
                arrival.recv.stop(code);
                Err(FlowError::NotAStream)
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
    /// 这一端往发起方发数据的优先级从这里起按 OPEN 里的服务等级来。
    ///
    /// # Errors
    ///
    /// REPLY 没有写出去（发起方已经放弃了这条流，或者连接断了）。
    pub async fn accept(mut self) -> Result<Flow<S, R>, FlowError> {
        let class = self.open.class;
        self.send.set_priority(relay::priority_of(class));
        let mut bytes = Vec::with_capacity(Reply::MAX_LEN);
        Reply::ok().encode(&mut bytes);
        self.send.write_all(&bytes).await.map_err(FlowError::Write)?;
        let (send, recv) = (self.send, self.recv);
        Ok(Flow::new(Parts { send, recv, role: TcpRole::Target, class, started: self.received, reply_pending: false }))
    }

    /// 拒绝：把 `reply` 发回去，然后结束这条流。
    ///
    /// 发送方向是正常结束的，这样 REPLY 一定送得到；接收方向叫停，让发起方别再发数据。
    /// 发起方已经不在了的话，REPLY 送不到也没有人需要它，所以这里不报错。
    pub async fn refuse(mut self, reply: &Reply) {
        refuse(self.peer, &mut self.send, &mut self.recv, reply).await;
    }
}

/// 把拒绝的 `reply` 发回去，然后结束这条流。流和数据报上下文的拒绝是同一个做法。
pub(crate) async fn refuse(peer: NodeId, send: &mut impl SendStream, recv: &mut impl RecvStream, reply: &Reply) {
    debug_assert!(!reply.status.is_ok(), "拒绝时不能回成功的状态");
    let mut bytes = Vec::with_capacity(Reply::MAX_LEN);
    reply.encode(&mut bytes);
    let sent = async {
        send.write_all(&bytes).await?;
        send.shutdown().await
    };
    if let Err(error) = sent.await {
        tracing::debug!(%peer, %error, "拒绝的 REPLY 没有送出去");
    }
    recv.stop(StreamCode(Reason::Cancelled.to_wire()));
}

/// 处理对端开过来的流。出口模块实现它。
pub trait Handler: Send + Sync + 'static {
    /// 处理一条流直到它结束。每条流在自己的任务里调用，互不阻塞。
    fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) -> impl Future<Output = ()> + Send;

    /// 处理对端要建立的一个数据报上下文，直到它结束。每个上下文在自己的任务里调用。
    ///
    /// 不处理数据报的实现不用写它：默认拒绝。
    fn handle_datagrams<S: SendStream, R: RecvStream, T: DatagramSender>(
        &self,
        request: datagram::Request<S, R, T>,
    ) -> impl Future<Output = ()> + Send {
        async move { request.refuse(&Reply::new(Status::NotEnabled, "这个节点不接受数据报上下文")).await }
    }
}

/// 接受对端开过来的流，读出 OPEN 之后交给 `handler`，直到 `acceptor` 关闭。
///
/// 流交给 [`Handler::handle`]，数据报上下文交给 [`Handler::handle_datagrams`]，各自一个任务。
/// 对端在上下文里发来的数据报也是在这里分给各个上下文的，所以同一个 `acceptor` 上只应该有一个 `serve` 在跑。
/// 这个函数返回之后，已经在处理的流不受影响，继续跑到自己结束；数据报上下文收不到新的数据报。
pub async fn serve<A: Acceptor + Datagrams, H: Handler>(acceptor: &A, handler: Arc<H>) {
    let mux = Mux::new(Recipient::Acceptor);
    let transport = acceptor.datagram_sender();
    let pump = mux.pump(acceptor);
    tokio::pin!(pump);
    let mut pumping = true;
    loop {
        let incoming = tokio::select! {
            incoming = acceptor.accept() => incoming,
            // 数据报那一边先关了：流照常接到 `acceptor` 自己关闭为止。
            () = &mut pump, if pumping => {
                pumping = false;
                continue;
            }
        };
        let Some(incoming) = incoming else { return };
        let (handler, mux, transport) = (Arc::clone(&handler), Arc::clone(&mux), transport.clone());
        tokio::spawn(async move {
            let peer = incoming.peer;
            match Arrival::read(incoming, OPEN_DEADLINE).await {
                Ok(Arrival::Stream(request)) => handler.handle(request).await,
                Ok(Arrival::Datagrams(arrival)) => handler.handle_datagrams(datagram::Request::new(arrival, mux, transport)).await,
                Err(error) => tracing::info!(%peer, ?error, "重置了一条流：没有读到合法的 OPEN"),
            }
        });
    }
}
