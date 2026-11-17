//! 出口对网络里的节点也设防：筛掉不该来的流、限制一个节点同时开的流、收回能力时断开已有的流、
//! 不替别人连节点自己的地址、UDP 端口转发占不满到出口的全部名额。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::{
    GW, LAPTOP, NAS, PUBLIC_IP, UdpTarget, World, assert_echo, assert_reset, drain, echo_server, eventually, node, peer_mut, socks5,
    udp_associate, udp_echo_server,
};
use i3t_flow::{Handler, Screening};
use i3t_model::netmap::{ExitRole, Forward, ForwardProtocol, ForwardTarget, PeerExit, Relay};
use i3t_proto::PublicKey;
use i3t_proto::flow::{Reason, Status};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::sleep;

/// 断言"这件事不会发生"时观察多久。
const QUIET: Duration = Duration::from_millis(200);

/// 出口给每个发起节点的同时处理的流数。
const MAX_STREAMS_PER_PEER: usize = 16_384;

fn public(port: u16) -> String {
    format!("{PUBLIC_IP}:{port}")
}

fn gw() -> Option<(&'static str, &'static str)> {
    Some(("gw", ""))
}

fn forward(id: u32, protocol: ForwardProtocol, bind: &str, target: u32) -> Forward {
    Forward { id, protocol, bind: bind.parse().unwrap(), target: node(target), allow_from: Vec::new(), class: None }
}

/// 经 gw 连 `host:port`，确认通了。
async fn established(world: &World, host: &str, port: u16) -> TcpStream {
    let mut stream = socks5(world.socks().await, gw(), host, port).await.connected();
    assert_echo(&mut stream).await;
    stream
}

/// 这条连接被重置了（出口那一侧不再允许它）。
async fn assert_cut(stream: &mut TcpStream) {
    assert_reset(&drain(stream).await.1);
}

#[tokio::test]
async fn streams_are_screened_by_the_access_rule_and_the_number_a_peer_already_has() {
    let world = World::new().await;
    let exit = &world.gw.exit;

    assert_eq!(exit.screen(node(LAPTOP), 0), Screening::Admit);
    assert_eq!(exit.screen(node(LAPTOP), MAX_STREAMS_PER_PEER - 1), Screening::Admit);
    let Screening::Refuse(reply) = exit.screen(node(LAPTOP), MAX_STREAMS_PER_PEER) else { panic!("流太多时应该拒绝") };
    assert_eq!(reply.status, Status::TooManyStreams);
    assert_eq!(exit.screen(node(NAS), 0), Screening::Admit, "各个节点各算各的");

    // 访问规则不允许的节点：不读它的 OPEN，直接重置。
    world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = false).await;
    assert_eq!(exit.screen(node(LAPTOP), 0), Screening::Reset(Reason::AccessRevoked));
    world.gw.update(|netmap| netmap.peers.retain(|peer| peer.id != node(LAPTOP))).await;
    assert_eq!(exit.screen(node(LAPTOP), 0), Screening::Reset(Reason::AccessRevoked));

    let stats = exit.stats();
    assert_eq!((stats.total, stats.refused, stats.active), (3, 3, 0), "筛掉的流算作被拒绝的请求");
}

i3t_lane::on_both_transports! {
    async fn taking_the_exit_or_its_port_back_resets_the_flows_it_was_carrying() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        world.gw.system.redirect(&public(80), echo_server().await);
        let mut on_443 = established(&world, PUBLIC_IP, 443).await;
        let mut on_80 = established(&world, PUBLIC_IP, 80).await;

        // 和这些流无关的变化不影响它们。
        world.gw.update(|netmap| peer_mut(netmap, NAS).keep_alive = true).await;
        world.gw.update(|netmap| netmap.me.exit.as_mut().unwrap().bandwidth_mbps = Some(1000)).await;
        assert_echo(&mut on_443).await;

        // 出口不再允许 443：那条流断开，80 的照常。
        world.gw.update(|netmap| netmap.me.exit.as_mut().unwrap().ports = Some(vec![80])).await;
        assert_cut(&mut on_443).await;
        assert_echo(&mut on_80).await;

        // 关掉出口：剩下的也断开。
        world.gw.update(|netmap| netmap.me.exit = None).await;
        assert_cut(&mut on_80).await;
        eventually("出口把两条流都记成了异常结束", || world.gw.exit.stats().aborted == 2 && world.gw.exit.stats().active == 0).await;
    }
}

