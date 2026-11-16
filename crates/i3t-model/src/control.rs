//! 节点和 controller 之间的消息。
//!
//! 承载它们的是一条 TCP 上的 TLS 1.3 连接，双方按公钥互相认证。每条消息是一帧：4 字节大端长度，后面是一个 JSON 对象，
//! 对象里的 `type` 字段说明是哪一种消息。一帧最长 [`MAX_FRAME_LEN`] 字节。
//!
//! 一条连接上的顺序：
//!
//! 1. 节点先发 [`ToController::Enroll`]（第一次加入）或 [`ToController::Hello`]（已经注册过）。
//! 2. controller 回 [`ToNode::Snapshot`]；不认这个节点时回 [`ToNode::Rejected`] 然后断开。
//! 3. 之后节点定期发 [`ToController::Report`]，配置有变化时 controller 再发 `Snapshot`，节点应用之后回 [`ToController::Ack`]。
//!
//! 保活：节点每 10 秒发一次 `Report`，它同时就是心跳。controller 连续 20 秒没有东西要发时发一个 [`ToNode::Heartbeat`]。
//! controller 35 秒收不到节点的任何消息就把它记为离线；节点 90 秒收不到 controller 的任何消息就断开重连。

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use i3t_proto::{NodeId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::netmap::{Attachment, Netmap};

/// 一帧最长多少字节。netmap 随对端数量增长，千级节点时在几百 KB。
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// 注册令牌：管理员在 WebUI 上生成，交给要加入的那台机器（`i3t join` 从标准输入读它）。
///
/// 里面有三样东西：到哪里找 controller、controller 的公钥（节点靠它确认连上的是真的 controller）、
/// 一次性的密钥（controller 靠它确认这台机器是管理员放进来的）。文本形式是 `i3t-` 加上这三样的 base64url。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollToken {
    /// controller 的节点协议地址，`主机:端口`。主机可以是域名。
    #[serde(rename = "c")]
    pub controller: String,
    #[serde(rename = "k")]
    pub key: PublicKey,
    #[serde(rename = "s")]
    pub secret: String,
}

impl EnrollToken {
    const PREFIX: &str = "i3t-";
}

impl fmt::Display for EnrollToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json = serde_json::to_vec(self).map_err(|_| fmt::Error)?;
        write!(f, "{}{}", Self::PREFIX, URL_SAFE_NO_PAD.encode(json))
    }
}

/// 令牌里有一次性的密钥，不能进日志。
impl fmt::Debug for EnrollToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollToken").field("controller", &self.controller).field("key", &self.key).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("不是一个注册令牌")]
pub struct InvalidEnrollToken;

impl FromStr for EnrollToken {
    type Err = InvalidEnrollToken;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s.trim().strip_prefix(Self::PREFIX).ok_or(InvalidEnrollToken)?;
        let json = URL_SAFE_NO_PAD.decode(body).map_err(|_| InvalidEnrollToken)?;
        serde_json::from_slice(&json).map_err(|_| InvalidEnrollToken)
    }
}

/// 节点发给 controller 的消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToController {
    /// 用一次性的注册令牌加入网络。`token` 是令牌里的那个一次性密钥（[`EnrollToken::secret`]），不是整段令牌文本。
    /// 节点的公钥不在消息里：它来自这条连接的 TLS 握手。
    Enroll { token: String, hello: Hello },
    Hello(Hello),
    Report(Report),
    /// 节点已经应用了这个版本的 netmap。
    Ack { version: u64 },
    /// 请 controller 把一条信令转给另一个节点。网络里没有 relay 时信令走这里。
    Signal { to: NodeId, signal: Signal },
    Heartbeat,
}

/// controller 发给节点的消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToNode {
    Snapshot(Box<Netmap>),
    Signal { from: NodeId, signal: Signal },
    /// 节点被吊销：立即关闭全部连接和中转会话，不再重连。
    Revoked,
    /// controller 不接受这个节点（令牌无效、公钥没有注册等）。
    Rejected { reason: String },
    Heartbeat,
}

/// 节点每次连上 controller 时的自我介绍。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hello {
    /// 程序的版本号。
    pub version: String,
    /// 操作系统和架构，例如 `linux-x86_64`。
    pub platform: String,
    #[serde(default)]
    pub declared: Declared,
    /// 节点绑定的 UDP 端口。controller 把它和看到的来源 IP 拼成一个候选端点。
    #[serde(default)]
    pub port: u16,
}

