//! UDP 从头走到尾：测试扮演的 SOCKS5 客户端 ──UDP──▶ 入口 ──数据报上下文──▶ 出口 ──UDP──▶ 测试扮演的目标。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{
    GW, LAPTOP, NAS, PATIENCE, PUBLIC_IP, UdpTarget, World, eventually, node, peer_mut, socks5_udp_packet, udp_associate,
    udp_associate_from, udp_echo_server,
};
use i3t_exit::ExitStats;
use i3t_model::netmap::{ExitRole, Forward, ForwardProtocol, ForwardTarget, PeerExit};
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

/// 断言"这件事不会发生"时观察多久。
const QUIET: Duration = Duration::from_millis(200);

fn public(port: u16) -> String {
    format!("{PUBLIC_IP}:{port}")
}

/// 另一个"公网地址"。
const OTHER_PUBLIC_IP: &str = "93.184.216.35";

fn gw_user() -> Option<(&'static str, &'static str)> {
    Some(("gw", ""))
}

/// 等出口把到现在为止的报文都处理完，返回那时的计数。
async fn settled(world: &World, what: &str, done: impl Fn(&ExitStats) -> bool) -> ExitStats {
    eventually(what, || done(&world.gw.exit.stats())).await;
    world.gw.exit.stats()
}

#[tokio::test]
async fn udp_through_the_exit_named_by_the_username_reaches_the_target_and_comes_back() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), udp_echo_server().await);
    let client = udp_associate(world.socks().await, gw_user()).await;
    assert!(client.relay.ip().is_loopback(), "中继端口绑在客户端连过来的那个地址上：{}", client.relay);

    client.send(PUBLIC_IP, 443, b"ping through the mesh").await;
    assert_eq!(client.recv().await, (public(443), b"ping through the mesh".to_vec()), "回来的报文带着它的来源");

    // 比连接一次能发的长的报文：在两个节点之间切开又拼上，两头的应用看不出来。
    let large: Vec<u8> = (0..9000u32).map(|index| u8::try_from(index % 251).unwrap()).collect();
    client.send(PUBLIC_IP, 443, &large).await;
    assert_eq!(client.recv().await, (public(443), large));

    // 都是从 gw 出去的，laptop 自己没有往任何地方发。
    let stats = settled(&world, "出口记完两个来回", |stats| stats.datagrams_from_target == 2).await;
    assert_eq!((stats.datagrams_to_target, stats.bytes_to_target, stats.bytes_from_target), (2, 21 + 9000, 21 + 9000));
    assert_eq!((stats.total, stats.active, stats.datagrams_refused), (1, 1, 0), "一个关联到一个出口是一个上下文");
    assert_eq!(world.gw.system.udp_destinations(), [public(443).parse::<SocketAddr>().unwrap()], "每个目标只检查一次");
    assert!(world.laptop.system.udp_destinations().is_empty());
}

#[tokio::test]
async fn one_context_is_one_port_on_the_exit_whatever_the_destination() {
    let world = World::new().await;
    let (first, second) = (UdpTarget::bind("127.0.0.1").await, UdpTarget::bind("127.0.0.1").await);
    world.gw.system.redirect(&public(443), first.addr());
    world.gw.system.redirect(&format!("{OTHER_PUBLIC_IP}:443"), second.addr());
    let client = udp_associate(world.socks().await, gw_user()).await;

    client.send(PUBLIC_IP, 443, b"to the first").await;
    client.send(OTHER_PUBLIC_IP, 443, b"to the second").await;
    let (data, seen_by_first) = first.recv().await;
    assert_eq!(data, b"to the first");
    let (data, seen_by_second) = second.recv().await;
    assert_eq!(data, b"to the second");
    assert_eq!(seen_by_first, seen_by_second, "两个目标看到的来源是同一个地址和端口");

    // 各自回来的报文带着各自的来源。
    second.send(b"from the second", seen_by_second).await;
    first.send(b"from the first", seen_by_first).await;
    let replies: BTreeSet<_> = [client.recv().await, client.recv().await].into();
    let expected = [(public(443), b"from the first".to_vec()), (format!("{OTHER_PUBLIC_IP}:443"), b"from the second".to_vec())];
    assert_eq!(replies, expected.into());

    // 没有往它发过报文的地址发到这个端口上的东西，不替它转。
    let stranger = UdpTarget::bind("127.0.0.1").await;
    stranger.send(b"unsolicited", seen_by_first).await;
    client.assert_silent(QUIET).await;

    // 另一个关联是另一个上下文，在出口上是另一个端口。
    let other = udp_associate(world.socks().await, gw_user()).await;
    other.send(PUBLIC_IP, 443, b"another association").await;
    assert_ne!(first.recv().await.1, seen_by_first);
}

