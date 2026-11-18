//! 同一个 TCP 端口上还接 ice3proxy（lane）：按第一个字节分开，中转的限制照旧。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Client, TestRelay, WAIT};
use i3t_proto::session::{Frame, Init};
use i3t_proto::tcp_frame;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::mpsc;

/// 和 relay 里的一样。
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(3);

async fn connect_from(local: &str, relay: &TestRelay) -> TcpStream {
    tokio::time::timeout(WAIT, async {
        loop {
            let socket = TcpSocket::new_v4().unwrap();
            socket.bind(local.parse().unwrap()).unwrap();
            match socket.connect(relay.addr).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("relay 的 TCP 端口一直连不上")
}

async fn closed_within(stream: &mut TcpStream, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    let mut buf = [0u8; 256];
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut buf)).await {
            Ok(Ok(0) | Err(_)) => return true,
            Ok(Ok(_)) => {}
            Err(_) => return false,
        }
    }
}

/// 一个 relay，同一个端口上交出来的连接进 `handed`。
fn relay_with_handoff(clients: &[&Client]) -> (TestRelay, mpsc::UnboundedReceiver<TcpStream>) {
    let relay = TestRelay::start(clients);
    let (sender, handed) = mpsc::unbounded_channel();
    relay.relay().set_tcp_handoff(Some(Arc::new(move |stream, _| {
        let _ = sender.send(stream);
    })));
    (relay, handed)
}

#[tokio::test]
async fn a_lane_is_handed_over_with_its_first_bytes_unread_while_a_tcp_leg_beside_it_works_as_before() {
    let honest = Client::new(2).await;
    let (relay, mut handed) = relay_with_handoff(&[&honest]);

    // 一条 lane（开头是 TLS 握手记录）和一条中转腿同时从同一个地址来。
    let mut lane = connect_from("127.0.0.1:0", &relay).await;
    let leg = connect_from("127.0.0.1:0", &relay).await;
    lane.write_all(&[0x16, 0x03, 0x01, 0x00, 0x02, 0xab, 0xcd]).await.unwrap();
    let (read_half, mut write_half) = leg.into_split();
    let mut framed = bytes::BytesMut::new();
    tcp_frame::put_frame(&mut framed, &Init { node: honest.id, nonce: [3; 8] }.to_bytes()).unwrap();
    write_half.write_all(&framed).await.unwrap();

    let mut taken = tokio::time::timeout(WAIT, handed.recv()).await.unwrap().unwrap();
    let mut first = [0; 7];
    taken.read_exact(&mut first).await.unwrap();
    assert_eq!(first, [0x16, 0x03, 0x01, 0x00, 0x02, 0xab, 0xcd], "交出来的连接一个字节都没被读走");

    let mut reader = i3t_underlay::tcp_leg::FrameReader::new(read_half);
    let reply = tokio::time::timeout(WAIT, reader.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(Frame::parse(&reply), Some(Frame::Challenge(_))), "中转腿照常握手");
    assert!(handed.try_recv().is_err(), "中转腿不会被交出去");
}

#[tokio::test]
async fn with_a_handoff_the_limit_on_tcp_legs_without_a_session_still_holds() {
    let honest = Client::new(2).await;
    let (relay, mut handed) = relay_with_handoff(&[&honest]);
    let mut held = Vec::new();
    for _ in 0..20 {
        let mut stream = connect_from("127.0.0.1:0", &relay).await;
        // 一个中转帧的开头（长度的高字节），之后什么都不发：认出来是中转，之后受中转的限制。
        stream.write_all(&[0x00]).await.unwrap();
        held.push(stream);
    }
    let mut refused = 0;
    for stream in &mut held {
        if closed_within(stream, Duration::from_millis(500)).await {
            refused += 1;
        }
    }
    assert_eq!(refused, 12, "同一个来源只留 8 条还没握手的中转腿");
    assert!(handed.try_recv().is_err());
}

#[tokio::test]
async fn a_connection_that_never_says_anything_is_cut_at_the_classification_deadline_and_not_handed_over() {
    let honest = Client::new(2).await;
    let (relay, mut handed) = relay_with_handoff(&[&honest]);
    let started = tokio::time::Instant::now();
    let mut silent = connect_from("127.0.0.1:0", &relay).await;
    assert!(closed_within(&mut silent, CLASSIFY_TIMEOUT + Duration::from_secs(2)).await, "迟迟不说话的连接要被断开");
    assert!(started.elapsed() >= CLASSIFY_TIMEOUT - Duration::from_millis(200), "等够了时限才断：{:?}", started.elapsed());
    assert!(handed.try_recv().is_err());
}

#[tokio::test]
async fn hostile_first_bytes_go_down_the_relay_path_and_are_refused_there() {
    let honest = Client::new(2).await;
    let (relay, mut handed) = relay_with_handoff(&[&honest]);
    // 不是中转的握手、也不是 TLS：HTTP、一个声称很长的帧、一个不认识的帧。都按中转处理，被中转的规矩挡掉。
    for garbage in [&b"GET / HTTP/1.1\r\n\r\n"[..], &[0x00, 0x05, 0x7f, 0x7f, 0x7f, 0x7f, 0x7f][..]] {
        let mut stream = connect_from("127.0.0.1:0", &relay).await;
        stream.write_all(garbage).await.unwrap();
        assert!(closed_within(&mut stream, Duration::from_secs(12)).await, "{garbage:?} 应该被断开");
    }
    assert!(handed.try_recv().is_err(), "这些连接都不该交给 ice3proxy");
}

#[tokio::test]
async fn without_a_handoff_a_connection_starting_like_tls_is_just_a_bad_tcp_leg() {
    let honest = Client::new(2).await;
    let relay = TestRelay::start(&[&honest]);
    let mut stream = connect_from("127.0.0.1:0", &relay).await;
    // 0x1603 当作帧长度（5635 字节），之后的不是握手：中转的规矩照旧，在握手时限内断开。
    stream.write_all(&[0x16, 0x03, 0x01, 0x00, 0x02, 0xab, 0xcd]).await.unwrap();
    assert!(closed_within(&mut stream, Duration::from_secs(12)).await);
}
