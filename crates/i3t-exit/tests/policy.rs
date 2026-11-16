//! 谁能用、能去哪里：访问规则、出口的能力与出站策略、接入凭据。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::{
    GW, LAPTOP, NAS, PATIENCE, PUBLIC_IP, Socks5, World, assert_echo, assert_reset, credential, drain, echo_server, eventually,
    http_connect, listener, peer_mut, socks5,
};
use i3t_exit::ResolveError;
use i3t_model::netmap::{ExitRole, ListenerProtocol, PeerExit};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

fn public(port: u16) -> String {
    format!("{PUBLIC_IP}:{port}")
}

/// 不认证的 SOCKS5 握手加一个 CONNECT，返回代理在那之后发回来的全部字节，直到它关连接。
async fn socks5_transcript(proxy: SocketAddr, ip: [u8; 4], port: u16) -> Vec<u8> {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    let [hi, lo] = port.to_be_bytes();
    let [a, b, c, d] = ip;
    stream.write_all(&[5, 1, 0, 5, 1, 0, 1, a, b, c, d, hi, lo]).await.unwrap();
    let (bytes, end) = drain(&mut stream).await;
    end.expect("被拒绝的请求应该是正常关闭的，客户端才读得到应答");
    bytes
}

/// 一个 HTTP CONNECT，返回代理发回来的全部字节，直到它关连接。
async fn http_transcript(proxy: SocketAddr, authority: &str) -> String {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream.write_all(format!("CONNECT {authority} HTTP/1.1\r\n\r\n").as_bytes()).await.unwrap();
    let (bytes, end) = drain(&mut stream).await;
    end.expect("被拒绝的请求应该是正常关闭的，客户端才读得到应答");
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn a_denied_access_rule_looks_the_same_from_either_side_and_like_a_node_that_does_not_exist() {
    let world = World::new().await;
    world.nas.system.redirect("192.168.77.5:80", echo_server().await);
    let (socks, http) = (world.socks().await, world.http().await);

    // 先确认这条路本来是通的。
    let mut stream = socks5(socks, None, "10.66.0.3", 80).await.connected();
    assert_echo(&mut stream).await;
    drop(stream);
    eventually("nas 处理完那条流", || world.nas.exit.stats().active == 0).await;

    // 一、根本没有这个节点。
    let unknown_socks = socks5_transcript(socks, [10, 66, 0, 99], 80).await;
    let unknown_http = http_transcript(http, "10.66.0.99:80").await;

    // 二、访问规则不允许 laptop 访问 nas：laptop 自己的 netmap 里就写着，入口直接拒绝。
    world.laptop.update(|netmap| peer_mut(netmap, NAS).outbound = false).await;
    let forbidden_socks = socks5_transcript(socks, [10, 66, 0, 3], 80).await;
    let forbidden_http = http_transcript(http, "nas.home.i3t:80").await;
    assert_eq!(world.nas.exit.stats().total, 1, "入口拒绝的请求到不了 nas");

    // 三、laptop 以为可以，nas 的 netmap 不允许 laptop 访问它：nas 拒绝。
    world.laptop.update(|netmap| peer_mut(netmap, NAS).outbound = true).await;
    world.nas.update(|netmap| peer_mut(netmap, LAPTOP).inbound = false).await;
    let refused_socks = socks5_transcript(socks, [10, 66, 0, 3], 80).await;
    let refused_http = http_transcript(http, "nas.home.i3t:80").await;
    assert_eq!((world.nas.exit.stats().total, world.nas.exit.stats().refused), (3, 2), "这两个请求是 nas 自己拒绝的");

    // 四、nas 的 netmap 里根本没有 laptop（controller 只下发互相能访问的节点）。
    world.nas.update(|netmap| netmap.peers.retain(|peer| peer.id != common::node(LAPTOP))).await;
    let absent_socks = socks5_transcript(socks, [10, 66, 0, 3], 80).await;

    assert_eq!(unknown_socks, [5, 0, 5, 2, 0, 1, 0, 0, 0, 0, 0, 0], "选中「不认证」，然后是 SOCKS5 的「规则不允许」");
    assert_eq!(forbidden_socks, unknown_socks);
    assert_eq!(refused_socks, unknown_socks);
    assert_eq!(absent_socks, unknown_socks);
    assert!(unknown_http.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{unknown_http}");
    assert_eq!(forbidden_http, unknown_http);
    assert_eq!(refused_http, unknown_http);
    assert_eq!(world.nas.system.connected().len(), 1, "被拒绝的请求没有连过任何地方");
}

#[tokio::test]
async fn an_exit_named_in_the_username_fails_the_same_way_whether_unknown_or_forbidden() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), echo_server().await);
    let socks = world.socks().await;

    world.laptop.update(|netmap| peer_mut(netmap, GW).outbound = false).await;
    let codes = [
        socks5(socks, Some(("10.66.0.99", "")), PUBLIC_IP, 443).await.failed(),
        socks5(socks, Some(("nobody", "")), PUBLIC_IP, 443).await.failed(),
        socks5(socks, Some(("10.66.0.2", "")), PUBLIC_IP, 443).await.failed(),
        socks5(socks, Some(("gw", "")), PUBLIC_IP, 443).await.failed(),
        socks5(socks, Some(("fd00::2", "")), PUBLIC_IP, 443).await.failed(),
        socks5(socks, Some(("gw+x-unknown", "")), PUBLIC_IP, 443).await.failed(),
    ];
    assert_eq!(codes, [2; 6]);
    let statuses = [
        http_connect(world.http().await, &public(443), Some(("10.66.0.99", ""))).await.0,
        http_connect(world.http().await, &public(443), Some(("gw", ""))).await.0,
    ];
    assert_eq!(statuses, [403; 2]);
    assert_eq!(world.gw.exit.stats().total, 0);

    // 出口一侧的访问规则也挡得住：laptop 以为可以，gw 不认。互联网目标用的是乐观回复，所以表现为连上之后被重置。
    world.laptop.update(|netmap| peer_mut(netmap, GW).outbound = true).await;
    world.gw.update(|netmap| peer_mut(netmap, LAPTOP).inbound = false).await;
    let mut stream = socks5(socks, Some(("gw", "")), PUBLIC_IP, 443).await.connected();
    assert_reset(&drain(&mut stream).await.1);
    assert_eq!(world.gw.exit.stats().refused, 1);
    assert!(world.gw.system.connected().is_empty());
}

