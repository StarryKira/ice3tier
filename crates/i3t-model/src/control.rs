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
//!
//! 新旧版本共存靠三条规矩：
//!
//! - 已有的消息只加字段，不改、不删；新加的字段都有默认值，收的一方忽略不认识的字段。
//! - 收到解析不了的一帧（多半是更新的版本才有的消息）就跳过它，连接接着用。
//! - 最初那一版的节点做不到上一条，收到不认识的消息会断开重连。所以 controller 只对 [`Hello::protocol`] 够新的节点
//!   发后来加的消息（见 [`PROTOCOL_REVISION`]）。
//!
//! 节点更新（白皮书 §10.5）在这条连接上的样子：
//!
//! ```text
//! controller → 节点   Update { 清单与签名, 时机 }
//! 节点 → controller   PackageRequest { 编号, 版本, 平台, 位置, 长度 }      一次一段，最多 MAX_PACKAGE_CHUNK 字节
//! controller → 节点   PackageChunk { 编号, 数据 } 或 PackageUnavailable { 编号, 原因 }
//! 节点 → controller   Report { …, update: 进行到哪一步 }                  每次变化都马上报一次
//! controller → 节点   UpdateCancel                                       还没换上去的话就不要换了
//! ```

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use i3t_proto::{NodeId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::netmap::{Attachment, Netmap};
use crate::update::{Platform, SelfUpdateBlocker, SignedManifest, UpdateStatus, UpdateTiming, Version};

/// 一帧最长多少字节。netmap 随对端数量增长，千级节点时在几百 KB。
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// 这个程序说的节点协议是第几版，节点在 [`Hello::protocol`] 里报给 controller。
///
/// - 0：最初的一版。收到不认识的消息会断开连接。
/// - 1：认识 [`ToNode::Update`]、[`ToNode::UpdateCancel`]、[`ToNode::PackageChunk`]、[`ToNode::PackageUnavailable`]、
///   [`ToNode::RouteQuery`]；从这一版起，解析不了的一帧只是被跳过。
pub const PROTOCOL_REVISION: u32 = 1;

/// 取更新包时一段最多多少字节。同一条连接上还有别的消息要走，所以不能大。
pub const MAX_PACKAGE_CHUNK: u32 = 256 * 1024;

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
    Enroll {
        token: String,
        hello: Hello,
    },
    Hello(Hello),
    Report(Report),
    /// 节点已经应用了这个版本的 netmap。
    Ack {
        version: u64,
    },
    /// 请 controller 把一条信令转给另一个节点。网络里没有 relay 时信令走这里。
    Signal {
        to: NodeId,
        signal: Signal,
    },
    Heartbeat,
    /// 取更新包里从 `offset` 开始的一段，最多 `len` 字节（不超过 [`MAX_PACKAGE_CHUNK`]）。
    /// controller 用带着同一个 `request` 的 [`ToNode::PackageChunk`] 或 [`ToNode::PackageUnavailable`] 回答。
    PackageRequest {
        /// 节点自己编的号，用来把回答对上。
        request: u64,
        version: Version,
        platform: Platform,
        offset: u64,
        len: u32,
    },
    /// 对 [`ToNode::RouteQuery`] 的回答。
    RouteAnswer {
        request: u64,
        answer: RouteAnswer,
    },
}

/// controller 发给节点的消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToNode {
    Snapshot(Box<Netmap>),
    Signal {
        from: NodeId,
        signal: Signal,
    },
    /// 节点被吊销：立即关闭全部连接和中转会话，不再重连。
    Revoked,
    /// controller 不接受这个节点（令牌无效、公钥没有注册等）。
    Rejected {
        reason: String,
    },
    Heartbeat,
    /// 更新到这份清单说的版本。节点自己验签，验不过就不装。
    ///
    /// 节点还在做上一次要求的更新时又收到一条，就按新的来（换了时机，或者有了更新的版本）；已经取过的包不会重新取。
    Update {
        manifest: SignedManifest,
        timing: UpdateTiming,
    },
    /// 上一次 [`ToNode::Update`] 要求的更新不要做了。程序还没有换上去的话节点就此停下；已经换了的不受影响。
    UpdateCancel,
    /// [`ToController::PackageRequest`] 要的那一段。可以比要的短，不会是空的。
    PackageChunk {
        request: u64,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    /// controller 给不了 [`ToController::PackageRequest`] 要的包（没有这个版本，或者没有这个平台的包）。再要也没有用。
    PackageUnavailable {
        request: u64,
        reason: String,
    },
    /// 问节点"到 `peer` 现在会怎么走"。节点用当前的测量值回答（[`ToController::RouteAnswer`]），不为此建立连接。
    RouteQuery {
        /// controller 编的号，回答里原样带回。
        request: u64,
        peer: NodeId,
    },
}