/// 一个查 `example.com` 的 A 记录的 DNS 请求。
fn dns_query(id: u16) -> Vec<u8> {
    let mut query = id.to_be_bytes().to_vec();
    // 标志：要求递归；一个问题，没有别的。
    query.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    query.extend_from_slice(b"\x07example\x03com\x00");
    // 类型 A，类 IN。
    query.extend_from_slice(&[0, 1, 0, 1]);
    query
}

/// 对 [`dns_query`] 的应答：问题原样带回，加一条指向 `answer` 的 A 记录。
fn dns_response(query: &[u8], answer: [u8; 4]) -> Vec<u8> {
    let mut response = query.to_vec();
    // 标志：这是应答，递归可用；一条回答。
    response[2..4].copy_from_slice(&[0x81, 0x80]);
    response[6..8].copy_from_slice(&[0, 1]);
    // 名字指回问题里的那个，类型 A，类 IN，TTL 60，4 字节的地址。
    response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
    response.extend_from_slice(&answer);
    response
}

#[tokio::test]
async fn a_dns_style_query_to_a_domain_name_is_resolved_on_the_exit_and_answered_from_the_resolved_address() {
    let world = World::new().await;
    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { ports: Some(vec![53]), ..ExitRole::default() })).await;
    world.gw.system.name("dns.example", &[PUBLIC_IP]);
    let server = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(53), server.addr());
    let client = udp_associate(world.socks().await, gw_user()).await;

    for id in [0x1234, 0x1235] {
        // 目标写的是域名：入口不解析，原样交给出口。
        client.send("dns.example", 53, &dns_query(id)).await;
        let (query, from) = server.recv().await;
        assert_eq!(query, dns_query(id));
        server.send(&dns_response(&query, [203, 0, 113, 7]), from).await;

        // 应答回来了，来源是域名解析出来的那个地址。
        let (source, response) = client.recv().await;
        assert_eq!(source, public(53));
        assert_eq!(response, dns_response(&dns_query(id), [203, 0, 113, 7]));
        assert_eq!(response[..2], id.to_be_bytes(), "应答对得上请求");
    }
    assert_eq!(world.gw.system.resolved(), ["dns.example"], "两个请求只问了一次解析器");
    assert!(world.laptop.system.resolved().is_empty(), "域名不在入口上解析");
}

