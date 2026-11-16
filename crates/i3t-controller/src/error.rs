//! controller 的错误类型。

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use i3t_model::admin::{ErrorCode, Timestamp};
use ipnet::Ipv4Net;

use crate::store::StoreError;

/// 管理操作失败的原因。[`Error::code`] 是给程序看的分类，`Display` 是给人看的说明。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("没有这个{0}")]
    NotFound(Entity),
    #[error("{0}")]
    Invalid(String),
    #[error("名字 {0} 已经被占用")]
    NameTaken(String),
    #[error("名字 {name} 属于一个刚删除的节点，还在冷却期（到 Unix 时间 {until} 为止），现在不能分给别的节点")]
    NameCoolingDown { name: String, until: Timestamp },
    #[error("虚拟 IP {0} 已经分给了另一个节点")]
    AddressTaken(Ipv4Addr),
    #[error("虚拟 IP {address} 属于一个刚删除的节点，还在冷却期（到 Unix 时间 {until} 为止），现在不能分给别的节点")]
    AddressCoolingDown { address: Ipv4Addr, until: Timestamp },
    #[error("虚拟 IP {address} 不能用：它必须在地址段 {cidr} 里，并且不是网络地址和广播地址")]
    AddressOutOfRange { address: Ipv4Addr, cidr: Ipv4Net },
    #[error("地址段 {cidr} 和网络 {other} 的地址段 {other_cidr} 重叠")]
    RangeOverlaps { cidr: Ipv4Net, other: String, other_cidr: Ipv4Net },
    #[error("地址段 {0} 里没有空闲的地址了")]
    RangeExhausted(Ipv4Net),
    #[error("网络里已经有节点，地址段只能扩大：新的地址段必须包含原来的 {0}")]
    RangeNotSuperset(Ipv4Net),
    #[error("网络 {0} 里还有节点，先删掉节点才能删网络")]
    NetworkNotEmpty(String),
    #[error("节点 {node} 没有声明{capability}。在节点上执行 `{command}` 之后才能启用", command = capability.command())]
    NotDeclared { node: String, capability: Capability },
    #[error("节点 {0} 还在线。只有已经下线的节点才能被继任")]
    PredecessorOnline(String),
    #[error("{0}")]
    Conflict(String),
    #[error("节点 {0} 不在线。让节点立即更新要它连着 controller")]
    NodeOffline(String),
    #[error("节点 {node} 不能自己更新：{reason}")]
    CannotSelfUpdate { node: String, reason: String },
    #[error("{0}")]
    NoUpdate(String),
    #[error("存储出错")]
    Store(#[from] StoreError),
    #[error("netmap 序列化失败")]
    Encode(#[from] serde_json::Error),
    #[error("系统的随机数源不可用")]
    Random,
    #[error("读写 {path} 失败")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("controller 的密钥文件 {path} 读不出来")]
    Key {
        path: PathBuf,
        #[source]
        source: i3t_crypto::KeyError,
    },
    #[error("生成 controller 的密钥失败")]
    KeyGeneration(#[source] i3t_crypto::KeyError),
    #[error("准备 TLS 配置失败")]
    Tls(#[from] i3t_crypto::tls::TlsError),
    #[error("绑定 {address} 失败")]
    Bind {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("网络端口出错")]
    Socket(#[source] std::io::Error),
}

impl Error {
    /// 给程序看的分类，管理 API 的应答里用它。
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Invalid(_) => ErrorCode::InvalidRequest,
            Self::NameTaken(_) => ErrorCode::NameTaken,
            Self::NameCoolingDown { .. } => ErrorCode::NameCoolingDown,
            Self::AddressTaken(_) => ErrorCode::AddressTaken,
            Self::AddressCoolingDown { .. } => ErrorCode::AddressCoolingDown,
            Self::AddressOutOfRange { .. } => ErrorCode::AddressOutOfRange,
            Self::RangeOverlaps { .. } => ErrorCode::RangeOverlaps,
            Self::RangeExhausted(_) => ErrorCode::RangeExhausted,
            Self::RangeNotSuperset(_) => ErrorCode::RangeNotSuperset,
            Self::NetworkNotEmpty(_) => ErrorCode::NetworkNotEmpty,
            Self::NotDeclared { .. } => ErrorCode::NotDeclared,
            Self::PredecessorOnline(_) => ErrorCode::PredecessorOnline,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::NodeOffline(_) => ErrorCode::NodeOffline,
            Self::CannotSelfUpdate { .. } => ErrorCode::CannotSelfUpdate,
            Self::NoUpdate(_) => ErrorCode::NoUpdate,
            Self::Store(_)
            | Self::Encode(_)
            | Self::Random
            | Self::Io { .. }
            | Self::Key { .. }
            | Self::KeyGeneration(_)
            | Self::Tls(_)
            | Self::Bind { .. }
            | Self::Socket(_) => ErrorCode::Internal,
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// 找不到的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    Network,
    Node,
    Token,
    Rule,
    Credential,
    Listener,
    Forward,
    PairSetting,
    PausedVersion,
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Network => "网络",
            Self::Node => "节点",
            Self::Token => "注册令牌",
            Self::Rule => "规则",
            Self::Credential => "凭据",
            Self::Listener => "监听器",
            Self::Forward => "端口转发",
            Self::PairSetting => "节点对设置",
            Self::PausedVersion => "被暂停的版本",
        })
    }
}

/// 要节点先在本地声明、管理员才能启用的三种能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Relay,
    Exit,
    Forward,
}

impl Capability {
    /// 在节点上声明这项能力要执行的命令。
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Relay => "i3t set can-relay on",
            Self::Exit => "i3t set can-exit on",
            Self::Forward => "i3t set can-forward on",
        }
    }
}

/// 写成"可以作为……"的整句，直接接在"节点没有声明"后面。
impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Relay => "可以作为 relay",
            Self::Exit => "可以作为出口",
            Self::Forward => "可以作为端口转发的目标",
        })
    }
}

/// controller 不接受一个节点的原因。原样放进 `Rejected` 消息发给节点，所以不能带内部细节。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Rejection {
    #[error("注册令牌无效：它不存在、已经用过，或者已经过期")]
    BadToken,
    #[error("这把公钥没有在 controller 上注册。节点可能已经被删除，要重新加入网络")]
    UnknownKey,
    #[error("这把公钥已经注册过了。要重新加入，先在 controller 上删掉原来的节点")]
    KeyAlreadyEnrolled,
    #[error("要继任的节点还在线。等它下线之后再用这个令牌加入")]
    PredecessorOnline,
    #[error("令牌预设的虚拟 IP 现在不能用：{0}")]
    AddressUnavailable(String),
    #[error("网络的地址段里没有空闲的地址了")]
    RangeExhausted,
    #[error("连接上的第一条消息必须是 enroll 或者 hello")]
    UnexpectedFirstMessage,
    #[error("controller 内部出错，稍后再试")]
    Internal,
}

/// 把错误连同它的来源链写成一行，日志里用。
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push('：');
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_capability_names_the_command_to_run_on_the_node() {
        let cases = [
            (Capability::Exit, "i3t set can-exit on"),
            (Capability::Relay, "i3t set can-relay on"),
            (Capability::Forward, "i3t set can-forward on"),
        ];
        for (capability, command) in cases {
            let error = Error::NotDeclared { node: "nas-01".to_owned(), capability };
            let message = error.to_string();
            assert!(message.contains("nas-01") && message.contains(command), "{message}");
            assert_eq!(error.code(), ErrorCode::NotDeclared);
        }
    }
}
