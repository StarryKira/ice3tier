use std::io;
use std::time::Duration;

use i3t_conn::OpenError;
use i3t_proto::NodeId;

use crate::frame::FrameError;

/// 一条 lane 上出的错：握手、读帧、写帧。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LaneError {
    #[error("socket 出错")]
    Io(#[source] io::Error),
    #[error("TLS 出错")]
    Tls(#[source] rustls::Error),
    #[error("帧不合法")]
    Frame(#[source] FrameError),
    #[error("对端不守协议：{0}")]
    Protocol(&'static str),
    #[error("对端关闭了连接")]
    Closed,
    #[error("{0:?} 之内没有完成")]
    Deadline(Duration),
}

impl LaneError {
    /// 失败能不能归因于对端的公钥不被认可（未知或已吊销）。
    pub(crate) fn is_unknown_key(&self) -> bool {
        matches!(
            self,
            Self::Tls(rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure))
                | Self::Tls(rustls::Error::AlertReceived(rustls::AlertDescription::BadCertificate))
        )
    }
}

/// 在 lane 上开流失败的原因。调用方据此改走一对节点之间共用的连接：开流请求还没有发出去，换一条路不会重复执行它。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenLaneError {
    #[error("没有为节点 {0} 打开独占 TCP")]
    NotEnabled(NodeId),
    #[error("节点 {0} 的控制 lane 不在，现在没有它停在这里的 lane")]
    NoControl(NodeId),
    #[error("节点 {0} 要走了（排空），不再在和它之间的 lane 上开新的流")]
    GoingAway(NodeId),
    #[error("节点 {peer} 停在这里的 lane 用完了，等了 {waited:?} 也没有补上")]
    PoolDry { peer: NodeId, waited: Duration },
    #[error("和节点 {0} 之间在用的 lane 到了上限")]
    TooMany(NodeId),
    #[error("拨不通节点 {peer}")]
    Dial {
        peer: NodeId,
        #[source]
        source: LaneError,
    },
}

impl OpenLaneError {
    /// 换成连接层契约里的开流错误：通过 [`i3t_conn::Connector`] 用 lane 的调用方只看得到这些。
    #[must_use]
    pub fn to_open_error(&self) -> OpenError {
        match self {
            Self::NotEnabled(peer) | Self::NoControl(peer) | Self::GoingAway(peer) | Self::TooMany(peer) => OpenError::NoRoute(*peer),
            Self::PoolDry { peer, .. } => OpenError::Timeout(*peer),
            Self::Dial { peer, source } => OpenError::ConnectionLost { peer: *peer, reason: source.to_string() },
        }
    }
}