#[tokio::test]
async fn a_node_that_is_not_an_enabled_exit_refuses_to_carry_traffic_to_the_internet() {
    let world = World::new().await;
    world.nas.system.redirect(&public(443), echo_server().await);
    world.nas.system.redirect("192.168.77.5:8080", echo_server().await);

    // laptop 的 netmap 里 nas 不是出口：入口直接拒绝。
    assert_eq!(socks5(world.socks().await, Some(("nas", "")), PUBLIC_IP, 443).await.failed(), 2);
    assert_eq!(http_connect(world.http().await, &public(443), Some(("nas", ""))).await.0, 403);
    assert_eq!(world.nas.exit.stats().total, 0);

    // laptop 以为 nas 是出口，nas 自己没有启用：能力由节点自己执行，不靠别人不来找它。
    world.laptop.update(|netmap| peer_mut(netmap, NAS).exit = Some(PeerExit::default())).await;
    let mut stream = socks5(world.socks().await, Some(("nas", "")), PUBLIC_IP, 443).await.connected();
    assert_reset(&drain(&mut stream).await.1);
    assert_eq!(world.nas.exit.stats().refused, 1);
    assert!(world.nas.system.connected().is_empty());

    // 访问 nas 自己的服务不需要它是出口。
    let mut stream = socks5(world.socks().await, None, "10.66.0.3", 8080).await.connected();
    assert_echo(&mut stream).await;

    // 启用之后就可以了。
    world.nas.update(|netmap| netmap.me.exit = Some(ExitRole::default())).await;
    let mut stream = socks5(world.socks().await, Some(("nas", "")), PUBLIC_IP, 443).await.connected();
    assert_echo(&mut stream).await;
}

#[tokio::test]
async fn the_exit_only_connects_to_the_ports_it_allows() {
    let world = World::new().await;
    let echo = echo_server().await;
    for port in [80, 443, 8443, 22] {
        world.gw.system.redirect(&public(port), echo);
    }

    // 默认只放行 80 和 443。
    for port in [80, 443] {
        let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, port).await.connected();
        assert_echo(&mut stream).await;
    }
    for port in [8443, 22] {
        let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, port).await.connected();
        assert_reset(&drain(&mut stream).await.1);
    }
    assert_eq!(world.gw.exit.stats().refused, 2);
    assert_eq!(world.gw.system.connected(), [public(80).parse().unwrap(), public(443).parse().unwrap()]);

    // 白名单可以改；没有白名单就是不限制。
    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { ports: Some(vec![8443]), ..ExitRole::default() })).await;
    let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, 8443).await.connected();
    assert_echo(&mut stream).await;
    let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, 443).await.connected();
    assert_reset(&drain(&mut stream).await.1);

    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { ports: None, ..ExitRole::default() })).await;
    let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, 22).await.connected();
    assert_echo(&mut stream).await;
}

