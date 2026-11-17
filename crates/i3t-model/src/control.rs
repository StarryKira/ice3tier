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
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use i3t_proto::{NodeId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::netmap::{Attachment, ForwardProtocol, ListenerProtocol, Netmap, Profile, RoutingMode};
use crate::update::{Platform, SelfUpdateBlocker, SignedManifest, UpdateStatus, UpdateTiming, Version};

pub mod diag;
pub mod exit_test;

pub use diag::{TestOrder, TestReport};
pub use exit_test::ExitTestResult;

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

/// [`Report::local_addresses`] 最多报几个。一台机器上的网卡地址一般只有几个；多的（容器、虚拟网桥）对谁都没有用。
pub const MAX_LOCAL_ADDRESSES: usize = 32;

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
    /// 装在 `Box` 里只是为了让消息的枚举不至于太大，JSON 里没有区别。
    Report(Box<Report>),
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
    /// 对 [`ToNode::ExitTest`] 的回答（[`exit_test`]）。
    ExitTestAnswer {
        request: u64,
        result: Box<ExitTestResult>,
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
        /// `reason` 的代码，节点按自己的语言说。旧版本的 controller 不给。
        #[serde(default)]
        cause: Option<RejectCause>,
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
    /// 让出口节点按它的出站策略试着连一个目标（[`exit_test`]），节点用 [`ToController::ExitTestAnswer`] 回答。
    ///
    /// 节点协议没有为它升版：比它旧的节点（第 1 版起）跳过这条消息，controller 等不到回答就说没有回答。
    ExitTest {
        request: u64,
        /// 域名或者地址（IPv6 不带方括号）。
        host: String,
        port: u16,
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
    #[serde(default, serialize_with = "Declared::serialize_for_older_controllers")]
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
///
/// 端口转发的目标不在这里：它只由管理员的开关决定（`Node::forward_target`），节点不声明。旧版本的节点报的 `forward` 被忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Declared {
    #[serde(default)]
    pub exit: bool,
    #[serde(default)]
    pub relay: bool,
    /// 节点上声明的 relay 地址（`i3t set relay-address`）：只有人知道的那一种，机房或者路由器上映射进来的地址。
    /// 节点启用为 relay 时，它排在 controller 找出来的地址前面（白皮书 §7.2）。旧版本的节点不报。
    #[serde(default)]
    pub relay_address: Option<SocketAddr>,
}

impl Declared {
    /// [`Hello`] 里的写法：多带一个总是 `true` 的 `forward`。
    ///
    /// 旧版本的 controller 只给报了 `forward` 的节点下发端口转发，节点不报时还会把指向它的转发全部停用。
    /// 新的节点总是报 `true`，旧的 controller 因此照旧下发、照旧让管理员配；配不配由管理员定，和新的规则一样。
    fn serialize_for_older_controllers<S: serde::Serializer>(declared: &Self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            #[serde(flatten)]
            declared: &'a Declared,
            forward: bool,
        }
        Wire { declared, forward: true }.serialize(serializer)
    }
}

/// 节点的定期上报，默认每 10 秒一次。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Report {
    /// 本节点当前的 UDP 候选端点：网卡地址、反射地址、端口映射。
    #[serde(default)]
    pub endpoints: Vec<SocketAddr>,
    /// 本机网卡上的地址（回环和链路本地的除外），不管它们在不在网络的地址段里，最多 [`MAX_LOCAL_ADDRESSES`] 个。
    /// `endpoints` 里分不出哪些是网卡上的，也去掉了地址段里的那些，所以另报：controller 拿它当 relay 的地址（白皮书 §7.2），
    /// 并且在它落进网络的地址段时提醒（§4.6）。旧版本的节点不报，读成 `None`。是切片不是 `Vec`，同上，为了消息的枚举不至于太大。
    #[serde(default)]
    pub local_addresses: Option<Box<[IpAddr]>>,
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
    /// 节点正在用的性能档位。旧版本的节点不报。装在 `Box` 里只是为了让消息的枚举不至于太大，JSON 里没有区别。
    #[serde(default)]
    pub performance: Option<Box<PerformanceReport>>,
    /// 透明访问（Linux 上的套接字级拦截）的状况。没有打开时也报（`off`）；旧版本的节点不报。装在 `Box` 里，同上。
    #[serde(default)]
    pub intercept: Option<Box<InterceptReport>>,
    /// 节点试过的 relay 连没连上（白皮书 §7.2：谁连得上，它就是谁的 relay），最多 [`MAX_RELAY_TRIES`] 个，有会话的排在前面。
    /// 每次上报都是完整的一份。旧版本的节点不报。
    #[serde(default)]
    pub relays: Vec<RelayTry>,
    /// 到各个对端的共用连接上现在开着几条流、两个方向各走了多少字节（白皮书 §7.7），只有现在有连接的对端，
    /// 最多 [`MAX_PEER_USE`] 个，流量大的在前。每次上报都是完整的一份。旧版本的节点不报。
    #[serde(default)]
    pub peer_use: Vec<PeerUse>,
    /// 本节点是 relay 时：上一个统计时段里它替哪些节点对转发了多少（白皮书 §7.7 的"谁在用这个 relay"）。不是 relay、旧版本的节点不报。
    #[serde(default)]
    pub relay_use: Option<RelayUse>,
    /// 上一次上报之后的换路，最多 [`MAX_REPORTED_SWITCHES`] 条，早的在前。报过就不再报。旧版本的节点不报。
    #[serde(default)]
    pub switches: Vec<RouteSwitch>,
}

