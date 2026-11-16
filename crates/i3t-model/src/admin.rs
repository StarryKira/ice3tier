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
use std::net::{Ipv4Addr, SocketAddr};
use std::num::ParseIntError;
use std::str::FromStr;

use i3t_proto::flow::Class;
use i3t_proto::{NodeId, PublicKey};
use ipnet::{IpNet, Ipv4Net};
use serde::{Deserialize, Deserializer, Serialize};

use crate::control::{
    BackboneLink, Declared, DirectKind, ExitObservation, ExitReport, LaneReport, LegTransport, ListenState, ListenTraffic, PathCandidate,
    PeerLanes, RelayState, Route, RouteSince, SelfUpdate,
};
use crate::netmap::{Attachment, ExitRole, ForwardProtocol, LaneMode, ListenerProtocol, Reach, RoutingMode};
use crate::update::{FailureKind, UpdateStatus, UpdateTiming, Version};

pub mod diag;

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

/// 出错时的应答体。HTTP 状态码说明大类，[`ApiError::code`] 给程序看，[`ApiError::message`] 给人看。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
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
    /// 请求本身不合法：JSON 解析不了、名字或标签的写法不对、地址不合法等。
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
    /// 管理员打的标签，按字典序。内置标签 `all` 每个节点都有，不在这里列出。
    pub tags: Vec<String>,
    /// 节点在本地声明的"可以承担"。没有声明的能力启用不了。
    pub declared: Declared,
    /// 启用为 relay 时有值。
    pub relay: Option<RelaySettings>,
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
    /// 因为有节点更新失败而暂停自动更新的版本。
    pub paused: Vec<PausedVersion>,
    pub nodes: UpdateCounts,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 整个替换节点的标签。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
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
}

/// 区分"字段没写"和"字段写了 `null`"：前者读成 `None`，后者读成 `Some(None)`。
fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// relay 的设置。启用 relay 时由管理员填写。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelaySettings {
    /// 公网可达的 UDP 地址。`udp` 和 `tcp` 至少要有一个地址。
    #[serde(default)]
    pub udp: Vec<SocketAddr>,
    /// 公网可达的 TCP 地址，给 UDP 用不了的节点用。
    #[serde(default)]
    pub tcp: Vec<SocketAddr>,
    #[serde(default)]
    pub region: String,
    /// 转发的带宽上限；`null` 表示不限。
    #[serde(default)]
    pub bandwidth_mbps: Option<u32>,
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
    /// 发送方考虑过的候选路径，按代价从低到高，现在走的那条标着 `current`。有没有同 `mode`：`null` 是不知道，
    /// 空数组是发送方手上还没有任何测量值。
    #[serde(default)]
    pub candidates: Option<Vec<PathCandidate>>,
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
    /// 用它加入的节点自动带上的标签。
    pub tags: Vec<String>,
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
    #[serde(default)]
    pub tags: Vec<String>,
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

/// 一条访问规则：带标签 `src` 的节点可以访问带标签 `dst` 的节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u32,
    pub name: String,
    pub src: String,
    pub dst: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRule {
    #[serde(default)]
    pub name: String,
    pub src: String,
    pub dst: String,
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
    pub src: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst: Option<String>,
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
}

/// 改一对节点的设置。没有写的项不变（这一对还没有设置时取默认值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PairInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<LaneMode>,
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
}

/// 独占 TCP 的设置不起作用的一个原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LaneBlock {
    /// 两个节点都没有启用 relay：没有哪一边的 TCP 端口在听。
    NoRelay,
    /// 启用了 relay 的这个节点还没有已知的公网 TCP 地址。
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
}

/// 访问关系图的标签视图：每个标签是一个点，每条规则是一条连线。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGraph {
    /// 内置的 `all`、节点上的标签、规则里提到的标签，按字典序。
    pub tags: Vec<TagMembers>,
    pub rules: Vec<Rule>,
    /// 没有被任何启用的规则连通的节点。
    pub unconnected: Vec<NodeRef>,
    /// 还没有确认当前版本配置的节点（例如不在线）。它们还在按旧的规则判定。
    pub unacknowledged: Vec<NodeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagMembers {
    pub tag: String,
    pub nodes: Vec<NodeRef>,
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
#[error("应该写成 <种类>:<编号>，例如 node:3；种类是 network、node、token、rule、credential、listener、forward 之一")]
pub struct InvalidLogRef;

/// 操作记录里提到的东西是哪一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Network,
    Node,
    Token,
    Rule,
    Credential,
    Listener,
    Forward,
    #[serde(other)]
    Unknown,
}