#[tokio::test]
async fn a_name_that_resolves_to_a_forbidden_address_is_refused_before_anything_is_connected() {
    let world = World::new().await;
    let echo = echo_server().await;
    let gw = &world.gw.system;
    gw.set_interfaces(&["127.0.0.1/8", "192.168.77.5/24", "198.20.0.5/24"]);
    gw.redirect(&public(443), echo);
    // 即使这些地址真的连得上，也不该去连。
    for target in
        ["127.0.0.1:443", "192.168.77.10:443", "10.0.0.8:443", "169.254.169.254:443", "10.66.0.3:443", "198.20.0.77:443", "100.64.0.9:443"]
    {
        gw.redirect(target, echo);
    }
    gw.name("loopback.example", &["127.0.0.1"]);
    gw.name("lan.example", &["192.168.77.10"]);
    gw.name("private.example", &["10.0.0.8"]);
    gw.name("metadata.example", &["169.254.169.254"]);
    gw.name("overlay.example", &["10.66.0.3"]);
    gw.name("neighbour.example", &["198.20.0.77"]);
    gw.name("cgnat.example", &["100.64.0.9"]);
    gw.name("mapped.example", &["::ffff:10.0.0.8"]);
    gw.name("ula.example", &["fd00::8"]);
    // 把内网地址夹在公网地址后面：只要有一个不该连，整个请求都拒绝。
    gw.name("rebind.example", &[PUBLIC_IP, "127.0.0.1"]);
    gw.name("good.example", &[PUBLIC_IP]);

    let names = [
        "loopback.example",
        "lan.example",
        "private.example",
        "metadata.example",
        "overlay.example",
        "neighbour.example",
        "cgnat.example",
        "mapped.example",
        "ula.example",
        "rebind.example",
    ];
    for name in names {
        let mut stream = socks5(world.socks().await, Some(("gw", "")), name, 443).await.connected();
        assert_reset(&drain(&mut stream).await.1);
    }
    // 直接写地址也一样。回环和运营商级 NAT 的地址在入口那边算不上局域网字面量，是按互联网目标发过来的。
    for literal in ["127.0.0.1", "100.64.0.9", "0.0.0.0", "::1", "224.0.0.1"] {
        let mut stream = socks5(world.socks().await, Some(("gw", "")), literal, 443).await.connected();
        assert_reset(&drain(&mut stream).await.1);
    }
    assert!(world.gw.system.connected().is_empty(), "检查在连接之前：{:?}", world.gw.system.connected());
    let stats = world.gw.exit.stats();
    assert_eq!((stats.refused, stats.failed_by_target, stats.failed_by_exit), (15, 0, 0));

    let mut stream = socks5(world.socks().await, Some(("gw", "")), "good.example", 443).await.connected();
    assert_echo(&mut stream).await;
}

#[tokio::test]
async fn failures_are_attributed_to_the_target_or_to_the_exit() {
    let world = World::new().await;
    let gw = &world.gw.system;
    gw.name_fails("broken-resolver.example", ResolveError::Failed("SERVFAIL".into()));
    gw.name("empty.example", &[]);
    gw.redirect(&public(443), common::closed_port().await);

    for name in ["no-such-name.example", "empty.example", "broken-resolver.example", PUBLIC_IP, "1.1.1.1"] {
        let mut stream = socks5(world.socks().await, Some(("gw", "")), name, 443).await.connected();
        assert_reset(&drain(&mut stream).await.1);
    }
    // 两个域名不存在、一个拒绝连接、一个不可达算目标的；解析器出错算出口的。
    let stats = world.gw.exit.stats();
    assert_eq!((stats.failed_by_target, stats.failed_by_exit, stats.refused), (4, 1, 0));
}