#[tokio::test]
async fn an_internet_target_needs_the_exit_to_be_enabled_and_the_port_to_be_allowed() {
    let world = World::new().await;
    let target = UdpTarget::bind("127.0.0.1").await;
    for port in [53, 443] {
        world.gw.system.redirect(&public(port), target.addr());
    }
    let client = udp_associate(world.socks().await, gw_user()).await;

    // 端口不在出口允许的范围里（默认只有 80 和 443）：报文被丢掉。
    client.send(PUBLIC_IP, 53, b"dns").await;
    let stats = settled(&world, "出口拒绝了发往 53 端口的报文", |stats| stats.datagrams_refused == 1).await;
    assert_eq!((stats.refused, stats.datagrams_to_target), (1, 0));
    // 同一个上下文里，允许的端口照常通；再发往被拒绝的端口时直接丢，不再检查一遍。
    client.send(PUBLIC_IP, 443, b"quic").await;
    assert_eq!(target.recv().await.0, b"quic");
    client.send(PUBLIC_IP, 53, b"dns again").await;
    let stats = settled(&world, "第二个发往 53 端口的报文也被丢掉", |stats| stats.datagrams_refused == 2).await;
    assert_eq!(stats.refused, 1, "被拒绝的目标只记一次原因");

    // gw 自己的 netmap 里没有启用出口，而 laptop 以为它是：gw 拒绝，不信任入口的判断。
    // netmap 一变，之前记着的检查结果就作废，所以刚才还通的 443 也不通了。
    world.gw.update(|netmap| netmap.me.exit = None).await;
    client.send(PUBLIC_IP, 443, b"quic again").await;
    let stats = settled(&world, "没有启用出口的节点拒绝了报文", |stats| stats.datagrams_refused == 3).await;
    assert_eq!((stats.refused, stats.datagrams_to_target, stats.failed_by_exit), (2, 1, 0));
    target.assert_untouched(QUIET).await;

    // 入口知道一个节点不是出口时，自己就丢了，连上下文都不开。
    let nas_client = udp_associate(world.socks().await, Some(("nas", ""))).await;
    nas_client.send(PUBLIC_IP, 443, b"via a node that is no exit").await;
    sleep(QUIET).await;
    assert_eq!(world.nas.exit.stats().total, 0);
}

#[tokio::test]
async fn addresses_that_are_not_public_are_refused_even_when_a_domain_name_resolves_to_them() {
    let world = World::new().await;
    world.gw.system.name("rebind.example", &[PUBLIC_IP, "10.0.0.5"]);
    world.gw.system.name("fine.example", &[PUBLIC_IP]);
    let target = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());
    for forbidden in ["127.0.0.1:443", "100.64.0.1:443", "10.0.0.5:443", "10.66.0.2:443"] {
        world.gw.system.redirect(forbidden, target.addr());
    }
    let client = udp_associate(world.socks().await, gw_user()).await;

    // 回环、运营商级 NAT 这些写成字面量的地址，入口按互联网目标交给出口，出口的出站策略拒绝。
    // 解析出来的地址里只要有一个不该去，整个域名都拒绝。
    let refused = ["127.0.0.1", "100.64.0.1", "rebind.example"];
    for host in refused {
        client.send(host, 443, b"should not arrive").await;
    }
    let stats = settled(&world, "出口拒绝了三个目标", |stats| stats.datagrams_refused == 3).await;
    assert_eq!((stats.refused, stats.datagrams_to_target), (3, 0));
    target.assert_untouched(QUIET).await;
    assert!(world.gw.system.udp_destinations().is_empty(), "被拒绝的目标一个报文都没有发出去");

    // 对照：解析到公网地址的域名是通的。
    client.send("fine.example", 443, b"arrives").await;
    assert_eq!(target.recv().await.0, b"arrives");
}