/// [`Report::peer_use`] 最多报几个对端。
pub const MAX_PEER_USE: usize = 256;

/// 到一个对端的共用连接现在的用量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerUse {
    pub peer: NodeId,
    /// 现在开着的流（共用连接上的，两边谁开的都算；独占 TCP 上的不在这里）。
    pub flows: u32,
    /// 本节点启动以来在和它的连接上发出、收到的字节数（线上的，含 QUIC 的开销），连接关了重建也接着累计。
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// [`RelayUse::pairs`] 最多报几对。
pub const MAX_RELAY_USERS: usize = 16;

/// relay 在一个统计时段里替各个节点对转发了多少（白皮书 §7.7）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayUse {
    /// 统计时段多长（秒）。
    pub window_secs: u32,
    /// 转发得最多的几对，最多 [`MAX_RELAY_USERS`] 对，多的在前。
    #[serde(default)]
    pub pairs: Vec<RelayPairUse>,
    /// 其余的节点对加起来转发了多少字节。
    #[serde(default)]
    pub other_bytes: u64,
}

/// 一个节点对在一个统计时段里经这个 relay 转发的字节数，含中转头。`from` 是发出包的节点，`to` 是收包的节点，两个方向分开算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayPairUse {
    pub from: NodeId,
    pub to: NodeId,
    pub bytes: u64,
}

/// [`Report::switches`] 最多报几条。
pub const MAX_REPORTED_SWITCHES: usize = 64;

/// 一次换路（白皮书 §7.5、§13 的决策日志）：到这个对端的路从什么换成了什么、为什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSwitch {
    pub peer: NodeId,
    pub from: Route,
    pub to: Route,
    pub cause: RouteCause,
    /// 多少秒之前换的。
    #[serde(default)]
    pub age_secs: u32,
}

/// [`Report::relays`] 最多报几个。节点试的是 netmap 里的每个 relay（挂靠和经过的用会话，别的每分钟用 UDP 问一次），
/// 网络里 relay 再多，报的也只有这么多：有会话的、连得上的排在前面。
pub const MAX_RELAY_TRIES: usize = 32;

/// 节点最近一次试一个 relay 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayTry {
    pub relay: NodeId,
    pub outcome: TryOutcome,
    /// 多少秒之前试的；现在有会话的是 0。报的是相对的时间：节点的钟不一定准，controller 换成自己的时间。
    #[serde(default)]
    pub age_secs: u32,
    /// 这个 relay 看到的本节点的 IP（中转会话的应答里带着的），没有会话时没有。controller 拿它定节点的出口地址。
    #[serde(default)]
    pub seen_as: Option<IpAddr>,
}