/// 二进制内容在 JSON 里写成不带填充的 base64url，和公钥、签名的写法一致。
mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = <std::borrow::Cow<'de, str> as Deserialize>::deserialize(deserializer)?;
        URL_SAFE_NO_PAD.decode(text.as_bytes()).map_err(serde::de::Error::custom)
    }
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
    /// 这台机器的主机名。节点第一次加入时 controller 用它起名字；取不到时是空的。
    #[serde(default)]
    pub hostname: String,
    /// 节点说的节点协议是第几版，见 [`PROTOCOL_REVISION`]。最初那一版的节点不报，读成 0。
    #[serde(default)]
    pub protocol: u32,
    /// 节点能不能自己更新（白皮书 §10.5）。不懂更新的旧版本节点不报，读成 `None`。
    #[serde(default)]
    pub self_update: Option<SelfUpdate>,
}

/// 节点能不能自己更新。不能的节点 controller 不会让它更新，界面上标出原因和手动更新的做法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfUpdate {
    pub able: bool,
    /// 不能的原因。比本程序新的节点报了一个这里不认识的原因时读成 `None`：只知道它不能。
    #[serde(default, deserialize_with = "known_blocker")]
    pub blocker: Option<SelfUpdateBlocker>,
}

impl SelfUpdate {
    pub const ABLE: Self = Self { able: true, blocker: None };
}

/// 节点启动时查出来的结果：有原因就是不能，没有就是能。
impl From<Option<SelfUpdateBlocker>> for SelfUpdate {
    fn from(blocker: Option<SelfUpdateBlocker>) -> Self {
        Self { able: blocker.is_none(), blocker }
    }
}

/// 不认识的原因读成 `None`，不让整条 `Hello` 因此解析失败。
fn known_blocker<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<SelfUpdateBlocker>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| serde_json::from_value(value).ok()))
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
    /// 到各个对端当前怎么走。`routes_full` 为假时只有和上次上报不同的。
    #[serde(default)]
    pub routes: Vec<RouteEntry>,
    /// `routes` 是不是完整的路由表。每次连上 controller 之后的第一次上报是完整的，controller 用它整个替换手上的那一份；
    /// 之后的上报只带变化的条目。
    #[serde(default)]
    pub routes_full: bool,
    /// 节点上的更新进行到哪一步（白皮书 §10.5）。每次上报都带着；一有变化节点就马上报一次，不等定时上报。
    /// 不懂更新的旧版本节点不报，读成 `None`。
    #[serde(default)]
    pub update: Option<UpdateStatus>,
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
    Direct {
        addr: SocketAddr,
    },
    /// 经 relay，按经过的顺序列出。
    Relay {
        relays: Vec<NodeId>,
    },
}

/// 节点对"到某个对端现在会怎么走"的回答，取自它的路由层当前的结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAnswer {
    pub peer: NodeId,
    /// 现在走的路。`None` 表示这个对端不在节点的 netmap 里（访问规则没有把它们连通），节点对它没有任何结论。
    pub route: Option<Route>,
    /// 这条路上最近测到的往返延迟。
    #[serde(default)]
    pub rtt_ms: Option<u32>,
    /// 这条路是什么时候、因为什么换上的。节点这次启动以来没有换过时是 `None`。
    #[serde(default)]
    pub since: Option<RouteSince>,
}

