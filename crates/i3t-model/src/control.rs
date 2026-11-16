//! 节点和 controller 之间的消息。
//!
//! 承载它们的是一条 TCP 上的 TLS 1.3 连接，双方按公钥互相认证。每条消息是一帧：4 字节大端长度，后面是一个 JSON 对象，
//! 对象里的 `type` 字段说明是哪一种消息。一帧最长 [`MAX_FRAME_LEN`] 字节。
//!
//! 一条连接上的顺序：
//!
//! 1. 节点先发 [`ToController::Enroll`]（第一次加入）或 [`ToController::Hello`]（已经注册过）。
//! 2. controller 回 [`ToNode::Snapshot`]；不认这个节点时回 [`ToNode::Rejected`] 然后断开；
//!    自己出了错、这一次接待不了时回 [`ToNode::TryLater`] 然后断开，节点按退避重连。
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
//!
//! 主动测试（白皮书 §7.6）在这条连接上的样子见 [`diag`]。

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use i3t_proto::{NodeId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::netmap::{Attachment, ForwardProtocol, ListenerProtocol, Netmap, RoutingMode};
use crate::update::{Platform, SelfUpdateBlocker, SignedManifest, UpdateStatus, UpdateTiming, Version};

pub mod diag;

pub use diag::{TestOrder, TestReport};

/// 一帧最长多少字节。netmap 随对端数量增长，千级节点时在几百 KB。
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// 这个程序说的节点协议是第几版，节点在 [`Hello::protocol`] 里报给 controller。
///
/// - 0：最初的一版。收到不认识的消息会断开连接。
/// - 1：认识 [`ToNode::Update`]、[`ToNode::UpdateCancel`]、[`ToNode::PackageChunk`]、[`ToNode::PackageUnavailable`]、
///   [`ToNode::RouteQuery`]；从这一版起，解析不了的一帧只是被跳过。
/// - 2：认识 [`ToNode::TryLater`]、[`ToNode::BackboneQuery`]；回答路由查询时带 [`RouteAnswer::detail`]，
///   上报时带 [`Report::route_kinds`]。
/// - 3：认识 netmap 里对端的 [`lane`](crate::netmap::Peer::lane)（独占 TCP），上报时带 [`Report::lanes`]。
///   比它旧的节点不懂独占 TCP，controller 据此说明节点对的设置为什么不起作用。
/// - 4：认识 [`ToNode::Test`]、[`ToNode::TestEnd`]，会回 [`ToController::TestReport`]（主动测试），
///   在 [`Hello::throughput_tests`] 里说本机接不接受吞吐测试。
pub const PROTOCOL_REVISION: u32 = 4;

/// 从这一版节点协议起节点懂独占 TCP。
pub const LANE_REVISION: u32 = 3;

/// 从这一版节点协议起节点懂主动测试。
pub const TEST_REVISION: u32 = 4;

/// 取更新包时一段最多多少字节。同一条连接上还有别的消息要走，所以不能大。
pub const MAX_PACKAGE_CHUNK: u32 = 256 * 1024;

/// 注册令牌：管理员在 WebUI 上生成，交给要加入的那台机器（`i3t join` 从标准输入读它）。
///
/// 里面有三样东西：到哪里找 controller、controller 的公钥（节点靠它确认连上的是真的 controller）、
/// 一次性的密钥（controller 靠它确认这台机器是管理员放进来的）。文本形式是 `i3t-` 加上这三样的 base64url。
///
/// 一次性密钥排在 JSON 的最前面：这样令牌文本的开头一小段（[`EnrollToken::VISIBLE_PREFIX_LEN`] 个字符）随令牌而不同，
/// 界面上可以拿它认出是哪一个令牌。字段按名字解析，先后不影响以前的版本读它。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollToken {
    #[serde(rename = "s")]
    pub secret: String,
    /// controller 的节点协议地址，`主机:端口`。主机可以是域名。
    #[serde(rename = "c")]
    pub controller: String,
    #[serde(rename = "k")]
    pub key: PublicKey,
}

impl EnrollToken {
    const PREFIX: &str = "i3t-";