i3t_lane::on_both_transports! {
    async fn turning_the_lan_switch_off_resets_lan_flows_and_leaves_internet_flows_alone() {
        let world = World::new().await;
        world.gw.system.redirect("192.168.77.10:8080", echo_server().await);
        world.gw.system.redirect(&public(443), echo_server().await);
        world.laptop.update(|netmap| peer_mut(netmap, GW).exit = Some(PeerExit { lan: true, class: None, optimistic: None })).await;
        world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { lan: true, ..ExitRole::default() })).await;
        let mut lan = established(&world, "192.168.77.10", 8080).await;
        let mut internet = established(&world, PUBLIC_IP, 443).await;

        world.gw.update(|netmap| netmap.me.exit.as_mut().unwrap().lan = false).await;
        assert_cut(&mut lan).await;
        assert_echo(&mut internet).await;
    }
}

i3t_lane::on_both_transports! {
    async fn deleting_a_forward_resets_the_connections_it_carries() {
        let world = World::new().await;
        let echo = echo_server().await;
        // laptop 监听、指向 nas 回环上服务的转发；nas 虚拟 IP 上、指向同一个服务的转发。
        world.laptop.update(|netmap| netmap.me.forwards.push(forward(7, ForwardProtocol::Tcp, "127.0.0.1:0", NAS))).await;
        world
            .nas
            .update(|netmap| {
                netmap.me.forwards.push(forward(9, ForwardProtocol::Tcp, "10.66.0.3:8080", NAS));
                netmap.me.forward_targets = vec![
                    ForwardTarget { id: 7, protocol: ForwardProtocol::Tcp, addr: echo, listener: node(LAPTOP), proxy_protocol: None },
                    ForwardTarget { id: 9, protocol: ForwardProtocol::Tcp, addr: echo, listener: node(NAS), proxy_protocol: None },
                ];
            })
            .await;
        let mut listed = TcpStream::connect(world.laptop.forward(7).await).await.unwrap();
        assert_echo(&mut listed).await;
        let mut virtual_ip = socks5(world.socks().await, None, "10.66.0.3", 8080).await.connected();
        assert_echo(&mut virtual_ip).await;

        world.nas.update(|netmap| netmap.me.forward_targets.retain(|entry| entry.id != 7)).await;
        assert_cut(&mut listed).await;
        assert_echo(&mut virtual_ip).await;

        world.nas.update(|netmap| netmap.me.forwards.clear()).await;
        assert_cut(&mut virtual_ip).await;
    }
}

#[tokio::test]
async fn turning_the_lan_switch_off_stops_udp_in_both_directions_but_other_changes_do_not() {
    let world = World::new().await;
    let printer = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect("192.168.77.10:161", printer.addr());
    world.laptop.update(|netmap| peer_mut(netmap, GW).exit = Some(PeerExit { lan: true, class: None, optimistic: None })).await;
    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { lan: true, ..ExitRole::default() })).await;
    let client = udp_associate(world.socks().await, gw()).await;
    client.send("192.168.77.10", 161, b"snmp").await;
    let (_, exit_port) = printer.recv().await;

    // 一次无关的变化之后，打印机发回来的报文照样送到。
    world.gw.update(|netmap| peer_mut(netmap, NAS).keep_alive = true).await;
    printer.send(b"trap 1", exit_port).await;
    assert_eq!(client.recv().await.1, b"trap 1");

    // 开关关掉之后，打印机再发什么都不再送回来，客户端发的也不再发出去，即使客户端一直不说话。
    world.gw.update(|netmap| netmap.me.exit.as_mut().unwrap().lan = false).await;
    sleep(QUIET).await;
    printer.send(b"trap 2", exit_port).await;
    client.assert_silent(QUIET).await;
    client.send("192.168.77.10", 161, b"snmp again").await;
    printer.assert_untouched(QUIET).await;
}

