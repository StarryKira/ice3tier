//! 管理 API 的数据结构：WebUI 和命令行通过它读写 controller 里的东西。
//!
//! 这里的类型就是请求体和应答体的 JSON 形式；路径、方法和各个错误码出现的场合写在 `docs/admin-api.md`。
//!
//! 几条通用的约定：
//!
//! - 时间都是 Unix 秒（[`Timestamp`]）。
//! - 名字以 `Create`、`Update` 开头或以 `Input` 结尾的是请求体，其余是应答体。
//! - `Update*` 里的字段都可以省略，省略表示不改。个别字段可以写 `null`，表示清掉这项设置，见各字段的说明。
//! - 令牌的明文只在创建它的那一次应答里出现（[`CreatedEnrollToken`]、[`CreatedCredential`]），controller 只存哈希。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::ParseIntError;
use std::str::FromStr;

use i3t_proto::flow::Class;
use i3t_proto::{NodeId, PublicKey};
use ipnet::{IpNet, Ipv4Net};
use serde::{Deserialize, Deserializer, Serialize};

use crate::control::{
    BackboneLink, Declared, DirectKind, ExitObservation, ExitReport, InterceptReport, LaneReport, LegTransport, ListenState, ListenTraffic,
    PathCandidate, PeerLanes, PerformanceReport, RelayState, RelayUse, Route, RouteCause, RouteSince, SelfUpdate, TargetCause, TryOutcome,
};
use crate::netmap::{
    Attachment, Congestion, ExitRole, ForwardProtocol, LaneMode, ListenerProtocol, Profile, ProxyProtocol, Reach, RoutingMode,
};
use crate::update::{FailureKind, SelfUpdateBlocker, UpdateCause, UpdateStatus, UpdateTiming, Version};

pub mod diag;
pub mod stats;

/// 管理 API 所有路径的前缀。
pub const API_PREFIX: &str = "/api/v1";

/// Unix 秒。
pub type Timestamp = u64;

/// 网络的编号。controller 在建网时分配，删掉之后不复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NetworkId(pub u32);

impl fmt::Display for NetworkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for NetworkId {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// 分组的编号。controller 在建分组时分配，在整个 controller 里唯一，删掉之后不复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(pub u32);

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for GroupId {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// 出错时的应答体。HTTP 状态码说明大类，[`ApiError::code`] 给程序看，[`ApiError::message`] 给人看。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// 比 `code` 更具体的一层：`not_found` 时是找不到的东西（`node`、`network` 等），`conflict` 时是挡着的东西，
    /// `invalid_request` 时是哪里不对（`zero`、`too_long` 等），`cannot_self_update`、`no_update` 时是原因。
    /// 取值见 `docs/admin-api.md` 第 2 节；controller 说不出来时是 `null`。
    #[serde(default)]
    pub what: Option<String>,
    /// 请求里出问题的字段（请求体的字段名、查询参数名或路径参数名）；不是某一个字段的问题时是 `null`。
    #[serde(default)]
    pub field: Option<String>,
}

/// 给程序看的错误码。以后可能增加，不认识的值读成 [`ErrorCode::Unknown`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// 没带管理令牌，或者令牌不对。
    Unauthorized,
    /// 路径里指的东西不存在，或者不属于路径里的那个网络。
    NotFound,
    /// 请求本身不合法：JSON 解析不了、名字的写法不对、地址不合法、要改内置分组等。
    InvalidRequest,
    /// 名字已经被同一个网络里的另一个东西占用。
    NameTaken,
    /// 名字属于一个刚删除的节点，还在冷却期。
    NameCoolingDown,
    /// 虚拟 IP 已经分给了另一个节点。
    AddressTaken,
    /// 虚拟 IP 属于一个刚删除的节点，还在冷却期。
    AddressCoolingDown,
    /// 虚拟 IP 不在网络的地址段里，或者是网络地址、广播地址。
    AddressOutOfRange,
    /// 地址段和另一个网络的重叠。
    RangeOverlaps,
    /// 地址段里没有空闲地址了。
    RangeExhausted,
    /// 网络里已经有节点，地址段只能扩大，不能更换。
    RangeNotSuperset,
    /// 网络里还有节点，不能删除。
    NetworkNotEmpty,
    /// 节点没有在本地声明可以承担这项能力。
    NotDeclared,
    /// 要被继任的节点还在线。
    PredecessorOnline,
    /// 和已有的配置冲突（例如同一个监听地址上已经有一条转发）。
    Conflict,
    /// 要操作的节点不在线（让节点立即更新要它在线）。
    NodeOffline,
    /// 节点不能自己更新：在容器里、是包管理器装的、程序所在的目录不可写，或者它的版本太旧不懂更新。
    CannotSelfUpdate,
    /// 没有可以让这个节点更新到的版本：还没有查到发布、它已经是最新的、发布里没有它的平台的包，或者 controller 自己太旧。
    NoUpdate,
    /// 同一对节点的上一次主动测试刚做过（或者一次全网矩阵刚开始过），要隔一会儿再测。
    TooSoon,
    /// 这个网络正在跑一次全网矩阵，或者要取消的测试已经结束了。
    Busy,
    /// controller 内部出错。
    Internal,
    #[serde(other)]
    Unknown,
}

/// 指代一个节点时附带的最少信息，免得界面再查一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRef {
    pub id: NodeId,
    pub name: String,
    pub virtual_ip: Ipv4Addr,
}

/// 指代一个分组时附带的名字（现在的名字）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    pub id: GroupId,
    pub name: String,
    /// 是不是内置的 `All`：界面不用按名字认它。比这个字段旧的 controller 不给，读成 `false`。
    #[serde(default)]
    pub builtin: bool,
}

/// controller 自己的情况。也用来检查地址和令牌对不对。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// controller 程序的版本号。
    pub version: String,
    /// controller 的公钥，写在每个注册令牌里。
    pub controller_key: PublicKey,
    /// 节点连 controller 用的 `主机:端口`，写在每个注册令牌里。
    pub public_addr: String,
    pub networks: u32,
    pub nodes: u32,
    pub online: u32,
    /// 已知的最新发布比 controller 自己新时，是那个版本号。controller 不会自己更新，这只是提醒管理员手动更新。
    #[serde(default)]
    pub newer_version: Option<Version>,
    /// controller 的数据目录（数据库、密钥、更新包的缓存都在里面），备份时拷贝它。不是从数据目录打开的（测试）时是 `null`。
    #[serde(default)]
    pub data_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub id: NetworkId,
    pub name: String,
    /// 节点全名的后缀，固定是 `<名字>.i3t`。
    pub domain: String,
    pub cidr: Ipv4Net,
    pub routing: RoutingMode,
    pub max_relay_hops: u8,
    pub reach: Reach,
    pub created_at: Timestamp,
    /// 网络里有多少个节点。
    pub nodes: u32,
    /// 网络的默认服务等级；`null` 表示 `auto`。
    #[serde(default)]
    pub class: Option<Class>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreateNetwork {
    /// 小写字母、数字和连字符，最长 63 个字符。建好之后不能改。
    pub name: String,
    /// 省略时 controller 挑一个不和别的网络重叠的私有 /16。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cidr: Option<Ipv4Net>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingMode>,
    /// 1 到 4。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_relay_hops: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<Class>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateNetwork {
    /// 网络里已经有节点时，新的地址段必须包含原来的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cidr: Option<Ipv4Net>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_relay_hops: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    /// 写 `null` 表示改回 `auto`。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub class: Option<Option<Class>>,
}

/// 一个网络的总览。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    pub network: NetworkId,
    pub nodes: u32,
    pub online: u32,
    /// 启用了 relay 的节点数。是 0 时，打不通洞的节点对之间不可达。
    pub relays: u32,
    pub exits: u32,
    pub rules: u32,
    pub listeners: u32,
    pub forwards: u32,
    pub credentials: u32,
    /// 还没被用掉、也没过期的注册令牌。
    pub pending_tokens: u32,
    pub online_nodes: Vec<NodeRef>,
    /// 两端都在线、访问规则允许、但节点上报"没有可用的路"的节点对：直连不通，也没有 relay 可走。
    pub unreachable_pairs: Vec<NodePair>,
    /// 在线节点最近一次上报的活跃流之和。一条流两端的节点各报一次，所以这里算两次。
    #[serde(default)]
    pub flows: u64,
    /// 在线节点最近一次上报的流量之和（各自启动以来经它的流发出和收到的字节数）。经过两个节点的流量两边都算。
    #[serde(default)]
    pub tx_bytes: u64,
    #[serde(default)]
    pub rx_bytes: u64,
    /// 网络里各个节点的提醒（[`Node::warnings`]），按节点编号排。
    #[serde(default)]
    pub warnings: Vec<NetworkWarning>,
}

/// 一对节点，`a` 的编号小于 `b`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodePair {
    pub a: NodeRef,
    pub b: NodeRef,
}