#[tokio::test]
async fn a_lan_target_needs_the_lan_switch_and_an_address_on_an_attached_subnet() {
    let world = World::new().await;
    let printer = UdpTarget::bind("127.0.0.1").await;
    // gw 的假网卡在 192.168.77.5/24 上。
    world.gw.system.redirect("192.168.77.10:161", printer.addr());
    world.gw.system.redirect("192.168.1.10:161", printer.addr());
    // laptop 以为 gw 开了局域网开关，所以入口放行；gw 自己其实没开。
    world.laptop.update(|netmap| peer_mut(netmap, GW).exit = Some(PeerExit { lan: true, class: None })).await;
    let client = udp_associate(world.socks().await, gw_user()).await;

    client.send("192.168.77.10", 161, b"snmp").await;
    let stats = settled(&world, "没开局域网开关的出口拒绝了报文", |stats| stats.datagrams_refused == 1).await;
    assert_eq!((stats.refused, stats.datagrams_to_target), (1, 0));
    printer.assert_untouched(QUIET).await;

    // 打开开关：直连网段里的地址通了，端口不受出口的端口白名单限制；回来的报文带着局域网里的来源。
    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { lan: true, ..ExitRole::default() })).await;
    client.send("192.168.77.10", 161, b"snmp").await;
    let (data, from) = printer.recv().await;
    assert_eq!(data, b"snmp");
    printer.send(b"snmp reply", from).await;
    assert_eq!(client.recv().await, ("192.168.77.10:161".to_owned(), b"snmp reply".to_vec()));

    // 不在直连网段里的私有地址还是不行。
    client.send("192.168.1.10", 161, b"elsewhere").await;
    settled(&world, "不在直连网段里的地址被拒绝", |stats| stats.datagrams_refused == 2).await;
    printer.assert_untouched(QUIET).await;
}

#[tokio::test]
async fn an_exit_with_an_upstream_proxy_does_not_send_udp_around_it() {
    let world = World::new().await;
    let target = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());
    let upstream = Some("127.0.0.1:1".parse().unwrap());
    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { upstream_socks5: upstream, ..ExitRole::default() })).await;
    let client = udp_associate(world.socks().await, gw_user()).await;

    client.send(PUBLIC_IP, 443, b"must not bypass the upstream").await;
    settled(&world, "配了上游代理的出口拒绝了 UDP", |stats| stats.datagrams_refused == 1).await;
    target.assert_untouched(QUIET).await;
}

#[tokio::test]
async fn the_access_rule_is_checked_when_the_context_opens_and_revoking_it_ends_the_context() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), udp_echo_server().await);
    let client = udp_associate(world.socks().await, gw_user()).await;
    client.send(PUBLIC_IP, 443, b"allowed").await;
    assert_eq!(client.recv().await.1, b"allowed");

    // gw 不再允许 laptop 访问它：已经开着的上下文被重置。
    world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = false).await;
    let stats = settled(&world, "访问权限收回之后上下文被重置", |stats| stats.active == 0).await;
    assert_eq!(stats.aborted, 1);

    // 入口再去开上下文时被拒绝，报文到不了任何地方。
    client.send(PUBLIC_IP, 443, b"no longer allowed").await;
    let stats = settled(&world, "新的上下文被拒绝", |stats| stats.total == 2).await;
    assert_eq!((stats.refused, stats.datagrams_to_target), (1, 1));
    client.assert_silent(QUIET).await;
}

#[tokio::test]
async fn an_overlay_address_is_a_udp_service_on_that_node_whatever_the_username_says() {
    let world = World::new().await;
    // nas 的假网卡地址是 192.168.77.5，把它上面的 5353 引到测试服务。nas 不是出口。
    world.nas.system.redirect("192.168.77.5:5353", udp_echo_server().await);

    for username in [None, gw_user()] {
        let client = udp_associate(world.socks().await, username).await;
        for host in ["10.66.0.3", "nas.home.i3t"] {
            client.send(host, 5353, b"mdns").await;
            // 回来的报文报的是那个节点的虚拟 IP：客户端当初就是往那里发的。
            assert_eq!(client.recv().await, ("10.66.0.3:5353".to_owned(), b"mdns".to_vec()), "{username:?} {host}");
        }
    }
    assert_eq!(world.gw.exit.stats().total, 0, "组网地址不会被交给出口");
    let tried = world.nas.system.udp_destinations();
    assert!(!tried.is_empty() && tried.iter().all(|addr| !addr.ip().is_loopback()), "只用非回环地址去找本机的服务：{tried:?}");
}

