//! 出口上的约束（配额、建连预算、地址族）、排空，以及监听在虚拟 IP 上、目标是别的节点的端口转发。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::{
    GW, LAPTOP, NAS, PUBLIC_IP, Target, UdpTarget, World, assert_echo, assert_reset, drain, echo_server, node, peer_mut, proxy, socks5,
    udp_associate,
};
use i3t_exit::{Exit, Outbound};
use i3t_model::netmap::{ExitRole, Forward, ForwardProtocol, ForwardTarget, IpFamily, ListenerProtocol};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

/// 在本机起一个服务：每条连接一上来就写 `len` 字节，然后关掉。
async fn sender(len: usize) -> SocketAddr {
    let target = Target::bind("127.0.0.1").await;
    let addr = target.addr();
    tokio::spawn(async move {
        loop {
            let mut stream = target.accept().await;
            tokio::spawn(async move {
                let _ = stream.write_all(&vec![7; len]).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    addr
}

/// laptop 上再加一个经 gw 出去、关掉了乐观回复的 SOCKS5 代理，好让客户端拿到出口给的准确的错误码。
async fn careful_socks(world: &World) -> SocketAddr {
    world
        .laptop
        .update(|netmap| {
            let mut careful = proxy(5, ListenerProtocol::Socks5, "127.0.0.1:0", GW);
            careful.optimistic = Some(false);
            netmap.me.proxies.push(careful);
        })
        .await;
    world.laptop.proxy(5).await
}

i3t_lane::on_both_transports! {
    async fn a_spent_quota_cuts_the_running_flow_refuses_new_ones_and_survives_a_restart() {
        let world = World::new().await;
        world.gw.system.redirect(&format!("{PUBLIC_IP}:443"), sender(300_000).await);
        world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { quota_bytes: Some(100_000), ..ExitRole::default() })).await;
        let careful = careful_socks(&world).await;

        // 下载到一半配额用完：流在那一刻被断开，客户端看到的是重置，不是正常结束。
        let mut stream = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        let (data, end) = drain(&mut stream).await;
        assert_reset(&end);
        assert!(data.len() <= 100_000, "配额之外的数据不再送出：收到 {}", data.len());
        let quota = world.gw.exit.stats().quota.unwrap();
        assert_eq!(quota.limit_bytes, 100_000);
        assert!(quota.used_bytes >= 100_000, "{quota:?}");
        assert!(quota.period.start <= quota.period.end);

        // 新的请求直接拒绝，状态说明是配额用完了：SOCKS5 回 0x02。
        assert_eq!(socks5(careful, None, PUBLIC_IP, 443).await.failed(), 2);
        assert_eq!(world.gw.exit.stats().refused, 1);

        // 节点重启：用量从存下来的那一份接着记，配额还是用完的。
        let saved = world.gw.exit.quota_state();
        assert_eq!(saved.used_bytes, quota.used_bytes);
        let mut netmap = common::netmap(GW);
        netmap.me.exit = Some(ExitRole { quota_bytes: Some(100_000), ..ExitRole::default() });
        let (_sender, receiver) = watch::channel(Arc::new(netmap));
        let outbound = Outbound::new(receiver.clone(), world.gw.system.clone());
        let (connector, _) = world.mesh.join(node(9));
        let restarted = Exit::new(receiver, outbound, Arc::new(connector)).with_quota_state(saved);
        assert_eq!(restarted.stats().quota.map(|quota| quota.used_bytes), Some(saved.used_bytes));
    }
}

i3t_lane::on_both_transports! {
    async fn the_connect_budget_stops_one_initiator_from_hammering_one_target() {
        let world = World::new().await;
        world.gw.system.redirect(&format!("{PUBLIC_IP}:443"), echo_server().await);
        world.gw.system.redirect(&format!("{PUBLIC_IP}:80"), echo_server().await);
        world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { connects_per_minute: Some(2), ..ExitRole::default() })).await;
        let careful = careful_socks(&world).await;

        for _ in 0..2 {
            let mut stream = socks5(careful, None, PUBLIC_IP, 443).await.connected();
            assert_echo(&mut stream).await;
        }
        assert_eq!(socks5(careful, None, PUBLIC_IP, 443).await.failed(), 1, "第三次超过了预算");
        let mut other = socks5(careful, None, PUBLIC_IP, 80).await.connected();
        assert_echo(&mut other).await;
        assert_eq!(world.gw.exit.stats().refused, 1);
        assert_eq!(world.gw.system.connected().len(), 3, "被拒绝的那一次没有去连目标");
    }
}

i3t_lane::on_both_transports! {
    async fn an_ipv4_only_exit_connects_only_to_ipv4_addresses() {
        let world = World::new().await;
        let echo = echo_server().await;
        world.gw.system.redirect(&format!("{PUBLIC_IP}:443"), echo);
        world.gw.system.name("dual.example", &["2606:4700::1", PUBLIC_IP]);
        world.gw.system.name("v6.example", &["2606:4700::1"]);
        world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { family: IpFamily::Ipv4, ..ExitRole::default() })).await;
        let careful = careful_socks(&world).await;

        let mut stream = socks5(careful, None, "dual.example", 443).await.connected();
        assert_echo(&mut stream).await;
        assert_eq!(world.gw.system.connected(), [format!("{PUBLIC_IP}:443").parse().unwrap()], "IPv6 的地址没有去试");
        assert_eq!(socks5(careful, None, "v6.example", 443).await.failed(), 2);
        assert_eq!(socks5(careful, None, "2606:4700::1", 443).await.failed(), 2);
        assert_eq!(world.gw.system.connected().len(), 1);
    }
}

