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

use crate::control::{Declared, Route};
use crate::netmap::{Attachment, ExitRole, ForwardProtocol, Reach, RoutingMode};

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
