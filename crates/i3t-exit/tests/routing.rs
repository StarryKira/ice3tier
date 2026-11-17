//! 一个请求往哪里去：用户名指定出口、组网目标、默认动作、局域网、端口转发。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{
    ECHO_LEN, GW, LAPTOP, NAS, PUBLIC_IP, Target, World, assert_echo, assert_reset, closed_port, credential, drain, echo_server,
    eventually, http_connect, listener, node, non_loopback_address, peer_mut, socks5,
};
use i3t_access::{ListenState, TargetCause};
use i3t_model::netmap::{DefaultAction, ExitRole, Forward, ForwardProtocol, ForwardTarget, ListenerProtocol, PeerExit};
use tokio::net::TcpStream;

fn public(port: u16) -> String {
    format!("{PUBLIC_IP}:{port}")
}

i3t_lane::on_both_transports! {
    async fn the_username_names_the_exit_by_virtual_ip_or_by_name() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);

        for username in ["10.66.0.2", "gw", "GW", "gw.home.i3t", "10.66.0.2+c-i", "gw+c-b"] {
            let mut stream = socks5(world.socks().await, Some((username, "")), PUBLIC_IP, 443).await.connected();
            assert_echo(&mut stream).await;
        }
        for username in ["10.66.0.2", "gw"] {
            let (status, mut stream) = http_connect(world.http().await, &public(443), Some((username, ""))).await;
            assert_eq!(status, 200);
            assert_echo(&mut stream).await;
        }

        // 都是从 gw 出去的，laptop 自己没有连任何地方。
        assert_eq!(world.gw.system.connected(), vec![public(443).parse().unwrap(); 8]);
        assert!(world.laptop.system.connected().is_empty());
        eventually("出口把 8 条流都记完", || {
            world.gw.exit.stats().active == 0 && world.gw.exit.stats().bytes_from_target == 8 * ECHO_LEN
        })
        .await;
        let stats = world.gw.exit.stats();
        assert_eq!((stats.total, stats.bytes_to_target, stats.aborted), (8, 8 * ECHO_LEN, 0));
    }
}

i3t_lane::on_both_transports! {
    async fn domain_targets_are_resolved_on_the_exit_with_a_cache() {
        let world = World::new().await;
        world.gw.system.name("api.example.com", &[PUBLIC_IP]);
        world.gw.system.redirect(&public(443), echo_server().await);

        for host in ["api.example.com", "API.Example.COM", "api.example.com."] {
            let mut stream = socks5(world.socks().await, Some(("gw", "")), host, 443).await.connected();
            assert_echo(&mut stream).await;
        }
        let (status, mut stream) = http_connect(world.http().await, "api.example.com:443", Some(("gw", ""))).await;
        assert_eq!(status, 200);
        assert_echo(&mut stream).await;

        assert_eq!(world.gw.system.resolved(), ["api.example.com"], "四条流只问了一次解析器");
        assert!(world.laptop.system.resolved().is_empty(), "域名不在入口上解析");
    }
}

i3t_lane::on_both_transports! {
    async fn an_ipv6_literal_target_goes_through_the_exit() {
        let world = World::new().await;
        world.gw.system.redirect("[2606:4700::1111]:443", echo_server().await);
        let mut stream = socks5(world.socks().await, Some(("gw", "")), "2606:4700::1111", 443).await.connected();
        assert_echo(&mut stream).await;
        let (status, mut stream) = http_connect(world.http().await, "[2606:4700::1111]:443", Some(("gw", ""))).await;
        assert_eq!(status, 200);
        assert_echo(&mut stream).await;
    }
}

