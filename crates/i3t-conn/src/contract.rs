//! 端到端连接对上层的契约。
//!
//! 一条流是两个独立的方向：[`SendStream`] 和 [`RecvStream`]。每个方向有两种结束方式，上层必须能区分：
//!
//! | | 发送方做了什么 | 接收方看到什么 |
//! |---|---|---|
//! | 正常结束 | `AsyncWriteExt::shutdown`，写入的数据全部送达 | 读到 EOF（`read` 返回 0） |
//! | 异常结束 | [`SendStream::reset`]，或者没有 `shutdown` 就把流丢弃 | 读出错，[`StreamAborted::of`] 能取出原因码 |
//!
//! 被截断的数据不能表现成正常结束，所以**没有 `shutdown` 就丢弃发送方向等同于重置**。
//!
//! 接收方也可以主动叫停（[`RecvStream::stop`]）：发送方之后的写会出错，[`StreamAborted::of`] 同样能取出原因码。
//!
//! 流之外，一对节点之间还可以互相发数据报（[`Datagrams`]）：一份一份的，每份有长度上限。
//! 走 [`DatagramSender::send_datagram`] 的可能丢、可能乱序、不重传；走 [`DatagramSender::send_reliable`] 的不丢，
//! 但同样不保证先后。数据报只在已经有的连接上发，不会为它去建连接：上层总是先开一条流，再发和这条流有关的数据报。

use std::future::Future;
use std::io;

use bytes::Bytes;
use i3t_proto::NodeId;
use i3t_proto::flow::Recipient;
use tokio::io::{AsyncRead, AsyncWrite};

/// 流异常结束时带给对端的原因码。各个值的含义由上层定义，这一层只负责送达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamCode(pub u32);

/// 同一条连接上流与流之间的发送次序：连接一次发不完所有流的数据时，先发优先级高的流，同一级的轮流发。
///
/// 它只管本端往外发的方向，对端怎么排它自己的发送由对端决定。数据报不排在这里面，它们总是先于流的数据发出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Priority {
    /// 让位：别的流有数据要发时先发它们的。
    Low,
    #[default]
    Normal,
    /// 优先：先于别的流发出。
    High,
}

/// 一条流的发送方向。
pub trait SendStream: AsyncWrite + Unpin + Send + 'static {
    /// 异常结束这个方向。还没送达的数据可能被丢弃，对端的读会出错并拿到 `code`。
    /// 已经正常结束或者已经重置过的流上再调用没有效果。
    fn reset(&mut self, code: StreamCode);

    /// 改这个方向的发送优先级，随时可以改。新开的流是 [`Priority::Normal`]。
    ///
    /// 不分优先级的实现可以不管它，所以有一个什么都不做的默认实现。
    fn set_priority(&mut self, priority: Priority) {
        let _ = priority;
    }
}

/// 一条流的接收方向。
pub trait RecvStream: AsyncRead + Unpin + Send + 'static {
    /// 不再读了，请对端停止发送。对端之后的写会出错并拿到 `code`。
    fn stop(&mut self, code: StreamCode);
}

/// 流的某个方向被对端异常结束了。它被包在读写返回的 [`io::Error`] 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StreamAborted {
    /// 读的时候发现对端重置了发送方向。
    #[error("对端重置了这条流，原因码 {}", .0.0)]
    Reset(StreamCode),
    /// 写的时候发现对端叫停了接收方向。
    #[error("对端不再接收这条流，原因码 {}", .0.0)]
    Stopped(StreamCode),
}

impl StreamAborted {
    /// 从读写返回的错误里取出"被对端异常结束"这件事；别的错误（连接断开等）返回 `None`。
    #[must_use]
    pub fn of(error: &io::Error) -> Option<Self> {
        error.get_ref()?.downcast_ref::<Self>().copied()
    }

    #[must_use]
    pub const fn code(self) -> StreamCode {
        match self {
            Self::Reset(code) | Self::Stopped(code) => code,
        }
    }
}

impl From<StreamAborted> for io::Error {
    fn from(aborted: StreamAborted) -> Self {
        let kind = match aborted {
            StreamAborted::Reset(_) => io::ErrorKind::ConnectionReset,
            StreamAborted::Stopped(_) => io::ErrorKind::BrokenPipe,
        };
        Self::new(kind, aborted)
    }
}

/// 到对端开流。
pub trait Connector: Send + Sync + 'static {
    type Send: SendStream;
    type Recv: RecvStream;

    /// 到 `peer` 开一条新的流。和它之间还没有连接时先建连接（多一次握手往返）；已经有连接时不需要任何往返。
    fn open(&self, peer: NodeId) -> impl Future<Output = Result<(Self::Send, Self::Recv), OpenError>> + Send;
}

