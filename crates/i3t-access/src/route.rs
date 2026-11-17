//! 一个代理请求往哪里去。
//!
//! 按这个顺序判断：
//!
//! 1. 目标是组网地址（虚拟地址段里的 IP，或者带网络域名后缀的节点名）：访问那个节点自己的服务，和用户名无关。
//!    虚拟地址段里的目标永远不会被当成互联网目标交给出口。
//! 2. 监听器固定了出口：从那个节点出去，用户名里写的出口不算数。
//! 3. 用户名写了出口：从那个节点出去。
//! 4. 都没有：按监听器的默认动作，就地出站、拒绝，或者走默认的出口节点。
//!
//! 要用到的节点必须是 netmap 里本节点可以访问的对端。它不在 netmap 里和访问规则不允许是同一个结果：
//! controller 本来就只下发互相能访问的节点，两种情况对调用者也只有一种表现。

use std::net::{IpAddr, SocketAddr};

use i3t_model::Netmap;
use i3t_model::netmap::{self, DefaultAction, Listener, Peer};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Host, Target};

use crate::egress::EgressPolicy;
use crate::selector::{ExitName, Selector};

/// 请求最后怎么走。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// 到 `peer` 开一条流。
    Flow {
        peer: NodeId,
        target: Target,
        /// 不等对端的 REPLY 就告诉客户端连上了。互联网目标默认这样做，省一个全程往返；
        /// 组网目标要等，客户端才能拿到准确的错误（比如端口没开时是"连接被拒绝"）。
        optimistic: bool,
    },
    /// 就地出站。
    Local { policy: EgressPolicy },
}

/// 请求被入口自己拒绝的原因。只进日志，客户端看到的都一样。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Denied {
    #[error("目标节点不在 netmap 里，或者访问规则不允许本节点访问它")]
    NoSuchNode,
    #[error("出口节点不在 netmap 里，或者访问规则不允许本节点访问它")]
    NoSuchExit,
    #[error("那个节点没有启用出口")]
    ExitNotEnabled,
    #[error("那个出口节点没有打开「允许访问本机局域网」")]
    LanNotEnabled,
    #[error("目标是本节点自己的虚拟 IP 或名字，入口不替本机连自己")]
    Myself,
    #[error("没有指定出口，监听器的默认动作是拒绝")]
    RejectedByDefault,
    #[error("非回环的监听器要就地出站，本节点必须启用出口")]
    LocalNeedsExit,
}

/// 决定一个请求怎么走。
pub(crate) fn decide(netmap: &Netmap, listener: &Listener, selector: &Selector, host: &Host, port: u16) -> Result<Route, Denied> {
    if let Some(node) = mesh_node(netmap, host) {
        return Ok(Route::Flow { peer: node?.id, target: Target::Node { port }, optimistic: false });
    }

    let exit = match (listener.fixed_exit, &selector.exit) {
        (Some(fixed), _) => netmap.peer(fixed),
        (None, Some(ExitName::Ip(ip))) => netmap.peer_by_ip(*ip),
        (None, Some(ExitName::Name(name))) => netmap.peer_by_name(name.as_str()),
        (None, None) => match listener.effective_default_action() {
            DefaultAction::Exit { node } => netmap.peer(node),
            DefaultAction::Reject => return Err(Denied::RejectedByDefault),
            DefaultAction::Local => return local(netmap, listener),
        },
    };
    let exit = exit.filter(|peer| peer.outbound).ok_or(Denied::NoSuchExit)?;
    // 对端有没有启用出口，netmap 里有一份。在这里就查，客户端能马上得到失败，不用等对端来拒绝。
    let role = exit.exit.ok_or(Denied::ExitNotEnabled)?;

    // IPv4 映射的 IPv6 地址按 IPv4 发，对端的策略按同一个地址判断。
    let host = match host {
        Host::Ip(ip) => Host::Ip(ip.to_canonical()),
        Host::Name(name) => Host::Name(name.clone()),
    };
    match host {
        Host::Ip(ip) if is_lan_literal(ip) => {
            if !role.lan {
                return Err(Denied::LanNotEnabled);
            }
            Ok(Route::Flow { peer: exit.id, target: Target::Lan { addr: SocketAddr::new(ip, port) }, optimistic: false })
        }
        host => Ok(Route::Flow { peer: exit.id, target: Target::Internet { host, port }, optimistic: true }),
    }
}

