//! 不认识的人连上来不说话、说一半、开很多条：controller 的两个监听都不能因此把正经的节点和管理员挡在外面。

// clippy.toml 里的 allow-unwrap-in-tests 只认 `#[test]` 函数本身，认不出这里的辅助函数。
#![allow(clippy::unwrap_used, reason = "整个文件都是测试代码，出错就该直接失败")]

mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use common::{Running, fast_timing};
use i3t_controller::node_server::CLIENT_HELLO_TIMEOUT;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::time::timeout;

/// 从回环网段里的 `source` 这个地址连 `to`。Linux 上整个 127.0.0.0/8 都是本机，可以拿来装成不同的来源。
async fn connect_from(source: [u8; 4], to: SocketAddr) -> TcpStream {
    let socket = TcpSocket::new_v4().unwrap();
    socket.bind(SocketAddr::from((source, 0))).unwrap();
    socket.connect(to).await.unwrap()
}

/// 对方是不是在 `within` 之内关掉了这条连接。
async fn closed_within(stream: &mut TcpStream, within: Duration) -> bool {
    let mut buffer = [0; 64];
    loop {
        match timeout(within, stream.read(&mut buffer)).await {
            Ok(Ok(0) | Err(_)) => return true,
            Ok(Ok(_)) => {}
            Err(_) => return false,
        }
    }
}

/// 数一数这些连接里有几条已经被对方关掉了。
async fn count_closed(streams: &mut [TcpStream]) -> usize {
    let mut closed = 0;
    for stream in streams {
        if closed_within(stream, Duration::from_millis(20)).await {
            closed += 1;
        }
    }
    closed
}

#[tokio::test]
async fn idle_connections_do_not_lock_registered_nodes_out() {
    let running = Running::start(fast_timing()).await;
    let network = running.network("home");
    let mut node = running.join(network.id, &[]).await;
    node.disconnect();

    // 比以前全部的名额（256）还多的、只连不说话的连接，和节点来自同一个地址。
    let mut idle = Vec::new();
    for _ in 0..300 {
        idle.push(TcpStream::connect(running.node_addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    let reconnected = timeout(Duration::from_secs(5), node.connect(&running)).await;
    assert!(matches!(reconnected, Ok(Ok(()))), "名额满了也要挤掉不说话的连接，让节点进来：{reconnected:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "不用等那些连接超时：{:?}", started.elapsed());
    assert!(count_closed(&mut idle).await >= 300 - 16, "同一个来源最多留 16 条还没报到的连接");
}

#[tokio::test]
async fn a_flood_from_one_source_is_trimmed_without_touching_other_sources() {
    let running = Running::start(fast_timing()).await;
    let mut neighbour = connect_from([127, 0, 0, 3], running.node_addr).await;
    let mut flood = Vec::new();
    for _ in 0..40 {
        flood.push(connect_from([127, 0, 0, 2], running.node_addr).await);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count_closed(&mut flood).await, 40 - 16, "一个来源超出的部分从最老的挤掉");
    assert!(!closed_within(&mut neighbour, Duration::from_millis(50)).await, "别的来源的连接不受影响");
}

#[tokio::test]
async fn a_connection_that_never_starts_the_handshake_is_dropped_early() {
    let running = Running::start(fast_timing()).await;
    let mut silent = TcpStream::connect(running.node_addr).await.unwrap();
    let started = Instant::now();
    assert!(closed_within(&mut silent, CLIENT_HELLO_TIMEOUT + Duration::from_secs(2)).await);
    let waited = started.elapsed();
    assert!(waited >= CLIENT_HELLO_TIMEOUT - Duration::from_millis(100), "{waited:?}");
    assert!(waited < fast_timing().handshake_timeout, "不说话的连接不用等满整个握手时限：{waited:?}");
}

/// 用一条新连接发一个带令牌的请求，返回状态行。
async fn admin_status(running: &Running) -> String {
    let mut stream = TcpStream::connect(running.admin_addr).await.unwrap();
    let request = format!(
        "GET /api/v1/status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        running.admin_token
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), stream.read_to_end(&mut response)).await.unwrap().unwrap();
    String::from_utf8_lossy(&response).lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn idle_admin_connections_are_capped_per_source_and_the_admin_still_gets_in() {
    let running = Running::start(fast_timing()).await;
    let mut idle = Vec::new();
    for _ in 0..200 {
        idle.push(TcpStream::connect(running.admin_addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(count_closed(&mut idle).await >= 200 - 32, "同一个来源最多留 32 条管理 API 的连接");
    assert_eq!(admin_status(&running).await, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn an_admin_connection_that_sends_half_a_request_is_closed() {
    let running = Running::start(fast_timing()).await;
    let mut slow = TcpStream::connect(running.admin_addr).await.unwrap();
    slow.write_all(b"GET /api/v1/status HTTP/1.1\r\nHost: loc").await.unwrap();
    assert!(closed_within(&mut slow, Duration::from_secs(13)).await, "请求头 10 秒之内没有读完就关掉");
    assert_eq!(admin_status(&running).await, "HTTP/1.1 200 OK");
}