i3t_lane::on_both_transports! {
    async fn a_draining_exit_refuses_new_flows_with_its_own_status_after_the_access_check() {
        let world = World::new().await;
        world.gw.system.redirect(&format!("{PUBLIC_IP}:443"), echo_server().await);
        let careful = careful_socks(&world).await;
        let mut established = socks5(careful, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut established).await;

        world.gw.exit.set_draining(Some("节点正在排空，准备更新到 0.2.0 后重启".to_owned()));
        assert_eq!(socks5(careful, None, PUBLIC_IP, 443).await.failed(), 1, "SOCKS5 回 0x01：换一个出口或者稍后再试");
        assert_eq!(socks5(careful, None, "10.66.0.2", 22).await.failed(), 1, "节点自己的服务也不接");
        let stats = world.gw.exit.stats();
        assert_eq!((stats.drained, stats.refused, stats.failed_by_exit, stats.draining), (2, 2, 0, true), "排空不算出口坏了");
        assert_echo(&mut established).await;

        // 不能访问这个节点的发起方看不出它在排空。
        world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = false).await;
        assert_eq!(socks5(careful, None, PUBLIC_IP, 443).await.failed(), 2);
        assert_eq!(world.gw.exit.stats().drained, 2);

        world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = true).await;
        world.gw.exit.set_draining(None);
        let mut stream = socks5(careful, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut stream).await;
    }
}

fn tcp_forward(id: u32, bind: &str, target: u32) -> Forward {
    Forward { id, protocol: ForwardProtocol::Tcp, bind: bind.parse().unwrap(), target: node(target), allow_from: Vec::new(), class: None }
}