    /// 令牌文本开头可以给人看的字符数：`i3t-`、`{"s":"` 的编码，加上露出密钥开头约 4 个字符的 6 个字符。
    /// 密钥有 256 位，露出的不到 30 位。
    pub const VISIBLE_PREFIX_LEN: usize = 18;
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
    /// 对 [`ToNode::BackboneQuery`] 的回答：节点手上骨干矩阵的每一行。
    BackboneAnswer {
        request: u64,
        #[serde(default)]
        rows: Vec<BackboneRow>,
    },
    /// 主动测试里这一端看到的结果（[`diag`]）。
    TestReport(Box<TestReport>),
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
    /// controller 这一次接待不了这个节点：是它自己出了错（例如存储暂时不可用），不是不认这个节点。
    /// 节点按退避重连，不能像 [`ToNode::Rejected`] 那样就此停下。
    ///
    /// controller 发完就关闭连接，所以不认识这条消息的旧节点同样会重连。
    TryLater {
        reason: String,
    },
    /// 问节点手上的 relay 骨干矩阵（白皮书 §7.7），节点用 [`ToController::BackboneAnswer`] 回答。
    /// 只发给 [`Hello::protocol`] 至少是 2 的节点。
    BackboneQuery {
        request: u64,
    },
    /// 一个主动测试的指令（[`diag`]）。只发给 [`Hello::protocol`] 至少是 [`TEST_REVISION`] 的节点。
    Test(Box<TestOrder>),
    /// 这个测试结束了（做完了，或者被取消了）：关掉它的时间窗，停下手上的事，还没报的马上报。
    TestEnd {
        test: u64,
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
    /// 节点接不接受吞吐测试（本机配置文件里的开关，默认接受）。只有节点协议至少是 [`TEST_REVISION`] 时才有意义。
    #[serde(default)]
    pub throughput_tests: bool,
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
    /// 这次运行的编号：节点进程每次启动时随机取一个，不是 0。下面几项里累计的数都从进程启动时算起，
    /// controller 靠它分辨"节点重启了，计数从头开始"和"计数还在往上涨"。旧版本的节点不报，读成 0。
    #[serde(default)]
    pub run: u64,
    /// 入口上各个接入凭据的用量。这次运行里没有用过的凭据不在里面。
    #[serde(default)]
    pub credentials: Vec<CredentialUsage>,
    /// 本节点替别人连目标（出口一侧）的计数，以及它自己的限制用到了哪里。旧版本的节点不报。
    /// 装在 `Box` 里只是为了让消息的枚举不至于太大，JSON 里没有区别。
    #[serde(default)]
    pub exit: Option<Box<ExitReport>>,
    /// 本节点作为发起方经各个出口出去的流：发起一方自己看到的字节数和失败，用来和出口的上报交叉核对。
    #[serde(default)]
    pub exits: Vec<ExitObservation>,
    /// 代理监听器的状态，包括节点按本机配置文件自己开的那几个。
    #[serde(default)]
    pub listeners: Vec<ListenerReport>,
    /// 本节点负责监听的端口转发的状态。
    #[serde(default)]
    pub forwards: Vec<ForwardReport>,
    /// `routes` 里各条路的细节（直连是怎么建立的、经 relay 时第一段走 UDP 还是 TCP），和 `routes` 是同一批对端：
    /// `routes` 只带变化的条目时这里也一样。节点说不出细节的对端不在里面。旧版本的节点不报。
    #[serde(default)]
    pub route_kinds: Vec<RouteKind>,
    /// 独占 TCP（lane）的状况。没有为任何对端打开时也报（全是 0）；旧版本的节点不报。
    #[serde(default)]
    pub lanes: Option<Box<LaneReport>>,
}

/// 一个节点上独占 TCP（lane）的状况。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LaneReport {
    /// 全节点的合计，从节点启动算起，对端去掉之后也不减。
    #[serde(default)]
    pub total: LaneCounts,
    /// 别人拨进来、没有握手成功的连接，按原因分。分不到对端名下，只有全节点的。
    #[serde(default)]
    pub handshake_failures: LaneHandshakeFailures,
    /// 每个打开了独占 TCP 的对端。
    #[serde(default)]
    pub peers: Vec<PeerLanes>,
}