/// 节点列表里的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub network: NetworkId,
    pub key: PublicKey,
    pub name: String,
    /// 全名：`<名字>.<网络的域名后缀>`。
    pub fqdn: String,
    pub virtual_ip: Ipv4Addr,
    /// 它所在的分组，按名字排（不分大小写）。内置分组 `All` 每个节点都在，不在这里列出。
    pub groups: Vec<GroupRef>,
    /// 节点在本地声明的"可以承担"。没有声明的能力启用不了。
    pub declared: Declared,
    /// 启用为 relay 时有值：它的地址（controller 找出来的）和设置。
    pub relay: Option<RelayView>,
    /// 启用为出口时有值。
    pub exit: Option<ExitRole>,
    pub dial_out_only: bool,
    /// 覆盖网络默认值的选路模式；`null` 表示用网络的。
    pub routing: Option<RoutingMode>,
    pub reach: Option<Reach>,
    /// 管理员固定的挂靠 relay。
    pub pinned_relays: Vec<NodeId>,
    pub platform: String,
    pub version: String,
    pub online: bool,
    /// 最近一次收到它的消息的时间；从来没有连上过时是 `null`。
    pub last_seen_at: Option<Timestamp>,
    pub created_at: Timestamp,
    /// 这个节点的 netmap 现在是第几版。
    pub netmap_version: u64,
    /// 最近一次发给它的版本。
    pub sent_version: u64,
    /// 它确认已经应用的版本。小于 `netmap_version` 说明它还在用旧的配置。
    pub acked_version: u64,
    /// 节点上报的主机名；旧版本的节点不报，是空的。
    #[serde(default)]
    pub hostname: String,
    /// 这个节点的程序更新（白皮书 §10.5）。
    #[serde(default)]
    pub update: NodeUpdate,
    /// 节点挂靠的 relay 和到它们的延迟，取自它最近的上报（离线时是它离线前报的）。
    /// 以前只在节点详情里有；放进列表里，界面数"有几个节点挂靠在这个 relay 上"只要一次请求。
    #[serde(default)]
    pub attached: Vec<Attachment>,
    /// 出口地址（[`Node::egress_ip`]）所在的国家或地区，ISO 3166-1 的两个大写字母（`"JP"`），
    /// controller 查自带的离线地址库得出。没有出口地址、库里没有时是 `null`。
    #[serde(default)]
    pub country: Option<String>,
    /// 出口地址：这个节点发往互联网的流量出去时，互联网看到的公网地址。controller 看到的来源地址是公网地址时就是它；
    /// 不是（节点在 controller 所在的局域网里、或者同一台机器上）时取 relay 看到的公网地址，几个 relay 看到的不一样时取看到它的
    /// relay 最多的那个（一样多时取最近一次看到的）。只从内网地址被看到过时是 `null`。出口配了上游代理时，经它出去的流量其实是从
    /// 代理那里出去的，这里不知道。
    #[serde(default)]
    pub egress_ip: Option<IpAddr>,
    /// 管理员给它选的性能档位；`null` 表示没有选，节点用本机配置文件里的（默认 `balanced`）。节点实际在用的见节点详情的 `performance`。
    #[serde(default)]
    pub profile: Option<Profile>,
    /// controller 发现的、值得提醒的情况，按节点最近一次报的东西算（离线时是它离线前报的）。没有时是空的。
    #[serde(default)]
    pub warnings: Vec<NodeWarning>,
}

/// 一个节点的程序更新：能不能自己更新、有没有新版本、进行到哪一步。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NodeUpdate {
    /// 节点自己报的能不能自己更新。`null` 表示它的版本太旧，不懂更新，只能手动换程序。
    pub self_update: Option<SelfUpdate>,
    /// 现在可以让它更新到的版本：已知的发布比它新、里面有它的平台的包、controller 也够新。没有时是 `null`。
    /// 不管它能不能自己更新都给出，界面据此标"有新版本"。
    pub available: Option<Version>,
    /// 节点本次连接期间最近一次报的进度。不在线、或者没有更新在进行时是 `idle`。
    pub status: UpdateStatus,
    /// controller 已经在这次连接上发给它的更新指令。
    pub ordered: Option<UpdateOrder>,
    /// controller 想让它更新、但还没有下发指令的原因。
    pub waiting: Option<UpdateWait>,
    /// 它最近一次"这个版本在这台机器上不能用"的失败。有这条记录时，自动更新不会再让它装同一个版本。
    pub failure: Option<UpdateFailure>,
}

/// controller 发给节点的一条更新指令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOrder {
    pub version: Version,
    /// `idle` 是自动更新，`now` 和 `after_drain` 是管理员强制的。
    pub timing: UpdateTiming,
}

/// controller 想让一个节点更新、但还没有下发指令的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateWait {
    /// 这个平台的包 controller 还没有从发布源取回来。
    Package,
    /// 它是 relay：同一个网络里的 relay 一个一个更新，要等前一个更新完重新上线。
    RelayTurn,
    #[serde(other)]
    Unknown,
}

/// 一次归因于版本的更新失败。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateFailure {
    pub version: Version,
    pub kind: FailureKind,
    /// 节点报的原因。
    pub reason: String,
    /// controller 记下它的时间。
    pub at: Timestamp,
    /// `reason` 的代码和它要用到的值。旧版本的节点不报，这个字段出现之前的记录也没有。
    #[serde(default)]
    pub cause: Option<UpdateCause>,
}

/// 更新的全局设置和现状。全部网络共用一份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOverview {
    /// 自动更新的开关。开着时，有新版本的节点在空闲的时候自己更新。
    pub auto_update: bool,
    /// 发布源：一个 `http(s)://` 地址前缀，或者 controller 所在机器上的一个目录。
    pub source: String,
    /// `source` 是不是项目默认的发布源。
    pub source_is_default: bool,
    /// controller 自己的版本。
    pub controller_version: Version,
    /// 已知的最新正式版；还没有查到过时是 `null`。
    pub release: Option<ReleaseInfo>,
    /// 最近一次查发布源的时间；还没有查过时是 `null`。
    pub checked_at: Option<Timestamp>,
    /// 最近一次查发布源失败的原因；成功时是 `null`。失败时 `release` 保持上一次查到的。
    pub check_error: Option<String>,
    /// `check_error` 的代码和它要用到的值；成功时是 `null`。
    #[serde(default)]
    pub check_cause: Option<ReleaseCause>,
    /// 因为有节点更新失败而暂停自动更新的版本。
    pub paused: Vec<PausedVersion>,
    pub nodes: UpdateCounts,
}

/// 查发布源、取更新包为什么没有成功：一个代码，加上把它写成一句话要用到的值。和它在一起的是中文原话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ReleaseCause {
    /// 发布源的写法不对。
    InvalidSource,
    /// 从这个地址取不到。`error` 是 HTTP 库的原话（连不上、HTTP 状态码等）。
    FetchFailed { url: String, error: String },
    /// 发布源是本机的目录，读写里面的文件失败。`error` 是系统的原话。
    Io { path: String, error: String },
    /// 发布源给的签名文件不是一个签名。
    BadSignatureFile,
    /// 清单的签名不是任何一把受信任的发布公钥签的。
    BadSignature,
    /// 签名正确，但清单的内容读不懂。
    BadManifest,
    /// 发布源上最新的是预发布版，不取。
    Prerelease { version: Version },
    /// 清单里的文件名不是一个单纯的文件名。
    BadFileName { file: String },
    /// 取回来的包和清单对不上。
    Mismatch { file: String, expected_size: u64, actual_size: u64 },
    /// controller 内部的原因（比如没有放包的缓存目录），细节在它的日志里。
    Internal,
    /// 比这个程序新的 controller 给的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 已知的最新正式版。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: Version,
    /// 发布时间。
    pub published: Timestamp,
    /// 这个版本的节点需要的 controller 最低版本。
    pub min_controller: Version,
    /// controller 比 `min_controller` 旧：这个版本不会下发给任何节点，要先手动更新 controller。
    pub controller_too_old: bool,
    /// 这个版本比 controller 自己新：提醒管理员手动更新 controller。
    pub newer_than_controller: bool,
    /// 发布里各个平台的节点程序，和它们在 controller 上的缓存情况。
    pub packages: Vec<PackageInfo>,
}

/// 发布里一个平台的节点程序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInfo {
    /// 平台名，例如 `linux-x86_64`。
    pub platform: String,
    pub file: String,
    /// 字节数。
    pub size: u64,
    pub state: PackageState,
    /// 最近一次取这个包失败的原因。
    pub error: Option<String>,
    /// `error` 的代码和它要用到的值。
    #[serde(default)]
    pub cause: Option<ReleaseCause>,
}

/// 一个平台的包在 controller 上的缓存情况。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    /// 没有要更新的节点用这个平台，所以没有取。
    NotNeeded,
    /// 要用，正在取，或者排着队等着取。
    Fetching,
    /// 已经取回来，核对过大小和 SHA-256。
    Cached,
    /// 没有取成，下次查发布源时再试。
    Failed,
    #[serde(other)]
    Unknown,
}

/// 一个被暂停自动更新的版本，和导致暂停的那次失败。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedVersion {
    pub version: Version,
    /// 报告失败的节点；它后来被删掉了的话是 `null`。
    pub node: Option<NodeRef>,
    /// 那个节点所在的网络。
    pub network: Option<NetworkId>,
    pub reason: String,
    pub at: Timestamp,
    /// `reason` 的代码和它要用到的值。旧版本的节点不报。
    #[serde(default)]
    pub cause: Option<UpdateCause>,
}

/// 全部网络的节点按更新情况数出来的数。一个节点只算在一项里，按下面的顺序取第一个符合的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateCounts {
    pub total: u32,
    /// 正在更新：controller 已经下发了指令，还没有结束。
    pub updating: u32,
    /// 有新版本，但上一次装它失败了。
    pub failed: u32,
    /// 有新版本，但节点不能自己更新，要手动更新。
    pub manual: u32,
    /// 有新版本，可以更新。
    pub outdated: u32,
    /// 没有可以更新到的版本。
    pub up_to_date: u32,
}

/// 改更新的全局设置。字段都可以省略。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
    /// 写 `null` 表示改回项目默认的发布源。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub source: Option<Option<String>>,
}