#[tokio::test]
async fn without_a_username_a_loopback_listener_sends_udp_from_this_machine() {
    let world = World::new().await;
    let echo = udp_echo_server().await;
    world.laptop.system.redirect(&public(53), echo);
    let client = udp_associate(world.socks().await, None).await;

    // 回环监听器的调用者是本机的程序：不经过任何出口，也不套出口的端口限制。
    client.send(PUBLIC_IP, 53, b"local egress").await;
    let (source, data) = client.recv().await;
    assert_eq!((source, data), (echo.to_string(), b"local egress".to_vec()), "来源是报文实际从哪里回来的");
    assert_eq!(world.laptop.system.udp_destinations(), [public(53).parse::<SocketAddr>().unwrap()]);
    assert_eq!(world.gw.exit.stats().total, 0);
}

#[tokio::test]
async fn the_association_and_its_contexts_end_when_the_tcp_connection_closes() {
    let world = World::new().await;
    let target = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());
    let client = udp_associate(world.socks().await, gw_user()).await;
    client.send(PUBLIC_IP, 443, b"while the control connection is open").await;
    target.recv().await;
    assert_eq!(world.gw.exit.stats().active, 1);

    // TCP 连接一关，关联就结束：出口上的上下文立刻跟着结束（不用等空闲回收），中继端口也不再转发。
    let common::UdpAssociation { control, socket, relay } = client;
    drop(control);
    let stats = settled(&world, "TCP 连接关掉之后出口上的上下文结束", |stats| stats.active == 0).await;
    assert_eq!(stats.aborted, 0, "这是正常结束");
    socket.send_to(&socks5_udp_packet(PUBLIC_IP, 443, b"after the end"), relay).await.unwrap();
    target.assert_untouched(QUIET).await;
}

#[tokio::test]
async fn only_the_client_that_made_the_association_may_use_its_relay_port() {
    let world = World::new().await;
    let target = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());
    let packet = socks5_udp_packet(PUBLIC_IP, 443, b"from somebody else");

    // 客户端发过第一个报文之后，同一个地址的别的端口发来的不认。
    let client = udp_associate(world.socks().await, gw_user()).await;
    client.send(PUBLIC_IP, 443, b"from the client").await;
    let (_, exit_port) = target.recv().await;
    let other_port = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    other_port.send_to(&packet, client.relay).await.unwrap();
    // 别的地址发来的更不认（这里是回环上的另一个地址）。
    let other_address = UdpSocket::bind("127.0.0.2:0").await.unwrap();
    other_address.send_to(&packet, client.relay).await.unwrap();
    target.assert_untouched(QUIET).await;

    // 回来的报文只发给客户端，不会发给那两个。
    target.send(b"reply", exit_port).await;
    assert_eq!(client.recv().await.1, b"reply");
    let mut buf = [0; 16];
    assert!(timeout(QUIET, other_port.recv_from(&mut buf)).await.is_err());

    // 请求里写明了自己地址和端口的客户端：别人抢在它前面发也没有用。
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let [hi, lo] = socket.local_addr().unwrap().port().to_be_bytes();
    let declared = udp_associate_from(world.socks().await, gw_user(), socket, [127, 0, 0, 1, hi, lo]).await;
    other_port.send_to(&packet, declared.relay).await.unwrap();
    target.assert_untouched(QUIET).await;
    declared.send(PUBLIC_IP, 443, b"from the declared port").await;
    assert_eq!(target.recv().await.0, b"from the declared port");
}