i3t_lane::on_both_transports! {
    async fn a_service_on_a_node_is_reached_on_its_non_loopback_address_only() {
        let Some(address) = non_loopback_address() else {
            eprintln!("跳过：这台机器只有回环网卡，没法在非回环地址上起服务");
            return;
        };
        let world = World::new().await;
        world.nas.system.use_real_interfaces();
        let on_lan_address = Target::bind(&address.to_string()).await.echo();
        let on_loopback_only = echo_server().await;

        // 监听在非回环地址上的服务：按虚拟 IP、按名字都能访问，不需要 nas 是出口，也不需要用户名。
        let mut stream = socks5(world.socks().await, None, "10.66.0.3", on_lan_address.port()).await.connected();
        assert_echo(&mut stream).await;
        let (status, mut stream) = http_connect(world.http().await, &format!("nas.home.i3t:{}", on_lan_address.port()), None).await;
        assert_eq!(status, 200);
        assert_echo(&mut stream).await;

        // 只监听 127.0.0.1 的服务：连不上，而且客户端拿到的是准确的"连接被拒绝"。
        assert_eq!(socks5(world.socks().await, None, "10.66.0.3", on_loopback_only.port()).await.failed(), 5);
        let (status, _) = http_connect(world.http().await, &format!("10.66.0.3:{}", on_loopback_only.port()), None).await;
        assert_eq!(status, 502);

        let tried = world.nas.system.connected();
        assert!(!tried.is_empty() && tried.iter().all(|addr| !addr.ip().is_loopback()), "出口只用非回环地址连本机的服务：{tried:?}");
    }
}

i3t_lane::on_both_transports! {
    async fn an_overlay_target_is_a_service_on_that_node_whatever_the_username_says() {
        let world = World::new().await;
        // nas 的假网卡地址是 192.168.77.5，把它上面的 8080 引到测试服务。
        world.nas.system.redirect("192.168.77.5:8080", echo_server().await);

        for username in [None, Some(("gw", "")), Some(("+c-i", ""))] {
            let mut stream = socks5(world.socks().await, username, "10.66.0.3", 8080).await.connected();
            assert_echo(&mut stream).await;
        }
        assert_eq!(world.gw.exit.stats().total, 0, "组网地址不会被交给出口去连");
        assert_eq!(world.nas.exit.stats().total, 3);
    }
}

#[tokio::test]
async fn without_a_username_a_loopback_listener_dials_from_this_machine() {
    let world = World::new().await;
    let echo = echo_server().await;
    world.laptop.system.name("intranet.corp", &["127.0.0.1"]);

    // 回环监听器不加限制：本机的程序本来就能自己去连这些地址。
    let mut stream = socks5(world.socks().await, None, "127.0.0.1", echo.port()).await.connected();
    assert_echo(&mut stream).await;
    let (status, mut stream) = http_connect(world.http().await, &format!("intranet.corp:{}", echo.port()), None).await;
    assert_eq!(status, 200);
    assert_echo(&mut stream).await;

    // 就地出站是连上了才告诉客户端，所以失败有准确的错误码。
    assert_eq!(socks5(world.socks().await, None, "127.0.0.1", closed_port().await.port()).await.failed(), 5);
    assert_eq!(socks5(world.socks().await, None, "no-such-host.corp", 80).await.failed(), 4);
    assert_eq!(http_connect(world.http().await, "no-such-host.corp:80", None).await.0, 502);

    assert_eq!((world.gw.exit.stats().total, world.nas.exit.stats().total), (0, 0), "没有任何东西经过别的节点");
}

i3t_lane::on_both_transports! {
    async fn the_default_action_can_be_reject_or_a_fixed_exit() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        world.laptop.update(|netmap| netmap.me.listeners[0].default_action = Some(DefaultAction::Reject)).await;

        // 拒绝：没有写出口的请求被拒绝，写了出口的照常。
        assert_eq!(socks5(world.socks().await, None, PUBLIC_IP, 443).await.failed(), 2);
        let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, 443).await.connected();
        assert_echo(&mut stream).await;

        // 默认走 gw：不写用户名也从 gw 出去；写了别的出口就按写的来。
        world.laptop.update(|netmap| netmap.me.listeners[0].default_action = Some(DefaultAction::Exit { node: node(GW) })).await;
        let mut stream = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut stream).await;
        assert_eq!(socks5(world.socks().await, Some(("nas", "")), PUBLIC_IP, 443).await.failed(), 2, "nas 不是出口");
        assert!(world.laptop.system.connected().is_empty());
    }
}