/// 让节点立即更新。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForceUpdate {
    /// `now`（马上重启，正在进行的流会断）或者 `after_drain`（不再接新的流，手上的结束之后再重启）。
    pub timing: UpdateTiming,
}

/// "全部更新"的结果：每个节点落在哪一类。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ForcedUpdates {
    /// 要更新到的版本。
    pub version: Option<Version>,
    /// 指令已经发出去的节点。
    pub ordered: Vec<NodeRef>,
    /// 已经记下、稍后下发的节点：等包取回来，或者是 relay 在等轮到它。
    pub queued: Vec<NodeRef>,
    /// 没有动的节点和原因。
    pub skipped: Vec<SkippedUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedUpdate {
    pub node: NodeRef,
    /// 给人看的原因：不在线、不能自己更新、已经是最新的、没有它的平台的包。
    pub reason: String,
    /// `reason` 的代码和它要用到的值。
    #[serde(default)]
    pub cause: Option<SkipCause>,
}

/// 一个节点为什么不能立即更新。`POST /update/force` 跳过它的原因；单独让它更新时，同一个代码在错误的 `what` 里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SkipCause {
    /// 节点不在线。
    Offline,
    /// 节点的版本太旧，不懂更新。
    TooOld,
    /// 节点说它不能自己更新；`blocker` 是它说的原因，说不出来时是 `null`。
    CannotSelfUpdate { blocker: Option<SelfUpdateBlocker> },
    /// 还没有从发布源查到任何发布。
    NoRelease,
    /// 最新的发布 `version` 要求 controller 至少是 `min_controller`，现在的 controller 是 `controller`。
    ControllerTooOld { version: Version, min_controller: Version, controller: Version },
    /// 节点报的版本号看不懂。
    UnreadableVersion { version: String },
    /// 节点已经是最新的。
    UpToDate { running: Version, latest: Version },
    /// 发布里没有它的平台的包。
    NoPackage { version: Version, platform: String },
    /// 比这个程序新的 controller 给的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 继续一个被暂停的版本的自动更新。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeUpdate {
    pub version: Version,
}

/// 节点详情：列表里的字段，加上它最近上报的东西。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDetail {
    #[serde(flatten)]
    pub node: Node,
    /// 节点上报的 UDP 候选端点。
    pub endpoints: Vec<SocketAddr>,
    /// controller 看到的来源 IP 加上节点自报的 UDP 端口。
    pub observed_endpoint: Option<SocketAddr>,
    // 挂靠的 relay（`attached`）原来在这里，现在在 `Node` 里，JSON 里还是同一层的同一个字段。
    /// 映射出来的端口是不是随对端变化。是的话这台机器所在的网络基本打不了洞。节点不在线或者还不知道时是 `null`。
    pub mapping_varies: Option<bool>,
    /// 下面三项是节点本次连接期间最近一次上报的值，不在线时是 0。
    pub flows: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// 节点本次连接期间最近一次报的独占 TCP（lane）的状况。不在线、旧版本、或者还没报过时是 `null`。
    #[serde(default)]
    pub lanes: Option<LaneReport>,
    /// 上面这些上报的值（`lanes`、`flows`、流量、`mapping_varies`）是多少秒之前报的；这次连接上还没有上报过是 `null`。
    #[serde(default)]
    pub reported_secs_ago: Option<u64>,
    /// 节点本次连接期间最近一次报的、它实际在用的性能档位。不在线、旧版本、或者还没报过时是 `null`。
    #[serde(default)]
    pub performance: Option<PerformanceReport>,
    /// 节点本次连接期间最近一次报的透明访问的状况。不在线、旧版本、或者还没报过时是 `null`。
    #[serde(default)]
    pub intercept: Option<InterceptReport>,
    /// 这个节点是 relay 时：报过试它的结果的每个节点，按节点编号排，最多是网络里的节点数。不是 relay 时是 `null`。
    #[serde(default)]
    pub reached_by: Option<Vec<ReachEntry>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 整个替换节点所在的分组。`All` 的编号会被忽略：每个节点本来就在里面。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<GroupId>>,
    /// 改虚拟 IP 会让引用旧地址的配置失效。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_ip: Option<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_out_only: Option<bool>,
    /// 写 `null` 表示改回用网络的默认值。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub routing: Option<Option<RoutingMode>>,
    /// 写 `null` 表示改回用网络的默认值。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub reach: Option<Option<Reach>>,
    /// 整个替换固定的挂靠 relay；空数组表示不固定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_relays: Option<Vec<NodeId>>,
    /// 性能档位。写 `null` 表示不再指定，节点改用本机配置文件里的。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub profile: Option<Option<Profile>>,
}

/// 区分"字段没写"和"字段写了 `null`"：前者读成 `None`，后者读成 `Some(None)`。
fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// relay 的设置：启用 relay 时的请求体（`PUT …/relay`），也是 controller 存着的那一份。
///
/// 地址不在这里：controller 自己找出来（白皮书 §7.2），见 [`RelayView::addresses`]。
/// 早先的请求体里的 `udp`、`tcp`、`region` 照收，没有作用（不认识的字段一律忽略）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelaySettings {
    /// 转发的带宽上限；`null` 表示不限。
    #[serde(default)]
    pub bandwidth_mbps: Option<u32>,
}

/// 一个启用了的 relay（`Node.relay`）：别的节点经哪些地址连它，和它的设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayView {
    /// controller 找出来的地址，只读，按节点去试的顺序排，最多 [`MAX_RELAY_ADDRESSES`] 个。
    /// 每个地址 UDP 和 TCP 都用。还不知道任何地址（节点从没连上过）时是空的，relay 照样是启用的。
    pub addresses: Vec<RelayAddress>,
    /// 转发的带宽上限；`null` 表示不限。
    pub bandwidth_mbps: Option<u32>,
    /// 在线的节点里有几个连得上它（白皮书 §7.2，节点自己上报的）。只是情况，不是毛病：只有一部分节点连得上的 relay 照样是那些节点的 relay。
    #[serde(default)]
    pub reach: RelayReach,
    /// 谁在用它：最近一个统计时段里它替哪些节点对转发了多少，取自它最近一次上报。不在线、版本太旧不报时是 `null`。
    #[serde(default)]
    pub users: Option<RelayUse>,
}

/// 在线的节点（不算 relay 自己）最近一次试一个 relay 的结果，数出来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelayReach {
    /// 连得上的（UDP 或者 TCP）。
    pub reached: u32,
    /// 试过、连不上的。
    pub unreached: u32,
    /// 还没有报过的（刚上线，或者版本太旧不报）。
    pub unknown: u32,
}

/// 一个节点最近一次试某个 relay 的结果（节点详情的 `reached_by` 里的一项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachEntry {
    pub node: NodeRef,
    pub online: bool,
    pub outcome: TryOutcome,
    /// 什么时候试的。有会话的节点每次上报都更新它；结果没有变时 controller 最多五分钟记一次。
    pub at: Timestamp,
}

/// 一个 relay 最多有几个地址。多出来的（多半是网卡上的）不下发：节点每个都要试，试的都是同一台机器。
pub const MAX_RELAY_ADDRESSES: usize = 8;

/// relay 的一个地址和它是从哪里来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayAddress {
    pub address: SocketAddr,
    pub source: AddressSource,
}

/// relay 的地址是从哪里来的（白皮书 §7.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressSource {
    /// 在节点上声明的（`i3t set relay-address`）：机房或者路由器上映射进来的地址。
    Declared,
    /// controller 看到的：节点连 controller 时的来源 IP，加上节点自报的端口。
    Observed,
    /// 节点网卡上的地址，加上节点自报的端口。同一个局域网里的节点靠它连上。
    Local,
    /// 比这个程序新的 controller 给的、这里不认识的来源。
    #[serde(other)]
    Unknown,
}

/// controller 发现的、值得提醒的情况。只是提醒，不挡着任何事。不认识的 `code` 读成 [`NodeWarning::Unknown`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum NodeWarning {
    /// 节点的一个网卡地址落在网络的虚拟地址段里（白皮书 §4.6）：在这台机器上，连这个地址段里的虚拟 IP 可能连到真实网络上去，
    /// 透明访问也装不上。`address` 是那个网卡地址。
    RangeOverlap { address: IpAddr },
    /// 这个 relay 启用着，在线的节点里试过它的有 `tried` 个，一个也没有连上。只是如实说一句：哪天有节点连得上它，它就是那些节点的 relay。
    RelayUnreached { tried: u32 },
    #[serde(other)]
    Unknown,
}

/// 网络的总览里的一条提醒：哪个节点，什么情况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkWarning {
    pub node: NodeRef,
    #[serde(flatten)]
    pub warning: NodeWarning,
}

/// 把一个节点指定为另一个已下线节点的继任。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Succeed {
    /// 被继任的节点。它必须不在线，继任之后它被删除。
    pub predecessor: NodeId,
}

/// 一个节点最近上报的路由表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteTable {
    pub node: NodeRef,
    /// 节点不在线时路由表是空的。
    pub online: bool,
    pub routes: Vec<NodeRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRoute {
    pub peer: NodeRef,
    pub route: Route,
    pub rtt_ms: Option<u32>,
    /// 这一条是多少秒之前的：节点最近一次上报到现在过了多久。节点不在线时是它离线前最后报的。
    #[serde(default)]
    pub age_secs: u64,
    /// controller 看到这条路换上来已经多少秒了：从它第一次报这条路算起。controller 重启之后从头算。
    #[serde(default)]
    pub since_secs: u64,
    /// 走直连时，这条直连是怎么建立起来的；节点没有说（版本太旧，或者它也不知道）时是 `null`。
    #[serde(default)]
    pub direct: Option<DirectKind>,
    /// 经 relay 时，节点到第一个 relay 的那一段走的是 UDP 还是 TCP；节点没有说时是 `null`。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
    /// 和这个对端的共用连接上现在开着几条流、节点启动以来两个方向各走了多少字节，取自节点最近一次上报。
    /// 现在没有连接、节点不在线或者版本太旧不报时是 `null`。
    #[serde(default)]
    pub flows: Option<u32>,
    #[serde(default)]
    pub tx_bytes: Option<u64>,
    #[serde(default)]
    pub rx_bytes: Option<u64>,
}

