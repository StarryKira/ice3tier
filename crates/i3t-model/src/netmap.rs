//! netmap：一个节点运行所需要的全部配置。
//!
//! controller 按最小权限为每个节点单独算一份：只有它能访问或者能访问它的对端，只有它用得上的凭据。
//! 节点不需要知道标签和访问规则，它拿到的是编译之后的结果（每个对端上的 [`Peer::inbound`] 和 [`Peer::outbound`]）。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

use i3t_proto::flow::Class;
use i3t_proto::{NodeId, PublicKey};
use ipnet::{IpNet, Ipv4Net};
use serde::{Deserialize, Serialize};

/// 一个节点的完整配置。版本号每次变更加一，节点用它确认自己应用到了哪一版。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Netmap {
    pub version: u64,
    pub network: Network,
    /// 本节点自己。
    pub me: LocalNode,
    /// 和本节点有访问关系的对端。没有被任何访问规则连通的节点不在这里。
    #[serde(default)]
    pub peers: Vec<Peer>,
    /// 网络里全部启用了的 relay，不受访问规则限制。一个 relay 同时是对端时，两个列表里都有它。
    #[serde(default)]
    pub relays: Vec<Relay>,
    /// 只有本节点启用为 relay 时才有：网络里全部节点的编号和公钥，用来认证中转会话。
    #[serde(default)]
    pub relay_clients: Vec<RelayClient>,
    /// 本节点上要求凭据的监听器认哪些凭据。
    #[serde(default)]
    pub credentials: Vec<Credential>,
}

impl Netmap {
    #[must_use]
    pub fn peer(&self, id: NodeId) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.id == id)
    }

    #[must_use]
    pub fn relay(&self, id: NodeId) -> Option<&Relay> {
        self.relays.iter().find(|relay| relay.id == id)
    }

    /// 按虚拟 IP 找对端。
    #[must_use]
    pub fn peer_by_ip(&self, ip: Ipv4Addr) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.virtual_ip == ip)
    }

    /// 按名字找对端。`name` 可以是短名字（`nas-01`），也可以是全名（`nas-01.<网络的域名后缀>`），不区分大小写。
    #[must_use]
    pub fn peer_by_name(&self, name: &str) -> Option<&Peer> {
        let short = self.network.short_name(name);
        self.peers.iter().find(|peer| peer.name.eq_ignore_ascii_case(short))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub name: String,
    /// 节点全名的后缀，例如 `home.i3t`：节点 `nas-01` 的全名是 `nas-01.home.i3t`。
    pub domain: String,
    /// 虚拟地址段。落在这一段里的目标一律是组网目标，不会被当成互联网目标交给出口。
    pub cidr: Ipv4Net,
    #[serde(default)]
    pub routing: RoutingMode,
    /// 一条路径最多经过几个 relay。
    #[serde(default = "default_max_relay_hops")]
    pub max_relay_hops: u8,
    #[serde(default)]
    pub reach: Reach,
    /// 网络的默认服务等级：用户名、监听器、端口转发、出口都没有指定等级时用它；没有值时是 `auto`。
    #[serde(default)]
    pub class: Option<Class>,
}

impl Network {
    /// 去掉全名里的域名后缀；本来就是短名字时原样返回。
    #[must_use]
    pub fn short_name<'a>(&self, name: &'a str) -> &'a str {
        let name = name.strip_suffix('.').unwrap_or(name);
        let Some(split) = name.len().checked_sub(self.domain.len() + 1) else { return name };
        match name.split_at_checked(split) {
            Some((short, rest)) if rest.strip_prefix('.').is_some_and(|domain| domain.eq_ignore_ascii_case(&self.domain)) => short,
            _ => name,
        }
    }
}

const fn default_max_relay_hops() -> u8 {
    2
}

/// 选路模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// 直连可用就走直连；否则只经一个 relay，取离目的节点最近的。
    #[default]
    DirectFirst,
    /// 直连和各条 relay 路径按实测代价比较，取最低的。
    LowestCost,
}