i3t_lane::on_both_transports! {
    async fn a_fixed_exit_carries_everything_that_is_not_a_mesh_target() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        world.nas.system.redirect("192.168.77.5:8080", echo_server().await);
        world.laptop.update(|netmap| netmap.me.listeners[1].fixed_exit = Some(node(GW))).await;

        // 不写用户名、写了别的节点，都从固定的出口出去。
        for auth in [None, Some(("nas", "")), Some(("10.66.0.99", ""))] {
            let (status, mut stream) = http_connect(world.http().await, &public(443), auth).await;
            assert_eq!(status, 200);
            assert_echo(&mut stream).await;
        }
        assert_eq!(world.gw.exit.stats().total, 3);

        // 组网地址仍然是访问那个节点的服务。
        let (status, mut stream) = http_connect(world.http().await, "10.66.0.3:8080", None).await;
        assert_eq!(status, 200);
        assert_echo(&mut stream).await;
        assert_eq!(world.nas.exit.stats().total, 1);
    }
}

#[tokio::test]
async fn local_egress_on_a_non_loopback_listener_needs_the_exit_role_and_obeys_its_policy() {
    let world = World::new().await;
    let echo = echo_server().await;
    world.laptop.system.redirect(&public(443), echo);
    world.laptop.system.redirect(&public(8443), echo);
    world
        .laptop
        .update(|netmap| {
            let mut open_to_lan = listener("lan", ListenerProtocol::Socks5, "0.0.0.0:0");
            open_to_lan.default_action = Some(DefaultAction::Local);
            netmap.me.listeners.push(open_to_lan);
            netmap.credentials.push(credential("gateway", "token", None));
        })
        .await;
    let lan = world.laptop.listener("lan").await;
    let auth = Some(("", "token"));

    // laptop 不是出口：不替别人从本机出去。
    assert_eq!(socks5(lan, auth, PUBLIC_IP, 443).await.failed(), 2);
    assert!(world.laptop.system.connected().is_empty());

    // 启用出口之后可以，但套用出口的策略：端口白名单（这里写了只放行 80 和 443），内网地址不行。
    world.laptop.update(|netmap| netmap.me.exit = Some(ExitRole { ports: Some(vec![80, 443]), ..ExitRole::default() })).await;
    let mut stream = socks5(lan, auth, PUBLIC_IP, 443).await.connected();
    assert_echo(&mut stream).await;
    assert_eq!(socks5(lan, auth, PUBLIC_IP, 8443).await.failed(), 2);
    assert_eq!(socks5(lan, auth, "127.0.0.1", 443).await.failed(), 2);
    assert_eq!(socks5(lan, auth, "192.168.77.10", 443).await.failed(), 2);
    assert_eq!(world.laptop.system.connected(), [public(443).parse().unwrap()]);

    // 同一台机器上的回环监听器不受这些限制。
    let mut stream = socks5(world.socks().await, None, "127.0.0.1", echo.port()).await.connected();
    assert_echo(&mut stream).await;
}