/// 整个网络的路由矩阵：每个节点到它的各个对端现在怎么走，取自各节点最近的上报（白皮书 §7.7）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteMatrix {
    /// 网络里的全部节点，按编号排。
    pub nodes: Vec<MatrixNode>,
    /// 有上报的格子，按 `from`、`to` 排。没有出现的格子是没有数据：两个节点之间没有访问关系，或者 `from` 还没有上报过。
    pub entries: Vec<MatrixEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixNode {
    #[serde(flatten)]
    pub node: NodeRef,
    pub online: bool,
}

/// 路由矩阵里的一格：`from` 到 `to` 这个方向。两个方向各有一格，路可以不一样。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixEntry {
    pub from: NodeId,
    pub to: NodeId,
    pub route: Route,
    pub rtt_ms: Option<u32>,
    /// 这一格是多少秒之前的：`from` 最近一次上报到现在过了多久。节点每 10 秒上报一次，在线时一般不超过 10。
    pub age_secs: u64,
    /// 这一格不是 `from` 现在这次连接报的，已经过期：它不在线（这是它离线之前最后报的），或者刚重新连上还没有上报。
    pub stale: bool,
    /// 同 [`NodeRoute::direct`]。
    #[serde(default)]
    pub direct: Option<DirectKind>,
    /// 同 [`NodeRoute::relay_leg`]。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
}

/// 两个节点之间两个方向的当前路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathView {
    /// `from` 到 `to`。
    pub forward: PathLeg,
    /// `to` 到 `from`。
    pub reverse: PathLeg,
}

/// 一个方向的路径：发送方现在怎么把包送到对端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathLeg {
    /// 发送方：这一段是它说的。
    pub from: NodeRef,
    pub to: NodeRef,
    pub source: PathSource,
    /// 现在走的路；没有数据时是 `null`。
    pub route: Option<Route>,
    pub rtt_ms: Option<u32>,
    /// 这份数据是多少秒之前的。现问的是 0；没有数据时是 `null`。
    pub age_secs: Option<u64>,
    /// 这条路是多久之前换上的、为什么换。只有现问到、并且节点这次启动以来换过路时才有。
    pub since: Option<RouteSince>,
    /// 走直连时，这条直连是怎么建立起来的。现问到的取自回答，否则取自上报；都没有时是 `null`。
    #[serde(default)]
    pub direct: Option<DirectKind>,
    /// 经 relay 时，发送方到第一个 relay 的那一段走的是 UDP 还是 TCP；不知道时是 `null`。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
    /// 发送方用的选路模式。只有现问到、并且发送方的版本会说时才有。
    #[serde(default)]
    pub mode: Option<RoutingMode>,
    /// 发送方的路由层说的"为什么是现在这条路"。有没有同 `mode`。
    #[serde(default)]
    pub reason: Option<String>,
    /// `reason` 的代码和它要用到的值。发送方的版本太旧不报时是 `null`。
    #[serde(default)]
    pub cause: Option<RouteCause>,
    /// 发送方考虑过的候选路径，按代价从低到高，现在走的那条标着 `current`。有没有同 `mode`：`null` 是不知道，
    /// 空数组是发送方手上还没有任何测量值。
    #[serde(default)]
    pub candidates: Option<Vec<PathCandidate>>,
    /// 这对节点之间的连接自己量的往返时间（毫秒）。`rtt_ms` 是路由层按各段的探测推算的，和它放在一起看：差得多说明测量失真。
    /// 只有现问到、有连接、并且发送方的版本会说时才有。
    #[serde(default)]
    pub connection_rtt_ms: Option<u32>,
    /// 这个方向最近的换路，新的在前。一对节点两个方向一共留 [`KEEP_SWITCHES`] 条，controller 从节点的上报里攒的，重启之后还在。
    #[serde(default)]
    pub switches: Vec<SwitchView>,
}

/// 一对节点（两个方向一共）留几条换路的记录。
pub const KEEP_SWITCHES: usize = 20;

/// 一次换路：什么时候、从什么换成了什么、为什么（原因的代码，[`RouteCause`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchView {
    pub at: Timestamp,
    pub from: Route,
    pub to: Route,
    pub cause: RouteCause,
}

/// 网络里 relay 之间的延迟矩阵（白皮书 §7.7），每个 relay 一行。
///
/// 这些行是 relay 自己测、自己签名之后发给别的 relay 和挂靠它的节点的；controller 现问几个在线的节点，取每个 relay 最新的那一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackboneView {
    /// 网络里启用了 relay 的节点，按编号排。
    pub relays: Vec<BackboneRelay>,
    /// 这次回答了的节点。没有一个回答时（没有在线的、版本够新的节点）每个 relay 的行都是空的。
    pub answered: Vec<NodeRef>,
}

/// 骨干矩阵里一个 relay 的那一行。问到的节点手上都没有这一行时，`state`、`load_percent`、`age_ms`、`reported_by` 都是 `null`，
/// `links` 是空的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackboneRelay {
    pub relay: NodeRef,
    /// 这个 relay 现在连着 controller。
    pub online: bool,
    pub state: Option<RelayState>,
    /// 带宽占用的百分比；没有设上限的 relay 是 0。
    pub load_percent: Option<u8>,
    /// 这一行是多少毫秒之前的：回答的节点收到它到现在过了多久。超过两分钟的行节点不再留，所以不会更旧。
    pub age_ms: Option<u64>,
    /// 这一行是从哪个节点问来的。
    pub reported_by: Option<NodeId>,
    /// 它到别的 relay 的链路。`to` 是 relay 的节点编号。
    pub links: Vec<BackboneLink>,
    /// 在线的节点里有几个连得上它，同 [`RelayView::reach`]。
    #[serde(default)]
    pub reach: RelayReach,
}

/// 一段路径的数据是从哪里来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSource {
    /// 刚刚问的发送方，它用当前的测量值答的。
    Query,
    /// 发送方最近一次上报的路由表。它现在不在线、版本太旧不会回答，或者没有按时回答。
    Report,
    /// 没有数据：发送方没有上报过到这个对端的路，现在也问不到。
    None,
    /// 发送方说对端不在它的 netmap 里：访问规则没有把这两个节点连通（或者配置还没有下发到）。
    NotPeers,
    #[serde(other)]
    Unknown,
}

/// 一个注册令牌。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollTokenInfo {
    pub id: u32,
    pub network: NetworkId,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    /// 用它加入的节点自动加入的分组，按名字排。不含 `All`。
    pub groups: Vec<GroupRef>,
    /// 预设的虚拟 IP。
    pub virtual_ip: Option<Ipv4Addr>,
    /// 用它加入的节点继任这个节点。
    pub successor_of: Option<NodeId>,
    /// 令牌文本的开头一小段，不是秘密，用来认出是哪一个令牌。这个字段出现之前生成的令牌是空的。
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub state: TokenState,
    /// 被用掉的时间。
    #[serde(default)]
    pub used_at: Option<Timestamp>,
    /// 用它加入的节点；还没用、或者那个节点后来被删掉了时是 `null`。
    #[serde(default)]
    pub used_by: Option<NodeRef>,
}

/// 一个注册令牌现在的情况。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenState {
    /// 还能用。
    #[default]
    Pending,
    /// 已经有节点用它加入了。
    Used,
    /// 没有被用过，已经过期。
    Expired,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreateEnrollToken {
    /// 多少秒之后过期，省略时是一天。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
    /// 用它加入的节点自动加入的分组。`All` 的编号会被忽略。
    #[serde(default)]
    pub groups: Vec<GroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_ip: Option<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_of: Option<NodeId>,
}

/// 创建注册令牌的应答。`token` 是整段令牌文本（`i3t-…`），只在这里出现一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedEnrollToken {
    #[serde(flatten)]
    pub info: EnrollTokenInfo,
    pub token: String,
}

/// 一条访问规则：分组 `src` 里的节点可以访问分组 `dst` 里的节点。
///
/// 规则记的是分组的编号，名字只是给人看的：分组改名之后规则照常生效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u32,
    pub name: String,
    pub src: GroupRef,
    pub dst: GroupRef,
    pub direction: Direction,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// `src <-> dst`：两边可以互相访问。
    #[default]
    Both,
    /// `src -> dst`：只有 `src` 一侧能发起。
    OneWay,
}

/// 新建一条规则。`src`、`dst` 是这个网络里已有的分组。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRule {
    #[serde(default)]
    pub name: String,
    pub src: GroupId,
    pub dst: GroupId,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<GroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst: Option<GroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

const fn enabled_by_default() -> bool {
    true
}

/// 一个接入凭据。令牌本身查不回来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub id: u32,
    pub name: String,
    /// 同时进行的连接数上限；`null` 表示不限。
    pub max_concurrent: Option<u32>,
    pub created_at: Timestamp,
    /// 令牌的开头一小段，不是秘密，用来认出是哪一个凭据。这个字段出现之前签发的凭据是空的。
    #[serde(default)]
    pub prefix: String,
    /// 流量上限，字节：经各个入口用它搬运的字节数（两个方向相加）到了这个数，它就停用，直到上限调高或者用量清零。
    /// `null` 表示不限。
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub state: CredentialState,
    /// 吊销的时间；没有吊销是 `null`。
    #[serde(default)]
    pub revoked_at: Option<Timestamp>,
    #[serde(default)]
    pub usage: CredentialUsage,
}