/// 在整个网络里把一个标签改名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameTag {
    pub from: String,
    pub to: String,
}

/// 改名的结果：各改了几处。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenamedTag {
    pub from: String,
    pub to: String,
    /// 带着这个标签的节点。
    pub nodes: u32,
    /// 提到这个标签的规则（`src` 或 `dst`）。
    pub rules: u32,
    /// 还没用掉的注册令牌里预设了这个标签的。
    pub tokens: u32,
}

/// 启用（或修改）relay 的应答：节点，加上 controller 从自己这边试连这些地址的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEnabled {
    #[serde(flatten)]
    pub node: Node,
    /// 每个填写的地址一项。只是参考：controller 和别的节点不在同一个网络位置上，它连不上不等于别人连不上。
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Udp,
    Tcp,
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
            "virtual_ip": "10.66.0.1", "tags": [], "declared": {}, "relay": null, "exit": null, "dial_out_only": false,
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
            "id": 1, "network": 1, "created_at": 1, "expires_at": 2, "tags": [], "virtual_ip": null, "successor_of": null
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
        assert_eq!(input, PairInput { keep_alive: None, lane: Some(LaneMode::Always) });
        assert_eq!(serde_json::from_str::<PairInput>(r#"{"keep_alive": true}"#).unwrap().lane, None, "旧的请求体照样能用");
        assert!(serde_json::from_str::<PairInput>(r#"{"lane": "sometimes"}"#).is_err());

        let a = NodeId::new(1).unwrap();
        let b = NodeId::new(2).unwrap();
        let view = PairView {
            setting: PairSetting { a, b, keep_alive: true, lane: LaneMode::UdpFallback },
            lane_effect: Some(LaneEffect {
                effective: false,
                blocked: vec![LaneBlock::NoRelay, LaneBlock::NodeTooOld { node: b }],
                dialers: Vec::new(),
                summary: "x".to_owned(),
            }),
            lanes: Vec::new(),
        };
        let value = serde_json::to_value(&view).unwrap();
        assert_eq!((&value["a"], &value["keep_alive"], &value["lane"]), (&json!(1), &json!(true), &json!("udp_fallback")));
        assert_eq!(value["lane_effect"]["blocked"], json!([{ "reason": "no_relay" }, { "reason": "node_too_old", "node": 2 }]));
        assert_eq!(serde_json::from_value::<PairView>(value).unwrap(), view);
        // 旧的 controller 给的节点对没有独占 TCP 的设置。
        let old: PairSetting = serde_json::from_value(json!({ "a": 1, "b": 2, "keep_alive": false })).unwrap();
        assert_eq!(old.lane, LaneMode::Off);
    }

    #[test]
    fn node_detail_keeps_attached_in_the_same_place_now_that_it_lives_in_node() {
        let node: Node = serde_json::from_value(json!({
            "id": 1, "network": 1, "key": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE", "name": "node-1", "fqdn": "node-1.home.i3t",
            "virtual_ip": "10.66.0.1", "tags": [], "declared": {}, "relay": null, "exit": null, "dial_out_only": false,
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
        };
        let value = serde_json::to_value(&detail).unwrap();
        assert_eq!(value["attached"], json!([{ "relay": 9, "rtt_ms": 12 }]));
        assert_eq!(serde_json::from_value::<NodeDetail>(value).unwrap(), detail);
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
                state: ListenState::PortInUse { detail: "地址已经在用".into() },
                traffic: ListenTraffic { connections: 0, bytes_up: 1, bytes_down: 2 },
            },
        };
        let value = serde_json::to_value(&listener).unwrap();
        assert_eq!(
            value,
            json!({
                "id": null, "builtin": true, "name": "local-socks5", "bind": "127.0.0.1:1080", "protocol": "socks5",
                "state": "port_in_use", "detail": "地址已经在用", "connections": 0, "bytes_up": 1, "bytes_down": 2
            })
        );
        assert_eq!(serde_json::from_value::<NodeListener>(value).unwrap(), listener);
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
        let rule: CreateRule = serde_json::from_value(json!({ "src": "laptops", "dst": "servers" })).unwrap();
        assert_eq!(rule.direction, Direction::Both);
        assert!(rule.enabled);
        assert_eq!(serde_json::to_value(Direction::OneWay).unwrap(), json!("one_way"));
    }
}