/// 试一个 relay 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TryOutcome {
    /// 经 UDP 连上了（中转会话，或者每分钟一次的询问得到了回答）。
    Udp,
    /// UDP 连不上，经 TCP 连上了。
    Tcp,
    /// UDP 和 TCP 都试了，都没连上。
    Unreached,
    /// 比本程序新的一方说的、这里不认识的结果。
    #[serde(other)]
    Unknown,
}

impl TryOutcome {
    /// 连上了没有。
    #[must_use]
    pub const fn reached(self) -> bool {
        match self {
            Self::Udp | Self::Tcp => true,
            Self::Unreached | Self::Unknown => false,
        }
    }
}

/// 透明访问在一个节点上的状况（白皮书 §9.5）：本机的程序不配代理，直接连别的节点的虚拟 IP 或者名字。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterceptReport {
    #[serde(flatten)]
    pub state: InterceptState,
    /// 正在进行的被拦截的连接和 UDP 会话。
    #[serde(default)]
    pub active: u32,
    /// 这次运行以来经透明访问发出、收到的字节数。
    #[serde(default)]
    pub up_bytes: u64,
    #[serde(default)]
    pub down_bytes: u64,
}

/// 透明访问是开着、关着，还是开不起来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InterceptState {
    /// 没有打开（默认）。
    Off,
    /// 打开着，系统里的规则都装好了。
    On {
        /// UDP 也被拦截。
        udp: bool,
        /// 别的节点经透明访问连到本机的服务时，服务看到的是那个节点的虚拟 IP。
        source_address: bool,
        names: NameService,
    },
    /// 本机配置里打开了，但是装不上。系统里什么都没有留下。
    Failed { cause: InterceptCause },
    /// 这个平台上没有透明访问（只有 Linux 有）。
    Unsupported,
    /// 比这个程序新的节点报的、这里不认识的状态。
    #[serde(other)]
    Unknown,
}

/// 透明访问打开时，`<节点>.<网络的域名>` 怎么解析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NameService {
    /// systemd-resolved 把网络的域名交给本节点的名字服务：本机的程序直接用名字就行。
    Resolved,
    /// 名字服务开着（本节点虚拟 IP 的 53 端口），这台机器没有 systemd-resolved：名字要写进 `/etc/hosts`（`i3t hosts`），
    /// 或者让本机的解析器把网络的域名转给它。
    NoResolver,
    /// 名字服务没有开起来。按虚拟 IP 访问照常可用。
    Failed { cause: InterceptCause },
    #[serde(other)]
    Unknown,
}

/// 透明访问（或者它的名字服务）开不起来的原因，给界面按自己的语言写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum InterceptCause {
    /// 节点进程缺这些能力：`CAP_NET_ADMIN`、`CAP_NET_BIND_SERVICE`（写法和 systemd 的 `AmbientCapabilities=` 一样）。
    NoPermission { missing: Vec<String> },
    /// 系统里没有这个程序：`nft`（nftables）、`ip`（iproute2）。
    ProgramMissing { program: String },
    /// 一个程序报错退出，`detail` 是它的原话（比如内核不认某条规则）。
    CommandFailed { program: String, detail: String },
    /// 这个地址的端口被占着。
    PortInUse { address: SocketAddr },
    /// 别的原因绑不上这个地址，`detail` 是系统的原话。
    BindFailed { address: SocketAddr, detail: String },
    /// 策略路由用的那张路由表里有别的程序的路由。
    TableInUse { table: u32 },
    /// 已经有一块同名的网卡，不是 ice3tier 建的。
    LinkInUse { link: String },
    /// 网络的地址段和这台机器所在的一个真实网络重叠，装上会把它从那个网络上切下来。`with` 是重叠的那个地址或者路由，
    /// 写法和 `ip` 一样：`192.168.1.5/24 dev eth0`、`10.0.0.0/8 dev tun0`。
    RangeInUse { with: String },
    /// 比这个程序新的节点报的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 节点正在用的性能档位（白皮书 §12.1）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PerformanceReport {
    /// 现在生效的档位：管理员选了的话是它，否则是本机配置文件里的。比本程序新的节点报了不认识的档位时读成 `balanced`。
    #[serde(default, deserialize_with = "known_profile")]
    pub profile: Profile,
    /// 运行时的工作线程数。
    #[serde(default)]
    pub threads: u32,
    /// 档位或者本机的设置改过、其中要重启才生效的部分（工作线程数）还没有生效。
    #[serde(default)]
    pub restart_pending: bool,
    /// 本机配置文件单独改了的项，写的是配置文件里的键名（`threads`、`idle_close_secs`、`return_memory` 等）。
    #[serde(default)]
    pub overrides: Vec<String>,
}