/// 一个凭据现在能不能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    #[default]
    Active,
    /// 流量上限用完了。入口拒绝它的新连接，断开正在进行的连接。
    Exhausted,
    /// 被吊销了。入口不再认它；记录留着，看得到它用过多少。
    Revoked,
    #[serde(other)]
    Unknown,
}

/// 一个凭据的用量，各个入口上报的汇总。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CredentialUsage {
    /// 签发（或者上次清零）以来经它搬运的字节数，两个方向相加。流量上限比的就是它。
    pub bytes: u64,
    /// 现在在线的入口上正在进行的连接数。
    pub connections: u32,
    /// 最近一次有连接用它的时间；从来没有用过是 `null`。
    pub last_used_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredential {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

/// 改一个凭据的上限，不用重新签发令牌。字段都可以省略。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateCredential {
    /// 写 `null` 表示不限。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<Option<u32>>,
    /// 写 `null` 表示不限。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<Option<u64>>,
    /// `true` 时把用量清零（比如开始一个新的计费周期）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset_usage: bool,
}

/// 创建凭据的应答。`token` 是调用者填在代理密码字段里的东西，只在这里出现一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedCredential {
    #[serde(flatten)]
    pub credential: Credential,
    pub token: String,
}

/// 一个节点上的一个代理入口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listener {
    pub id: u32,
    /// 它开在哪个节点上。
    pub node: NodeId,
    #[serde(flatten)]
    pub config: crate::netmap::Listener,
    /// 节点最近一次报的这个监听器的状态。节点不在线、还没有报过，或者版本太旧不报时是 `null`。
    #[serde(default)]
    pub status: Option<ListenStatus>,
}

/// 节点报的一个监听（代理监听器或者端口转发的监听一侧）现在的状态、连接数和流量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenStatus {
    #[serde(flatten)]
    pub state: ListenState,
    #[serde(flatten)]
    pub traffic: ListenTraffic,
}

/// 一个节点上的全部监听：controller 下发的监听器、节点按本机配置自己开的回环入口、端口转发的监听一侧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeListeners {
    pub node: NodeRef,
    pub online: bool,
    /// 节点最近一次上报是多少秒之前；没有上报过是 `null`。
    pub reported_secs_ago: Option<u64>,
    pub listeners: Vec<NodeListener>,
    pub forwards: Vec<NodeForwardListen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeListener {
    /// controller 上的监听器编号；节点自己开的回环入口没有，是 `null`。
    pub id: Option<u32>,
    /// 节点按本机的配置文件自己开的回环入口（`local-socks5`、`local-http`）。
    pub builtin: bool,
    pub name: String,
    pub bind: SocketAddr,
    pub protocol: ListenerProtocol,
    #[serde(flatten)]
    pub status: ListenStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeForwardListen {
    /// 端口转发的编号。
    pub id: u32,
    pub bind: SocketAddr,
    pub protocol: ForwardProtocol,
    #[serde(flatten)]
    pub status: ListenStatus,
}

/// 创建或者整个替换一个监听器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerInput {
    pub node: NodeId,
    #[serde(flatten)]
    pub config: crate::netmap::Listener,
}

/// 一条端口转发。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub id: u32,
    pub name: String,
    pub protocol: ForwardProtocol,
    pub listen_node: NodeId,
    /// 监听节点上的监听地址。IP 是监听节点的虚拟 IP 时不占用操作系统的端口，只对组网里被允许的节点可达。
    pub bind: SocketAddr,
    pub target_node: NodeId,
    /// 目标节点去连的地址：它的回环地址，或者它所在局域网里的地址。
    pub target_addr: SocketAddr,
    /// 来源白名单；空表示不限制。
    pub allow_from: Vec<IpNet>,
    pub enabled: bool,
    pub status: ForwardStatus,
    /// 这条转发上的流的服务等级；`null` 表示用网络的默认值。
    #[serde(default)]
    pub class: Option<Class>,
    /// 监听节点最近一次报的这条转发的监听状态：在监听（端口写 0 时 `local` 是节点挑定的端口）、端口被占用、绑定失败等，
    /// 以及连接数和流量。节点不在线或者还没有报过时是 `null`。
    #[serde(default)]
    pub listen: Option<ListenStatus>,
    /// 这条转发的流最近一次在目标那里失败（目标连不上）：原因和时间。之后有流成功了、从来没有失败过、
    /// 或者监听节点不在线（或者版本太旧不报）时是 `null`。
    #[serde(default)]
    pub target_failure: Option<ForwardFailure>,
    /// 目标节点连上目标之后先发一个 PROXY protocol 的头（`"v1"`、`"v2"`），服务据此看到发起节点的虚拟 IP；`null` 是不发。只有 TCP 的转发有。
    #[serde(default)]
    pub proxy_protocol: Option<ProxyProtocol>,
}

/// 端口转发的流最近一次在目标那里失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardFailure {
    pub cause: TargetCause,
    /// 什么时候，按 controller 的时钟。
    pub at: Timestamp,
}

/// 创建或者整个替换一条端口转发。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardInput {
    #[serde(default)]
    pub name: String,
    pub protocol: ForwardProtocol,
    pub listen_node: NodeId,
    pub bind: SocketAddr,
    pub target_node: NodeId,
    pub target_addr: SocketAddr,
    #[serde(default)]
    pub allow_from: Vec<IpNet>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<Class>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_protocol: Option<ProxyProtocol>,
}

/// 一条转发现在的情况，按 controller 知道的算。排在前面的优先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardStatus {
    /// 目标节点没有声明（或者撤回了声明）可以作为端口转发的目标。这时转发一定是停用的，也启用不了。
    TargetNotDeclared,
    /// 被停用了。
    Disabled,
    /// 访问规则不允许监听节点访问目标节点，转发没有下发。
    AccessDenied,
    ListenerOffline,
    TargetOffline,
    /// 已经下发，两端都在线。
    Active,
}

/// 一对节点之间的设置。`a` 的编号小于 `b`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairSetting {
    pub a: NodeId,
    pub b: NodeId,
    /// 这一对节点之间的连接常驻，不因为没有流而关闭。
    pub keep_alive: bool,
    /// 这一对节点之间承载 TCP 连接的流用不用独占 TCP（lane）。
    #[serde(default)]
    pub lane: LaneMode,
    /// 这一对节点之间的连接用哪种拥塞控制；`null` 表示用节点的默认（BBR）。
    #[serde(default)]
    pub congestion: Option<Congestion>,
}

/// 改一对节点的设置。没有写的项不变（这一对还没有设置时取默认值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PairInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<LaneMode>,
    /// 写 `null` 表示改回节点的默认。
    #[serde(default, deserialize_with = "nullable", skip_serializing_if = "Option::is_none")]
    pub congestion: Option<Option<Congestion>>,
}

/// 节点对视图里的一项：设置，加上独占 TCP 起不起作用、两边报上来的状况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairView {
    #[serde(flatten)]
    pub setting: PairSetting,
    /// 独占 TCP 的设置起不起作用。`lane` 是 `off` 时没有。
    #[serde(default)]
    pub lane_effect: Option<LaneEffect>,
    /// 两个节点各自报上来的、和对方之间的 lane（不在线或者没报的节点不在里面）。
    #[serde(default)]
    pub lanes: Vec<PairLaneSide>,
}

/// 独占 TCP 的设置起不起作用，为什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneEffect {
    /// 至少一个节点能拨另一个节点的 TCP 端口，两个节点都懂独占 TCP，访问规则连通了它们。
    pub effective: bool,
    /// 不起作用的原因，`effective` 为真时是空的。
    #[serde(default)]
    pub blocked: Vec<LaneBlock>,
    /// 拨号的节点：另一个节点启用了 relay、有已知的 TCP 地址。两个都能拨时各自用自己拨的 lane。
    #[serde(default)]
    pub dialers: Vec<NodeId>,
    /// 给人看的说明。独占 TCP 只承载 TCP 流：这一对节点之间共用的连接和它的路照常存在，UDP 应用的数据和别的流走它，
    /// 路由视图里显示的是共用连接的路。
    pub summary: String,
    /// 设置是 `udp_fallback` 时，两个节点各自开新的 TCP 流现在走不走独占 TCP。只有在线、并且报了这一项的节点在里面；
    /// 设置是别的值时是空的。
    #[serde(default)]
    pub opening: Vec<LaneOpening>,
}

/// 一个节点开新的 TCP 流现在走不走独占 TCP（`udp_fallback` 时），按它自己看到的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneOpening {
    /// 开流的节点。
    pub node: NodeId,
    /// 它到对方的共用连接现在有没有 UDP 的路。
    pub udp_path: bool,
    /// 它现在开的新 TCP 流走独占 TCP：设置起作用，并且共用连接上没有 UDP 的路。取不到 lane 时（控制连接不在、池空了）
    /// 照样悄悄改走共用的连接，见 `lanes` 里的 `control` 和 `fallbacks`。
    pub lanes: bool,
}

