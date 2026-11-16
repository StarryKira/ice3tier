//! netmap：一个节点运行所需要的全部配置。
//!
//! controller 按最小权限为每个节点单独算一份：只有它能访问或者能访问它的对端，只有它用得上的凭据。
//! 节点不需要知道标签和访问规则，它拿到的是编译之后的结果（每个对端上的 [`Peer::inbound`] 和 [`Peer::outbound`]）。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

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
}

impl Default for ExitRole {
    fn default() -> Self {
        Self { ports: default_exit_ports(), lan: false, bandwidth_mbps: None, upstream_socks5: None }
    }
}

// serde 的 `default = "..."` 要的是一个返回字段类型的函数，所以这里返回 Option。
fn default_exit_ports() -> Option<Vec<u16>> {
    Some(vec![80, 443])
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub relay: NodeId,
    pub rtt_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PeerExit {
    #[serde(default)]
    pub lan: bool,
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
    Exit { node: NodeId },
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
    fn overlay_addresses_are_recognised() {
        let network = minimal().network;
        assert!(is_overlay_address(&network, "10.66.200.1".parse().unwrap()));
        assert!(!is_overlay_address(&network, "10.67.0.1".parse().unwrap()));
        assert!(!is_overlay_address(&network, "fd00::1".parse().unwrap()));
    }
}
