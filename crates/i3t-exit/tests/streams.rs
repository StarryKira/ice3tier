//! 一条流从头到尾：失败怎么报、半关闭、异常结束，以及 netmap 在运行时变化。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::io;

use common::{
    ECHO_LEN, GW, LAPTOP, NAS, PATIENCE, PUBLIC_IP, Target, World, assert_echo, assert_reset, closed_port, drain, echo_server, eventually,
    http_connect, peer_mut, proxy, socks5,
};
use i3t_access::ListenState;
use i3t_model::netmap::ListenerProtocol;
use i3t_proto::PublicKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

fn public(port: u16) -> String {
    format!("{PUBLIC_IP}:{port}")
}

i3t_lane::on_both_transports! {
    async fn internet_targets_report_a_failure_as_a_reset_and_mesh_targets_with_a_precise_code() {
        let world = World::new().await;
        let closed = closed_port().await;
        world.gw.system.redirect(&public(443), closed);
        world.nas.system.redirect("192.168.77.5:8080", closed);

        // 互联网目标：入口不等出口的应答就告诉客户端"连上了"，省一个全程往返；目标拒绝连接只能表现为随后的重置。
        let mut stream = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        assert_reset(&drain(&mut stream).await.1);
        let (status, mut stream) = http_connect(world.http().await, &public(443), None).await;
        assert_eq!(status, 200);
        assert_reset(&drain(&mut stream).await.1);

        // 组网目标：等出口的应答，客户端拿到的是"连接被拒绝"本身。
        assert_eq!(socks5(world.socks().await, None, "10.66.0.3", 8080).await.failed(), 5);
        assert_eq!(http_connect(world.http().await, "10.66.0.3:8080", None).await.0, 502);

        // 节点上那个端口不可达（不是拒绝）时也有区别。
        assert_eq!(socks5(world.socks().await, None, "10.66.0.3", 8081).await.failed(), 3);

        assert_eq!((world.gw.exit.stats().failed_by_target, world.nas.exit.stats().failed_by_target), (2, 3));
    }
}

i3t_lane::on_both_transports! {
    async fn the_first_bytes_of_the_client_reach_the_target_and_nothing_is_lost_or_reordered() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.gw.system.redirect(&public(443), target.addr());

        let mut client = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        // 一条分两次到的 TLS 记录，后面紧跟着更多数据。
        let mut hello = vec![0x16, 0x03, 0x01, 0x04, 0x00];
        hello.extend((0..=u8::MAX).cycle().take(1024));
        client.write_all(&hello[..300]).await.unwrap();
        client.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        client.write_all(&hello[300..]).await.unwrap();
        client.write_all(b"and then the rest of the handshake").await.unwrap();

        let mut server = target.accept().await;
        let mut received = vec![0; hello.len() + 34];
        timeout(PATIENCE, server.read_exact(&mut received)).await.unwrap().unwrap();
        assert_eq!(&received[..hello.len()], hello);
        assert_eq!(&received[hello.len()..], b"and then the rest of the handshake");
    }
}

i3t_lane::on_both_transports! {
    async fn a_server_that_speaks_first_is_heard_even_though_the_client_stays_silent() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.nas.system.redirect("192.168.77.5:22", target.addr());
        world.gw.system.redirect(&public(443), target.addr());

        // 组网目标（比如 SSH）和互联网目标都一样：客户端不说话，目标的问候照样到。
        for (auth, host, port) in [(None, "10.66.0.3", 22), (None, PUBLIC_IP, 443)] {
            let mut client = socks5(world.socks().await, auth, host, port).await.connected();
            let mut server = target.accept().await;
            server.write_all(b"SSH-2.0-test\r\n").await.unwrap();
            let mut banner = [0; 14];
            timeout(PATIENCE, client.read_exact(&mut banner)).await.unwrap().unwrap();
            assert_eq!(&banner, b"SSH-2.0-test\r\n");
            client.write_all(b"SSH-2.0-client\r\n").await.unwrap();
            let mut reply = [0; 16];
            timeout(PATIENCE, server.read_exact(&mut reply)).await.unwrap().unwrap();
            assert_eq!(&reply, b"SSH-2.0-client\r\n");
        }
    }
}