i3t_lane::on_both_transports! {
    async fn a_lan_target_needs_the_lan_switch_and_a_directly_attached_subnet() {
        let world = World::new().await;
        world.gw.system.redirect("192.168.77.10:8080", echo_server().await);
        let lan_target = || async { socks5(world.socks().await, Some(("gw", "")), "192.168.77.10", 8080).await };

        // 开关关着，入口从 netmap 里就知道：直接拒绝，不去打扰出口。
        assert_eq!(lan_target().await.failed(), 2);
        assert_eq!(world.gw.exit.stats().total, 0);

        // 入口以为开着，出口自己知道没开：出口拒绝。局域网目标要等出口的应答，所以客户端拿到的还是错误码。
        world.laptop.update(|netmap| peer_mut(netmap, GW).exit = Some(PeerExit { lan: true, class: None, optimistic: None })).await;
        assert_eq!(lan_target().await.failed(), 2);
        assert_eq!(world.gw.exit.stats().refused, 1);
        assert!(world.gw.system.connected().is_empty());

        // 开关打开：可以访问直连网段里的地址，而且不受端口白名单限制（8080 不在 80、443 里）。
        world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { lan: true, ..ExitRole::default() })).await;
        let mut stream = lan_target().await.connected();
        assert_echo(&mut stream).await;
        let (status, mut stream) = http_connect(world.http().await, "192.168.77.10:8080", Some(("gw", ""))).await;
        assert_eq!(status, 200);
        assert_echo(&mut stream).await;

        // 路由器后面的别的网段、回环、覆盖网、元数据地址都不算局域网。
        for host in ["192.168.99.10", "10.0.0.1", "169.254.169.254", "fd00::10"] {
            assert_eq!(socks5(world.socks().await, Some(("gw", "")), host, 8080).await.failed(), 2, "{host}");
        }
        assert_eq!(world.gw.system.connected(), ["192.168.77.10:8080".parse().unwrap(); 2]);
    }
}

fn tcp_forward(id: u32, bind: &str, target: u32) -> Forward {
    Forward { id, protocol: ForwardProtocol::Tcp, bind: bind.parse().unwrap(), target: node(target), allow_from: Vec::new(), class: None }
}

i3t_lane::on_both_transports! {
    async fn a_port_forward_carries_each_connection_to_the_address_on_the_target_node() {
        let world = World::new().await;
        // 目标在 nas 的回环上：平时不可达，端口转发是访问它的显式途径。nas 也不需要是出口。
        let echo = echo_server().await;
        world.laptop.update(|netmap| netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS))).await;
        world
            .nas
            .update(|netmap| {
                netmap.me.forward_targets.push(ForwardTarget { id: 7, protocol: ForwardProtocol::Tcp, addr: echo, listener: node(LAPTOP), proxy_protocol: None });
            })
            .await;

        for _ in 0..3 {
            let mut stream = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
            assert_echo(&mut stream).await;
        }
        assert_eq!(world.nas.system.connected(), [echo; 3]);
        assert_eq!(world.nas.exit.stats().total, 3);
    }
}

i3t_lane::on_both_transports! {
    async fn a_forward_is_only_served_to_the_node_it_was_issued_for() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.laptop.update(|netmap| netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS))).await;
        let forward = world.laptop.forward(7).await;

        // nas 上没有这一条：连接被重置。
        let mut stream = TcpStream::connect(forward).await.unwrap();
        assert_reset(&drain(&mut stream).await.1);

        // nas 上有这一条，但它是下发给 gw 监听的；UDP 的那一条也不算。
        world
            .nas
            .update(|netmap| {
                netmap.me.forward_targets = vec![
                    ForwardTarget { id: 7, protocol: ForwardProtocol::Tcp, addr: target.addr(), listener: node(GW), proxy_protocol: None },
                    ForwardTarget { id: 7, protocol: ForwardProtocol::Udp, addr: target.addr(), listener: node(LAPTOP), proxy_protocol: None },
                ];
            })
            .await;
        let mut stream = TcpStream::connect(forward).await.unwrap();
        assert_reset(&drain(&mut stream).await.1);
        target.assert_untouched(Duration::from_millis(50)).await;
        assert_eq!((world.nas.exit.stats().refused, world.nas.exit.stats().total), (2, 2));

        // gw 监听同一条转发时可以用。
        world.gw.update(|netmap| netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS))).await;
        let mut stream = TcpStream::connect(world.gw.forward(7).await).await.unwrap();
        let echo = target.echo();
        assert_echo(&mut stream).await;
        assert_eq!(world.nas.system.connected(), [echo]);
    }
}