/// 一条流的服务等级（白皮书 §8.4），优先级从高到低：用户名里的等级标记、监听器或端口转发的设置、
/// 出口的默认值（只有经出口出去的流才看它）、网络的默认值。都没有时是 `auto`。
pub(crate) fn class_for(netmap: &Netmap, username: Option<Class>, configured: Option<Class>, route: &Route) -> Class {
    let exit = match route {
        Route::Flow { peer, target, .. } if target.needs_exit() => {
            netmap.peer(*peer).and_then(|peer| peer.exit).and_then(|exit| exit.class)
        }
        Route::Flow { .. } | Route::Local { .. } => None,
    };
    username.or(configured).or(exit).or(netmap.network.class).unwrap_or_default()
}

/// 一条流用不用乐观回复（白皮书 §8.1），先看到哪一个设置用哪一个：监听器的、出口的（只有经出口出去的流才看它）、
/// 按目标种类的默认值 `by_kind`。监听器排在出口前面：它是这个入口的主人为这个入口定的，出口的设置管的是经它出去的所有入口。
pub(crate) fn optimistic_for(netmap: &Netmap, listener: Option<bool>, peer: NodeId, target: &Target, by_kind: bool) -> bool {
    let exit = target.needs_exit().then(|| netmap.peer(peer).and_then(|peer| peer.exit).and_then(|exit| exit.optimistic)).flatten();
    listener.or(exit).unwrap_or(by_kind)
}

/// 目标指的是不是组网里的一个节点。不是组网目标时返回 `None`；是的话给出那个对端，或者不能访问它的原因。
pub(crate) fn mesh_node<'a>(netmap: &'a Netmap, host: &Host) -> Option<Result<&'a Peer, Denied>> {
    let peer = match host {
        Host::Ip(ip) => {
            let IpAddr::V4(ip) = ip.to_canonical() else { return None };
            if !netmap::is_overlay_address(&netmap.network, IpAddr::V4(ip)) {
                return None;
            }
            if ip == netmap.me.virtual_ip {
                return Some(Err(Denied::Myself));
            }
            netmap.peer_by_ip(ip)
        }
        Host::Name(name) => {
            // 只有带着网络域名后缀的才是节点名。单独一个 `nas-01` 可能是局域网里的主机名，不能抢过来。
            let full = name.as_str().strip_suffix('.').unwrap_or(name.as_str());
            let short = netmap.network.short_name(full);
            if short.len() == full.len() {
                return None;
            }
            if short.eq_ignore_ascii_case(&netmap.me.name) {
                return Some(Err(Denied::Myself));
            }
            netmap.peer_by_name(short)
        }
    };
    Some(peer.filter(|peer| peer.outbound).ok_or(Denied::NoSuchNode))
}

fn local(netmap: &Netmap, listener: &Listener) -> Result<Route, Denied> {
    if listener.bind.ip().is_loopback() {
        Ok(Route::Local { policy: EgressPolicy::Unrestricted })
    } else if netmap.me.exit.is_some() {
        Ok(Route::Local { policy: EgressPolicy::Exit })
    } else {
        Err(Denied::LocalNeedsExit)
    }
}