/// 和一个对端之间的 lane。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLanes {
    pub peer: NodeId,
    /// 本节点拨不拨它（它的 TCP 端口本节点连得上，并且按设置现在要用）。
    #[serde(default)]
    pub dials: bool,
    /// 和它之间的控制 lane 在不在。不在时新的 TCP 流走共用的连接。
    #[serde(default)]
    pub control: bool,
    /// 它拨来、停在本节点等本节点开流时取用的空闲 lane。
    #[serde(default)]
    pub idle: u64,
    /// 本节点拨好、留给自己开流的空闲 lane。
    #[serde(default)]
    pub ready: u64,
    /// 本节点拨出去、停在它那里等它取用的 lane。
    #[serde(default)]
    pub parked_at_peer: u64,
    #[serde(default, flatten)]
    pub counts: LaneCounts,
}

/// 独占 TCP 的计数和水位。全节点的合计和每个对端的是同一组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LaneCounts {
    /// 正在承载流的 lane。
    #[serde(default)]
    pub in_use: u64,
    /// 正在拨号（连接或握手）的 lane。
    #[serde(default)]
    pub dialling: u64,
    /// 从对端停在本节点的 lane 里取用的次数。
    #[serde(default)]
    pub claims: u64,
    /// 其中池是空的、要等补拨的次数，和一共等了多少毫秒。
    #[serde(default)]
    pub claims_waited: u64,
    #[serde(default)]
    pub claim_wait_ms: u64,
    /// 取不到 lane 的次数（池空等满了、控制 lane 不在、对方要走了、当场拨也没拨通）。
    #[serde(default)]
    pub claims_failed: u64,
    /// 本来要走 lane、因为取不到改走了共用连接的流。用户看不出来，只在这里记着。
    #[serde(default)]
    pub fallbacks: u64,
    /// 本节点拨号失败的次数。
    #[serde(default)]
    pub dial_failures: u64,
    /// 握手成功、但因为用途或者数量不对被关掉的 lane（旧的 epoch、超过上限、没有为对方打开）。
    #[serde(default)]
    pub refused: u64,
    /// 在 lane 的 TLS 里收发的流数据字节数（两个方向合计）。
    #[serde(default)]
    pub sealed_bytes: u64,
    /// 不再加密的流在程序里搬的字节数。
    #[serde(default)]
    pub passthrough_bytes: u64,
    /// 不再加密的流在两条 TCP 之间直接搬（Linux 上是 splice）的字节数。
    #[serde(default)]
    pub spliced_bytes: u64,
}

/// 拨进来、没有握手成功的连接的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LaneHandshakeFailures {
    /// 没有在时限之内完成（包括连上来什么都不发的）。
    #[serde(default)]
    pub deadline: u64,
    /// 公钥不认识或者已经吊销。
    #[serde(default)]
    pub unknown_key: u64,
    /// 别的 TLS 错误。
    #[serde(default)]
    pub tls: u64,
    /// 握手之后的第一个帧不对，或者根本不是 TLS。
    #[serde(default)]
    pub protocol: u64,
    /// 还没握手完的连接太多，被挤掉的。
    #[serde(default)]
    pub busy: u64,
}

/// 一个接入凭据在一个入口上的用量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialUsage {
    /// 凭据在 controller 上的编号（netmap 里的 [`Credential::id`](crate::netmap::Credential::id)）。
    pub id: u32,
    /// 这次运行以来经这个凭据搬运的字节数：客户端发出的和收到的相加。
    pub bytes: u64,
    /// 现在正在进行的连接（UDP ASSOCIATE 也算一条）。
    #[serde(default)]
    pub connections: u32,
    /// 最近一次有连接用它是多少秒之前。用间隔而不是时刻：节点和 controller 的时钟不一定对得上。
    #[serde(default)]
    pub last_used_secs_ago: Option<u64>,
}