/// 独占 TCP 的设置不起作用的一个原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LaneBlock {
    /// 两个节点都没有启用 relay：没有哪一边的 TCP 端口在听。
    NoRelay,
    /// 启用了 relay 的这个节点还没有已知的地址（它还没有连上过 controller）。
    NoTcpAddress { node: NodeId },
    /// 这个节点的版本太旧，不懂独占 TCP（或者还从来没有连上过 controller）。
    NodeTooOld { node: NodeId },
    /// 访问规则没有连通这两个节点：它们互相不在对方的 netmap 里。
    NotConnected,
}

/// 一个节点报上来的、和对方之间的 lane。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairLaneSide {
    /// 报的节点。
    pub node: NodeId,
    pub lanes: PeerLanes,
    /// 这些数是多少秒之前报的。
    #[serde(default)]
    pub reported_secs_ago: Option<u64>,
}

/// 访问关系图的分组视图：每个分组是一个点，每条规则是一条连线。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGraph {
    /// 全部分组，和分组列表一样：`All` 在最前面，其余按名字排。
    pub groups: Vec<Group>,
    pub rules: Vec<Rule>,
    /// 没有被任何启用的规则连通的节点。
    pub unconnected: Vec<NodeRef>,
    /// 还没有确认当前版本配置的节点（例如不在线）。它们还在按旧的规则判定。
    pub unacknowledged: Vec<NodeRef>,
}

/// 访问关系图的节点视图：选中一个节点，它能访问谁、谁能访问它、各是哪些规则允许的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAccess {
    pub node: NodeRef,
    pub can_reach: Vec<AccessEdge>,
    pub reachable_by: Vec<AccessEdge>,
    /// 这个节点是否已经确认当前版本的配置。
    pub acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessEdge {
    pub node: NodeRef,
    /// 允许这条访问的规则的编号。
    pub rules: Vec<u32>,
}

/// 访问检查的结果：`from` 能不能访问 `to`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessCheck {
    pub from: NodeRef,
    pub to: NodeRef,
    pub allowed: bool,
    /// 允许这条访问的规则；不允许时是空的。
    pub rules: Vec<Rule>,
}

/// 一条操作记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub at: Timestamp,
    /// 这次操作属于哪个网络；和网络无关的操作是 `null`。
    pub network: Option<NetworkId>,
    /// 做了什么，例如 `node.rename`。
    pub action: String,
    /// 给人看的说明。
    pub detail: String,
    /// 这条记录说的是哪些东西，界面可以据此做成链接。这个字段出现之前的记录是空的。
    #[serde(default)]
    pub refs: Vec<LogRef>,
    /// 按名字给出的参数：只用 `action` 和它们就能把这条记录写成一句话。东西的名字是当时的名字，挨着它的编号。
    /// 每种 `action` 有哪些参数见 `docs/admin-api.md` 第 3 节；这个字段出现之前的记录是空的。
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// 操作记录里提到的一样东西。东西后来被删掉了，编号还在。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogRef {
    pub kind: RefKind,
    pub id: u32,
}

impl LogRef {
    #[must_use]
    pub const fn new(kind: RefKind, id: u32) -> Self {
        Self { kind, id }
    }

    #[must_use]
    pub const fn node(id: NodeId) -> Self {
        Self { kind: RefKind::Node, id: id.get() }
    }
}

/// `node:3` 这样的写法，用在 `GET /log` 的 `ref` 查询参数里。
impl fmt::Display for LogRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = serde_json::to_value(self.kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default();
        write!(f, "{kind}:{}", self.id)
    }
}

impl FromStr for LogRef {
    type Err = InvalidLogRef;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, id) = s.split_once(':').ok_or(InvalidLogRef)?;
        let kind: RefKind = serde_json::from_value(serde_json::Value::String(kind.to_owned())).map_err(|_| InvalidLogRef)?;
        if kind == RefKind::Unknown {
            return Err(InvalidLogRef);
        }
        Ok(Self { kind, id: id.parse().map_err(|_| InvalidLogRef)? })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("应该写成 <种类>:<编号>，例如 node:3；种类是 network、node、group、token、rule、credential、listener、forward 之一")]
pub struct InvalidLogRef;

/// 操作记录里提到的东西是哪一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Network,
    Node,
    Group,
    Token,
    Rule,
    Credential,
    Listener,
    Forward,
    #[serde(other)]
    Unknown,
}

/// 一个分组：它的成员和用到它的规则。分组列表、单个分组、改名和增减成员的应答都是它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    /// 写的时候的大小写。在一个网络里不分大小写唯一。
    pub name: String,
    /// 内置分组 `All`：每个节点都在里面，不能改名、删除，也不能增减成员。
    pub builtin: bool,
    /// 成员，按编号排。`All` 的成员是网络里的全部节点。
    pub nodes: Vec<NodeRef>,
    /// `src` 或 `dst` 是它的规则（包括停用的），按编号排。删除这个分组时它们被一起删掉。
    pub rules: Vec<Rule>,
}

/// 新建一个分组。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateGroup {
    pub name: String,
}

/// 改一个分组。只有名字可改。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// 批量增减一个分组的成员。已经在里面的再加入、本来就不在的移出都什么也不做。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GroupMembers {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<NodeId>,
}

/// 删掉一个分组的应答：删掉了什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedGroup {
    pub id: GroupId,
    pub name: String,
    /// 原来的成员。
    pub nodes: Vec<NodeRef>,
    /// 一起删掉的规则，删之前的样子。
    pub rules: Vec<Rule>,
    /// 预设了它、还没用掉的注册令牌：它们不再预设它。用掉了的令牌上它也被去掉，不列在这里。
    pub tokens: Vec<u32>,
}

/// 启用（或修改）relay 的应答：节点，加上 controller 从自己这边试连它的地址的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEnabled {
    #[serde(flatten)]
    pub node: Node,
    /// relay 的每个地址两项，先 UDP 后 TCP。只是参考：谁连得上它，它就是谁的 relay，controller 连不上不等于别人连不上。
    pub reachability: Vec<Reachability>,
}

/// controller 试连一个 relay 地址的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reachability {
    pub protocol: Transport,
    pub addr: SocketAddr,
    pub reachable: bool,
    /// UDP 是地址查询的往返时间，TCP 是建连接的时间；没连上是 `null`。
    pub rtt_ms: Option<u32>,
    /// 没连上的原因；连上了是 `null`。
    pub error: Option<String>,
    /// `error` 的代码；连上了是 `null`。
    #[serde(default)]
    pub cause: Option<ReachCause>,
}

/// controller 为什么没有连上 relay 的一个地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ReachCause {
    /// 对方拒绝了连接（TCP 端口上没有程序在听）。
    Refused,
    /// 时限之内没有回答（UDP 的地址查询），或者连接超时。
    Timeout,
    /// controller 到那个地址的网络不通。
    Unreachable,
    /// controller 这边出的错（绑不出 UDP 端口、发不出去）。
    Local,
    /// 别的原因，见 `error`。
    Other,
    /// 比这个程序新的 controller 给的、这里不认识的原因。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Udp,
    Tcp,
}

/// 让一个出口试着连一个目标（`POST …/exits/{node}/test`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitTestInput {
    /// `主机[:端口]`：域名、IPv4 地址，或者方括号里的 IPv6 地址；不写端口是 443。
    pub target: String,
}

/// 一次出口测试的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitTestView {
    pub node: NodeRef,
    /// 测的目标，`主机:端口`。
    pub target: String,
    /// 出口节点回答了没有。没有回答（版本太旧不懂这条消息，或者时限之内没有回答）时 `result` 是 `null`。
    pub answered: bool,
    #[serde(default)]
    pub result: Option<crate::control::ExitTestResult>,
}

/// 一个出口：它的设置、它自己报的计数和限制、健康，以及用它的节点各自看到的情况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitView {
    pub node: NodeRef,
    pub online: bool,
    pub settings: ExitRole,
    pub health: ExitHealth,
    /// 最近 5 分钟里出口报的建流请求数，和其中归因于出口自己的失败数。健康只看这两个数。
    pub recent_requests: u64,
    pub recent_failures: u64,
    /// 出口最近一次报的计数（它这次运行以来的累计）、配额用到了哪里、是不是在排空。没有报过时是 `null`。
    pub usage: Option<ExitReport>,
    /// `usage` 是多少秒之前报的。
    pub reported_secs_ago: Option<u64>,
    /// 用过这个出口的节点各自看到的计数（各自这次运行以来的累计），和出口自己报的交叉核对。
    pub observed: Vec<ExitObserved>,
}

/// 出口的健康：只看归因于出口自己的失败（解析器出错、内部错误），目标的问题和被拒绝都不算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitHealth {
    /// 最近 5 分钟里有请求，出口自己的失败不到一成。
    Ok,
    /// 最近 5 分钟里出口自己的失败至少 3 次，并且占请求的一成以上。
    Degraded,
    /// 不在线，或者最近 5 分钟里没有请求，说不上来。
    Unknown,
}