/// 对可用路径的限制，和选路模式无关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    #[default]
    Any,
    /// 不使用 relay。
    DirectOnly,
    /// 不建立直连，不向对端暴露公网地址。
    RelayOnly,
}

/// 本节点自己的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalNode {
    pub id: NodeId,
    pub name: String,
    pub virtual_ip: Ipv4Addr,
    /// 覆盖网络的默认选路模式。
    #[serde(default)]
    pub routing: Option<RoutingMode>,
    #[serde(default)]
    pub reach: Option<Reach>,
    /// 只拨出：不接受别人主动发来的连接，不参与打洞。
    #[serde(default)]
    pub dial_out_only: bool,
    /// 启用为 relay 时有值。没有值时本节点不转发任何包。
    #[serde(default)]
    pub relay: Option<RelayRole>,
    /// 启用为出口时有值。没有值时本节点拒绝一切目标在本机之外的建流请求（下发给它的端口转发除外）。
    #[serde(default)]
    pub exit: Option<ExitRole>,
    #[serde(default)]
    pub listeners: Vec<Listener>,
    /// 本节点负责监听的端口转发。
    #[serde(default)]
    pub forwards: Vec<Forward>,
    /// 本节点作为目标的端口转发。只有这里列出的地址，本节点才替别人去连。
    #[serde(default)]
    pub forward_targets: Vec<ForwardTarget>,
    /// 管理员固定的挂靠 relay；空表示自己挑延迟最低的。
    #[serde(default)]
    pub pinned_relays: Vec<NodeId>,
    /// 管理员给这个节点选的性能档位；没有值时节点用本机配置文件里的。比本程序新的 controller 下发了这里不认识的档位时
    /// 也读成没有值。
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "tolerant_profile")]
    pub profile: Option<Profile>,
}

/// 性能档位（白皮书 §12.1）：一组预设，节点按它定线程数、缓冲和空闲回收的时间。各项的值在节点上定（`i3t-agent`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// 低占用：给一个核、几百 MB 内存的机器长期运行。
    LowFootprint,
    /// 默认。
    #[default]
    Balanced,
    /// 高速度：流量大、对端多的机器。
    HighThroughput,
}

impl Profile {
    pub const ALL: [Self; 3] = [Self::LowFootprint, Self::Balanced, Self::HighThroughput];