/// 不认识的档位读成默认的，不让整条上报因此解析失败。
fn known_profile<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Profile, D::Error> {
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(raw.as_str().and_then(|text| text.parse().ok()).unwrap_or_default())
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
    /// 本节点到它的共用连接现在有没有 UDP 的路（直连，或者经 UDP 连着的 relay）。设置是 `udp_fallback` 时，
    /// 没有才在 lane 上开新的 TCP 流。旧版本的节点不报。
    #[serde(default)]
    pub udp_path: Option<bool>,
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
    /// 这条转发的流最近一次在目标那里失败；之后有流成功了就是 `None`。旧版本的节点不报。
    #[serde(default)]
    pub target_failure: Option<TargetFailureReport>,
}

/// 端口转发的流最近一次在目标那里失败（白皮书 §9.6 的"目标连不上"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetFailureReport {
    pub cause: TargetCause,
    /// 多少秒之前。用相对的时间，两边的时钟不用对齐。
    pub secs_ago: u64,
}

/// 端口转发的一条流在目标那里没有成功的原因，给界面按自己的语言写。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
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
    /// 监听节点到目标节点开不出流：目标节点不在线，或者两者之间没有路。
    NodeUnreachable,
    /// 别的原因（目标节点在排空、出口内部出错等），见监听节点的日志。
    Other,
    /// 比这个程序新的节点报的、这里不认识的原因。
    #[serde(other)]
    Unknown,
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
    PortInUse {
        detail: String,
        /// 原因的代码。旧版本的节点不报。
        #[serde(default)]
        cause: Option<BindCause>,
    },
    /// 别的原因没有绑上（地址不在本机、没有权限等），`detail` 是系统给的原因。
    Failed {
        detail: String,
        #[serde(default)]
        cause: Option<BindCause>,
    },
    /// 监听在本节点虚拟 IP 上的端口转发：不占操作系统的端口。
    Virtual,
    /// 这种用法节点不支持，`detail` 说明为什么。
    Unsupported {
        detail: String,
        #[serde(default)]
        cause: Option<BindCause>,
    },
    /// 比这个程序新的节点报的、这里不认识的状态。
    #[serde(other)]
    Unknown,
}

/// controller 不接受一个节点的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCause {
    /// 注册令牌不存在、已经用过或者已经过期。
    BadToken,
    /// 这把公钥没有在 controller 上注册（节点可能已经被删除）。
    UnknownKey,
    /// 这把公钥已经注册过了。
    KeyAlreadyEnrolled,
    /// 要继任的节点还在线。
    PredecessorOnline,
    /// 令牌预设的虚拟 IP 现在不能用。
    AddressUnavailable,
    /// 网络的地址段里没有空闲的地址了。
    RangeExhausted,
    /// 连接上的第一条消息不是注册或者报到。
    UnexpectedFirstMessage,
    /// 比这个程序新的 controller 给的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 一个监听没有绑上的原因，给界面按自己的语言写。`detail` 里是系统的原话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum BindCause {
    /// 端口被别的程序占着。
    AddressInUse,
    /// 没有权限（比如普通用户绑 1024 以下的端口）。
    PermissionDenied,
    /// 地址不在这台机器上。
    AddressNotAvailable,
    /// 监听在虚拟 IP 上的端口转发没有写端口。
    VirtualPortZero,
    /// 别的原因，见 `detail`。
    Other,
    /// 比这个程序新的节点报的、这里不认识的原因。
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
    /// 到这个对端的连接自己量的往返时间（毫秒，QUIC 的平滑 RTT），这对节点之间没有连接时没有。`rtt_ms` 是路由层按各段的探测
    /// 推算的，两个放在一起看：差得多说明哪一段的测量失真（白皮书 §13）。旧版本的节点不报。
    #[serde(default)]
    pub connection_rtt_ms: Option<u32>,
}