/// 写成字面量的地址是不是只在局域网里有意义：私有地址和链路本地地址。
///
/// 入口不知道出口节点的网卡上有哪些网段，只能按地址本身来分：这些地址按"对端的局域网"发过去，其余的按互联网目标发。
/// 出口节点会再核对它是不是真的在自己直连的网段里。出口测试（`i3t_exit::Outbound::probe`）按同一条分。
#[must_use]
pub fn is_lan_literal(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::{ExitRole, LaneMode, ListenerProtocol, LocalNode, Network, PeerExit};
    use i3t_proto::PublicKey;

    use super::*;

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn peer(raw: u8, name: &str, exit: Option<PeerExit>) -> Peer {
        Peer {
            id: node(u32::from(raw)),
            key: PublicKey::from_bytes([raw; 32]),
            name: name.into(),
            virtual_ip: [10, 66, 0, raw].into(),
            inbound: true,
            outbound: true,
            endpoints: Vec::new(),
            attached: Vec::new(),
            exit,
            dial_out_only: false,
            keep_alive: false,
            lane: LaneMode::Off,
            congestion: None,
        }
    }

    /// 本节点是 1 号 `laptop`；2 号 `gw` 是出口并开了局域网；3 号 `web` 是出口；4 号 `nas` 不是出口；5 号 `hidden` 不允许访问。
    fn netmap() -> Netmap {
        let mut hidden = peer(5, "hidden", Some(PeerExit { lan: true, class: None, optimistic: None }));
        hidden.outbound = false;
        Netmap {
            version: 1,
            network: Network {
                name: "home".into(),
                domain: "home.i3t".into(),
                cidr: "10.66.0.0/16".parse().unwrap(),
                routing: Default::default(),
                max_relay_hops: 2,
                reach: Default::default(),
                class: None,
            },
            me: LocalNode {
                id: node(1),
                name: "laptop".into(),
                virtual_ip: [10, 66, 0, 1].into(),
                routing: None,
                reach: None,
                dial_out_only: false,
                relay: None,
                exit: None,
                listeners: Vec::new(),
                forwards: Vec::new(),
                forward_targets: Vec::new(),
                pinned_relays: Vec::new(),
                profile: None,
                shared_relays: Vec::new(),
            },
            peers: vec![
                peer(2, "gw", Some(PeerExit { lan: true, class: None, optimistic: None })),
                peer(3, "web", Some(PeerExit { lan: false, class: Some(Class::Bulk), optimistic: None })),
                peer(4, "nas", None),
                hidden,
            ],
            relays: Vec::new(),
            relay_clients: Vec::new(),
            credentials: Vec::new(),
        }
    }

    fn listener(bind: &str) -> Listener {
        Listener {
            name: "l".into(),
            bind: bind.parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            allow_from: Vec::new(),
            require_credential: false,
            default_action: None,
            fixed_exit: None,
            class: None,
            optimistic: None,
        }
    }

    fn route(netmap: &Netmap, listener: &Listener, username: &str, host: &str, port: u16) -> Result<Route, Denied> {
        decide(netmap, listener, &username.parse().unwrap(), &host.parse().unwrap(), port)
    }

    fn internet(peer: u32, host: &str, port: u16) -> Result<Route, Denied> {
        Ok(Route::Flow { peer: node(peer), target: Target::Internet { host: host.parse().unwrap(), port }, optimistic: true })
    }

    fn service(peer: u32, port: u16) -> Result<Route, Denied> {
        Ok(Route::Flow { peer: node(peer), target: Target::Node { port }, optimistic: false })
    }

    #[test]
    fn username_names_the_exit_by_virtual_ip_or_by_name() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        for username in ["10.66.0.3", "web", "WEB", "web.home.i3t", "web+c-i"] {
            assert_eq!(route(&netmap, &loopback, username, "example.com", 443), internet(3, "example.com", 443), "{username}");
        }
        assert_eq!(route(&netmap, &loopback, "gw", "203.0.113.7", 80), internet(2, "203.0.113.7", 80));
    }

    #[test]
    fn unknown_and_forbidden_exits_fail_the_same_way() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        for username in ["10.66.0.9", "nobody", "10.66.0.5", "hidden", "10.66.0.1", "laptop", "198.51.100.1"] {
            assert_eq!(route(&netmap, &loopback, username, "example.com", 443), Err(Denied::NoSuchExit), "{username}");
        }
    }

    #[test]
    fn an_exit_must_have_the_capability_the_target_needs() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        assert_eq!(route(&netmap, &loopback, "nas", "example.com", 443), Err(Denied::ExitNotEnabled));
        assert_eq!(route(&netmap, &loopback, "nas", "192.168.1.10", 80), Err(Denied::ExitNotEnabled));
        assert_eq!(route(&netmap, &loopback, "web", "192.168.1.10", 80), Err(Denied::LanNotEnabled));
    }

    #[test]
    fn private_address_literals_go_to_the_exit_as_lan_targets_and_wait_for_the_reply() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        for host in ["192.168.1.10", "172.16.5.5", "10.1.2.3", "169.254.1.1", "fd00::10", "fe80::1", "::ffff:192.168.1.10"] {
            let addr = SocketAddr::new(host.parse::<IpAddr>().unwrap().to_canonical(), 80);
            let expected = Ok(Route::Flow { peer: node(2), target: Target::Lan { addr }, optimistic: false });
            assert_eq!(route(&netmap, &loopback, "gw", host, 80), expected, "{host}");
        }
        // 域名和别的地址一律按互联网目标发：解析出内网地址时由出口的策略拒绝。
        assert_eq!(route(&netmap, &loopback, "gw", "printer.lan", 80), internet(2, "printer.lan", 80));
        assert_eq!(route(&netmap, &loopback, "gw", "127.0.0.1", 80), internet(2, "127.0.0.1", 80));
        assert_eq!(route(&netmap, &loopback, "gw", "100.64.0.1", 80), internet(2, "100.64.0.1", 80));
    }

    #[test]
    fn overlay_addresses_and_node_names_are_services_on_that_node() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        for host in ["10.66.0.4", "nas.home.i3t", "NAS.Home.I3T", "nas.home.i3t.", "::ffff:10.66.0.4"] {
            assert_eq!(route(&netmap, &loopback, "", host, 8080), service(4, 8080), "{host}");
        }
        // 用户名写了别的出口也一样：组网地址不会被交给出口去连。
        assert_eq!(route(&netmap, &loopback, "web", "10.66.0.4", 22), service(4, 22));
        assert_eq!(route(&netmap, &loopback, "web", "web.home.i3t", 22), service(3, 22));
    }

    #[test]
    fn mesh_targets_that_cannot_be_reached_never_fall_through_to_an_exit() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        for host in ["10.66.0.9", "10.66.255.255", "10.66.0.5", "nobody.home.i3t", "hidden.home.i3t"] {
            assert_eq!(route(&netmap, &loopback, "", host, 80), Err(Denied::NoSuchNode), "{host}");
            assert_eq!(route(&netmap, &loopback, "web", host, 80), Err(Denied::NoSuchNode), "{host}");
        }
        assert_eq!(route(&netmap, &loopback, "", "10.66.0.1", 80), Err(Denied::Myself));
        assert_eq!(route(&netmap, &loopback, "", "laptop.home.i3t", 80), Err(Denied::Myself));
    }

    #[test]
    fn names_outside_the_network_domain_are_not_node_names() {
        let (netmap, loopback) = (netmap(), listener("127.0.0.1:1080"));
        let unrestricted = Ok(Route::Local { policy: EgressPolicy::Unrestricted });
        for host in ["nas", "nas.other.i3t", "home.i3t", "nas.home.i3t.example.com"] {
            assert_eq!(route(&netmap, &loopback, "", host, 80), unrestricted, "{host}");
        }
    }

    #[test]
    fn default_action_follows_the_bind_address_unless_configured() {
        let netmap = netmap();
        let loopback = listener("127.0.0.1:1080");
        assert_eq!(route(&netmap, &loopback, "", "example.com", 443), Ok(Route::Local { policy: EgressPolicy::Unrestricted }));
        // 只写了等级、没有写出口，也算没有指定出口。
        assert_eq!(route(&netmap, &loopback, "+c-b", "192.168.1.10", 80), Ok(Route::Local { policy: EgressPolicy::Unrestricted }));

        let lan = listener("0.0.0.0:1080");
        assert_eq!(route(&netmap, &lan, "", "example.com", 443), Err(Denied::RejectedByDefault));
        assert_eq!(route(&netmap, &lan, "web", "example.com", 443), internet(3, "example.com", 443));

        let mut rejecting = listener("127.0.0.1:1080");
        rejecting.default_action = Some(DefaultAction::Reject);
        assert_eq!(route(&netmap, &rejecting, "", "example.com", 443), Err(Denied::RejectedByDefault));
        assert_eq!(route(&netmap, &rejecting, "", "10.66.0.4", 22), service(4, 22));

        let mut via_web = listener("127.0.0.1:1080");
        via_web.default_action = Some(DefaultAction::Exit { node: node(3) });
        assert_eq!(route(&netmap, &via_web, "", "example.com", 443), internet(3, "example.com", 443));
        assert_eq!(route(&netmap, &via_web, "gw", "example.com", 443), internet(2, "example.com", 443));

        let mut via_missing = listener("127.0.0.1:1080");
        via_missing.default_action = Some(DefaultAction::Exit { node: node(9) });
        assert_eq!(route(&netmap, &via_missing, "", "example.com", 443), Err(Denied::NoSuchExit));
    }

    #[test]
    fn local_egress_on_a_non_loopback_listener_needs_this_node_to_be_an_exit() {
        let mut netmap = netmap();
        let mut lan = listener("192.168.1.5:1080");
        lan.default_action = Some(DefaultAction::Local);
        assert_eq!(route(&netmap, &lan, "", "example.com", 443), Err(Denied::LocalNeedsExit));

        netmap.me.exit = Some(ExitRole::default());
        assert_eq!(route(&netmap, &lan, "", "example.com", 443), Ok(Route::Local { policy: EgressPolicy::Exit }));
    }

    #[test]
    fn class_comes_from_the_username_then_the_listener_then_the_exit_then_the_network() {
        let mut netmap = netmap();
        let to_web = route(&netmap, &listener("127.0.0.1:1080"), "web", "example.com", 443).unwrap();
        let to_nas = route(&netmap, &listener("127.0.0.1:1080"), "", "10.66.0.4", 22).unwrap();
        let local = route(&netmap, &listener("127.0.0.1:1080"), "", "example.com", 443).unwrap();
        assert_eq!(class_for(&netmap, Some(Class::Interactive), Some(Class::Reliable), &to_web), Class::Interactive, "用户名最优先");
        assert_eq!(class_for(&netmap, None, Some(Class::Reliable), &to_web), Class::Reliable, "其次是监听器");
        assert_eq!(class_for(&netmap, None, None, &to_web), Class::Bulk, "再次是出口的默认值");
        assert_eq!(class_for(&netmap, None, None, &to_nas), Class::Auto, "不经出口的流不看出口的默认值");
        netmap.network.class = Some(Class::Reliable);
        assert_eq!(class_for(&netmap, None, None, &to_nas), Class::Reliable, "最后是网络的默认值");
        assert_eq!(class_for(&netmap, None, None, &to_web), Class::Bulk, "出口的默认值排在网络的前面");
        assert_eq!(class_for(&netmap, None, None, &local), Class::Reliable);
    }

    #[test]
    fn the_optimistic_reply_follows_the_listener_then_the_exit_then_the_kind_of_target() {
        let mut netmap = netmap();
        let web = netmap.peers.iter().find(|peer| peer.name == "web").unwrap().id;
        let (internet, nas_service) = (Target::Internet { host: "example.com".parse().unwrap(), port: 443 }, Target::Node { port: 22 });
        assert!(optimistic_for(&netmap, None, web, &internet, true), "没有设时按目标的种类");
        netmap.peers.iter_mut().find(|peer| peer.id == web).unwrap().exit.as_mut().unwrap().optimistic = Some(false);
        assert!(!optimistic_for(&netmap, None, web, &internet, true), "出口的设置盖过按种类的默认值");
        assert!(optimistic_for(&netmap, Some(true), web, &internet, true), "监听器的设置排在出口的前面");
        assert!(optimistic_for(&netmap, None, web, &nas_service, true), "不经出口的流不看出口的设置");
    }

    #[test]
    fn a_fixed_exit_overrides_the_username_but_not_mesh_targets() {
        let netmap = netmap();
        let mut fixed = listener("127.0.0.1:1080");
        fixed.fixed_exit = Some(node(3));
        assert_eq!(route(&netmap, &fixed, "", "example.com", 443), internet(3, "example.com", 443));
        assert_eq!(route(&netmap, &fixed, "gw", "example.com", 443), internet(3, "example.com", 443));
        assert_eq!(route(&netmap, &fixed, "", "10.66.0.4", 22), service(4, 22));

        fixed.fixed_exit = Some(node(5));
        assert_eq!(route(&netmap, &fixed, "gw", "example.com", 443), Err(Denied::NoSuchExit));
    }
}