#[tokio::test]
async fn fragmented_and_malformed_socks5_udp_packets_are_dropped() {
    let world = World::new().await;
    let target = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());
    let client = udp_associate(world.socks().await, gw_user()).await;

    let mut fragment = socks5_udp_packet(PUBLIC_IP, 443, b"a fragment");
    fragment[2] = 1;
    let mut reserved = socks5_udp_packet(PUBLIC_IP, 443, b"reserved bytes set");
    reserved[0] = 1;
    for bad in [&fragment[..], &reserved, &[0, 0, 0, 1, 93, 184][..], &[0, 0, 0, 9, 1, 2, 3, 4, 0, 80], &[]] {
        client.socket.send_to(bad, client.relay).await.unwrap();
    }
    target.assert_untouched(QUIET).await;
    assert_eq!(world.gw.exit.stats().total, 0, "没有一个报文走到要开上下文的地步");

    client.send(PUBLIC_IP, 443, b"well formed").await;
    assert_eq!(target.recv().await.0, b"well formed");
}

#[tokio::test]
async fn a_context_is_collected_after_the_idle_timeout_and_the_next_datagram_opens_a_new_one() {
    let world = World::with_udp_idle(Duration::from_millis(300)).await;
    let target = UdpTarget::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());
    let client = udp_associate(world.socks().await, gw_user()).await;

    // 有报文往来的时候不回收：前后一秒，一直是同一个上下文。
    for _ in 0..10 {
        client.send(PUBLIC_IP, 443, b"keeps the context alive").await;
        target.recv().await;
        sleep(Duration::from_millis(100)).await;
    }
    assert_eq!((world.gw.exit.stats().total, world.gw.exit.stats().active), (1, 1));

    // 静下来之后出口把它回收了，这是正常结束。TCP 连接还开着，关联还在。
    let stats = settled(&world, "空闲的上下文被回收", |stats| stats.active == 0).await;
    assert_eq!(stats.aborted, 0);

    // 再发报文：入口重新开一个上下文，照样通。
    client.send(PUBLIC_IP, 443, b"after the idle timeout").await;
    assert_eq!(target.recv().await.0, b"after the idle timeout");
    assert_eq!((world.gw.exit.stats().total, world.gw.exit.stats().active), (2, 1));
}

#[tokio::test]
async fn the_reliable_class_delivers_every_datagram_under_loss_while_the_default_class_may_drop() {
    const COUNT: u32 = 150;
    let world = World::new().await;
    world.gw.system.redirect(&public(443), udp_echo_server().await);
    let reliable = udp_associate(world.socks().await, Some(("gw+c-r", ""))).await;
    let plain = udp_associate(world.socks().await, gw_user()).await;

    // 两个节点之间不保证送达的数据报每三个丢一个，两个方向都算。
    let seen = AtomicUsize::new(0);
    world.mesh.set_datagram_filter(Some(Box::new(move |_, _, _| seen.fetch_add(1, Ordering::Relaxed) % 3 != 2)));

    for client in [&reliable, &plain] {
        for index in 0..COUNT {
            client.send(PUBLIC_IP, 443, &index.to_be_bytes()).await;
            // 客户端和中继端口之间是真的 UDP：别一口气塞满内核的缓冲区。
            if index % 10 == 9 {
                sleep(Duration::from_millis(2)).await;
            }
        }
    }

    // 等级是 reliable 的：每个报文都回来了，一个不少（先后不保证）。
    let mut echoed = BTreeSet::new();
    for _ in 0..COUNT {
        let (source, data) = reliable.recv().await;
        assert_eq!(source, public(443));
        assert!(echoed.insert(u32::from_be_bytes(data.try_into().unwrap())), "同一个报文回来了两次");
    }
    assert_eq!(echoed, (0..COUNT).collect());
    reliable.assert_silent(QUIET).await;

    // 默认等级的：去的路上丢一些，回来的路上再丢一些。
    let mut survived = 0;
    let mut buf = [0; 64];
    while timeout(QUIET, plain.socket.recv_from(&mut buf)).await.is_ok() {
        survived += 1;
    }
    assert!((1..COUNT).contains(&survived), "不保证送达的报文回来了 {survived} 个");
}