#[tokio::test]
async fn a_forward_towards_a_node_this_one_may_not_access_resets_the_connection() {
    let world = World::new().await;
    world
        .laptop
        .update(|netmap| {
            netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS));
            peer_mut(netmap, NAS).outbound = false;
        })
        .await;
    let mut stream = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
    assert_reset(&drain(&mut stream).await.1);
    assert_eq!(world.nas.exit.stats().total, 0, "入口自己就拒绝了，没有去找目标节点");
}

i3t_lane::on_both_transports! {
    async fn a_forward_bound_to_the_virtual_ip_is_reached_through_the_mesh_without_an_os_socket() {
        let world = World::new().await;
        let echo = echo_server().await;
        let status = world
            .nas
            .update(|netmap| {
                netmap.me.forwards.push(tcp_forward(9, "10.66.0.3:8080", NAS));
                netmap.me.forward_targets.push(ForwardTarget { id: 9, protocol: ForwardProtocol::Tcp, addr: echo, listener: node(NAS), proxy_protocol: None });
            })
            .await;
        assert_eq!(status.forwards[0].state, ListenState::Virtual, "虚拟 IP 不是网卡上的地址，绑不了也不需要绑");

        // 别的节点访问 nas 的虚拟 IP 的这个端口，就到了 nas 回环上的服务。
        let mut stream = socks5(world.socks().await, None, "10.66.0.3", 8080).await.connected();
        assert_echo(&mut stream).await;
        let (status, mut stream) = http_connect(world.http().await, "nas.home.i3t:8080", None).await;
        assert_eq!(status, 200);
        assert_echo(&mut stream).await;
        assert_eq!(world.nas.system.connected(), [echo; 2]);

        // 转发的来源白名单按发起节点的虚拟 IP 判断。
        world.nas.update(|netmap| netmap.me.forwards[0].allow_from = vec!["10.66.0.2/32".parse().unwrap()]).await;
        assert_eq!(socks5(world.socks().await, None, "10.66.0.3", 8080).await.failed(), 2);
        assert_eq!(world.nas.system.connected().len(), 2);
    }
}

#[tokio::test]
async fn a_forward_that_points_at_an_overlay_address_is_refused_instead_of_being_dialled() {
    let world = World::new().await;
    world.laptop.update(|netmap| netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS))).await;
    // 覆盖网的地址只在组网里有意义：nas 不把它当普通地址去连，透明访问打开时那样的连接会绕回组网。
    let overlay = "10.66.0.9:80".parse().unwrap();
    world
        .nas
        .update(|netmap| {
            netmap.me.forward_targets.push(ForwardTarget {
                id: 7,
                protocol: ForwardProtocol::Tcp,
                addr: overlay,
                listener: node(LAPTOP),
                proxy_protocol: None,
            });
        })
        .await;
    let mut stream = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
    assert_reset(&drain(&mut stream).await.1);
    assert!(world.nas.system.connected().is_empty(), "nas 没有去连那个地址");
}

/// `node` 上编号是 `id` 的转发最近一次在目标那里失败的原因。
fn target_cause(node: &common::Node, id: u32) -> Option<TargetCause> {
    node.access.status().forwards.iter().find(|forward| forward.id == id)?.target_failure.map(|failure| failure.cause)
}