/// 节点在本地声明的"可以承担"。声明本身不启用任何东西，启用由管理员在 WebUI 上决定；
/// 没有声明的能力 controller 启用不了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Declared {
    #[serde(default)]
    pub exit: bool,
    #[serde(default)]
    pub relay: bool,
    /// 可以被配成端口转发的目标。
    #[serde(default)]
    pub forward: bool,
}

/// 节点的定期上报，默认每 10 秒一次。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Report {
    /// 本节点当前的 UDP 候选端点：网卡地址、反射地址、端口映射。
    #[serde(default)]
    pub endpoints: Vec<SocketAddr>,
    /// 映射出来的端口是不是随对端变化。只有问过两个以上的观察点才知道；是的话这台机器所在的网络基本打不了洞。
    #[serde(default)]
    pub mapping_varies: Option<bool>,
    /// 本节点挂靠的 relay 和到它们的延迟。
    #[serde(default)]
    pub attached: Vec<Attachment>,
    /// 正在进行的流的数量。
    #[serde(default)]
    pub flows: u32,
    /// 启动以来经本节点的流发出和收到的字节数。
    #[serde(default)]
    pub tx_bytes: u64,
    #[serde(default)]
    pub rx_bytes: u64,
    /// 到各个对端当前怎么走，只报和上次不同的。
    #[serde(default)]
    pub routes: Vec<RouteEntry>,
}

/// 到一个对端当前走的路。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteEntry {
    pub peer: NodeId,
    pub route: Route,
    /// 这条路上最近测到的往返延迟。
    #[serde(default)]
    pub rtt_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case")]
pub enum Route {
    /// 没有可用的路。
    None,
    Direct { addr: SocketAddr },
    /// 经 relay，按经过的顺序列出。
    Relay { relays: Vec<NodeId> },
}

/// 节点之间的信令：建立直连之前要交换的东西。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Signal {
    /// 请对端一起打洞：这是我的候选端点，请你现在开始向它们发探测包，并把你的候选端点告诉我。
    PunchRequest { endpoints: Vec<SocketAddr> },
    /// 对打洞请求的回答。
    PunchReply { endpoints: Vec<SocketAddr> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enroll_token_round_trips_and_hides_its_secret() {
        let token = EnrollToken {
            controller: "ctl.example.com:20331".to_owned(),
            key: PublicKey::from_bytes([3; 32]),
            secret: "one-time-secret".to_owned(),
        };
        let text = token.to_string();
        assert!(text.starts_with("i3t-"));
        assert_eq!(format!("  {text}\n").parse::<EnrollToken>(), Ok(token.clone()));
        assert!(!format!("{token:?}").contains("one-time-secret"));
        for bad in ["", "i3t-", "i3t-!!!", "abc", "i3t-e30"] {
            assert_eq!(bad.parse::<EnrollToken>(), Err(InvalidEnrollToken), "{bad}");
        }
    }

    #[test]
    fn messages_carry_a_type_tag() {
        let ack = serde_json::to_value(ToController::Ack { version: 7 }).unwrap();
        assert_eq!(ack, serde_json::json!({ "type": "ack", "version": 7 }));
        let heartbeat = serde_json::to_value(ToNode::Heartbeat).unwrap();
        assert_eq!(heartbeat, serde_json::json!({ "type": "heartbeat" }));
    }

    #[test]
    fn hello_round_trips_and_tolerates_missing_fields() {
        let hello: ToController = serde_json::from_str(r#"{"type":"hello","version":"0.1.0","platform":"linux-x86_64"}"#).unwrap();
        let ToController::Hello(body) = &hello else { panic!("不是 hello：{hello:?}") };
        assert_eq!(body.declared, Declared::default());
        let again: ToController = serde_json::from_str(&serde_json::to_string(&hello).unwrap()).unwrap();
        assert_eq!(again, hello);
    }

    #[test]
    fn routes_and_signals_have_stable_json_shapes() {
        let relay = NodeId::new(9).unwrap();
        let route = serde_json::to_value(Route::Relay { relays: vec![relay] }).unwrap();
        assert_eq!(route, serde_json::json!({ "via": "relay", "relays": [9] }));
        let signal = serde_json::to_value(Signal::PunchRequest { endpoints: vec!["1.2.3.4:20330".parse().unwrap()] }).unwrap();
        assert_eq!(signal, serde_json::json!({ "kind": "punch_request", "endpoints": ["1.2.3.4:20330"] }));
    }
}