/// 一个节点经某个出口出去的流，它自己看到的计数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitObserved {
    pub by: NodeRef,
    #[serde(flatten)]
    pub counts: ExitObservation,
    /// 是多少秒之前报的。
    pub reported_secs_ago: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn update_node_tells_an_omitted_field_from_an_explicit_null() {
        let omitted: UpdateNode = serde_json::from_value(json!({})).unwrap();
        assert_eq!(omitted.routing, None);
        let cleared: UpdateNode = serde_json::from_value(json!({ "routing": null })).unwrap();
        assert_eq!(cleared.routing, Some(None));
        let set: UpdateNode = serde_json::from_value(json!({ "routing": "lowest_cost" })).unwrap();
        assert_eq!(set.routing, Some(Some(RoutingMode::LowestCost)));

        assert_eq!(serde_json::to_value(&omitted).unwrap(), json!({}));
        assert_eq!(serde_json::to_value(&cleared).unwrap(), json!({ "routing": null }));
    }

    #[test]
    fn a_node_from_an_older_controller_reads_without_the_update_fields() {
        let node: Node = serde_json::from_value(json!({
            "id": 1, "network": 1, "key": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE", "name": "node-1", "fqdn": "node-1.home.i3t",
            "virtual_ip": "10.66.0.1", "groups": [], "declared": {}, "relay": null, "exit": null, "dial_out_only": false,
            "routing": null, "reach": null, "pinned_relays": [], "platform": "linux-x86_64", "version": "0.1.0", "online": false,
            "last_seen_at": null, "created_at": 1, "netmap_version": 1, "sent_version": 1, "acked_version": 1
        }))
        .unwrap();
        assert_eq!((node.hostname.as_str(), node.update.clone()), ("", NodeUpdate::default()));
        assert_eq!(serde_json::to_value(&node).unwrap()["update"]["status"], json!({ "state": "idle" }));
    }

    #[test]
    fn update_settings_tell_an_omitted_source_from_a_reset_to_the_default() {
        let untouched: UpdateSettings = serde_json::from_value(json!({ "auto_update": false })).unwrap();
        assert_eq!((untouched.auto_update, untouched.source.clone()), (Some(false), None));
        let reset: UpdateSettings = serde_json::from_value(json!({ "source": null })).unwrap();
        assert_eq!(reset.source, Some(None));
        let mirror: UpdateSettings = serde_json::from_value(json!({ "source": "https://mirror.example/releases" })).unwrap();
        assert_eq!(mirror.source, Some(Some("https://mirror.example/releases".to_owned())));
        assert_eq!(serde_json::to_value(&reset).unwrap(), json!({ "source": null }));
    }

    #[test]
    fn update_and_route_views_have_stable_json_shapes() {
        let order = UpdateOrder { version: Version::new(0, 3, 0), timing: UpdateTiming::Idle };
        assert_eq!(serde_json::to_value(&order).unwrap(), json!({ "version": "0.3.0", "timing": "idle" }));
        assert_eq!(serde_json::to_value(UpdateWait::RelayTurn).unwrap(), json!("relay_turn"));
        assert_eq!(serde_json::from_value::<UpdateWait>(json!("added_later")).unwrap(), UpdateWait::Unknown);
        assert_eq!(serde_json::to_value(PackageState::NotNeeded).unwrap(), json!("not_needed"));
        assert_eq!(serde_json::to_value(PathSource::NotPeers).unwrap(), json!("not_peers"));
        assert_eq!(serde_json::to_value(ForceUpdate { timing: UpdateTiming::AfterDrain }).unwrap(), json!({ "timing": "after_drain" }));
        let node = MatrixNode {
            node: NodeRef { id: NodeId::new(2).unwrap(), name: "laptop".into(), virtual_ip: "10.66.0.2".parse().unwrap() },
            online: true,
        };
        assert_eq!(serde_json::to_value(&node).unwrap(), json!({ "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2", "online": true }));
    }

    #[test]
    fn route_views_from_an_older_controller_read_without_the_path_details() {
        let laptop = json!({ "id": 2, "name": "laptop", "virtual_ip": "10.66.0.2" });
        let leg: PathLeg = serde_json::from_value(json!({
            "from": laptop, "to": laptop, "source": "report", "route": { "via": "none" }, "rtt_ms": null, "age_secs": 3, "since": null
        }))
        .unwrap();
        assert_eq!((leg.direct, leg.relay_leg, leg.mode, leg.reason.clone(), leg.candidates.clone()), (None, None, None, None, None));
        let wire = serde_json::to_value(&leg).unwrap();
        for field in ["direct", "relay_leg", "mode", "reason", "candidates"] {
            assert_eq!(wire[field], json!(null), "新的字段总是写出来，不知道时是 null：{field}");
        }
        let entry: MatrixEntry = serde_json::from_value(
            json!({ "from": 1, "to": 2, "route": { "via": "none" }, "rtt_ms": null, "age_secs": 0, "stale": false }),
        )
        .unwrap();
        assert_eq!((entry.direct, entry.relay_leg), (None, None));
        let route: NodeRoute = serde_json::from_value(json!({ "peer": laptop, "route": { "via": "none" }, "rtt_ms": null })).unwrap();
        assert_eq!((route.direct, route.relay_leg), (None, None));
    }

    #[test]
    fn unknown_error_codes_are_tolerated() {
        let body: ErrorBody = serde_json::from_value(json!({ "error": { "code": "added_later", "message": "x" } })).unwrap();
        assert_eq!(body.error.code, ErrorCode::Unknown);
        assert_eq!(serde_json::to_value(ErrorCode::NotDeclared).unwrap(), json!("not_declared"));
    }

    #[test]
    fn listener_fields_sit_next_to_its_id_and_node() {
        let listener: Listener = serde_json::from_value(json!({
            "id": 3, "node": 7, "name": "lan", "bind": "192.168.1.5:1080", "protocol": "socks5",
            "require_credential": true, "default_action": { "action": "exit", "node": 9 }
        }))
        .unwrap();
        assert_eq!(listener.node, NodeId::new(7).unwrap());
        assert!(listener.config.require_credential);
        let value = serde_json::to_value(&listener).unwrap();
        assert_eq!(value["bind"], json!("192.168.1.5:1080"));
        assert_eq!(value["default_action"], json!({ "action": "exit", "node": 9 }));
    }

    #[test]
    fn responses_from_an_older_controller_read_with_the_new_fields_defaulted() {
        let credential: Credential =
            serde_json::from_value(json!({ "id": 1, "name": "phone", "max_concurrent": 8, "created_at": 5 })).unwrap();
        assert_eq!(
            (credential.state, credential.max_bytes, credential.usage, credential.prefix.as_str()),
            (CredentialState::Active, None, CredentialUsage::default(), "")
        );
        let token: EnrollTokenInfo = serde_json::from_value(json!({
            "id": 1, "network": 1, "created_at": 1, "expires_at": 2, "groups": [], "virtual_ip": null, "successor_of": null
        }))
        .unwrap();
        assert_eq!((token.state, token.used_at, token.used_by), (TokenState::Pending, None, None));
        let entry: LogEntry =
            serde_json::from_value(json!({ "id": 1, "at": 2, "network": null, "action": "admin.token", "detail": "x" })).unwrap();
        assert!(entry.refs.is_empty());
        let forward: ForwardInput = serde_json::from_value(json!({
            "protocol": "tcp", "listen_node": 1, "bind": "127.0.0.1:0", "target_node": 2, "target_addr": "127.0.0.1:22"
        }))
        .unwrap();
        assert_eq!((forward.class, forward.enabled), (None, true));
        let network: UpdateNetwork = serde_json::from_value(json!({ "class": null })).unwrap();
        assert_eq!(network.class, Some(None));
        assert_eq!(serde_json::to_value(UpdateNetwork::default()).unwrap(), json!({}));
    }

    #[test]
    fn a_pair_view_keeps_the_setting_flat_and_a_pair_input_may_leave_things_out() {
        let input: PairInput = serde_json::from_str(r#"{"lane": "always"}"#).unwrap();
        assert_eq!(input, PairInput { keep_alive: None, lane: Some(LaneMode::Always), congestion: None });
        assert_eq!(serde_json::from_str::<PairInput>(r#"{"keep_alive": true}"#).unwrap().lane, None, "旧的请求体照样能用");
        assert!(serde_json::from_str::<PairInput>(r#"{"lane": "sometimes"}"#).is_err());
        let cubic: PairInput = serde_json::from_str(r#"{"congestion": "cubic"}"#).unwrap();
        assert_eq!(cubic.congestion, Some(Some(Congestion::Cubic)));
        let cleared: PairInput = serde_json::from_str(r#"{"congestion": null}"#).unwrap();
        assert_eq!(cleared.congestion, Some(None), "null 是改回默认，和没写不一样");
        assert!(serde_json::from_str::<PairInput>(r#"{"congestion": "reno"}"#).is_err());

        let a = NodeId::new(1).unwrap();
        let b = NodeId::new(2).unwrap();
        let view = PairView {
            setting: PairSetting { a, b, keep_alive: true, lane: LaneMode::UdpFallback, congestion: Some(Congestion::Cubic) },
            lane_effect: Some(LaneEffect {
                effective: false,
                blocked: vec![LaneBlock::NoRelay, LaneBlock::NodeTooOld { node: b }],
                dialers: Vec::new(),
                summary: "x".to_owned(),
                opening: vec![LaneOpening { node: a, udp_path: false, lanes: false }],
            }),
            lanes: Vec::new(),
        };
        let value = serde_json::to_value(&view).unwrap();
        assert_eq!((&value["a"], &value["keep_alive"], &value["lane"]), (&json!(1), &json!(true), &json!("udp_fallback")));
        assert_eq!(value["congestion"], json!("cubic"));
        assert_eq!(value["lane_effect"]["blocked"], json!([{ "reason": "no_relay" }, { "reason": "node_too_old", "node": 2 }]));
        assert_eq!(value["lane_effect"]["opening"], json!([{ "node": 1, "udp_path": false, "lanes": false }]));
        assert_eq!(serde_json::from_value::<PairView>(value).unwrap(), view);
        // 旧的 controller 给的节点对没有独占 TCP 的设置。
        let old: PairSetting = serde_json::from_value(json!({ "a": 1, "b": 2, "keep_alive": false })).unwrap();
        assert_eq!((old.lane, old.congestion), (LaneMode::Off, None));
    }

    #[test]
    fn node_detail_keeps_attached_in_the_same_place_now_that_it_lives_in_node() {
        let node: Node = serde_json::from_value(json!({
            "id": 1, "network": 1, "key": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE", "name": "node-1", "fqdn": "node-1.home.i3t",
            "virtual_ip": "10.66.0.1", "groups": [], "declared": {}, "relay": null, "exit": null, "dial_out_only": false,
            "routing": null, "reach": null, "pinned_relays": [], "platform": "linux-x86_64", "version": "0.1.0", "online": true,
            "last_seen_at": null, "created_at": 1, "netmap_version": 1, "sent_version": 1, "acked_version": 1,
            "attached": [{ "relay": 9, "rtt_ms": 12 }]
        }))
        .unwrap();
        let detail = NodeDetail {
            node,
            endpoints: Vec::new(),
            observed_endpoint: None,
            mapping_varies: None,
            flows: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            lanes: None,
            reported_secs_ago: Some(4),
            performance: None,
            intercept: None,
            reached_by: None,
        };
        let value = serde_json::to_value(&detail).unwrap();
        assert_eq!(value["attached"], json!([{ "relay": 9, "rtt_ms": 12 }]));
        assert_eq!(value["reported_secs_ago"], json!(4));
        assert_eq!(serde_json::from_value::<NodeDetail>(value).unwrap(), detail);
    }

    #[test]
    fn a_profile_is_chosen_cleared_and_reported_as_codes() {
        let set: UpdateNode = serde_json::from_value(json!({ "profile": "low_footprint" })).unwrap();
        assert_eq!(set.profile, Some(Some(Profile::LowFootprint)));
        let cleared: UpdateNode = serde_json::from_value(json!({ "profile": null })).unwrap();
        assert_eq!(cleared.profile, Some(None), "null 是不再指定，和没写不一样");
        assert_eq!(serde_json::from_value::<UpdateNode>(json!({})).unwrap().profile, None);
        assert_eq!(serde_json::to_value(UpdateNode::default()).unwrap(), json!({}));

        let report =
            PerformanceReport { profile: Profile::HighThroughput, threads: 4, restart_pending: true, overrides: vec!["threads".into()] };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value, json!({ "profile": "high_throughput", "threads": 4, "restart_pending": true, "overrides": ["threads"] }));
        let newer: PerformanceReport = serde_json::from_value(json!({ "profile": "turbo", "threads": 8 })).unwrap();
        assert_eq!(newer.profile, Profile::Balanced, "比本程序新的档位读成默认的");
    }

    #[test]
    fn listen_status_sits_flat_next_to_the_listener_it_describes() {
        let listener = NodeListener {
            id: None,
            builtin: true,
            name: "local-socks5".into(),
            bind: "127.0.0.1:1080".parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            status: ListenStatus {
                state: ListenState::PortInUse { detail: "地址已经在用".into(), cause: Some(crate::control::BindCause::AddressInUse) },
                traffic: ListenTraffic { connections: 0, bytes_up: 1, bytes_down: 2 },
            },
        };
        let value = serde_json::to_value(&listener).unwrap();
        assert_eq!(
            value,
            json!({
                "id": null, "builtin": true, "name": "local-socks5", "bind": "127.0.0.1:1080", "protocol": "socks5",
                "state": "port_in_use", "detail": "地址已经在用", "cause": { "code": "address_in_use" },
                "connections": 0, "bytes_up": 1, "bytes_down": 2
            })
        );
        assert_eq!(serde_json::from_value::<NodeListener>(value).unwrap(), listener);
    }

    #[test]
    fn causes_are_written_as_a_code_with_their_values_beside_it() {
        let version = |text: &str| text.parse::<Version>().unwrap();
        let skipped = SkipCause::UpToDate { running: version("0.3.0"), latest: version("0.3.0") };
        assert_eq!(serde_json::to_value(&skipped).unwrap(), json!({ "code": "up_to_date", "running": "0.3.0", "latest": "0.3.0" }));
        let blocked = SkipCause::CannotSelfUpdate { blocker: Some(SelfUpdateBlocker::Container) };
        assert_eq!(serde_json::to_value(&blocked).unwrap(), json!({ "code": "cannot_self_update", "blocker": "container" }));
        let check = ReleaseCause::Prerelease { version: version("0.4.0-rc.1") };
        assert_eq!(serde_json::to_value(&check).unwrap(), json!({ "code": "prerelease", "version": "0.4.0-rc.1" }));
        assert_eq!(serde_json::to_value(ReachCause::Refused).unwrap(), json!({ "code": "refused" }));
        assert_eq!(serde_json::from_value::<ReachCause>(json!({ "code": "later" })).unwrap(), ReachCause::Unknown);
        assert_eq!(serde_json::from_value::<ReleaseCause>(json!({ "code": "later", "x": 1 })).unwrap(), ReleaseCause::Unknown);

        let error: ErrorBody = serde_json::from_value(json!({ "error": { "code": "not_found", "message": "没有这个节点" } })).unwrap();
        assert_eq!((error.error.what, error.error.field), (None, None), "旧的 controller 不给 what 和 field");
        let error = ErrorBody {
            error: ApiError {
                code: ErrorCode::InvalidRequest,
                message: "x".into(),
                what: Some("zero".into()),
                field: Some("bandwidth_mbps".into()),
            },
        };
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["error"]["what"], "zero");
        assert_eq!(value["error"]["field"], "bandwidth_mbps");

        let entry: LogEntry = serde_json::from_value(json!({
            "id": 1, "at": 2, "network": null, "action": "admin.token", "detail": "生成了新的管理令牌", "refs": []
        }))
        .unwrap();
        assert!(entry.params.is_empty(), "这个字段出现之前的记录没有参数");
    }

    #[test]
    fn log_refs_are_written_as_kind_and_id() {
        let reference = LogRef::new(RefKind::Credential, 4);
        assert_eq!(serde_json::to_value(reference).unwrap(), json!({ "kind": "credential", "id": 4 }));
        assert_eq!(reference.to_string(), "credential:4");
        assert_eq!("node:3".parse::<LogRef>(), Ok(LogRef::new(RefKind::Node, 3)));
        for bad in ["node", "node:x", "planet:3", ":3", "unknown:1"] {
            assert_eq!(bad.parse::<LogRef>(), Err(InvalidLogRef), "{bad}");
        }
    }

    #[test]
    fn update_credential_tells_an_omitted_limit_from_a_cleared_one() {
        let cleared: UpdateCredential = serde_json::from_value(json!({ "max_bytes": null })).unwrap();
        assert_eq!((cleared.max_bytes, cleared.max_concurrent, cleared.reset_usage), (Some(None), None, false));
        let set: UpdateCredential = serde_json::from_value(json!({ "max_bytes": 1000, "reset_usage": true })).unwrap();
        assert_eq!((set.max_bytes, set.reset_usage), (Some(Some(1000)), true));
        assert_eq!(serde_json::to_value(UpdateCredential::default()).unwrap(), json!({}));
    }

    #[test]
    fn rules_default_to_both_directions_and_enabled() {
        let rule: CreateRule = serde_json::from_value(json!({ "src": 2, "dst": 3 })).unwrap();
        assert_eq!((rule.src, rule.dst), (GroupId(2), GroupId(3)));
        assert_eq!(rule.direction, Direction::Both);
        assert!(rule.enabled);
        assert_eq!(serde_json::to_value(Direction::OneWay).unwrap(), json!("one_way"));
        assert!(serde_json::from_value::<CreateRule>(json!({ "src": "laptops", "dst": 3 })).is_err(), "分组按编号引用，不收名字");
    }

    #[test]
    fn groups_are_referenced_by_id_and_shown_with_their_name() {
        let laptops = GroupRef { id: GroupId(2), name: "Laptops".to_owned(), builtin: false };
        let rule = Rule {
            id: 5,
            name: "ssh".to_owned(),
            src: laptops.clone(),
            dst: GroupRef { id: GroupId(1), name: "All".to_owned(), builtin: true },
            direction: Direction::OneWay,
            enabled: true,
        };
        let value = serde_json::to_value(&rule).unwrap();
        assert_eq!(value["src"], json!({ "id": 2, "name": "Laptops", "builtin": false }));
        assert_eq!(value["dst"], json!({ "id": 1, "name": "All", "builtin": true }), "内置的 All 不用按名字认");
        assert_eq!(serde_json::from_value::<Rule>(value).unwrap(), rule);
        let old: GroupRef = serde_json::from_value(json!({ "id": 2, "name": "Laptops" })).unwrap();
        assert_eq!(old, laptops, "旧的 controller 不给 builtin");

        let group = Group { id: GroupId(2), name: "Laptops".to_owned(), builtin: false, nodes: Vec::new(), rules: vec![rule] };
        let value = serde_json::to_value(&group).unwrap();
        assert_eq!((&value["builtin"], &value["nodes"]), (&json!(false), &json!([])));
        assert_eq!(value["rules"][0]["id"], json!(5));

        let members: GroupMembers = serde_json::from_value(json!({ "add": [1, 4] })).unwrap();
        assert_eq!((members.add.len(), members.remove.len()), (2, 0), "两项都可以省略");
        assert_eq!(serde_json::to_value(GroupMembers::default()).unwrap(), json!({}));
        let update: UpdateNode = serde_json::from_value(json!({ "groups": [3, 1] })).unwrap();
        assert_eq!(update.groups, Some(vec![GroupId(3), GroupId(1)]));
        assert_eq!("7".parse::<GroupId>(), Ok(GroupId(7)));
        assert_eq!("group:7".parse::<LogRef>(), Ok(LogRef::new(RefKind::Group, 7)));
    }
}