/// 一条路的细节。说不出来的项是 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteKind {
    pub peer: NodeId,
    /// 走直连时，这条直连是怎么建立起来的。
    #[serde(default)]
    pub direct: Option<DirectKind>,
    /// 经 relay 时，本节点到第一个 relay 的那一段走的是 UDP 还是 TCP。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
    /// 直连现在在哪一步，不管现在走不走它。旧版本的节点不报。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_status: Option<DirectStatus>,
    /// 本节点没在用的那种选路模式此刻会选的路（默认模式下是延迟优先会选的，反过来也一样），按选路器自己的规则、用手上的候选算，
    /// 不为它另外探测。和现在的路一样、或者没有可比的时没有；旧版本的节点不报。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternative: Option<RouteAlternative>,
}

/// 另一种选路模式会选的路。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAlternative {
    pub route: Route,
    /// 这条路上的往返延迟（毫秒），和 [`RouteEntry::rtt_ms`] 的算法一样；没有测量值时没有。
    #[serde(default)]
    pub rtt_ms: Option<u32>,
}

/// 到一个对端的直连现在在哪一步（白皮书 §7.7）：取自路径层为直连的尝试和退避记着的状态，不为它另外探测。
/// 直连在一对节点第一次用到时才去试，所以两个从没说过话的节点之间是"还没有试过"，不是"失败了"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectStatus {
    pub state: DirectState,
    /// `verified` 时：这条直连是怎么建立起来的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DirectKind>,
    /// 最近一次失败的原因：`failed` 时是这一次的，`trying` 时是上一次的；通着、从没失败过时没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<DirectCause>,
    /// 最近一次尝试是多少秒之前开始的；从来没有试过时没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted_secs_ago: Option<u64>,
}

/// 直连的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectState {
    /// 还没有试过：这一对节点还没有用过，或者不能由本节点去试（只经 relay、对端只主动连接）。
    NotTried,
    /// 正在试。
    Trying,
    /// 验证通过，可以用（走不走它看选路）。
    Verified,
    /// 试过没有通，或者通过的直连后来断了。按退避再试。
    Failed,
    /// 比这个程序新的节点报的、这里不认识的状态。
    #[serde(other)]
    Unknown,
}

/// 直连上一次为什么没有成：`{ "code": … }`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DirectCause {
    /// 试满了一次的时限，对端的哪个候选地址都没有回答。
    NoAnswer,
    /// 手上没有对端的任何候选地址。
    NoCandidates,
    /// 用着的直连上探测连续超时。
    Timeouts,
    /// 用着的直连上探测丢了超过 20%。
    Loss,
    /// 两个节点闲着，用着的直连很久没有验证，放下了；再用到时重新试。
    Stale,
    /// 比这个程序新的节点报的、这里不认识的原因。
    #[serde(other)]
    Unknown,
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
    /// `reason` 的代码和它要用到的值。旧版本的节点不报。
    #[serde(default)]
    pub cause: Option<RouteCause>,
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

impl Route {
    /// 这条路经过的 relay，按经过的顺序；不经 relay 时是空的。
    #[must_use]
    pub fn relays(&self) -> &[NodeId] {
        match self {
            Self::Relay { relays } => relays,
            Self::None | Self::Direct { .. } => &[],
        }
    }
}

