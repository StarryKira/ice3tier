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

use std::future::Future;
use std::io;

use i3t_proto::NodeId;
use tokio::io::{AsyncRead, AsyncWrite};

/// 流异常结束时带给对端的原因码。各个值的含义由上层定义，这一层只负责送达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamCode(pub u32);

/// 一条流的发送方向。
pub trait SendStream: AsyncWrite + Unpin + Send + 'static {
    /// 异常结束这个方向。还没送达的数据可能被丢弃，对端的读会出错并拿到 `code`。
    /// 已经正常结束或者已经重置过的流上再调用没有效果。
    fn reset(&mut self, code: StreamCode);
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