#[tokio::test]
async fn a_udp_port_forward_carries_each_client_in_its_own_session_to_the_listed_address() {
    let world = World::new().await;
    let service = UdpTarget::bind("127.0.0.1").await;
    let forward = |id| Forward {
        id,
        protocol: ForwardProtocol::Udp,
        bind: "127.0.0.1:0".parse().unwrap(),
        target: node(GW),
        allow_from: vec![],
        class: None,
    };
    world.laptop.update(|netmap| netmap.me.forwards = vec![forward(7), forward(8), forward(9)]).await;
    // gw 不需要是出口；它只认下发给它、由 laptop 监听的那一条 UDP 转发。8 号在它那里是 TCP 的，9 号是别的节点监听的。
    let target = |id, protocol, listener| ForwardTarget { id, protocol, addr: service.addr(), listener: node(listener) };
    world
        .gw
        .update(|netmap| {
            netmap.me.exit = None;
            netmap.me.forward_targets = vec![
                target(7, ForwardProtocol::Udp, LAPTOP),
                target(8, ForwardProtocol::Tcp, LAPTOP),
                target(9, ForwardProtocol::Udp, NAS),
            ];
        })
        .await;
    let listening = world.laptop.forward(7).await;

    // 两个客户端各是一个会话，在目标节点上各是一个端口；回来的报文各回各的，而且是从监听的端口发回来的。
    let (first, second) = (UdpSocket::bind("127.0.0.1:0").await.unwrap(), UdpSocket::bind("127.0.0.1:0").await.unwrap());
    first.send_to(b"from the first client", listening).await.unwrap();
    let (data, first_seen) = service.recv().await;
    assert_eq!(data, b"from the first client");
    second.send_to(b"from the second client", listening).await.unwrap();
    let (data, second_seen) = service.recv().await;
    assert_eq!(data, b"from the second client");
    assert_ne!(first_seen, second_seen);

    service.send(b"to the second client", second_seen).await;
    service.send(b"to the first client", first_seen).await;
    let mut buf = [0; 64];
    for (client, expected) in [(&first, &b"to the first client"[..]), (&second, b"to the second client")] {
        let (len, from) = timeout(PATIENCE, client.recv_from(&mut buf)).await.expect("回来的报文应该到客户端").unwrap();
        assert_eq!((&buf[..len], from), (expected, listening));
    }
    let status = world.laptop.settled().await;
    assert_eq!(status.forwards.iter().find(|forward| forward.id == 7).unwrap().active.get(), 2, "两个会话");

    // 协议对不上、或者不是下发给这个监听节点的转发：目标节点不认，报文被丢掉。
    for id in [8, 9] {
        first.send_to(b"not a forward gw accepts", world.laptop.forward(id).await).await.unwrap();
    }
    let stats = settled(&world, "目标节点拒绝了另外两条转发", |stats| stats.datagrams_refused == 2).await;
    assert_eq!((stats.refused, stats.datagrams_to_target), (2, 2));
    service.assert_untouched(QUIET).await;
}