    /// 在 netmap 和管理接口里的写法。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LowFootprint => "low_footprint",
            Self::Balanced => "balanced",
            Self::HighThroughput => "high_throughput",
        }
    }

    /// 在配置文件和命令行里的写法。
    #[must_use]
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::LowFootprint => "low-footprint",
            Self::Balanced => "balanced",
            Self::HighThroughput => "high-throughput",
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 两种写法都认：`low_footprint`（JSON）和 `low-footprint`（配置文件、命令行）。
impl FromStr for Profile {
    type Err = UnknownProfile;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|profile| profile.as_str() == s || profile.spelling() == s).ok_or_else(|| UnknownProfile(s.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("不认识的性能档位 {0:?}：可以是 low-footprint、balanced、high-throughput")]
pub struct UnknownProfile(pub String);

/// netmap 里的档位：不认识的值读成没有值，不让整份 netmap 因为一个新的档位而读不了。
fn tolerant_profile<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Profile>, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?;
    Ok(text.and_then(|text| text.parse().ok()))
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelayRole {
    /// 转发的带宽上限；没有值表示不限。
    #[serde(default)]
    pub bandwidth_mbps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitRole {
    /// 互联网目标允许的端口。没有值表示不限制。局域网目标不受它限制。
    #[serde(default = "default_exit_ports")]
    pub ports: Option<Vec<u16>>,
    /// 允许把流送往本机网卡直连的网段。
    #[serde(default)]
    pub lan: bool,
    #[serde(default)]
    pub bandwidth_mbps: Option<u32>,
    /// 把流交给本机配置的一个 SOCKS5 代理去连目标，而不是自己直接连。
    #[serde(default)]
    pub upstream_socks5: Option<SocketAddr>,
    /// 每个账期的流量配额，单位字节；没有值表示不限。算的是出口替别人搬运的互联网和局域网目标的流量，
    /// 两个方向相加，流和数据报都算。用完之后新的请求被拒绝，正在搬运的流被断开，直到下一个账期。
    #[serde(default)]
    pub quota_bytes: Option<u64>,
    /// 账期从每个月的哪一天开始（UTC 的 0 点），1 到 28。
    #[serde(default = "default_quota_reset_day")]
    pub quota_reset_day: u8,
    /// 互联网目标只连哪一种地址。域名解析出来的地址里只取这一种；目标是另一种地址时拒绝。
    #[serde(default)]
    pub family: IpFamily,
    /// 建连预算：同一个发起节点每分钟最多让出口向同一个互联网或局域网目标（主机加端口）新建多少条连接；
    /// 没有值表示不限。可以短时间用完一分钟的额度，之后按每分钟这么多的速度恢复。
    #[serde(default)]
    pub connects_per_minute: Option<u32>,
    /// 经这个出口的流的默认服务等级，排在用户名和监听器的设置之后、网络的默认值之前。
    #[serde(default)]
    pub class: Option<Class>,
}

impl Default for ExitRole {
    fn default() -> Self {
        Self {
            ports: default_exit_ports(),
            lan: false,
            bandwidth_mbps: None,
            upstream_socks5: None,
            quota_bytes: None,
            quota_reset_day: default_quota_reset_day(),
            family: IpFamily::default(),
            connects_per_minute: None,
            class: None,
        }
    }
}

// serde 的 `default = "..."` 要的是一个返回字段类型的函数，所以这里返回 Option。
fn default_exit_ports() -> Option<Vec<u16>> {
    Some(vec![80, 443])
}

const fn default_quota_reset_day() -> u8 {
    1
}

/// 出口连互联网目标时用的地址族。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpFamily {
    /// IPv4 和 IPv6 都可以。
    #[default]
    Any,
    Ipv4,
    Ipv6,
}

impl IpFamily {
    /// 这个地址族允许不允许 `ip`。IPv4 映射的 IPv6 地址按 IPv4 算。
    #[must_use]
    pub fn admits(self, ip: IpAddr) -> bool {
        match (self, ip.to_canonical()) {
            (Self::Any, _) | (Self::Ipv4, IpAddr::V4(_)) | (Self::Ipv6, IpAddr::V6(_)) => true,
            (Self::Ipv4, IpAddr::V6(_)) | (Self::Ipv6, IpAddr::V4(_)) => false,
        }
    }
}

/// 一个和本节点有访问关系的对端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: NodeId,
    pub key: PublicKey,
    pub name: String,
    pub virtual_ip: Ipv4Addr,
    /// 这个对端可以访问本节点：本节点接受它开过来的流。
    pub inbound: bool,
    /// 本节点可以访问这个对端。
    pub outbound: bool,
    /// 对端上报的 UDP 候选端点。
    #[serde(default)]
    pub endpoints: Vec<SocketAddr>,
    /// 对端挂靠的 relay，按它到 relay 的延迟从低到高排。经 relay 到达它时，最后一跳必须是其中之一。
    #[serde(default)]
    pub attached: Vec<Attachment>,
    /// 对端启用了出口时有值。只用来让入口尽早失败，对端自己还会再查一遍。
    #[serde(default)]
    pub exit: Option<PeerExit>,
    #[serde(default)]
    pub dial_out_only: bool,
    /// 到这个对端的连接常驻，不因为没有流而关闭。
    #[serde(default)]
    pub keep_alive: bool,
    /// 和这个对端之间承载 TCP 连接的流用不用独占 TCP（lane）。一对节点各自的 netmap 里互相看到同一个值。
    /// 比本程序新的 controller 下发了这里不认识的值时按 `off` 处理。
    #[serde(default, deserialize_with = "tolerant_lane_mode")]
    pub lane: LaneMode,
}

