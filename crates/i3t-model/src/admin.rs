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

use i3t_proto::{NodeId, PublicKey};
use ipnet::{IpNet, Ipv4Net};
use serde::{Deserialize, Deserializer, Serialize};

use crate::control::{Declared, Route, RouteSince, SelfUpdate};
use crate::netmap::{Attachment, ExitRole, ForwardProtocol, Reach, RoutingMode};
use crate::update::{FailureKind, UpdateStatus, UpdateTiming, Version};

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
    /// 节点挂靠的 relay 和到它们的延迟。
    pub attached: Vec<Attachment>,
    /// 映射出来的端口是不是随对端变化。是的话这台机器所在的网络基本打不了洞。节点不在线或者还不知道时是 `null`。
    pub mapping_varies: Option<bool>,
    /// 下面三项是节点本次连接期间最近一次上报的值，不在线时是 0。
    pub flows: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
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

/// 一个还没被用掉的注册令牌。
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredential {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairInput {
    pub keep_alive: bool,
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
    fn rules_default_to_both_directions_and_enabled() {
        let rule: CreateRule = serde_json::from_value(json!({ "src": "laptops", "dst": "servers" })).unwrap();
        assert_eq!(rule.direction, Direction::Both);
        assert!(rule.enabled);
        assert_eq!(serde_json::to_value(Direction::OneWay).unwrap(), json!("one_way"));
    }
}
