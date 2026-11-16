//! 路由层的集成测试：几个节点在同一个进程里，都绑在回环地址上，上面跑真的 QUIC 流。
//! 每个测试说的是上层看得到的行为：流经两个 relay 通不通、矩阵到没到节点手上、relay 下线时流断不断、
//! 局域网里的对端找不找得到、路由器上映射出来的地址有没有成为候选端点。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod mesh;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use i3t_model::control::Route;
use i3t_model::netmap::RoutingMode;
use i3t_path::{DirectKind, RelayLeg};
use i3t_proto::backbone::RelayState;
use i3t_relay::DropReason;
use i3t_underlay::lan::{GROUP_V4, Interfaces, LanConfig};
use i3t_underlay::portmap::{GatewaySource, PortMapConfig};
use mesh::{Node, Peer, SOON, Transfer, View, drain_reasons, echo_once};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

fn megabyte() -> Vec<u8> {
    (0..1024 * 1024u32).map(|index| u8::try_from(index % 251).unwrap()).collect()
}

async fn eventually(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < deadline, "等了 {} 秒，{what} 还没有发生", limit.as_secs());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A 只够得着 R1，D 只够得着 R2，R1 和 R2 之间有骨干：A 到 D 唯一的路是 A → R1 → R2 → D。
/// 两个节点都用延迟优先模式；`a_max_hops` 是 A 的 netmap 里网络的最大中转级数。
async fn two_relay_chain(a: &Node, d: &Node, r1: &Node, r2: &Node, a_max_hops: u8) {
    a.block(&[d, r2]);
    d.block(&[a, r1]);
    for relay in [r1, r2] {
        relay.apply(&View { relays: &[r1, r2], relay_clients: Some(&[a, d]), ..View::default() });
        // netmap 是异步生效的；等 relay 启用了再让节点来连，免得头几个握手包落在"还没启用"上。
        relay.wait_relay_enabled().await;
    }
    let chain = View { relays: &[r1, r2], routing: Some("lowest_cost"), udp_only: true, ..View::default() };
    a.apply(&View { peers: &[Peer::via(d, &[(r2, 5)])], max_relay_hops: Some(a_max_hops), ..chain });
    d.apply(&View { peers: &[Peer::via(a, &[(r1, 5)])], ..chain });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_is_carried_over_a_two_relay_route() {
    let (a, d, r1, r2) = (Node::start(1), Node::start(2), Node::start(8), Node::start(9));
    two_relay_chain(&a, &d, &r1, &r2, 2).await;
    d.spawn_echo();

    echo_once(&a, &d, &megabyte()).await;
    assert_eq!(a.route_to(&d), Route::Relay { relays: vec![r1.id, r2.id] });
    assert_eq!(d.route_to(&a), Route::Relay { relays: vec![r2.id, r1.id] }, "每个节点为自己发出的包选路：回程从 R2 进、R1 出");
    for relay in [&r1, &r2] {
        let counters = relay.relay().counters();
        assert!(counters.forwarded_bytes > 2 * 1024 * 1024, "一兆字节来回各一次都经过两个 relay：{counters:?}");
        // 启动时另一个 relay 可能在这个 relay 的 netmap 生效之前就来握手了，那一个握手包算在"没启用"上；除此之外不该丢任何东西。
        assert_eq!(counters.dropped_total(), counters.dropped(DropReason::Disabled), "{counters:?}");
    }

    // 界面能看到的：模式、现在的路、候选和每一段的测量值、为什么选它。
    let report = a.paths.path_report(d.id).expect("D 是 A 的对端");
    assert_eq!((report.mode, &report.route), (RoutingMode::LowestCost, &Route::Relay { relays: vec![r1.id, r2.id] }));
    assert!(report.reason.contains("延迟优先") && report.reason.contains(&format!("经 relay {} → {}", r1.id, r2.id)), "{}", report.reason);
    let [only] = &report.candidates[..] else { panic!("只有一条走得通的候选：{:?}", report.candidates) };
    assert!(only.current && only.cost_ms > 0.0);
    assert_eq!((report.relay_leg, only.relay_leg), (Some(RelayLeg::Udp), Some(RelayLeg::Udp)), "到第一个 relay 走的是 UDP");
    let hops: Vec<_> = only.legs.iter().map(|leg| (leg.from, leg.to)).collect();
    assert_eq!(hops, [(a.id, r1.id), (r1.id, r2.id), (r2.id, d.id)]);
    assert!(a.paths.routes().iter().any(|entry| entry.peer == d.id && entry.rtt_ms.is_some()));

    // 一条已经在传的流照样经过这条路，一块不丢。
    let transfer = Transfer::start(&a, &d, 200).await;
    transfer.expect_progress(300).await;
    transfer.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_does_not_build_routes_longer_than_the_network_allows() {
    let (a, d, r1, r2) = (Node::start(1), Node::start(2), Node::start(8), Node::start(9));
    // A 的 netmap 里最大中转级数是 1：两级的路径不在候选里，哪怕矩阵已经说明走得通。
    two_relay_chain(&a, &d, &r1, &r2, 1).await;
    a.paths.activate(d.id);
    let knows_the_backbone = || {
        a.paths.backbone().iter().any(|row| row.relay == r1.id && row.links.iter().any(|link| link.to == r2.id && link.rtt_ms.is_some()))
    };
    eventually("R1 的矩阵行到达 A", 3 * SOON, knows_the_backbone).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(a.route_to(&d), Route::None);
    assert!(a.paths.path_report(d.id).unwrap().candidates.is_empty());

    // 网络把上限放到 2 之后，路就有了。
    a.apply(&View {
        peers: &[Peer::via(&d, &[(&r2, 5)])],
        relays: &[&r1, &r2],
        routing: Some("lowest_cost"),
        max_relay_hops: Some(2),
        udp_only: true,
        ..View::default()
    });
    a.wait_route(&d, "经两个 relay", |route| *route == Route::Relay { relays: vec![r1.id, r2.id] }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_multi_hop_route_that_silently_stops_delivering_is_abandoned_and_the_stream_survives() {
    let (a, d) = (Node::start(1), Node::start(2));
    let (r1, r2, r3) = (Node::start(7), Node::start(8), Node::start(9));
    a.block(&[&d, &r2]);
    for relay in [&r1, &r2, &r3] {
        relay.apply(&View { relays: &[&r1, &r2, &r3], relay_clients: Some(&[&a, &d]), ..View::default() });
        relay.wait_relay_enabled().await;
    }
    // A 到 D：从 R1 或者 R3 进、从 R2 出的两级路径都很便宜（D 到 R2 是 5ms）；D 也挂在 R3 上，但它到 R3 要 500ms，只是个备用。
    let lowest_cost = View { relays: &[&r1, &r2, &r3], routing: Some("lowest_cost"), udp_only: true, ..View::default() };
    a.apply(&View { peers: &[Peer::via(&d, &[(&r2, 5), (&r3, 500)])], ..lowest_cost });
    d.apply(&View { peers: &[Peer::via(&a, &[(&r1, 5), (&r3, 500)])], ..lowest_cost });
    d.spawn_echo();
    let transfer = Transfer::start(&a, &d, 200).await;
    let out_through_r2 = |route: &Route| matches!(route, Route::Relay { relays } if relays.len() == 2 && relays[1] == r2.id);
    a.wait_route(&d, "两级、从 R2 出", out_through_r2).await;
    transfer.expect_progress(200).await;
    let mut changes = a.paths.subscribe();

    // R2 发给 D 的包从此到不了（D 那一头的网络出了问题）。路径上的 relay 都不知道：它们照常往下转，
    // 也没有谁能把"送不到"告诉 A。D 发给 A 的包照样到得了，所以光看"有没有收到对端的包"也看不出来。
    // A 靠经这条路径发的端到端探测发现，把从 R2 出的路径一条条放弃，最后换到备用的 R3。
    let lost_from = r2.addr();
    d.port
        .set_filter(Some(Arc::new(move |direction, peer, _: &[u8]| !(direction == i3t_underlay::Direction::Inbound && peer == lost_from))));
    let noticed = Instant::now();
    // 还通的路是从 R3 出的那些（直接经 R3，或者先到 R1 再到 R3，代价差不多）。
    a.wait_route(&d, "从 R3 出", |route| matches!(route, Route::Relay { relays } if relays.last() == Some(&r3.id))).await;
    assert!(noticed.elapsed() < 2 * SOON, "用了 {:?} 才换到还通的那条路", noticed.elapsed());
    let reasons = drain_reasons(&mut changes, &d);
    assert!(reasons.iter().any(|reason| reason.contains("一直没有回音")), "{reasons:?}");
    transfer.expect_progress(400).await;
    transfer.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_backbone_matrix_reaches_the_nodes_through_the_relay_they_are_attached_to() {
    let (a, r1, r2, r3) = (Node::start(1), Node::start(7), Node::start(8), Node::start(9));
    // R3 和 R1 之间不通；A 只挂得上 R1。
    r1.block(&[&r3]);
    a.block(&[&r2, &r3]);
    for relay in [&r1, &r2, &r3] {
        relay.apply(&View { relays: &[&r1, &r2, &r3], relay_clients: Some(&[&a]), udp_only: true, ..View::default() });
        relay.wait_relay_enabled().await;
    }
    a.apply(&View { relays: &[&r1, &r2, &r3], udp_only: true, ..View::default() });

    // A 只和 R1 有会话，却拿到了三个 relay 各自测的那一行：R2、R3 的行是经骨干传到 R1、再由 R1 发给 A 的。
    let link = |from: &Node, to: &Node| {
        let rows = a.paths.backbone();
        let row = rows.iter().find(|row| row.relay == from.id)?;
        row.links.iter().find(|link| link.to == to.id).copied()
    };
    let complete = || {
        [(&r1, &r2), (&r2, &r1), (&r2, &r3), (&r3, &r2)].iter().all(|(from, to)| link(from, to).is_some_and(|link| link.rtt_ms.is_some()))
            && link(&r1, &r3).is_some()
            && link(&r3, &r1).is_some()
    };
    eventually("三个 relay 的矩阵行都到达 A", 3 * SOON, complete).await;
    assert_eq!(link(&r1, &r3).unwrap().rtt_ms, None, "连不上的一对在矩阵里记为不可达");
    assert_eq!(link(&r3, &r1).unwrap().rtt_ms, None);
    let rows = a.paths.backbone();
    assert_eq!(rows.iter().map(|row| row.relay).collect::<Vec<_>>(), [r1.id, r2.id, r3.id]);
    assert!(rows.iter().all(|row| row.state == RelayState::Serving && row.age_ms < 15_000), "{rows:?}");
    assert!(a.paths.attached().iter().any(|attachment| attachment.relay == r1.id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_that_announces_shutdown_is_left_at_once_and_the_stream_survives() {
    let (a, d) = (Node::start(1), Node::start(2));
    let (mut r1, r2, r3) = (Node::start(7), Node::start(8), Node::start(9));
    a.block(&[&d]);
    for relay in [&r1, &r2, &r3] {
        relay.apply(&View { relays: &[&r1, &r2, &r3], relay_clients: Some(&[&a, &d]), ..View::default() });
        relay.wait_relay_enabled().await;
    }
    // 默认模式：经对端挂靠的 relay 里排第一的那个，两个方向都是 R1。
    a.apply(&View { peers: &[Peer::via(&d, &[(&r1, 1), (&r2, 50)])], relays: &[&r1, &r2, &r3], ..View::default() });
    d.apply(&View { peers: &[Peer::via(&a, &[(&r1, 1), (&r2, 50)])], relays: &[&r1, &r2, &r3], ..View::default() });
    d.spawn_echo();
    let transfer = Transfer::start(&a, &d, 200).await;
    transfer.expect_progress(100).await;
    assert_eq!(a.route_to(&d), Route::Relay { relays: vec![r1.id] });
    // 三个 relay 的延迟都量出来之后再动手，这样挂靠的去向是确定的。
    eventually("A 和 D 各挂靠两个 relay", SOON, || a.paths.attached().len() == 2 && d.paths.attached().len() == 2).await;
    let mut changes = a.paths.subscribe();

    // R1 要下线了。它照常转发，但节点收到通知之后不等保活超时（那要好几秒），立刻换到别的 relay。
    let announced = Instant::now();
    r1.relay().announce_shutdown();
    let via_r2 = Route::Relay { relays: vec![r2.id] };
    a.wait_route(&d, "经 R2", |route| *route == via_r2).await;
    d.wait_route(&a, "经 R2", |route| *route == via_r2).await;
    assert!(announced.elapsed() < Duration::from_secs(2), "换路用了 {:?}，应该是收到通知就换", announced.elapsed());
    let reasons = drain_reasons(&mut changes, &d);
    assert!(reasons.iter().any(|reason| reason.contains("relay 通知即将下线")), "{reasons:?}");
    // 挂靠也从 R1 挪走了。
    eventually("A 不再挂靠 R1", SOON, || a.paths.attached().iter().all(|attachment| attachment.relay != r1.id)).await;
    assert!(a.paths.backbone().iter().any(|row| row.relay == r1.id && row.state == RelayState::Draining));

    // 然后 R1 真的退出。流一块没丢。
    r1.port.set_filter(Some(Arc::new(|_, _, _: &[u8]| false)));
    drop(r1.relay.take());
    transfer.expect_progress(600).await;
    transfer.finish().await;
    assert_eq!(a.route_to(&d), via_r2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_overloaded_relay_is_left_at_once_and_not_rejoined_the_moment_it_recovers() {
    let (a, d, r1, r2) = (Node::start(1), Node::start(2), Node::start(8), Node::start(9));
    a.block(&[&d]);
    // R1 的带宽上限是 1 Mbit/s，R2 不限。
    r1.apply(&View { relays: &[&r1, &r2], relay_clients: Some(&[&a, &d]), bandwidth_mbps: Some(1), ..View::default() });
    r2.apply(&View { relays: &[&r1, &r2], relay_clients: Some(&[&a, &d]), ..View::default() });
    for relay in [&r1, &r2] {
        relay.wait_relay_enabled().await;
    }
    a.apply(&View { peers: &[Peer::via(&d, &[(&r1, 1), (&r2, 50)])], relays: &[&r1, &r2], ..View::default() });
    d.apply(&View { peers: &[Peer::via(&a, &[(&r1, 1), (&r2, 50)])], relays: &[&r1, &r2], ..View::default() });
    a.paths.activate(d.id);
    let via = |relay: &Node| Route::Relay { relays: vec![relay.id] };
    a.wait_route(&d, "经 R1", |route| *route == via(&r1)).await;
    let mut changes = a.paths.subscribe();

    // 往 D 灌远超过 R1 上限的包（不经过连接层，所以没有拥塞控制替它收着）。R1 丢掉超出的部分，同时发现自己过载了，
    // 通知挂靠的节点；A 不用等延迟涨上去、也不用等丢包被测出来，收到通知就改走 R2。
    let packet = [0x40u8; 1200];
    let flood = async {
        loop {
            for _ in 0..40 {
                let _sent = a.paths.try_send(d.id, &i3t_path::Packets { contents: &packet, segment_size: None, ecn: None });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        () = flood => {}
        _ = a.wait_route(&d, "经 R2", |route| *route == via(&r2)) => {}
    }
    let reasons = drain_reasons(&mut changes, &d);
    assert!(reasons.iter().any(|reason| reason.contains("relay 通知过载")), "{reasons:?}");
    assert!(r1.relay().counters().dropped(DropReason::RateLimited) > 0);

    // 没有流量之后 R1 很快就说自己恢复了。A 不马上回去：每个节点各等一段随机的时间，免得大家一起回去又把它压垮。
    let recovered = || a.paths.backbone().iter().any(|row| row.relay == r1.id && row.state == RelayState::Serving);
    eventually("R1 的恢复通知到达 A", 3 * SOON, recovered).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(a.route_to(&d), via(&r2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv6_addresses_are_validated_with_the_same_probes_and_carry_a_stream() {
    let (Some(a), Some(b)) = (Node::start_on(1, "::1"), Node::start_on(2, "::1")) else {
        eprintln!("这台机器没有 IPv6 的回环地址，跳过");
        return;
    };
    assert!(a.addr().is_ipv6() && b.addr().is_ipv6());
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    b.spawn_echo();
    echo_once(&a, &b, &megabyte()).await;
    assert_eq!(a.route_to(&b), Route::Direct { addr: b.addr() });
    assert_eq!(a.paths.path_report(b.id).unwrap().direct, Some(DirectKind::Ipv6));
    assert_eq!(a.paths.endpoints(), [a.addr()]);
}

/// 一个转发 TCP 的中间人，放在节点和 relay 之间。relay 发往节点的方向上每隔 `every` 停顿一次、每次 `stall`：
/// 像一条丢包的 TCP，包不会丢，只会在重传时一起晚到。返回节点该去连的地址。
async fn stalling_tcp(relay: SocketAddr, every: Duration, stall: Duration) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((node, _)) = listener.accept().await {
            let Ok(upstream) = TcpStream::connect(relay).await else { continue };
            let ((mut node_read, mut node_write), (mut relay_read, mut relay_write)) = (node.into_split(), upstream.into_split());
            tokio::spawn(async move { tokio::io::copy(&mut node_read, &mut relay_write).await });
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let mut next_stall = Instant::now() + every;
                while let Ok(len @ 1..) = relay_read.read(&mut buf).await {
                    if Instant::now() >= next_stall {
                        tokio::time::sleep(stall).await;
                        next_stall = Instant::now() + every;
                    }
                    if node_write.write_all(&buf[..len]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tcp_only_node_weighs_its_own_tcp_legs_and_moves_off_the_one_that_stalls() {
    let (a, d, r1, r2) = (Node::start(1), Node::start(2), Node::start(8), Node::start(9));
    // A 完全用不了 UDP：到两个 relay 都只能走 TCP。到 R1 的那条顺畅；到 R2 的那条每 400ms 停顿 250ms。
    a.block(&[&d, &r1, &r2]);
    for relay in [&r1, &r2] {
        relay.apply(&View { relays: &[&r1, &r2], relay_clients: Some(&[&a, &d]), ..View::default() });
        relay.wait_relay_enabled().await;
    }
    let stalling = stalling_tcp(r2.addr(), Duration::from_millis(400), Duration::from_millis(250)).await;
    let tcp_via = [(&r2, stalling)];
    let lowest_cost =
        View { relays: &[&r1, &r2], routing: Some("lowest_cost"), max_relay_hops: Some(1), tcp_via: &tcp_via, ..View::default() };
    // 起先 D 只挂在 R2 上：A 只能经那条停顿的 TCP。
    a.apply(&View { peers: &[Peer::via(&d, &[(&r2, 5)])], ..lowest_cost });
    d.apply(&View { peers: &[Peer::via(&a, &[(&r1, 5)])], relays: &[&r1, &r2], ..View::default() });
    d.spawn_echo();
    let transfer = Transfer::start(&a, &d, 100).await;
    let (via_r1, via_r2) = (Route::Relay { relays: vec![r1.id] }, Route::Relay { relays: vec![r2.id] });
    assert_eq!(a.route_to(&d), via_r2);
    let mut changes = a.paths.subscribe();

    // D 也挂上了 R1，离两个 relay 一样近：A 往哪边走只取决于它自己那一段。顺畅的 TCP 在代价上明显便宜，
    // 连续三个评估窗口之后 A 换过去。只比平滑延迟的话两条差不多（都是回环），换不过去。
    a.apply(&View { peers: &[Peer::via(&d, &[(&r1, 5), (&r2, 5)])], ..lowest_cost });
    a.wait_route(&d, "经 R1（顺畅的那条 TCP）", |route| *route == via_r1).await;
    let reasons = drain_reasons(&mut changes, &d);
    assert!(reasons.iter().any(|reason| reason.contains("连续 3 个评估窗口")), "是按代价换的，不是 R2 断了：{reasons:?}");
    let report = a.paths.path_report(d.id).unwrap();
    let leg = |route: &Route| {
        let candidate = report.candidates.iter().find(|candidate| candidate.route == *route);
        candidate.unwrap_or_else(|| panic!("候选里应该有 {route:?}：{report:?}")).clone()
    };
    let (smooth, stalls) = (leg(&via_r1), leg(&via_r2));
    assert_eq!((report.relay_leg, smooth.relay_leg, stalls.relay_leg), (Some(RelayLeg::Tcp), Some(RelayLeg::Tcp), Some(RelayLeg::Tcp)));
    assert!(stalls.cost_ms > smooth.cost_ms + 20.0, "停顿的那条 TCP 应该贵得多：{report:?}");
    assert!(stalls.legs[0].jitter_ms > smooth.legs[0].jitter_ms + 5.0, "停顿测出来是抖动：{report:?}");

    // 之后一直留在 R1 上，流一块不丢。
    for _ in 0..3 {
        transfer.expect_progress(50).await;
        assert_eq!(a.route_to(&d), via_r1);
    }
    transfer.finish().await;
}

/// 这台机器的回环上组播通不通。不通的话（有的系统不让）局域网发现没法在测试里做。
async fn loopback_multicast_works(group: SocketAddrV4) -> bool {
    let probe = async {
        let listener = UdpSocket::bind(("0.0.0.0", group.port())).await.ok()?;
        listener.join_multicast_v4(*group.ip(), Ipv4Addr::LOCALHOST).ok()?;
        let sender = UdpSocket::bind("127.0.0.1:0").await.ok()?;
        let raw = socket2::SockRef::from(&sender);
        raw.set_multicast_if_v4(&Ipv4Addr::LOCALHOST).ok()?;
        raw.set_multicast_loop_v4(true).ok()?;
        sender.send_to(b"probe", group).await.ok()?;
        let mut buf = [0u8; 16];
        tokio::time::timeout(Duration::from_millis(500), listener.recv_from(&mut buf)).await.ok()?.ok()
    };
    let works = probe.await.is_some();
    if !works {
        eprintln!("这台机器的回环上组播不通，跳过局域网发现的测试");
    }
    works
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_on_the_same_segment_find_each_other_through_beacons_and_go_direct() {
    // 用系统现挑的空闲端口当组播端口，免得和并行的测试、和本机上真的节点撞上。
    let free = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let group = SocketAddrV4::new(GROUP_V4, free);
    if !loopback_multicast_works(group).await {
        return;
    }
    let (a, b, c) = (Node::start(1), Node::start(2), Node::start(3));
    // 谁都不知道谁的地址，也没有 relay 和信令：只有信标能让它们找到对方。
    a.apply(&View { peers: &[Peer::unknown(&b), Peer::unknown(&c)], ..View::default() });
    b.apply(&View { peers: &[Peer::unknown(&a)], ..View::default() });
    c.apply(&View { peers: &[Peer::unknown(&a)], ..View::default() });
    b.spawn_echo();
    let config = LanConfig {
        v4: group,
        v6: None,
        interfaces: Interfaces::Only(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]),
        interval: Duration::from_millis(100),
    };
    a.paths.start_lan_discovery(config.clone()).unwrap();
    b.paths.start_lan_discovery(config).unwrap();

    echo_once(&a, &b, b"found on the lan").await;
    assert_eq!(a.route_to(&b), Route::Direct { addr: b.addr() });
    assert_eq!(b.route_to(&a), Route::Direct { addr: a.addr() });
    let report = a.paths.path_report(b.id).unwrap();
    assert_eq!(report.direct, Some(DirectKind::Lan), "信标里看到的地址验证通过之后是局域网直连：{report:?}");
    // C 没有开局域网发现：没有人看得到它，它也看不到别人。
    a.paths.activate(c.id);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(a.route_to(&c), Route::None);
}

/// 一个只会 NAT-PMP 的假网关。公网地址是 `EXTERNAL`，映射出来的外部端口和内部端口同号。记下收到的映射请求。
struct FakeGateway {
    addr: SocketAddr,
    /// 每个映射请求的（操作码，内部端口，有效期秒数）。
    maps: Arc<Mutex<Vec<(u8, u16, u32)>>>,
}

const EXTERNAL: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 9);

impl FakeGateway {
    async fn start() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let maps = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&maps);
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            let epoch = 7u32.to_be_bytes();
            loop {
                let Ok((len, from)) = socket.recv_from(&mut buf).await else { return };
                // 偏移量照着 RFC 6886 写。
                let reply = match buf[..len] {
                    [0, 0] => [&[0, 128, 0, 0][..], &epoch, &EXTERNAL.octets()].concat(),
                    [0, opcode @ (1 | 2), _, _, i0, i1, _, _, l0, l1, l2, l3] => {
                        seen.lock().unwrap().push((opcode, u16::from_be_bytes([i0, i1]), u32::from_be_bytes([l0, l1, l2, l3])));
                        let external = if [l0, l1, l2, l3] == [0; 4] { [0, 0] } else { [i0, i1] };
                        [&[0, 128 + opcode, 0, 0][..], &epoch, &[i0, i1], &external, &[l0, l1, l2, l3]].concat()
                    }
                    // 别的（PCP 的请求）：按 NAT-PMP 的格式回"不支持的版本"。
                    _ => [&[0, 128, 0, 1][..], &epoch].concat(),
                };
                socket.send_to(&reply, from).await.unwrap();
            }
        });
        Self { addr, maps }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_only_node_publishes_no_endpoints_but_still_knows_its_own_public_addresses() {
    let (a, r) = (Node::start(1), Node::start(9));
    r.apply(&View { relay_clients: Some(&[&a]), ..View::default() });
    r.wait_relay_enabled().await;
    a.apply(&View { relays: &[&r], reach: Some("relay_only"), ..View::default() });
    // relay 看到的就是 A 的回环地址和端口。
    eventually("relay 看到的地址出现在本节点的公网地址里", SOON, || a.paths.public_addresses() == [a.addr()]).await;
    assert!(a.paths.endpoints().is_empty(), "只经 relay 的节点不向对端公布地址");

    // 可以直连的节点两样都有。
    let b = Node::start(2);
    r.apply(&View { relay_clients: Some(&[&a, &b]), ..View::default() });
    b.apply(&View { relays: &[&r], ..View::default() });
    eventually("可以直连的节点也知道", SOON, || b.paths.public_addresses() == [b.addr()]).await;
    assert_eq!(b.paths.endpoints(), [b.addr()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_port_mapped_on_the_gateway_becomes_a_candidate_endpoint_and_is_removed_on_shutdown() {
    let gateway = FakeGateway::start().await;
    let a = Node::start(1);
    let port = a.addr().port();
    assert_eq!(a.paths.endpoints(), [a.addr()], "还没有映射时只有本机地址");
    let source = GatewaySource::Fixed { local: IpAddr::V4(Ipv4Addr::LOCALHOST), natpmp_pcp: Some(gateway.addr), ssdp: None };
    a.paths.start_port_mapping(PortMapConfig { port, gateway: source });

    let mapped = SocketAddr::new(IpAddr::V4(EXTERNAL), port);
    eventually("映射出来的地址出现在候选端点里", SOON, || a.paths.endpoints().contains(&mapped)).await;
    eventually("也在本节点的公网地址里", SOON, || a.paths.public_addresses().contains(&mapped)).await;
    assert_eq!(gateway.maps.lock().unwrap().first(), Some(&(1, port, 7200)), "映射的是节点那个唯一的 UDP 端口");

    // 节点停下之前把映射删掉（有效期 0）。
    a.paths.shutdown().await;
    assert!(gateway.maps.lock().unwrap().contains(&(1, port, 0)), "{:?}", gateway.maps.lock().unwrap());
}
