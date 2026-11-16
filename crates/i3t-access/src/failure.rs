//! 代理的客户端能看到哪几种失败。

use i3t_proto::flow::Status;

use crate::http::Response;
use crate::socks5::ReplyCode;

/// 一次代理请求失败时告诉客户端的原因。两种代理协议各自把它翻译成自己的错误码。
///
/// 被拒绝只有 [`Failure::Denied`] 这一种表现：节点不存在、访问规则不允许、对方没有启用出口、
/// 出站策略不允许，客户端看到的都一样，所以不能拿它来枚举节点。具体原因在日志里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    Denied,
    /// 域名不存在。
    HostUnreachable,
    /// 目标所在的网络或主机不可达。
    NetworkUnreachable,
    ConnectionRefused,
    /// 连接目标超时。
    TimedOut,
    /// 到不了出口节点，或者出口节点自己出了问题。
    ExitUnavailable,
    /// 凭据的并发连接数到了上限。
    TooManyConnections,
}

impl Failure {
    pub(crate) const fn socks5(self) -> ReplyCode {
        match self {
            Self::Denied => ReplyCode::NotAllowed,
            // SOCKS5 没有专门的超时码（0x06 是 TTL 耗尽），按惯例归到主机不可达。
            Self::HostUnreachable | Self::TimedOut => ReplyCode::HostUnreachable,
            Self::NetworkUnreachable => ReplyCode::NetworkUnreachable,
            Self::ConnectionRefused => ReplyCode::ConnectionRefused,
            Self::ExitUnavailable | Self::TooManyConnections => ReplyCode::GeneralFailure,
        }
    }

    pub(crate) const fn http(self) -> Response {
        match self {
            Self::Denied => Response::Forbidden,
            Self::HostUnreachable | Self::NetworkUnreachable | Self::ConnectionRefused => Response::BadGateway,
            Self::TimedOut => Response::GatewayTimeout,
            Self::ExitUnavailable => Response::ServiceUnavailable,
            Self::TooManyConnections => Response::TooManyRequests,
        }
    }
}

/// 对端（或者就地出站的拨号器）给出的失败状态怎么告诉客户端。
impl From<Status> for Failure {
    fn from(status: Status) -> Self {
        match status {
            Status::NameNotFound => Self::HostUnreachable,
            Status::ConnectionRefused => Self::ConnectionRefused,
            Status::Unreachable => Self::NetworkUnreachable,
            Status::TimedOut => Self::TimedOut,
            Status::AccessDenied | Status::NotEnabled | Status::PolicyDenied | Status::UnknownForward => Self::Denied,
            // 拒绝里写着"成功"是对端的错，和它别的内部错误一样对待。
            Status::Ok | Status::ResolverFailure | Status::InternalError => Self::ExitUnavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_refusal_looks_the_same_to_the_client() {
        for status in [Status::AccessDenied, Status::NotEnabled, Status::PolicyDenied, Status::UnknownForward] {
            let failure = Failure::from(status);
            assert_eq!(failure.socks5(), ReplyCode::NotAllowed, "{status:?}");
            assert_eq!(failure.http(), Response::Forbidden, "{status:?}");
        }
    }

    #[test]
    fn target_failures_keep_their_precise_codes() {
        assert_eq!(Failure::from(Status::ConnectionRefused).socks5(), ReplyCode::ConnectionRefused);
        assert_eq!(Failure::from(Status::NameNotFound).socks5(), ReplyCode::HostUnreachable);
        assert_eq!(Failure::from(Status::Unreachable).socks5(), ReplyCode::NetworkUnreachable);
        assert_eq!(Failure::from(Status::TimedOut).socks5(), ReplyCode::HostUnreachable);
        assert_eq!(Failure::from(Status::ConnectionRefused).http(), Response::BadGateway);
        assert_eq!(Failure::from(Status::TimedOut).http(), Response::GatewayTimeout);
    }

    #[test]
    fn failures_of_the_exit_are_not_blamed_on_the_target() {
        for status in [Status::ResolverFailure, Status::InternalError] {
            assert_eq!(Failure::from(status).socks5(), ReplyCode::GeneralFailure);
            assert_eq!(Failure::from(status).http(), Response::ServiceUnavailable);
        }
        assert_eq!(Failure::TooManyConnections.http(), Response::TooManyRequests);
        assert_eq!(Failure::TooManyConnections.socks5(), ReplyCode::GeneralFailure);
    }
}