i3t_lane::on_both_transports! {
    async fn half_close_travels_end_to_end_in_both_directions() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.gw.system.redirect(&public(443), target.addr());

        // 客户端先发完：目标读到正常结束之后才回复，回复照样到客户端。
        let mut client = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        client.write_all(b"the whole request").await.unwrap();
        client.shutdown().await.unwrap();
        let mut server = target.accept().await;
        let (request, end) = drain(&mut server).await;
        assert_eq!(request, b"the whole request");
        end.expect("客户端半关闭，目标应该读到正常结束");
        server.write_all(b"the whole response").await.unwrap();
        drop(server);
        let (response, end) = drain(&mut client).await;
        assert_eq!(response, b"the whole response");
        end.expect("回复完整，客户端应该读到正常结束");

        // 目标先发完：客户端读到正常结束之后还能继续发。
        let (status, mut client) = http_connect(world.http().await, &public(443), None).await;
        assert_eq!(status, 200);
        let mut server = target.accept().await;
        server.write_all(b"greeting").await.unwrap();
        server.shutdown().await.unwrap();
        let (greeting, end) = drain(&mut client).await;
        assert_eq!(greeting, b"greeting");
        end.expect("目标半关闭，客户端应该读到正常结束");
        client.write_all(b"a late upload").await.unwrap();
        client.shutdown().await.unwrap();
        let (upload, end) = drain(&mut server).await;
        assert_eq!(upload, b"a late upload");
        end.unwrap();

        eventually("两条流都正常结束", || world.gw.exit.stats().active == 0).await;
        let stats = world.gw.exit.stats();
        assert_eq!((stats.total, stats.aborted, stats.bytes_to_target, stats.bytes_from_target), (2, 0, 17 + 13, 18 + 8));
    }
}

i3t_lane::on_both_transports! {
    async fn a_reset_by_the_target_reaches_the_client_as_a_reset_not_a_clean_end() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.gw.system.redirect(&public(443), target.addr());
        world.nas.system.redirect("192.168.77.5:8080", target.addr());

        // 经出口去互联网目标，和访问节点上的服务，都一样。
        for (auth, host, port) in [(None, PUBLIC_IP, 443), (None, "10.66.0.3", 8080)] {
            let mut client = socks5(world.socks().await, auth, host, port).await.connected();
            client.write_all(b"request").await.unwrap();
            let mut server = target.accept().await;
            server.read_exact(&mut [0; 7]).await.unwrap();
            // 回复写到一半，目标异常断开。
            server.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial bo").await.unwrap();
            server.set_zero_linger().unwrap();
            drop(server);

            let (_partial, end) = drain(&mut client).await;
            assert_reset(&end);
        }
        eventually("两个出口都记下了异常结束", || world.gw.exit.stats().aborted == 1 && world.nas.exit.stats().aborted == 1).await;
    }
}

i3t_lane::on_both_transports! {
    async fn a_reset_by_the_client_reaches_the_target_as_a_reset() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.gw.system.redirect(&public(443), target.addr());

        let mut client = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        client.write_all(b"half a requ").await.unwrap();
        let mut server = target.accept().await;
        server.read_exact(&mut [0; 11]).await.unwrap();
        client.set_zero_linger().unwrap();
        drop(client);

        let (_nothing, end) = drain(&mut server).await;
        assert_reset(&end);
        eventually("出口记下了异常结束", || world.gw.exit.stats().aborted == 1).await;
    }
}

#[tokio::test]
async fn losing_the_connection_between_the_nodes_resets_both_ends() {
    let world = World::new().await;
    let target = Target::bind("127.0.0.1").await;
    world.gw.system.redirect(&public(443), target.addr());

    let mut client = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
    client.write_all(b"request").await.unwrap();
    let mut server = target.accept().await;
    server.read_exact(&mut [0; 7]).await.unwrap();

    world.mesh.partition(common::node(LAPTOP), common::node(GW));
    assert_reset(&drain(&mut client).await.1);
    assert_reset(&drain(&mut server).await.1);

    // 到不了出口时：组网目标得到明确的失败，互联网目标（已经说了"连上了"）被重置。
    world.mesh.partition(common::node(LAPTOP), common::node(NAS));
    assert_eq!(socks5(world.socks().await, None, "10.66.0.3", 80).await.failed(), 1);
    assert_eq!(http_connect(world.http().await, "10.66.0.3:80", None).await.0, 503);
    let mut client = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
    assert_reset(&drain(&mut client).await.1);
    target.assert_untouched(std::time::Duration::from_millis(50)).await;
}