/// 出口一侧的计数。都是这次运行以来的累计，`active` 除外。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExitReport {
    /// 正在处理的流和数据报上下文。
    pub active: u64,
    /// 收到过的建流请求，包括数据报上下文的。
    pub total: u64,
    pub bytes_to_target: u64,
    pub bytes_from_target: u64,
    /// 没有建成的请求，按归因分：目标的问题、被规则或策略拒绝（含排空、配额、建连预算）、出口自己的问题。
    /// 只有最后一项说明出口不健康。
    pub failed_by_target: u64,
    pub refused: u64,
    pub failed_by_exit: u64,
    /// 建成之后没有正常结束的流和数据报上下文。
    pub aborted: u64,
    /// 节点正在排空，不接新的流。
    pub draining: bool,
    /// 配了流量配额时，本账期用到了哪里。
    pub quota: Option<QuotaUsage>,
}

/// 出口本账期的流量配额。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaUsage {
    /// 每个账期的配额，字节。
    pub limit_bytes: u64,
    /// 本账期已经用掉的，字节。可能略超过配额：超出的那一段流是在用完的那一刻被断开的。
    pub used_bytes: u64,
    /// 本账期的开始和结束，Unix 秒，按出口节点自己的时钟。
    pub period_start: u64,
    pub period_end: u64,
}

/// 发起方看到的、经一个出口出去的流。都是这次运行以来的累计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitObservation {
    pub exit: NodeId,
    /// 发往这个出口的建流请求，成功和失败的都算。
    #[serde(default)]
    pub flows: u64,
    /// 客户端经它发出的、收到的字节数。
    #[serde(default)]
    pub bytes_up: u64,
    #[serde(default)]
    pub bytes_down: u64,
    /// 出口回的拒绝，按归因分，和 [`ExitReport`] 里的分法相同。
    #[serde(default)]
    pub failed_by_target: u64,
    #[serde(default)]
    pub refused: u64,
    #[serde(default)]
    pub failed_by_exit: u64,
    /// 到出口开流就失败了（没有路、连接断了），出口根本没有看到这个请求。
    #[serde(default)]
    pub unreachable: u64,
}

/// 一个代理监听器现在的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerReport {
    pub name: String,
    /// 配置里写的绑定地址。
    pub bind: SocketAddr,
    pub protocol: ListenerProtocol,
    /// 节点按本机的配置文件自己开的回环入口（`local-socks5`、`local-http`），不是 controller 下发的。
    #[serde(default)]
    pub builtin: bool,
    pub state: ListenState,
    #[serde(flatten)]
    pub traffic: ListenTraffic,
}

/// 一条端口转发（监听一侧）现在的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardReport {
    pub id: u32,
    /// 配置里写的监听地址；端口写 0 时实际的端口在 [`ListenState::Listening`] 里。
    pub bind: SocketAddr,
    pub protocol: ForwardProtocol,
    pub state: ListenState,
    #[serde(flatten)]
    pub traffic: ListenTraffic,
}

/// 一个监听上的连接和流量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListenTraffic {
    /// 现在的连接数；UDP 的端口转发是会话数。
    pub connections: u32,
    /// 这次运行以来客户端发进来的字节数，和发回给客户端的字节数。
    pub bytes_up: u64,
    pub bytes_down: u64,
}

/// 一个监听现在的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ListenState {
    /// 正在监听。`local` 是实际绑定的地址：配置里端口写 0 时，这里是节点挑定的端口。
    Listening { local: SocketAddr },
    /// 端口已经被别的程序占着。节点不去抢，每隔一段时间再试。
    PortInUse { detail: String },
    /// 别的原因没有绑上（地址不在本机、没有权限等），`detail` 是系统给的原因。
    Failed { detail: String },
    /// 监听在本节点虚拟 IP 上的端口转发：不占操作系统的端口。
    Virtual,
    /// 这种用法节点不支持，`detail` 说明为什么。
    Unsupported { detail: String },
    /// 比这个程序新的节点报的、这里不认识的状态。
    #[serde(other)]
    Unknown,
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
    /// 选路的细节：模式、考虑过的候选路径和各自的代价、为什么是现在这条。旧版本的节点不报，对端不在 netmap 里时也没有。
    #[serde(default)]
    pub detail: Option<PathDetail>,
}

/// 一条路的细节。说不出来的项是 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteKind {
    pub peer: NodeId,
    /// 走直连时，这条直连是怎么建立起来的。
    #[serde(default)]
    pub direct: Option<DirectKind>,
    /// 经 relay 时，本节点到第一个 relay 的那一段走的是 UDP 还是 TCP。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
}