i3t_lane::on_both_transports! {
    async fn a_forward_on_a_virtual_ip_reaches_a_service_on_another_node_when_the_access_rules_allow_it() {
        let world = World::new().await;
        let echo = echo_server().await;
        // gw 把 nas 回环上的服务发布在 gw 自己的虚拟 IP 的 9100 端口上。
        world.gw.update(|netmap| netmap.me.forwards.push(tcp_forward(9, "10.66.0.2:9100", NAS))).await;
        world
            .nas
            .update(|netmap| {
                netmap.me.forward_targets.push(ForwardTarget { id: 9, protocol: ForwardProtocol::Tcp, addr: echo, listener: node(GW), proxy_protocol: None });
            })
            .await;

        let mut stream = socks5(world.socks().await, None, "10.66.0.2", 9100).await.connected();
        assert_echo(&mut stream).await;
        stream.shutdown().await.unwrap();
        assert!(drain(&mut stream).await.1.is_ok(), "正常结束一路传过去，也一路传回来");
        assert_eq!(world.nas.system.connected(), [echo], "是 nas 去连的目标");
        common::eventually("gw 记下接力的流", || world.gw.exit.stats().bytes_to_target >= common::ECHO_LEN).await;

        // 访问规则不允许 gw 访问 nas：gw 不替它接力，客户端拿到"不允许"。
        world.gw.update(|netmap| peer_mut(netmap, NAS).outbound = false).await;
        assert_eq!(socks5(world.socks().await, None, "10.66.0.2", 9100).await.failed(), 2);
        assert_eq!(world.nas.exit.stats().total, 1, "nas 根本没有收到第二个请求");

        // 允许了，但 nas 上没有下发这一条（或者下发给了别的监听节点）：nas 的拒绝原样转给客户端。
        world.gw.update(|netmap| peer_mut(netmap, NAS).outbound = true).await;
        world.nas.update(|netmap| netmap.me.forward_targets[0].listener = node(LAPTOP)).await;
        assert_eq!(socks5(world.socks().await, None, "10.66.0.2", 9100).await.failed(), 2);
        assert_eq!(world.nas.exit.stats().refused, 1);

        // 来源白名单照样按发起节点的虚拟 IP 查。
        world.nas.update(|netmap| netmap.me.forward_targets[0].listener = node(GW)).await;
        world.gw.update(|netmap| netmap.me.forwards[0].allow_from = vec!["10.66.0.3/32".parse().unwrap()]).await;
        assert_eq!(socks5(world.socks().await, None, "10.66.0.2", 9100).await.failed(), 2);
    }
}

#[tokio::test]
async fn a_udp_forward_on_a_virtual_ip_is_relayed_to_another_node_through_a_second_context() {
    let world = World::new().await;
    let service = UdpTarget::bind("127.0.0.1").await;
    // gw 把 nas 回环上的 UDP 服务发布在 gw 自己的虚拟 IP 的 5300 端口上。
    let forward = Forward { protocol: ForwardProtocol::Udp, ..tcp_forward(10, "10.66.0.2:5300", NAS) };
    world.gw.update(|netmap| netmap.me.forwards.push(forward)).await;
    world
        .nas
        .update(|netmap| {
            let target =
                ForwardTarget { id: 10, protocol: ForwardProtocol::Udp, addr: service.addr(), listener: node(GW), proxy_protocol: None };
            netmap.me.forward_targets.push(target);
        })
        .await;

    let client = udp_associate(world.socks().await, None).await;
    client.send("10.66.0.2", 5300, b"ping").await;
    let (data, from) = service.recv().await;
    assert_eq!(data, b"ping");
    service.send(b"pong", from).await;
    assert_eq!(client.recv().await, ("10.66.0.2:5300".to_owned(), b"pong".to_vec()), "回来的报文报成从 gw 的那个端口来的");
    assert_eq!(world.nas.system.udp_destinations(), [service.addr()], "是 nas 发给目标的");
    // 同一个会话里后面的报文走已经开着的接力上下文：nas 上只有一个上下文。
    client.send("10.66.0.2", 5300, b"again").await;
    assert_eq!(service.recv().await.0, b"again");
    assert_eq!(world.nas.exit.stats().total, 1);

    // 访问规则不允许 gw 访问 nas：gw 不再替它接力，报文被丢掉。
    world.gw.update(|netmap| peer_mut(netmap, NAS).outbound = false).await;
    client.send("10.66.0.2", 5300, b"denied").await;
    service.assert_untouched(Duration::from_millis(300)).await;
}