i3t_lane::on_both_transports! {
    async fn proxies_follow_the_netmap_without_disturbing_established_connections() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        let socks_before = world.socks().await;
        let mut through_socks = socks5(socks_before, None, PUBLIC_IP, 443).await.connected();

        // 加一个代理（经 nas，nas 不是出口）：它起来了，原来的没有被动过。
        let status = world.laptop.update(|netmap| netmap.me.proxies.push(proxy(5, ListenerProtocol::Socks5, "127.0.0.1:0", NAS))).await;
        assert_eq!(status.proxies.len(), 5);
        let extra = world.laptop.proxy(5).await;
        assert_eq!(world.socks().await, socks_before, "没有变的代理不重新绑定");
        assert_eq!(socks5(extra, None, PUBLIC_IP, 443).await.failed(), 2, "nas 不是出口");
        // nas 的假网卡地址是 192.168.77.5：组网目标不经出口，经这个代理照样能去。
        world.nas.system.redirect("192.168.77.5:8080", echo_server().await);
        let mut through_extra = socks5(extra, None, "10.66.0.3", 8080).await.connected();
        assert_echo(&mut through_extra).await;
        assert_echo(&mut through_socks).await;

        // 只改设置（出口、协议）、不改地址：不重新绑定，新的连接按新的设置。
        world
            .laptop
            .update(|netmap| {
                netmap.me.proxies[4].exit = common::node(GW);
                netmap.me.proxies[4].protocol = ListenerProtocol::Mixed;
            })
            .await;
        assert_eq!(world.laptop.proxy(5).await, extra);
        let mut by_new_exit = socks5(extra, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut by_new_exit).await;

        // 删掉它：端口不再接受连接，已经建立的连接照常工作。
        let status = world.laptop.update(|netmap| netmap.me.proxies.retain(|proxy| proxy.id != 5)).await;
        assert_eq!(status.proxies.len(), 4);
        let refused = TcpStream::connect(extra).await.expect_err("监听已经关了");
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
        assert_echo(&mut through_extra).await;
        assert_echo(&mut by_new_exit).await;
        assert_echo(&mut through_socks).await;
        assert_eq!(world.socks().await, socks_before);

        // 改地址：旧的关掉，新的起来。换的是端口：macOS 的回环网卡上只有 127.0.0.1，换不了 IP。
        let moved = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        world.laptop.update(|netmap| netmap.me.proxies[0].bind = moved).await;
        let socks_after = world.socks().await;
        assert_eq!(socks_after, moved);
        assert!(TcpStream::connect(socks_before).await.is_err());
        let mut through_moved = socks5(socks_after, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut through_moved).await;
        assert_echo(&mut through_socks).await;
    }
}

i3t_lane::on_both_transports! {
    async fn a_busy_port_is_a_reported_state_and_the_proxy_comes_up_once_the_port_is_free() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        let squatter = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let taken = squatter.local_addr().unwrap();

        let status = world
            .laptop
            .update(|netmap| {
                netmap.me.proxies.push(proxy(5, ListenerProtocol::Socks5, &taken.to_string(), GW));
                netmap.me.forwards.push(i3t_model::netmap::Forward {
                    id: 5,
                    protocol: i3t_model::netmap::ForwardProtocol::Tcp,
                    bind: taken,
                    target: common::node(NAS),
                    allow_from: Vec::new(),
                    class: None,
                });
            })
            .await;
        let busy = status.proxies.iter().find(|proxy| proxy.id == 5).unwrap();
        assert!(matches!(&busy.state, ListenState::PortInUse { reason } if !reason.is_empty()), "{:?}", busy.state);
        assert!(matches!(status.forwards[0].state, ListenState::PortInUse { .. }), "{:?}", status.forwards[0].state);

        // 别的代理不受影响。
        let mut stream = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut stream).await;

        // 端口让出来之后，下一次调整时代理就起来了（端口转发要的是同一个端口，这回轮到它失败）。
        drop(squatter);
        let status = world.laptop.update(|_| {}).await;
        let busy = status.proxies.iter().find(|proxy| proxy.id == 5).unwrap();
        assert_eq!(busy.state, ListenState::Listening { local: taken });
        assert!(matches!(status.forwards[0].state, ListenState::PortInUse { .. }));
        let mut stream = socks5(taken, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut stream).await;
    }
}