/// 直连是怎么建立起来的（白皮书 §7.7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectKind {
    /// 同一个局域网。
    Lan,
    /// 对端的 IPv6 地址。
    Ipv6,
    /// 对端的这个地址本来就连得上（公网地址、端口转发、端口映射），没有用到打洞。
    Reachable,
    /// 交换打洞信令之后打通的。
    Punched,
    /// 比这个程序新的节点报的、这里不认识的类型。
    #[serde(other)]
    Unknown,
}

/// 到 relay 的那一段用的传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegTransport {
    Udp,
    /// UDP 用不了时，经 relay 的 TCP 监听。
    Tcp,
    /// 比这个程序新的节点报的、这里不认识的传输。
    #[serde(other)]
    Unknown,
}

/// 发送方对一个对端的选路情况：它用什么模式、考虑过哪些路、各自多少代价、为什么是现在这条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathDetail {
    /// 发送方用的选路模式。比这个程序新的节点报了不认识的模式时是 `None`。
    #[serde(default, deserialize_with = "known")]
    pub mode: Option<RoutingMode>,
    /// 现在走直连时，这条直连是怎么建立起来的。
    #[serde(default)]
    pub direct: Option<DirectKind>,
    /// 现在经 relay 时，第一段走的是 UDP 还是 TCP。节点说不出来时是 `None`。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
    /// 路由层说的"为什么是现在这条路"，给人看的。
    #[serde(default)]
    pub reason: String,
    /// 考虑过的候选路径，按代价从低到高。没有任何测量值的路不在里面。
    #[serde(default)]
    pub candidates: Vec<PathCandidate>,
}

/// 一条考虑过的候选路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathCandidate {
    pub route: Route,
    /// 按白皮书 §7.4 算出来的代价，微秒。默认模式不拿它做选择，但照样算出来给人看。
    pub cost_us: u64,
    /// 各段的测量值，按经过的顺序。
    #[serde(default)]
    pub legs: Vec<PathSegment>,
    /// 路径上负载最高的 relay 的带宽占用百分比。
    #[serde(default)]
    pub load_percent: u8,
    /// 现在走的就是这一条。
    #[serde(default)]
    pub current: bool,
}

/// 路径上一段的测量值。目的节点到它挂靠的 relay 的那一段只知道延迟，抖动和丢包都是 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSegment {
    pub from: NodeId,
    pub to: NodeId,
    /// 往返延迟，微秒。
    pub rtt_us: u32,
    /// 抖动，微秒。
    #[serde(default)]
    pub jitter_us: u32,
    /// 丢包率，千分之几。
    #[serde(default)]
    pub loss_permille: u16,
}

/// 骨干矩阵的一行：一个 relay 自己报的状态、负载，和它到别的 relay 的链路。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackboneRow {
    pub relay: NodeId,
    #[serde(default)]
    pub state: RelayState,
    /// 带宽占用的百分比；没有设上限的 relay 是 0。
    #[serde(default)]
    pub load_percent: u8,
    /// 回答的节点是多少毫秒之前收到这一行的。
    pub age_ms: u64,
    #[serde(default)]
    pub links: Vec<BackboneLink>,
}

/// 骨干矩阵里一个 relay 到另一个 relay 的链路。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackboneLink {
    pub to: NodeId,
    /// 往返延迟，毫秒；`None` 表示连不上。
    pub rtt_ms: Option<u32>,
    /// 抖动，毫秒。
    #[serde(default)]
    pub jitter_ms: u32,
    /// 丢包率，千分之几。
    #[serde(default)]
    pub loss_permille: u16,
}

/// relay 现在能不能接新的转发，是它自己在骨干矩阵的行里说的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    /// 正常。
    #[default]
    Serving,
    /// 转发的流量到了带宽上限。
    Overloaded,
    /// 即将下线（升级、关机、被停用）。
    Draining,
    /// 比这个程序新的节点报的、这里不认识的状态。
    #[serde(other)]
    Unknown,
}