#[tokio::test]
async fn the_upstream_socks5_proxy_of_the_exit_is_asked_for_the_checked_address() {
    let world = World::new().await;
    let upstream = common::Target::bind("127.0.0.1").await;
    world.gw.system.name("api.example.com", &[PUBLIC_IP]);
    world.gw.update(|netmap| netmap.me.exit = Some(ExitRole { upstream_socks5: Some(upstream.addr()), ..ExitRole::default() })).await;

    let client = tokio::spawn({
        let socks = world.socks().await;
        async move {
            let mut stream = socks5(socks, Some(("gw", "")), "api.example.com", 443).await.connected();
            assert_echo(&mut stream).await;
        }
    });

    // 扮演上游代理：收到的是问候和请求合在一起的一次写，目标是解析并检查过的地址，不是域名。
    use tokio::io::AsyncReadExt;
    let mut proxied = upstream.accept().await;
    let mut request = [0; 13];
    proxied.read_exact(&mut request).await.unwrap();
    assert_eq!(request, [5, 1, 0, 5, 1, 0, 1, 93, 184, 216, 34, 0x01, 0xbb]);
    proxied.write_all(&[5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
    let (mut read, mut write) = proxied.split();
    let _ = timeout(PATIENCE, tokio::io::copy(&mut read, &mut write)).await;
    client.await.unwrap();

    // 策略照常在前面：不允许的端口根本不会去找上游。
    let mut stream = socks5(world.socks().await, Some(("gw", "")), "api.example.com", 8443).await.connected();
    assert_reset(&drain(&mut stream).await.1);
    upstream.assert_untouched(Duration::from_millis(50)).await;
}

/// laptop 上加两个对局域网开放的监听器，和一个凭据。
async fn open_to_lan(world: &World, max_concurrent: Option<u32>) -> (SocketAddr, SocketAddr) {
    world
        .laptop
        .update(|netmap| {
            netmap.me.listeners.push(listener("lan-socks", ListenerProtocol::Socks5, "0.0.0.0:0"));
            netmap.me.listeners.push(listener("lan-http", ListenerProtocol::Http, "0.0.0.0:0"));
            netmap.credentials.push(credential("other", "another-token", None));
            netmap.credentials.push(credential("gateway", "s3cret-token", max_concurrent));
        })
        .await;
    (world.laptop.listener("lan-socks").await, world.laptop.listener("lan-http").await)
}

#[tokio::test]
async fn a_listener_open_to_others_demands_the_credential_in_the_password_field() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), echo_server().await);
    let (socks, http) = open_to_lan(&world, None).await;

    // 没有带。
    assert!(matches!(socks5(socks, None, PUBLIC_IP, 443).await, Socks5::NoAcceptableMethod));
    assert_eq!(http_connect(http, &public(443), None).await.0, 407);

    // 带错了：别的字符串、空的、凭据的名字、令牌的哈希都不行。
    let hash = credential("gateway", "s3cret-token", None).token.to_string();
    for password in ["wrong", "", "gateway", "S3cret-token", hash.as_str()] {
        assert!(matches!(socks5(socks, Some(("gw", password)), PUBLIC_IP, 443).await, Socks5::AuthRejected), "{password:?}");
        assert_eq!(http_connect(http, &public(443), Some(("gw", password))).await.0, 407, "{password:?}");
    }
    assert_eq!(world.gw.exit.stats().total, 0);

    // 带对了：用户名照常写出口节点。
    let mut stream = socks5(socks, Some(("gw", "s3cret-token")), PUBLIC_IP, 443).await.connected();
    assert_echo(&mut stream).await;
    let (status, mut stream) = http_connect(http, &public(443), Some(("10.66.0.2", "another-token"))).await;
    assert_eq!(status, 200);
    assert_echo(&mut stream).await;

    // 凭据只证明"可以用这个入口"，去哪里还是按规则：没有写出口，非回环监听器的默认动作是拒绝。
    assert_eq!(socks5(socks, Some(("", "s3cret-token")), PUBLIC_IP, 443).await.failed(), 2);

    // 回环上的监听器不要求凭据。
    let mut stream = socks5(world.socks().await, Some(("gw", "")), PUBLIC_IP, 443).await.connected();
    assert_echo(&mut stream).await;
}

#[tokio::test]
async fn a_credential_is_held_to_its_concurrent_connection_limit() {
    let world = World::new().await;
    world.gw.system.redirect(&public(443), echo_server().await);
    let (socks, http) = open_to_lan(&world, Some(2)).await;
    let auth = Some(("gw", "s3cret-token"));

    let mut first = socks5(socks, auth, PUBLIC_IP, 443).await.connected();
    let (status, mut second) = http_connect(http, &public(443), auth).await;
    assert_eq!(status, 200);
    assert_echo(&mut first).await;
    assert_echo(&mut second).await;

    // 第三条超过了上限。别的凭据不受影响。
    assert_eq!(socks5(socks, auth, PUBLIC_IP, 443).await.failed(), 1);
    assert_eq!(http_connect(http, &public(443), auth).await.0, 429);
    let mut other = socks5(socks, Some(("gw", "another-token")), PUBLIC_IP, 443).await.connected();
    assert_echo(&mut other).await;

    // 放掉一条，名额就回来了。
    drop(first);
    let mut third = timeout(PATIENCE, async {
        loop {
            match socks5(socks, auth, PUBLIC_IP, 443).await {
                Socks5::Connected(stream) => break stream,
                Socks5::Failed(1) => sleep(Duration::from_millis(5)).await,
                other => panic!("不应该是 {other:?}"),
            }
        }
    })
    .await
    .expect("名额应该很快归还");
    assert_echo(&mut third).await;
    assert_echo(&mut second).await;
}