impl RouteCause {
    /// 这个原因里说到的 relay（原来经过的那几个）。
    #[must_use]
    pub fn relays(&self) -> &[NodeId] {
        match self {
            Self::RelayChanged { previous, .. } => previous,
            Self::Cheaper { previous, .. } => previous.relays(),
            Self::NoRouteYet
            | Self::TestPinned
            | Self::TestEnded
            | Self::DirectVerified
            | Self::DirectLost { .. }
            | Self::RelayReady
            | Self::LegChanged { .. }
            | Self::Unreachable { .. }
            | Self::PeerRemoved
            | Self::Lan
            | Self::LowestCost { .. }
            | Self::Failover { .. }
            | Self::Unknown => &[],
        }
    }
}

/// 一条路是多久之前换上的，以及换的原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSince {
    /// 多少秒之前。用间隔而不是时刻：节点和 controller 的时钟不一定对得上。
    pub secs_ago: u64,
    /// 路由层给的原因，给人看的。
    pub reason: String,
    /// `reason` 的代码和它要用到的值。旧版本的节点不报。
    #[serde(default)]
    pub cause: Option<RouteCause>,
}

/// 路由层为什么走现在这条路、为什么换到它：一个代码，加上把它写成一句话要用到的值。
///
/// 和它在一起的 `reason` 是同一件事的中文原话，留给日志和旧的客户端。"现在这条路"（经哪些 relay、第一段走什么）
/// 在同一个应答的 `route`、`relay_leg` 里，这里不重复。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RouteCause {
    /// 到这个对端还没有过路。
    NoRouteYet,
    /// 主动测试把路暂时固定在这里。
    TestPinned,
    /// 主动测试结束，回到选路规则选的路。
    TestEnded,
    /// 直连地址验证通过，改走直连。
    DirectVerified,
    /// 直连不能用了，退回经 relay。
    DirectLost { hint: RouteHint },
    /// 原来没有路，中转会话可用了。
    RelayReady,
    /// 还是经同一串 relay，到第一个 relay 的会话换了传输。`transport` 是 `null` 时第一个 relay 就是本节点自己。
    LegChanged { transport: Option<LegTransport> },
    /// 换了一串 relay。`previous` 是原来经过的 relay。
    RelayChanged { hint: RouteHint, previous: Vec<NodeId> },
    /// 没有路可走了。
    Unreachable { hint: RouteHint },
    /// 对端不在本节点的配置里了：访问规则不再连通它们，或者它被删掉了。
    PeerRemoved,
    /// 延迟优先：对端在同一个局域网里，局域网直连无条件优先。
    Lan,
    /// 延迟优先：`candidates` 条候选里代价最低。`cost_us` 是它的代价，不知道时是 `null`。
    LowestCost { candidates: u32, cost_us: Option<u64> },
    /// 延迟优先：原来的路不能用了，不等迟滞，立刻换到剩下的候选里代价最低的。
    Failover { hint: RouteHint, candidates: u32, cost_us: Option<u64> },
    /// 延迟优先：新的路连续 `windows` 个评估窗口都比原来的 `previous` 便宜得多。
    Cheaper { previous: Route, previous_cost_us: u64, cost_us: u64, windows: u32 },
    /// 比这个程序新的节点报的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 换路的直接起因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteHint {
    /// 直连探测连续超时。
    DirectTimeouts,
    /// 直连探测丢了超过 20%。
    DirectLoss,
    /// 对端不活跃，直连地址过期了。
    DirectStale,
    /// 直连不可用（没有更具体的起因）。
    DirectUnavailable,
    /// relay 通知即将下线。
    RelayDraining,
    /// relay 通知过载。
    RelayOverloaded,
    /// relay 说多级路径走不下去。
    RelayPathBroken,
    /// relay 说对端没有挂在它上面。
    PeerNotAttached,
    /// 经多个 relay 的路径上端到端的探测一直没有回音。
    PathProbeSilent,
    /// 刚有路时先连上的不是对端排在前面的 relay，那个 relay 的会话随后也建好了。
    PreferredRelayReady,
    /// 有了不绕路的中转会话（对端自己是 relay，或者对端挂在本节点的 relay 上）。
    Shortcut,
    /// 原来的 relay 不可用（没有更具体的起因）。
    RelayUnavailable,
    /// 中转会话断开，没有别的路。
    RelayLost,
    /// 原来的路不能用了（没有更具体的起因）。
    RouteFailed,
    /// 没有任何可用的路径。
    NoPath,
    /// 比这个程序新的节点报的、这里不认识的起因。
    #[serde(other)]
    Unknown,
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
    fn the_intercept_report_is_a_state_code_with_its_values_and_survives_newer_codes() {
        let on = InterceptReport {
            state: InterceptState::On { udp: true, source_address: true, names: NameService::Resolved },
            active: 2,
            up_bytes: 10,
            down_bytes: 20,
        };
        let json = serde_json::to_value(&on).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "state": "on", "udp": true, "source_address": true, "names": { "state": "resolved" },
                                "active": 2, "up_bytes": 10, "down_bytes": 20 })
        );
        assert_eq!(serde_json::from_value::<InterceptReport>(json).unwrap(), on);

        let failed = InterceptReport {
            state: InterceptState::Failed { cause: InterceptCause::NoPermission { missing: vec!["CAP_NET_ADMIN".into()] } },
            active: 0,
            up_bytes: 0,
            down_bytes: 0,
        };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["cause"], serde_json::json!({ "code": "no_permission", "missing": ["CAP_NET_ADMIN"] }));
        assert_eq!(serde_json::from_value::<InterceptReport>(json).unwrap(), failed);

        let overlap = InterceptCause::RangeInUse { with: "192.168.1.5/24 dev eth0".into() };
        assert_eq!(
            serde_json::to_value(&overlap).unwrap(),
            serde_json::json!({ "code": "range_in_use", "with": "192.168.1.5/24 dev eth0" })
        );
        let newer: InterceptReport = serde_json::from_value(serde_json::json!({ "state": "paused" })).unwrap();
        assert_eq!(newer.state, InterceptState::Unknown);
        let newer: InterceptReport =
            serde_json::from_value(serde_json::json!({ "state": "failed", "cause": { "code": "solar_flare" } })).unwrap();
        assert_eq!(newer.state, InterceptState::Failed { cause: InterceptCause::Unknown });
        let older: Report = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(older.intercept, None, "旧的节点不报");
    }

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
    fn a_hello_tells_older_controllers_it_may_be_a_forward_target_and_an_old_declaration_is_ignored() {
        let declared = Declared { exit: true, ..Declared::default() };
        let hello = Hello { declared, ..Hello::default() };
        let sent = serde_json::to_value(&hello).unwrap();
        assert_eq!(sent["declared"], serde_json::json!({ "exit": true, "relay": false, "relay_address": null, "forward": true }));
        assert_eq!(serde_json::to_value(declared).unwrap().get("forward"), None, "管理接口里的声明没有 forward");

        let old: Hello = serde_json::from_value(serde_json::json!({
            "version": "0.1.0", "platform": "linux-x86_64", "declared": { "exit": false, "relay": true, "forward": true }
        }))
        .unwrap();
        assert_eq!(old.declared, Declared { relay: true, ..Declared::default() });
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
            target_failure: Some(TargetFailureReport { cause: TargetCause::Refused, secs_ago: 4 }),
        };
        assert_eq!(
            serde_json::to_value(&forward).unwrap(),
            serde_json::json!({
                "id": 7, "bind": "127.0.0.1:0", "protocol": "tcp", "state": { "state": "listening", "local": "127.0.0.1:41000" },
                "connections": 2, "bytes_up": 10, "bytes_down": 20, "target_failure": { "cause": { "code": "refused" }, "secs_ago": 4 }
            })
        );
        let older: ForwardReport = serde_json::from_value(serde_json::json!({
            "id": 7, "bind": "127.0.0.1:0", "protocol": "tcp", "state": { "state": "virtual" }
        }))
        .unwrap();
        assert_eq!(older.target_failure, None, "旧版本的节点不报目标连不上");
        let newer: TargetFailureReport =
            serde_json::from_value(serde_json::json!({ "cause": { "code": "later" }, "secs_ago": 1 })).unwrap();
        assert_eq!(newer.cause, TargetCause::Unknown);
        let in_use =
            serde_json::to_value(ListenState::PortInUse { detail: "Address in use".into(), cause: Some(BindCause::AddressInUse) }).unwrap();
        assert_eq!(
            in_use,
            serde_json::json!({ "state": "port_in_use", "detail": "Address in use", "cause": { "code": "address_in_use" } })
        );
        let old: ListenState = serde_json::from_value(serde_json::json!({ "state": "failed", "detail": "x" })).unwrap();
        assert_eq!(old, ListenState::Failed { detail: "x".into(), cause: None }, "旧版本的节点不报原因的代码");
        let newer: ListenState =
            serde_json::from_value(serde_json::json!({ "state": "failed", "detail": "x", "cause": { "code": "later" } })).unwrap();
        assert_eq!(newer, ListenState::Failed { detail: "x".into(), cause: Some(BindCause::Unknown) });
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
            since: Some(RouteSince { secs_ago: 30, reason: "直连打通了".to_owned(), cause: Some(RouteCause::DirectVerified) }),
            detail: None,
            connection_rtt_ms: Some(14),
        };
        let wire = serde_json::to_value(ToController::RouteAnswer { request: 9, answer: answer.clone() }).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "type": "route_answer", "request": 9,
                "answer": { "peer": 4, "route": { "via": "direct", "addr": "1.2.3.4:20330" }, "rtt_ms": 12,
                            "since": { "secs_ago": 30, "reason": "直连打通了", "cause": { "code": "direct_verified" } }, "detail": null,
                            "connection_rtt_ms": 14 }
            })
        );
        let unknown: RouteAnswer = serde_json::from_value(serde_json::json!({ "peer": 4, "route": null })).unwrap();
        assert_eq!(unknown, RouteAnswer { peer, route: None, rtt_ms: None, since: None, detail: None, connection_rtt_ms: None });
    }

    #[test]
    fn a_route_answer_carries_the_candidates_and_tolerates_values_added_later() {
        let (me, relay, peer) = (NodeId::new(1).unwrap(), NodeId::new(9).unwrap(), NodeId::new(4).unwrap());
        let detail = PathDetail {
            mode: Some(RoutingMode::LowestCost),
            direct: None,
            relay_leg: Some(LegTransport::Tcp),
            reason: "经 relay 代价最低".to_owned(),
            cause: Some(RouteCause::LowestCost { candidates: 2, cost_us: Some(41_500) }),
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
        let answer = RouteAnswer {
            peer,
            route: Some(Route::Relay { relays: vec![relay] }),
            rtt_ms: Some(39),
            since: None,
            detail: Some(detail),
            connection_rtt_ms: None,
        };
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
        let kind = RouteKind {
            peer: NodeId::new(4).unwrap(),
            direct: Some(DirectKind::Punched),
            relay_leg: None,
            direct_status: None,
            alternative: None,
        };
        let report = Report { route_kinds: vec![kind], ..Report::default() };
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["route_kinds"], serde_json::json!([{ "peer": 4, "direct": "punched", "relay_leg": null }]));
        assert_eq!(serde_json::from_value::<Report>(wire).unwrap(), report);
    }

    #[test]
    fn the_direct_status_has_a_code_for_its_state_and_cause_and_reads_unknown_values() {
        let failed =
            DirectStatus { state: DirectState::Failed, kind: None, cause: Some(DirectCause::NoAnswer), attempted_secs_ago: Some(42) };
        let wire = serde_json::to_value(failed).unwrap();
        assert_eq!(wire, serde_json::json!({ "state": "failed", "cause": { "code": "no_answer" }, "attempted_secs_ago": 42 }));
        let never: DirectStatus = serde_json::from_value(serde_json::json!({ "state": "not_tried" })).unwrap();
        assert_eq!((never.state, never.cause, never.attempted_secs_ago), (DirectState::NotTried, None, None));
        let newer: DirectStatus =
            serde_json::from_value(serde_json::json!({ "state": "parked", "cause": { "code": "firewall", "port": 1 } })).unwrap();
        assert_eq!((newer.state, newer.cause), (DirectState::Unknown, Some(DirectCause::Unknown)));
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