/// 读不懂的值读成 `None`，不让整条消息因此解析失败：比这个程序新的节点可能报了这里还没有的取值。
fn known<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> Result<Option<T>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| serde_json::from_value(value).ok()))
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
        assert!(text.starts_with("i3t-eyJzIjoi"), "密钥排在最前面：{text}");
        let other = EnrollToken { secret: "another-secret".to_owned(), ..token.clone() };
        assert_ne!(text[..EnrollToken::VISIBLE_PREFIX_LEN], other.to_string()[..EnrollToken::VISIBLE_PREFIX_LEN], "开头一小段就分得出来");
        let older = format!(
            "i3t-{}",
            URL_SAFE_NO_PAD
                .encode(r#"{"c":"ctl.example.com:20331","k":"AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM","s":"one-time-secret"}"#)
        );
        assert_eq!(older.parse::<EnrollToken>(), Ok(token.clone()), "以前的写法照样读得出来");
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
    fn a_report_from_an_older_node_reads_without_usage_and_status() {
        let old: Report = serde_json::from_value(serde_json::json!({ "flows": 3, "tx_bytes": 10, "rx_bytes": 20 })).unwrap();
        assert_eq!((old.run, old.exit.clone(), old.credentials.len(), old.exits.len()), (0, None, 0, 0));
        assert!(old.listeners.is_empty() && old.forwards.is_empty());
    }

    #[test]
    fn usage_and_listen_status_have_stable_json_shapes() {
        let forward = ForwardReport {
            id: 7,
            bind: "127.0.0.1:0".parse().unwrap(),
            protocol: ForwardProtocol::Tcp,
            state: ListenState::Listening { local: "127.0.0.1:41000".parse().unwrap() },
            traffic: ListenTraffic { connections: 2, bytes_up: 10, bytes_down: 20 },
        };
        assert_eq!(
            serde_json::to_value(&forward).unwrap(),
            serde_json::json!({
                "id": 7, "bind": "127.0.0.1:0", "protocol": "tcp", "state": { "state": "listening", "local": "127.0.0.1:41000" },
                "connections": 2, "bytes_up": 10, "bytes_down": 20
            })
        );
        let in_use = serde_json::to_value(ListenState::PortInUse { detail: "Address in use".into() }).unwrap();
        assert_eq!(in_use, serde_json::json!({ "state": "port_in_use", "detail": "Address in use" }));
        assert_eq!(serde_json::from_value::<ListenState>(serde_json::json!({ "state": "later" })).unwrap(), ListenState::Unknown);

        let usage = CredentialUsage { id: 3, bytes: 4096, connections: 1, last_used_secs_ago: Some(5) };
        let wire = serde_json::to_value(&usage).unwrap();
        assert_eq!(wire, serde_json::json!({ "id": 3, "bytes": 4096, "connections": 1, "last_used_secs_ago": 5 }));
        assert_eq!(serde_json::from_value::<CredentialUsage>(wire).unwrap(), usage);

        let exit = ExitReport {
            quota: Some(QuotaUsage { limit_bytes: 100, used_bytes: 40, period_start: 1, period_end: 2 }),
            ..ExitReport::default()
        };
        let wire = serde_json::to_value(&exit).unwrap();
        assert_eq!(wire["quota"], serde_json::json!({ "limit_bytes": 100, "used_bytes": 40, "period_start": 1, "period_end": 2 }));
        assert_eq!(serde_json::from_value::<ExitReport>(serde_json::json!({ "total": 5 })).unwrap().total, 5);
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
            detail: None,
        };
        let wire = serde_json::to_value(ToController::RouteAnswer { request: 9, answer: answer.clone() }).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "type": "route_answer", "request": 9,
                "answer": { "peer": 4, "route": { "via": "direct", "addr": "1.2.3.4:20330" }, "rtt_ms": 12,
                            "since": { "secs_ago": 30, "reason": "直连打通了" }, "detail": null }
            })
        );
        let unknown: RouteAnswer = serde_json::from_value(serde_json::json!({ "peer": 4, "route": null })).unwrap();
        assert_eq!(unknown, RouteAnswer { peer, route: None, rtt_ms: None, since: None, detail: None });
    }

    #[test]
    fn a_route_answer_carries_the_candidates_and_tolerates_values_added_later() {
        let (me, relay, peer) = (NodeId::new(1).unwrap(), NodeId::new(9).unwrap(), NodeId::new(4).unwrap());
        let detail = PathDetail {
            mode: Some(RoutingMode::LowestCost),
            direct: None,
            relay_leg: Some(LegTransport::Tcp),
            reason: "经 relay 代价最低".to_owned(),
            candidates: vec![PathCandidate {
                route: Route::Relay { relays: vec![relay] },
                cost_us: 41_500,
                legs: vec![
                    PathSegment { from: me, to: relay, rtt_us: 20_250, jitter_us: 500, loss_permille: 3 },
                    PathSegment { from: relay, to: peer, rtt_us: 19_000, jitter_us: 0, loss_permille: 0 },
                ],
                load_percent: 12,
                current: true,
            }],
        };
        let answer =
            RouteAnswer { peer, route: Some(Route::Relay { relays: vec![relay] }), rtt_ms: Some(39), since: None, detail: Some(detail) };
        let wire = serde_json::to_value(&answer).unwrap();
        assert_eq!(wire["detail"]["mode"], "lowest_cost");
        assert_eq!(wire["detail"]["relay_leg"], "tcp");
        assert_eq!(
            wire["detail"]["candidates"][0]["legs"][0],
            serde_json::json!({ "from": 1, "to": 9, "rtt_us": 20_250, "jitter_us": 500, "loss_permille": 3 })
        );
        assert_eq!(serde_json::from_value::<RouteAnswer>(wire).unwrap(), answer);

        let newer: PathDetail = serde_json::from_value(serde_json::json!({
            "mode": "some_new_mode", "direct": "some_new_kind", "relay_leg": "quic", "reason": "", "candidates": []
        }))
        .unwrap();
        assert_eq!((newer.mode, newer.direct, newer.relay_leg), (None, Some(DirectKind::Unknown), Some(LegTransport::Unknown)));
    }

    #[test]
    fn backbone_queries_answers_and_try_later_have_stable_json_shapes() {
        let relay = NodeId::new(9).unwrap();
        assert_eq!(
            serde_json::to_value(ToNode::BackboneQuery { request: 5 }).unwrap(),
            serde_json::json!({ "type": "backbone_query", "request": 5 })
        );
        let try_later = serde_json::to_value(ToNode::TryLater { reason: "存储出错".to_owned() }).unwrap();
        assert_eq!(try_later, serde_json::json!({ "type": "try_later", "reason": "存储出错" }));
        let row = BackboneRow {
            relay,
            state: RelayState::Overloaded,
            load_percent: 97,
            age_ms: 1200,
            links: vec![BackboneLink { to: NodeId::new(3).unwrap(), rtt_ms: None, jitter_ms: 0, loss_permille: 1000 }],
        };
        let wire = serde_json::to_value(ToController::BackboneAnswer { request: 5, rows: vec![row.clone()] }).unwrap();
        assert_eq!(wire["rows"][0]["state"], "overloaded");
        assert_eq!(wire["rows"][0]["links"][0], serde_json::json!({ "to": 3, "rtt_ms": null, "jitter_ms": 0, "loss_permille": 1000 }));
        assert_eq!(serde_json::from_value::<ToController>(wire).unwrap(), ToController::BackboneAnswer { request: 5, rows: vec![row] });
        let later: BackboneRow = serde_json::from_value(serde_json::json!({ "relay": 9, "state": "resting", "age_ms": 0 })).unwrap();
        assert_eq!((later.state, later.load_percent, later.links.len()), (RelayState::Unknown, 0, 0));
    }

    #[test]
    fn route_kinds_are_optional_in_a_report() {
        let old: Report = serde_json::from_value(serde_json::json!({ "routes": [], "routes_full": true })).unwrap();
        assert!(old.route_kinds.is_empty());
        let kind = RouteKind { peer: NodeId::new(4).unwrap(), direct: Some(DirectKind::Punched), relay_leg: None };
        let report = Report { route_kinds: vec![kind], ..Report::default() };
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["route_kinds"], serde_json::json!([{ "peer": 4, "direct": "punched", "relay_leg": null }]));
        assert_eq!(serde_json::from_value::<Report>(wire).unwrap(), report);
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