#[tokio::test]
async fn a_forward_reports_a_target_that_refuses_on_the_listening_node_and_clears_once_it_answers() {
    let world = World::new().await;
    let closed = closed_port().await;
    // 一条普通的转发：laptop 监听，nas 去连；一条监听在 nas 虚拟 IP 上、nas 自己去连的转发。
    world.laptop.update(|netmap| netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS))).await;
    world
        .nas
        .update(|netmap| {
            netmap.me.forwards.push(tcp_forward(9, "10.66.0.3:8080", NAS));
            for (id, listener) in [(7, LAPTOP), (9, NAS)] {
                netmap.me.forward_targets.push(ForwardTarget {
                    id,
                    protocol: ForwardProtocol::Tcp,
                    addr: closed,
                    listener: node(listener),
                    proxy_protocol: None,
                });
            }
        })
        .await;

    let mut stream = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
    assert_reset(&drain(&mut stream).await.1);
    assert_eq!(target_cause(&world.laptop, 7), Some(TargetCause::Refused), "监听节点从 nas 的拒绝里知道原因");
    assert_eq!(socks5(world.socks().await, None, "10.66.0.3", 8080).await.failed(), 5);
    assert_eq!(target_cause(&world.nas, 9), Some(TargetCause::Refused), "虚拟 IP 上的转发记在 nas 自己那里");

    let echo = echo_server().await;
    world.nas.update(|netmap| netmap.me.forward_targets.iter_mut().for_each(|target| target.addr = echo)).await;
    let mut stream = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
    assert_echo(&mut stream).await;
    drop(stream);
    let mut stream = socks5(world.socks().await, None, "10.66.0.3", 8080).await.connected();
    assert_echo(&mut stream).await;
    assert_eq!(target_cause(&world.nas, 9), None, "连上了就清掉");
    drop(stream);
    tokio::time::timeout(common::PATIENCE, async {
        while target_cause(&world.laptop, 7).is_some() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("一条流成功之后监听节点上的记录清掉");
}

i3t_lane::on_both_transports! {
    async fn a_forward_with_proxy_protocol_tells_the_service_the_initiators_virtual_ip() {
        use i3t_model::netmap::ProxyProtocol;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let world = World::new().await;
        let (v1, v2) = (Target::bind("127.0.0.1").await, Target::bind("127.0.0.1").await);
        world.laptop.update(|netmap| netmap.me.forwards.push(tcp_forward(7, "127.0.0.1:0", NAS))).await;
        world
            .nas
            .update(|netmap| {
                netmap.me.forwards.push(tcp_forward(9, "10.66.0.3:8080", NAS));
                let target = |id, addr, listener, proxy_protocol| ForwardTarget {
                    id,
                    protocol: ForwardProtocol::Tcp,
                    addr,
                    listener: node(listener),
                    proxy_protocol: Some(proxy_protocol),
                };
                netmap.me.forward_targets.push(target(7, v1.addr(), LAPTOP, ProxyProtocol::V1));
                netmap.me.forward_targets.push(target(9, v2.addr(), NAS, ProxyProtocol::V2));
            })
            .await;

        // 普通的转发：laptop 监听，服务看到的是 laptop 的虚拟 IP，后面紧跟着客户端的数据。
        let mut client = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
        client.write_all(b"hello").await.unwrap();
        let mut service = v1.accept().await;
        let port = service.peer_addr().unwrap().port();
        let expected = format!("PROXY TCP4 10.66.0.1 127.0.0.1 {port} {}\r\nhello", v1.addr().port());
        let mut received = vec![0; expected.len()];
        tokio::time::timeout(common::PATIENCE, service.read_exact(&mut received)).await.unwrap().unwrap();
        assert_eq!(String::from_utf8(received).unwrap(), expected);

        // 监听在 nas 虚拟 IP 上的转发：经它进来的是 laptop（SOCKS5 入口在 laptop 上），v2 的头。
        let _client = socks5(world.socks().await, None, "10.66.0.3", 8080).await.connected();
        let mut service = v2.accept().await;
        let mut header = [0; 28];
        tokio::time::timeout(common::PATIENCE, service.read_exact(&mut header)).await.unwrap().unwrap();
        assert_eq!(&header[..16], b"\r\n\r\n\0\r\nQUIT\n\x21\x11\x00\x0c");
        assert_eq!(&header[16..20], &[10, 66, 0, 1], "来源是发起节点的虚拟 IP");
        assert_eq!(&header[20..24], &[127, 0, 0, 1]);
        assert_eq!(u16::from_be_bytes([header[24], header[25]]), service.peer_addr().unwrap().port());
        assert_eq!(u16::from_be_bytes([header[26], header[27]]), v2.addr().port());
    }
}