/// 接受对端开过来的流。
pub trait Acceptor: Send + Sync + 'static {
    type Send: SendStream;
    type Recv: RecvStream;

    /// 等下一条对端开过来的流。本端关闭之后返回 `None`。
    ///
    /// 对端是谁由连接的握手确定（按公钥认证），所以 [`Incoming::peer`] 伪造不了。
    fn accept(&self) -> impl Future<Output = Option<Incoming<Self::Send, Self::Recv>>> + Send;
}

/// 对端开过来的一条流。
#[derive(Debug)]
pub struct Incoming<S, R> {
    pub peer: NodeId,
    pub send: S,
    pub recv: R,
}

/// 一对节点之间的数据报。
///
/// 收和发分开：收的一方只有一个（[`recv_datagram`](Self::recv_datagram) 的每个 `recipient` 各有一个人在等），
/// 发的一方可以有很多，所以发送用的是一个可以克隆、可以带进任务里的句柄。
pub trait Datagrams: Send + Sync + 'static {
    type Sender: DatagramSender;

    /// 发数据报用的句柄。
    fn datagram_sender(&self) -> Self::Sender;

    /// 等下一个对端发来的、给 `recipient` 这一侧的数据报。本端关闭之后返回 `None`。
    ///
    /// 这一层不解释数据报的内容，只看首字节里上层写明的"给哪一侧"（[`Recipient::peek`]），看不出来的直接丢掉。
    /// 两侧各有一个有上限的队列：没有人来取、或者取得太慢时，新到的数据报被丢掉。
    /// 同一个 `recipient` 只应该有一个人在等。
    fn recv_datagram(&self, recipient: Recipient) -> impl Future<Output = Option<Datagram>> + Send;
}

/// 往对端发数据报。
pub trait DatagramSender: Clone + Send + Sync + 'static {
    /// 现在往 `peer` 发一个数据报最长可以多少字节。和它之间没有连接、或者它不接收数据报时返回 `None`。
    ///
    /// 这个值跟着连接探测到的路径 MTU 变，每次发之前都要问。它只限制 [`send_datagram`](Self::send_datagram)。
    fn max_datagram_len(&self, peer: NodeId) -> Option<usize>;

    /// 发一个不保证送达的数据报。不等待：连接来不及发时，排在最前面的旧数据报被丢掉，给新的让位。
    ///
    /// # Errors
    ///
    /// 和 `peer` 之间没有连接，或者数据报超过了 [`max_datagram_len`](Self::max_datagram_len)。
    fn send_datagram(&self, peer: NodeId, datagram: Bytes) -> Result<(), DatagramSendError>;

    /// 发一个保证送达的数据报：丢了由连接重传，长度不受 [`max_datagram_len`](Self::max_datagram_len) 限制。
    /// 每个数据报各走各的，一个被重传不耽误别的，所以到达的先后不保证。
    ///
    /// 返回时数据报已经交给连接，还不一定到了对端。对端来不及收时会在这里等，这就是反压。
    ///
    /// # Errors
    ///
    /// 和 `peer` 之间没有连接，或者连接在交出去之前断了。
    fn send_reliable(&self, peer: NodeId, datagram: Bytes) -> impl Future<Output = Result<(), DatagramSendError>> + Send;
}

/// 对端发来的一个数据报。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    /// 发它的节点。来自连接的握手，伪造不了。
    pub peer: NodeId,
    pub payload: Bytes,
}

/// 数据报没有发出去的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DatagramSendError {
    #[error("和节点 {0} 之间没有连接")]
    NotConnected(NodeId),
    #[error("节点 {0} 不接收数据报")]
    Unsupported(NodeId),
    #[error("数据报有 {len} 字节，这条连接现在最长只能发 {max} 字节")]
    TooLarge { len: usize, max: usize },
    #[error("和节点 {peer} 的连接断开了：{reason}")]
    ConnectionLost { peer: NodeId, reason: String },
}

/// 开流失败的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    #[error("节点 {0} 不在本节点的 netmap 里")]
    UnknownPeer(NodeId),
    #[error("到节点 {0} 没有可用的路径")]
    NoRoute(NodeId),
    #[error("和节点 {0} 建立连接超时")]
    Timeout(NodeId),
    #[error("和节点 {peer} 的连接断开了：{reason}")]
    ConnectionLost { peer: NodeId, reason: String },
    #[error("本端正在关闭")]
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_survives_the_trip_through_io_error() {
        let reset: io::Error = StreamAborted::Reset(StreamCode(7)).into();
        assert_eq!(reset.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(StreamAborted::of(&reset), Some(StreamAborted::Reset(StreamCode(7))));

        let stopped: io::Error = StreamAborted::Stopped(StreamCode(9)).into();
        assert_eq!(stopped.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(StreamAborted::of(&stopped).map(StreamAborted::code), Some(StreamCode(9)));

        assert_eq!(StreamAborted::of(&io::Error::other("连接断开")), None);
        assert_eq!(StreamAborted::of(&io::ErrorKind::UnexpectedEof.into()), None);
    }
}