#[tokio::test]
async fn a_udp_forward_session_is_collected_after_the_idle_timeout_and_only_admits_allowed_sources() {
    let world = World::with_udp_idle(Duration::from_millis(300)).await;
    let service = UdpTarget::bind("127.0.0.1").await;
    let bind = "127.0.0.1:0".parse().unwrap();
    let open = Forward { id: 7, protocol: ForwardProtocol::Udp, bind, target: node(GW), allow_from: vec![], class: None };
    let guarded = Forward { id: 8, allow_from: vec!["192.168.0.0/16".parse().unwrap()], ..open.clone() };
    world.laptop.update(|netmap| netmap.me.forwards = vec![open, guarded]).await;
    world
        .gw
        .update(|netmap| {
            netmap.me.forward_targets = [7, 8]
                .map(|id| ForwardTarget { id, protocol: ForwardProtocol::Udp, addr: service.addr(), listener: node(LAPTOP) })
                .to_vec();
        })
        .await;
    let active = |world: &World, id| world.laptop.access.status().forwards.iter().find(|forward| forward.id == id).unwrap().active.get();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    client.send_to(b"first", world.laptop.forward(7).await).await.unwrap();
    let (_, first_seen) = service.recv().await;
    assert_eq!(active(&world, 7), 1);

    // 静下来之后会话被回收，两头都是：监听节点上的会话，目标节点上的上下文。
    eventually("空闲的会话被回收", || active(&world, 7) == 0 && world.gw.exit.stats().active == 0).await;
    // 客户端再发报文：新的会话，在目标节点上是新的端口。
    client.send_to(b"second", world.laptop.forward(7).await).await.unwrap();
    let (data, second_seen) = service.recv().await;
    assert_eq!(data, b"second");
    assert_ne!(first_seen, second_seen);
    assert_eq!(world.gw.exit.stats().total, 2);

    // 来源（127.0.0.1）不在白名单里的转发：报文被丢掉，连会话都不建。
    client.send_to(b"not allowed", world.laptop.forward(8).await).await.unwrap();
    service.assert_untouched(QUIET).await;
    assert_eq!((active(&world, 8), world.gw.exit.stats().total), (0, 2));
}

#[tokio::test]
async fn a_udp_forward_on_the_virtual_ip_is_reached_through_the_node_port_without_an_os_port() {
    let world = World::new().await;
    let service = udp_echo_server().await;
    // nas 把自己回环上的服务发布在自己的虚拟 IP 的 5353 端口上：监听节点和目标节点是同一个。
    let forward = Forward {
        id: 9,
        protocol: ForwardProtocol::Udp,
        bind: "10.66.0.3:5353".parse().unwrap(),
        target: node(NAS),
        allow_from: vec![],
        class: None,
    };
    world
        .nas
        .update(|netmap| {
            netmap.me.forwards = vec![forward];
            netmap.me.forward_targets = vec![ForwardTarget { id: 9, protocol: ForwardProtocol::Udp, addr: service, listener: node(NAS) }];
        })
        .await;

    let client = udp_associate(world.socks().await, None).await;
    client.send("10.66.0.3", 5353, b"published on the virtual ip").await;
    assert_eq!(client.recv().await, ("10.66.0.3:5353".to_owned(), b"published on the virtual ip".to_vec()));
    assert_eq!(world.nas.system.udp_destinations(), [service], "去的是转发指向的地址，不是本机的非回环地址");
}

#[tokio::test]
async fn a_peer_may_hold_only_so_many_contexts_at_once() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), udp_echo_server().await);
    let socks = world.socks().await;

    // 每个关联到 gw 是一个上下文。64 个都通。
    let mut clients = Vec::new();
    for index in 0..64u32 {
        let client = udp_associate(socks, gw_user()).await;
        client.send(PUBLIC_IP, 443, &index.to_be_bytes()).await;
        assert_eq!(client.recv().await.1, index.to_be_bytes());
        clients.push(client);
    }
    assert_eq!(world.gw.exit.stats().active, 64);

    // 第 65 个被出口拒绝：它不会多占一个端口。
    let one_too_many = udp_associate(socks, gw_user()).await;
    one_too_many.send(PUBLIC_IP, 443, b"over the limit").await;
    let stats = settled(&world, "第 65 个上下文被拒绝", |stats| stats.total == 65 && stats.active == 64).await;
    assert_eq!(stats.refused, 1);
    one_too_many.assert_silent(QUIET).await;

    // 结束一个，就空出一个。入口在上一次被拒绝之后会歇几秒才重新去开，所以这里用一个新的关联。
    clients.pop();
    settled(&world, "结束的上下文让出了名额", |stats| stats.active == 63).await;
    let replacement = udp_associate(socks, gw_user()).await;
    replacement.send(PUBLIC_IP, 443, b"fits again").await;
    assert_eq!(replacement.recv().await.1, b"fits again");
}
