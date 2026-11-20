//! 端口转发的目标连不连得上。
//!
//! 看得到的是监听一侧：它开到目标节点的流被目标节点拒绝时，REPLY 里的状态就是原因（目标节点知道为什么：连接被拒绝、超时、
//! 不可达、这条转发没有下发给它），开不出流时是到目标节点不通。所以记在监听节点上，随它的上报（`ForwardReport`）到 controller。
//! 一条流成功了（目标节点回了成功、或者数据已经在往回走），记录就清掉。
//!
//! 监听在虚拟 IP 上的转发由出口模块接住（`i3t_exit`），它把同样的结果记到同一份里（[`ForwardHealth`] 是可以共用的）。
//! UDP 的转发只看得到会话开不开得起来：报文发出去之后目标有没有收到，UDP 本身不说。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use i3t_flow::{End, Summary};
use i3t_proto::flow::Status;

/// 端口转发的一条流在目标那里没有成功的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TargetCause {
    /// 目标地址拒绝了连接：那个端口上没有程序在听。
    Refused,
    /// 连目标地址超时。
    Timeout,
    /// 目标地址所在的网络或主机不可达。
    Unreachable,
    /// 目标节点没有这条转发：它没有声明可以作为端口转发的目标，或者它的配置还不是最新的。
    NotDeclared,
    /// 目标节点的访问规则不允许监听节点访问它。
    AccessDenied,
    /// 到目标节点开不出流：它不在线，或者没有路。
    NodeUnreachable,
    /// 别的原因（目标节点在排空、内部出错等），见监听节点的日志。
    Other,
}

impl TargetCause {
    /// 目标节点在 REPLY 里给的状态是哪一种原因。成功时是 `None`。
    #[must_use]
    pub const fn of(status: Status) -> Option<Self> {
        Some(match status {
            Status::Ok => return None,
            Status::ConnectionRefused => Self::Refused,
            Status::TimedOut => Self::Timeout,
            Status::Unreachable | Status::NameNotFound => Self::Unreachable,
            Status::UnknownForward => Self::NotDeclared,
            Status::AccessDenied => Self::AccessDenied,
            Status::NotEnabled
            | Status::PolicyDenied
            | Status::Draining
            | Status::QuotaExhausted
            | Status::RateLimited
            | Status::TooManyStreams
            | Status::ResolverFailure
            | Status::InternalError => Self::Other,
        })
    }

    /// 一条经过搬运（不等 REPLY）的流说明了什么：失败的原因，成功是 `Ok`，说明不了（比如客户端自己先断了）是 `None`。
    pub(crate) fn of_summary(summary: &Summary) -> Option<Result<(), Self>> {
        match &summary.end {
            End::Refused(reply) => Some(Err(Self::of(reply.status).unwrap_or(Self::Other))),
            End::BadReply(_) => Some(Err(Self::Other)),
            End::Clean => Some(Ok(())),
            // 对端发回了数据，说明 REPLY 是成功的；之后怎么结束和目标连不连得上无关。
            _ if summary.from_flow.first_byte.is_some() => Some(Ok(())),
            _ => None,
        }
    }
}

/// 一条转发最近一次在目标那里失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetFailure {
    pub cause: TargetCause,
    pub at: Instant,
}

/// 每条端口转发最近一次在目标那里的失败，成功一次就清掉。克隆出来的是同一份。
#[derive(Debug, Clone, Default)]
pub struct ForwardHealth(Arc<Mutex<HashMap<u32, TargetFailure>>>);

impl ForwardHealth {
    /// 记下编号是 `forward` 的转发的一条流的结果。
    pub fn record(&self, forward: u32, outcome: Result<(), TargetCause>) {
        let mut failures = self.0.lock().expect("forward health mutex poisoned");
        match outcome {
            Ok(()) => {
                if failures.remove(&forward).is_some() {
                    tracing::info!(forward, "端口转发的目标又连上了");
                }
            }
            Err(cause) => {
                if failures.insert(forward, TargetFailure { cause, at: Instant::now() }).is_none_or(|before| before.cause != cause) {
                    tracing::info!(forward, ?cause, "端口转发的目标连不上");
                }
            }
        }
    }

    /// 这条转发最近一次失败；之后成功过或者从来没有失败过时是 `None`。
    #[must_use]
    pub fn last_failure(&self, forward: u32) -> Option<TargetFailure> {
        self.0.lock().expect("forward health mutex poisoned").get(&forward).copied()
    }

    /// 只留还在配置里的转发的记录。
    pub(crate) fn retain(&self, forwards: impl Fn(u32) -> bool) {
        self.0.lock().expect("forward health mutex poisoned").retain(|id, _| forwards(*id));
    }
}

#[cfg(test)]
mod tests {
    use i3t_flow::Traffic;
    use i3t_proto::flow::Reply;

    use super::*;

    #[test]
    fn a_failure_is_kept_until_a_stream_succeeds() {
        let health = ForwardHealth::default();
        assert_eq!(health.last_failure(3), None);
        health.record(3, Err(TargetCause::Refused));
        health.record(3, Err(TargetCause::Timeout));
        assert_eq!(health.last_failure(3).map(|failure| failure.cause), Some(TargetCause::Timeout), "最近的一次");
        assert_eq!(health.last_failure(4), None, "各条转发分开记");
        health.record(3, Ok(()));
        assert_eq!(health.last_failure(3), None);
    }

    #[test]
    fn the_status_of_the_reply_names_the_cause() {
        assert_eq!(TargetCause::of(Status::Ok), None);
        assert_eq!(TargetCause::of(Status::ConnectionRefused), Some(TargetCause::Refused));
        assert_eq!(TargetCause::of(Status::TimedOut), Some(TargetCause::Timeout));
        assert_eq!(TargetCause::of(Status::Unreachable), Some(TargetCause::Unreachable));
        assert_eq!(TargetCause::of(Status::UnknownForward), Some(TargetCause::NotDeclared));
        assert_eq!(TargetCause::of(Status::AccessDenied), Some(TargetCause::AccessDenied));
        assert_eq!(TargetCause::of(Status::Draining), Some(TargetCause::Other));
    }

    #[test]
    fn a_relayed_stream_tells_success_failure_or_nothing() {
        let summary = |end, first_byte| Summary { from_tcp: Traffic::default(), from_flow: Traffic { bytes: 0, first_byte }, end };
        let refused = summary(End::Refused(Reply::new(Status::ConnectionRefused, "")), None);
        assert_eq!(TargetCause::of_summary(&refused), Some(Err(TargetCause::Refused)));
        assert_eq!(TargetCause::of_summary(&summary(End::Clean, None)), Some(Ok(())));
        let answered = summary(End::Tcp(std::io::ErrorKind::ConnectionReset.into()), Some(std::time::Duration::from_millis(3)));
        assert_eq!(TargetCause::of_summary(&answered), Some(Ok(())));
        let client_left = summary(End::Tcp(std::io::ErrorKind::ConnectionReset.into()), None);
        assert_eq!(TargetCause::of_summary(&client_left), None);
    }
}