i3t_lane::on_both_transports! {
    async fn the_exit_does_not_dial_its_own_public_address_or_those_of_other_nodes() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        let node_address: SocketAddr = public(20330).parse().unwrap();
        let refused = || async {
            let mut stream = socks5(world.socks().await, gw(), PUBLIC_IP, 443).await.connected();
            assert_cut(&mut stream).await;
        };
        drop(established(&world, PUBLIC_IP, 443).await);

        // 出口自己对外的地址（路由器映射出来的、relay 看到的）：家用路由器常把发往它自己公网地址的连接交给管理页面。
        *world.gw.own_endpoints.lock().unwrap() = vec![node_address];
        refused().await;
        world.gw.own_endpoints.lock().unwrap().clear();

        // netmap 里别的节点报上来的端点。
        world.gw.update(|netmap| peer_mut(netmap, NAS).endpoints = vec![node_address]).await;
        refused().await;
        world.gw.update(|netmap| peer_mut(netmap, NAS).endpoints.clear()).await;

        // relay 的地址。
        let relay = Relay {
            id: node(9),
            key: PublicKey::from_bytes([9; 32]),
            name: "r".into(),
            udp: Vec::new(),
            tcp: vec![node_address],
        };
        world.gw.update(|netmap| netmap.relays = vec![relay]).await;
        refused().await;

        eventually("出口记下了三次拒绝", || world.gw.exit.stats().refused == 3).await;
        assert_eq!(world.gw.system.connected().len(), 1, "被拒绝的都没有去连");

        // 组网里的目标不受影响：gw 自己的服务照样经覆盖网访问。
        world.gw.system.redirect("192.168.77.5:80", echo_server().await);
        *world.gw.own_endpoints.lock().unwrap() = vec!["192.168.77.5:20330".parse().unwrap(), node_address];
        let mut service = socks5(world.socks().await, None, "10.66.0.2", 80).await.connected();
        assert_echo(&mut service).await;
    }
}

#[tokio::test]
async fn udp_through_the_exit_does_not_reach_node_addresses_either() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), udp_echo_server().await);
    *world.gw.own_endpoints.lock().unwrap() = vec![public(20330).parse().unwrap()];
    let client = udp_associate(world.socks().await, gw()).await;

    client.send(PUBLIC_IP, 443, b"to the router").await;
    client.assert_silent(QUIET).await;
    eventually("出口拒绝了这个目标", || world.gw.exit.stats().datagrams_refused == 1).await;
    assert!(world.gw.system.udp_destinations().is_empty(), "一个报文也没有发出去");
}

#[tokio::test]
async fn a_full_udp_forward_leaves_room_for_other_udp_to_the_same_node() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), udp_echo_server().await);
    let service = udp_echo_server().await;
    world.laptop.update(|netmap| netmap.me.forwards.push(forward(7, ForwardProtocol::Udp, "127.0.0.1:0", GW))).await;
    world
        .gw
        .update(|netmap| {
            netmap.me.forward_targets.push(ForwardTarget {
                id: 7,
                protocol: ForwardProtocol::Udp,
                addr: service,
                listener: node(LAPTOP),
                proxy_protocol: None,
            });
        })
        .await;
    let listening = world.laptop.forward(7).await;

    // 一大群来源把转发占满：它最多开 64 个会话，每个是到 gw 的一个上下文。
    let mut clients = Vec::new();
    for _ in 0..80 {
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"x", listening).await.unwrap();
        clients.push(client);
    }
    eventually("转发开满了它的会话", || world.gw.exit.stats().active == 64).await;
    sleep(QUIET).await;
    assert_eq!(world.gw.exit.stats().active, 64, "转发不会开出更多的上下文");

    // laptop 经 gw 的别的 UDP 不受影响。
    let other = udp_associate(world.socks().await, gw()).await;
    other.send(PUBLIC_IP, 443, b"still room").await;
    assert_eq!(other.recv().await.1, b"still room");
}