/// 一条路是多久之前换上的，以及换的原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSince {
    /// 多少秒之前。用间隔而不是时刻：节点和 controller 的时钟不一定对得上。
    pub secs_ago: u64,
    /// 路由层给的原因，给人看的。
    pub reason: String,
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
    fn a_hello_from_the_first_version_reads_as_protocol_zero_without_update_support() {
        let hello: Hello = serde_json::from_str(r#"{"version":"0.1.0","platform":"linux-x86_64","port":20330}"#).unwrap();
        assert_eq!((hello.protocol, hello.self_update, hello.hostname.as_str()), (0, None, ""));
    }

    #[test]
    fn self_update_has_a_stable_shape_and_tolerates_an_unknown_blocker() {
        let able = serde_json::to_value(SelfUpdate::ABLE).unwrap();
        assert_eq!(able, serde_json::json!({ "able": true, "blocker": null }));
        let blocked = SelfUpdate::from(Some(SelfUpdateBlocker::Container));
        assert_eq!(serde_json::to_value(blocked).unwrap(), serde_json::json!({ "able": false, "blocker": "container" }));
        assert_eq!(serde_json::from_value::<SelfUpdate>(serde_json::json!({ "able": false, "blocker": "container" })).unwrap(), blocked);

        let hello: Hello = serde_json::from_value(serde_json::json!({
            "version": "9.0.0", "platform": "linux-x86_64", "protocol": 7,
            "self_update": { "able": false, "blocker": "a_reason_added_later" }
        }))
        .unwrap();
        assert_eq!(hello.self_update, Some(SelfUpdate { able: false, blocker: None }), "只知道它不能，原因不认识");
        assert_eq!(hello.protocol, 7);
    }

    #[test]
    fn update_messages_have_stable_json_shapes() {
        let request = ToController::PackageRequest {
            request: 3,
            version: Version::new(0, 2, 0),
            platform: "linux-x86_64".parse().unwrap(),
            offset: 262_144,
            len: MAX_PACKAGE_CHUNK,
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({ "type": "package_request", "request": 3, "version": "0.2.0", "platform": "linux-x86_64", "offset": 262_144, "len": 262_144 })
        );
        let chunk = ToNode::PackageChunk { request: 3, data: vec![0xfb, 0xff, 0x00] };
        let wire = serde_json::to_value(&chunk).unwrap();
        assert_eq!(wire, serde_json::json!({ "type": "package_chunk", "request": 3, "data": "-_8A" }));
        assert_eq!(serde_json::from_value::<ToNode>(wire).unwrap(), chunk);
        assert_eq!(serde_json::to_value(ToNode::UpdateCancel).unwrap(), serde_json::json!({ "type": "update_cancel" }));

        let order: ToNode = serde_json::from_value(serde_json::json!({
            "type": "update", "timing": "after_drain", "manifest": { "manifest": "{}", "signature": "A".repeat(86) }
        }))
        .unwrap();
        assert!(matches!(order, ToNode::Update { timing: UpdateTiming::AfterDrain, .. }));
    }

    #[test]
    fn a_report_carries_the_update_status_and_says_whether_the_route_table_is_complete() {
        let old: Report = serde_json::from_str(r#"{"flows":2}"#).unwrap();
        assert_eq!((old.update.clone(), old.routes_full), (None, false));
        let report = Report { routes_full: true, update: Some(UpdateStatus::WaitingForIdle { version: Version::new(0, 2, 0) }), ..old };
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["update"], serde_json::json!({ "state": "waiting_for_idle", "version": "0.2.0" }));
        assert_eq!(wire["routes_full"], true);
        assert_eq!(serde_json::from_value::<Report>(wire).unwrap(), report);
    }

    #[test]
    fn route_queries_and_answers_have_stable_json_shapes() {
        let peer = NodeId::new(4).unwrap();
        let query = serde_json::to_value(ToNode::RouteQuery { request: 9, peer }).unwrap();
        assert_eq!(query, serde_json::json!({ "type": "route_query", "request": 9, "peer": 4 }));
        let answer = RouteAnswer {
            peer,
            route: Some(Route::Direct { addr: "1.2.3.4:20330".parse().unwrap() }),
            rtt_ms: Some(12),
            since: Some(RouteSince { secs_ago: 30, reason: "直连打通了".to_owned() }),
        };
        let wire = serde_json::to_value(ToController::RouteAnswer { request: 9, answer: answer.clone() }).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "type": "route_answer", "request": 9,
                "answer": { "peer": 4, "route": { "via": "direct", "addr": "1.2.3.4:20330" }, "rtt_ms": 12,
                            "since": { "secs_ago": 30, "reason": "直连打通了" } }
            })
        );
        let unknown: RouteAnswer = serde_json::from_value(serde_json::json!({ "peer": 4, "route": null })).unwrap();
        assert_eq!(unknown, RouteAnswer { peer, route: None, rtt_ms: None, since: None });
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