i3t_lane::on_both_transports! {
    async fn revoking_a_peer_resets_its_established_flows_and_refuses_new_ones() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);
        // nas 上也开一个入口，它的流不应该受牵连。
        world.nas.update(|netmap| netmap.me.proxies.push(proxy(1, ListenerProtocol::Socks5, "127.0.0.1:0", GW))).await;

        let mut from_laptop = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        let mut from_nas = socks5(world.nas.proxy(1).await, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut from_laptop).await;
        assert_echo(&mut from_nas).await;
        assert_eq!(world.gw.exit.stats().active, 2);

        // 一次和它无关的 netmap 变化不影响任何流。
        world.gw.update(|netmap| peer_mut(netmap, NAS).keep_alive = true).await;
        assert_echo(&mut from_laptop).await;

        // 访问规则收回了 laptop 的许可：它已经建立的流被重置，而不只是拒绝新的。
        world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = false).await;
        assert_reset(&drain(&mut from_laptop).await.1);
        assert_echo(&mut from_nas).await;

        let mut refused = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        assert_reset(&drain(&mut refused).await.1);
        eventually("出口记下了这两件事", || world.gw.exit.stats().aborted == 1 && world.gw.exit.stats().refused == 1).await;
        assert_eq!(world.gw.exit.stats().active, 1);

        // 许可恢复之后又可以了。
        world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = true).await;
        let mut again = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut again).await;
        assert_echo(&mut from_nas).await;
    }
}

i3t_lane::on_both_transports! {
    async fn rotating_the_initiators_key_resets_both_tcp_ends() {
        let world = World::new().await;
        let target = Target::bind("127.0.0.1").await;
        world.gw.system.redirect(&public(443), target.addr());
        let mut client = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        // 首批字节是一条 ClientHello 记录，让 ice3proxy 测试走 passthrough。
        let hello = *b"\x16\x03\x01\x00\x04\x01\x00\x00\x00";
        client.write_all(&hello).await.unwrap();
        let mut server = target.accept().await;
        let mut received = [0; 9];
        timeout(PATIENCE, server.read_exact(&mut received)).await.unwrap().unwrap();
        assert_eq!(received, hello);
        server.write_all(b"reply").await.unwrap();
        let mut reply = [0; 5];
        timeout(PATIENCE, client.read_exact(&mut reply)).await.unwrap().unwrap();
        assert_eq!(&reply, b"reply");
        if i3t_lane::testing::transport() == i3t_lane::testing::Transport::Lanes {
            assert!(world.gw.lanes.status().total.spliced_bytes > 0, "出口应当走原始 TCP 搬运");
        }
        assert_eq!(world.gw.exit.stats().active, 1);

        // 只改出口用于检查放行凭据的 netmap。测试 transport 的节点表保持原样。
        world.gw.update(|netmap| peer_mut(netmap, LAPTOP).key = PublicKey::from_bytes([0x42; 32])).await;
        let ((_, client_end), (_, server_end)) = tokio::join!(drain(&mut client), drain(&mut server));
        assert_reset(&client_end);
        assert_reset(&server_end);
        eventually("出口记录了主动吊销", || world.gw.exit.stats().aborted == 1).await;
    }
}

i3t_lane::on_both_transports! {
    async fn the_exit_counts_what_it_carried() {
        let world = World::new().await;
        world.gw.system.redirect(&public(443), echo_server().await);

        let mut held = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
        assert_echo(&mut held).await;
        for _ in 0..3 {
            let mut stream = socks5(world.socks().await, None, PUBLIC_IP, 443).await.connected();
            assert_echo(&mut stream).await;
            assert_echo(&mut stream).await;
        }
        eventually("三条流结束，一条还开着", || world.gw.exit.stats().active == 1).await;
        let stats = world.gw.exit.stats();
        // 字节数在流结束时才记：还开着的那一条不在里面。
        assert_eq!((stats.total, stats.bytes_to_target, stats.bytes_from_target), (4, 6 * ECHO_LEN, 6 * ECHO_LEN));
        assert_eq!((stats.aborted, stats.refused, stats.failed_by_target, stats.failed_by_exit), (0, 0, 0, 0));
    }
}