/// netmap 里的独占 TCP 设置：不认识的值按 `off`，不让整份 netmap 因为一个新的取值而读不了。
fn tolerant_lane_mode<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<LaneMode, D::Error> {
    let text = String::deserialize(deserializer)?;
    Ok(text.parse().unwrap_or_default())
}

/// 一对节点之间承载 TCP 连接的流用不用独占 TCP（lane，每条流一条只属于它的 TCP）。
///
/// 要两个节点里至少有一个启用了 relay、它的 TCP 端口另一个连得上（见 [`Relay::tcp`]），否则不起作用，按 `off` 走。
/// UDP 应用的数据和别的流始终走共用的连接。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneMode {
    /// 不用：所有的流都在共用的连接上。
    #[default]
    Off,
    /// 共用的连接到对端的路上没有 UDP（UDP 不通、只能经 relay 的 TCP 中转）时，新的 TCP 流走独占 TCP。
    UdpFallback,
    /// 能开出独占 TCP 时新的 TCP 流都走它。
    Always,
}

impl LaneMode {
    /// 在 netmap、管理接口和命令行里的写法。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::UdpFallback => "udp_fallback",
            Self::Always => "always",
        }
    }
}

impl fmt::Display for LaneMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LaneMode {
    type Err = UnknownLaneMode;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [Self::Off, Self::UdpFallback, Self::Always]
            .into_iter()
            .find(|mode| mode.as_str() == s)
            .ok_or_else(|| UnknownLaneMode(s.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("不认识的独占 TCP 设置 {0:?}：可以是 off、udp_fallback、always")]
pub struct UnknownLaneMode(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub relay: NodeId,
    pub rtt_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PeerExit {
    #[serde(default)]
    pub lan: bool,
    /// 这个出口的默认服务等级（[`ExitRole::class`]）。入口给经它出去的流挑等级时要用。
    #[serde(default)]
    pub class: Option<Class>,
}

/// 一个启用了的 relay。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    pub id: NodeId,
    pub key: PublicKey,
    pub name: String,
    /// 公网可达的 UDP 地址。
    #[serde(default)]
    pub udp: Vec<SocketAddr>,
    /// 公网可达的 TCP 地址，给 UDP 用不了的节点用。
    #[serde(default)]
    pub tcp: Vec<SocketAddr>,
    #[serde(default)]
    pub region: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayClient {
    pub id: NodeId,
    pub key: PublicKey,
}

/// 一个代理入口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listener {
    pub name: String,
    pub bind: SocketAddr,
    pub protocol: ListenerProtocol,
    /// 来源白名单；空表示不限制。
    #[serde(default)]
    pub allow_from: Vec<IpNet>,
    /// 要求调用者在密码字段里带凭据。绑定在非回环地址上的监听器必须要求。
    #[serde(default)]
    pub require_credential: bool,
    /// 没有写用户名、目标又不是组网地址时怎么办。没有值时按绑定地址取默认：回环地址是就地出站，其他地址是拒绝。
    #[serde(default)]
    pub default_action: Option<DefaultAction>,
    /// 固定出口：这个监听器上的流量都从这个节点出去，给不能设置代理用户名的应用用。
    #[serde(default)]
    pub fixed_exit: Option<NodeId>,
    /// 这个监听器上的流的默认服务等级。用户名里写了等级时以用户名为准；没有值时看出口和网络的默认值。
    #[serde(default)]
    pub class: Option<Class>,
    /// 乐观回复（白皮书 §8.1）：`true` 时不等对端的应答就告诉客户端连上了，`false` 时一律等。
    /// 没有值时按目标的种类取默认：互联网目标不等，组网目标（节点的服务、局域网）等。
    #[serde(default)]
    pub optimistic: Option<bool>,
}

impl Listener {
    /// 这个监听器实际生效的默认动作。
    #[must_use]
    pub fn effective_default_action(&self) -> DefaultAction {
        self.default_action.unwrap_or(if self.bind.ip().is_loopback() { DefaultAction::Local } else { DefaultAction::Reject })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenerProtocol {
    Socks5,
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DefaultAction {
    /// 就地出站：从本节点自己的网络出去。
    Local,
    Reject,
    /// 固定走某个出口节点。
    Exit {
        node: NodeId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardProtocol {
    Tcp,
    Udp,
}

/// 一条端口转发的监听一侧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    /// 转发的编号，建流请求里靠它指明是哪一条。
    pub id: u32,
    pub protocol: ForwardProtocol,
    /// 监听地址。IP 是本节点的虚拟 IP 时不占用操作系统的端口，只对组网里被允许的节点可达。
    pub bind: SocketAddr,
    pub target: NodeId,
    #[serde(default)]
    pub allow_from: Vec<IpNet>,
    /// 这条转发上的流的服务等级；没有值时用网络的默认值。
    #[serde(default)]
    pub class: Option<Class>,
}

/// 一条端口转发的目标一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardTarget {
    pub id: u32,
    pub protocol: ForwardProtocol,
    /// 本节点要去连的地址：回环地址或者本机所在局域网里的地址。
    pub addr: SocketAddr,
    /// 监听这条转发的节点。只有它开过来的流才能用这一条。
    pub listener: NodeId,
}

/// 一个接入凭据。入口只保存令牌的哈希。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub name: String,
    pub token: TokenHash,
    #[serde(default)]
    pub max_concurrent: Option<u32>,
    /// controller 上的编号。入口按它上报每个凭据的用量；旧版本的 controller 不给，读成 0。
    #[serde(default)]
    pub id: u32,
    /// 这个凭据的流量上限已经用完：入口拒绝它的新连接，并断开它正在进行的连接。
    /// 用量是 controller 汇总各个入口的上报算出来的，所以从用完到这里变成 `true` 有一个上报周期左右的滞后。
    #[serde(default)]
    pub exhausted: bool,
}

/// 凭据令牌的 SHA-256。文本形式是 64 个小写十六进制字符。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash([u8; Self::LEN]);

impl TokenHash {
    pub const LEN: usize = 32;

    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

impl fmt::Display for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// 哈希不是秘密，但打出来也没有用，日志里只留开头。
impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenHash({:02x}{:02x}{:02x}{:02x}…)", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("凭据的哈希应该是 64 个十六进制字符")]
pub struct InvalidTokenHash;

impl FromStr for TokenHash {
    type Err = InvalidTokenHash;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s.as_bytes();
        if digits.len() != 2 * Self::LEN {
            return Err(InvalidTokenHash);
        }
        let mut bytes = [0; Self::LEN];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            let text = std::str::from_utf8(pair).map_err(|_| InvalidTokenHash)?;
            *byte = u8::from_str_radix(text, 16).map_err(|_| InvalidTokenHash)?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for TokenHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TokenHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// 一个地址是不是落在网络的虚拟地址段里。
#[must_use]
pub fn is_overlay_address(network: &Network, addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => network.cidr.contains(&v4),
        IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_is_optional_and_an_unknown_one_reads_as_none() {
        let mut netmap = minimal();
        assert_eq!(netmap.me.profile, None);
        assert!(!serde_json::to_string(&netmap).unwrap().contains("profile"), "没有选档位时 netmap 里不出现这个字段");
        netmap.me.profile = Some(Profile::LowFootprint);
        let json = serde_json::to_value(&netmap).unwrap();
        assert_eq!(json["me"]["profile"], "low_footprint");
        assert_eq!(serde_json::from_value::<Netmap>(json.clone()).unwrap(), netmap);
        let mut newer = json;
        newer["me"]["profile"] = "turbo".into();
        assert_eq!(serde_json::from_value::<Netmap>(newer).unwrap().me.profile, None);
    }

    #[test]
    fn profiles_read_in_both_spellings() {
        for profile in Profile::ALL {
            assert_eq!(profile.as_str().parse::<Profile>().unwrap(), profile);
            assert_eq!(profile.spelling().parse::<Profile>().unwrap(), profile);
        }
        assert!("fast".parse::<Profile>().is_err());
    }

    fn minimal() -> Netmap {
        serde_json::from_value(serde_json::json!({
            "version": 3,
            "network": { "name": "home", "domain": "home.i3t", "cidr": "10.66.0.0/16" },
            "me": { "id": 1, "name": "laptop", "virtual_ip": "10.66.0.1" },
            "peers": [{
                "id": 2, "key": PublicKey::from_bytes([2; 32]).to_string(), "name": "nas-01", "virtual_ip": "10.66.0.2",
                "inbound": true, "outbound": true
            }]
        }))
        .unwrap()
    }

    #[test]
    fn omitted_fields_take_their_defaults() {
        let netmap = minimal();
        assert_eq!(netmap.network.routing, RoutingMode::DirectFirst);
        assert_eq!(netmap.network.max_relay_hops, 2);
        assert_eq!(netmap.network.reach, Reach::Any);
        assert!(netmap.me.relay.is_none() && netmap.me.exit.is_none());
        assert!(netmap.relays.is_empty() && netmap.credentials.is_empty());
        assert!(netmap.peers[0].endpoints.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored_and_json_round_trips() {
        let mut value = serde_json::to_value(minimal()).unwrap();
        value["added_in_a_newer_version"] = serde_json::json!({ "x": 1 });
        let netmap: Netmap = serde_json::from_value(value).unwrap();
        assert_eq!(netmap, minimal());
    }

    #[test]
    fn a_lane_setting_from_a_newer_controller_reads_as_off_instead_of_breaking_the_netmap() {
        assert_eq!(minimal().peers[0].lane, LaneMode::Off, "旧的 controller 不发这一项");
        let mut value = serde_json::to_value(minimal()).unwrap();
        value["peers"][0]["lane"] = serde_json::json!("udp_fallback");
        assert_eq!(serde_json::from_value::<Netmap>(value.clone()).unwrap().peers[0].lane, LaneMode::UdpFallback);
        value["peers"][0]["lane"] = serde_json::json!("only_on_tuesdays");
        assert_eq!(serde_json::from_value::<Netmap>(value).unwrap().peers[0].lane, LaneMode::Off);
        // 管理接口和命令行收的设置不宽容：写错了要说出来。
        assert!(serde_json::from_str::<LaneMode>(r#""only_on_tuesdays""#).is_err());
        assert!("sometimes".parse::<LaneMode>().unwrap_err().to_string().contains("udp_fallback"));
        for mode in [LaneMode::Off, LaneMode::UdpFallback, LaneMode::Always] {
            assert_eq!(serde_json::to_value(mode).unwrap(), mode.as_str());
            assert_eq!(mode.as_str().parse::<LaneMode>(), Ok(mode));
        }
    }

    #[test]
    fn exit_ports_default_to_web_and_null_means_unrestricted() {
        let default: ExitRole = serde_json::from_str("{}").unwrap();
        assert_eq!(default.ports, Some(vec![80, 443]));
        let any: ExitRole = serde_json::from_str(r#"{"ports": null}"#).unwrap();
        assert_eq!(any.ports, None);
    }

    #[test]
    fn peers_are_found_by_ip_and_by_short_or_full_name() {
        let netmap = minimal();
        let nas = NodeId::new(2).unwrap();
        assert_eq!(netmap.peer_by_ip("10.66.0.2".parse().unwrap()).map(|peer| peer.id), Some(nas));
        for name in ["nas-01", "NAS-01", "nas-01.home.i3t", "nas-01.HOME.i3t."] {
            assert_eq!(netmap.peer_by_name(name).map(|peer| peer.id), Some(nas), "{name}");
        }
        for name in ["nas", "nas-01.other.i3t", "home.i3t", ".home.i3t"] {
            assert_eq!(netmap.peer_by_name(name), None, "{name}");
        }
    }

    #[test]
    fn listener_default_action_follows_the_bind_address() {
        let listener = |bind: &str| Listener {
            name: "l".into(),
            bind: bind.parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            allow_from: Vec::new(),
            require_credential: false,
            default_action: None,
            fixed_exit: None,
            class: None,
            optimistic: None,
        };
        assert_eq!(listener("127.0.0.1:1080").effective_default_action(), DefaultAction::Local);
        assert_eq!(listener("192.168.1.5:1080").effective_default_action(), DefaultAction::Reject);
        assert_eq!(listener("0.0.0.0:1080").effective_default_action(), DefaultAction::Reject);
    }

    #[test]
    fn token_hash_text_form_round_trips() {
        let hash = TokenHash::from_bytes([0x0f; 32]);
        let text = hash.to_string();
        assert_eq!(text, "0f".repeat(32));
        assert_eq!(text.parse::<TokenHash>(), Ok(hash));
        assert_eq!("0f".parse::<TokenHash>(), Err(InvalidTokenHash));
        assert_eq!("zz".repeat(32).parse::<TokenHash>(), Err(InvalidTokenHash));
    }

    #[test]
    fn limits_classes_and_credential_state_default_when_an_older_controller_omits_them() {
        let exit: ExitRole = serde_json::from_value(serde_json::json!({ "ports": [443], "lan": true })).unwrap();
        assert_eq!(
            (exit.quota_bytes, exit.quota_reset_day, exit.family, exit.connects_per_minute, exit.class),
            (None, 1, IpFamily::Any, None, None)
        );
        assert_eq!(exit, ExitRole { ports: Some(vec![443]), lan: true, ..ExitRole::default() });
        let credential: Credential =
            serde_json::from_value(serde_json::json!({ "name": "phone", "token": "0f".repeat(32), "max_concurrent": 2 })).unwrap();
        assert_eq!((credential.id, credential.exhausted), (0, false));
        let forward: Forward =
            serde_json::from_value(serde_json::json!({ "id": 4, "protocol": "tcp", "bind": "127.0.0.1:0", "target": 2 })).unwrap();
        assert_eq!((forward.class, forward.bind.port()), (None, 0));
        assert_eq!(minimal().network.class, None);
        let peer_exit: PeerExit = serde_json::from_value(serde_json::json!({ "lan": true, "class": "bulk" })).unwrap();
        assert_eq!(peer_exit, PeerExit { lan: true, class: Some(Class::Bulk) });
        let quota: ExitRole = serde_json::from_value(serde_json::json!({ "family": "ipv4", "quota_bytes": 1_000_000 })).unwrap();
        assert_eq!((quota.family, quota.quota_bytes), (IpFamily::Ipv4, Some(1_000_000)));
    }

    #[test]
    fn an_address_family_admits_only_its_own_addresses() {
        let (v4, v6, mapped): (IpAddr, IpAddr, IpAddr) =
            ("1.1.1.1".parse().unwrap(), "2606:4700::1".parse().unwrap(), "::ffff:1.1.1.1".parse().unwrap());
        assert!(IpFamily::Any.admits(v4) && IpFamily::Any.admits(v6));
        assert!(IpFamily::Ipv4.admits(v4) && IpFamily::Ipv4.admits(mapped) && !IpFamily::Ipv4.admits(v6));
        assert!(IpFamily::Ipv6.admits(v6) && !IpFamily::Ipv6.admits(v4) && !IpFamily::Ipv6.admits(mapped));
    }

    #[test]
    fn overlay_addresses_are_recognised() {
        let network = minimal().network;
        assert!(is_overlay_address(&network, "10.66.200.1".parse().unwrap()));
        assert!(!is_overlay_address(&network, "10.67.0.1".parse().unwrap()));
        assert!(!is_overlay_address(&network, "fd00::1".parse().unwrap()));
    }
}
