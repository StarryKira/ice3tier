//! 流的两种结束方式在对端看起来是什么样：`contract.rs` 里那张表的每一格。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod common;

use std::io::ErrorKind;
use std::time::Duration;

use common::{Node, Peer, SOON, View};
use i3t_conn::{Acceptor, Connector, DROPPED_STREAM_CODE, Incoming, RecvHalf, RecvStream, SendHalf, SendStream, StreamAborted, StreamCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 两个能直连的节点，以及 A 到 B 的一条流的四个半边：(A 的发送, A 的接收, B 的发送, B 的接收)。
/// 流要有数据才会在对端出现，所以 A 先写了 `greeting`。
async fn stream_between(a: &Node, b: &Node, greeting: &[u8]) -> (SendHalf, RecvHalf, SendHalf, RecvHalf) {
    a.apply(&View { peers: &[Peer::direct(b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(a)], ..View::default() });
    let (mut a_send, a_recv) = tokio::time::timeout(SOON, a.endpoint.open(b.id)).await.expect("开流超时").unwrap();
    a_send.write_all(greeting).await.unwrap();
    let Incoming { peer, send, mut recv } = tokio::time::timeout(SOON, b.endpoint.accept()).await.expect("等对端开过来的流超时").unwrap();
    assert_eq!(peer, a.id, "流是谁开过来的由握手确定");
    let mut received = vec![0u8; greeting.len()];
    recv.read_exact(&mut received).await.unwrap();
    assert_eq!(received, greeting);
    (a_send, a_recv, send, recv)
}

/// 读到出错为止，返回之前读到的字节数和那个错误。读到 EOF 算测试失败。
async fn read_until_error(recv: &mut RecvHalf) -> (usize, std::io::Error) {
    let mut total = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match tokio::time::timeout(SOON, recv.read(&mut buf)).await.expect("等流出错超时") {
            Ok(0) => panic!("流正常结束了（读了 {total} 字节之后），本该出错"),
            Ok(len) => total += len,
            Err(error) => return (total, error),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_delivers_everything_and_then_end_of_file_in_each_direction_independently() {
    let (a, b) = (Node::start(1), Node::start(2));
    let (mut a_send, mut a_recv, mut b_send, mut b_recv) = stream_between(&a, &b, b"hello").await;

    // A → B 方向正常结束，B → A 方向还开着。
    a_send.write_all(&[7u8; 200_000]).await.unwrap();
    a_send.shutdown().await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(SOON, b_recv.read_to_end(&mut rest)).await.expect("等 EOF 超时").unwrap();
    assert_eq!(rest, vec![7u8; 200_000], "正常结束之前写的数据全部送达");

    b_send.write_all(b"still open").await.unwrap();
    let mut reply = [0u8; 10];
    a_recv.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"still open");

    b_send.shutdown().await.unwrap();
    assert_eq!(tokio::time::timeout(SOON, a_recv.read(&mut reply)).await.expect("等 EOF 超时").unwrap(), 0);
    // 正常结束之后再 shutdown 没有效果，也不出错。
    a_send.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_makes_the_peers_read_fail_with_the_code() {
    let (a, b) = (Node::start(1), Node::start(2));
    let (mut a_send, _a_recv, _b_send, mut b_recv) = stream_between(&a, &b, b"hello").await;

    a_send.write_all(b"this may or may not arrive").await.unwrap();
    a_send.reset(StreamCode(7));
    let (_, error) = read_until_error(&mut b_recv).await;
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(7))));
    assert_eq!(error.kind(), ErrorKind::ConnectionReset);

    // 重置之后这个方向不能再写，再重置没有效果。
    assert!(a_send.write_all(b"more").await.is_err());
    a_send.reset(StreamCode(8));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_send_half_without_shutdown_is_a_reset_not_an_end_of_file() {
    let (a, b) = (Node::start(1), Node::start(2));
    let (mut a_send, _a_recv, _b_send, mut b_recv) = stream_between(&a, &b, b"hello").await;

    a_send.write_all(&[1u8; 50_000]).await.unwrap();
    drop(a_send);
    let (received, error) = read_until_error(&mut b_recv).await;
    assert!(received <= 50_000);
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(DROPPED_STREAM_CODE)), "被截断的数据不能表现成正常结束");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_shutdown_neither_reset_nor_drop_disturbs_the_data() {
    let (a, b) = (Node::start(1), Node::start(2));
    let (mut a_send, _a_recv, _b_send, mut b_recv) = stream_between(&a, &b, b"hello").await;

    a_send.write_all(&[3u8; 300_000]).await.unwrap();
    a_send.shutdown().await.unwrap();
    a_send.reset(StreamCode(5));
    drop(a_send);
    let mut rest = Vec::new();
    tokio::time::timeout(SOON, b_recv.read_to_end(&mut rest)).await.expect("等 EOF 超时").unwrap();
    assert_eq!(rest.len(), 300_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_makes_the_peers_writes_fail_with_the_code() {
    let (a, b) = (Node::start(1), Node::start(2));
    let (mut a_send, _a_recv, _b_send, mut b_recv) = stream_between(&a, &b, b"hello").await;

    b_recv.stop(StreamCode(9));
    // 叫停要一个往返才到发送方，之前的写还会成功。
    let error = tokio::time::timeout(SOON, async {
        loop {
            if let Err(error) = a_send.write_all(&[0u8; 1024]).await {
                break error;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("叫停之后写应该出错");
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Stopped(StreamCode(9))));
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    assert_eq!(StreamAborted::of(&error).map(StreamAborted::code), Some(StreamCode(9)));

    // 被叫停的方向 shutdown 也不能报成功：对端没有收全。
    let error = a_send.shutdown().await.expect_err("被叫停之后不能算正常结束");
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Stopped(StreamCode(9))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_directions_of_a_stream_end_independently() {
    let (a, b) = (Node::start(1), Node::start(2));
    let (mut a_send, mut a_recv, mut b_send, mut b_recv) = stream_between(&a, &b, b"hello").await;

    // A → B 被重置，B → A 照常用，最后正常结束。
    a_send.reset(StreamCode(1));
    let (_, error) = read_until_error(&mut b_recv).await;
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(1))));

    b_send.write_all(b"the other direction is fine").await.unwrap();
    b_send.shutdown().await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(SOON, a_recv.read_to_end(&mut reply)).await.expect("等 EOF 超时").unwrap();
    assert_eq!(reply, b"the other direction is fine");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_streams_share_one_connection_without_mixing_their_data() {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    b.spawn_echo();

    let streams = (0..64u8).map(|index| {
        let endpoint = std::sync::Arc::clone(&a.endpoint);
        let target = b.id;
        tokio::spawn(async move {
            let (mut send, mut recv) = endpoint.open(target).await.unwrap();
            let payload = vec![index; 20_000 + usize::from(index) * 100];
            send.write_all(&payload).await.unwrap();
            send.shutdown().await.unwrap();
            let mut echoed = Vec::new();
            recv.read_to_end(&mut echoed).await.unwrap();
            assert!(echoed == payload, "第 {index} 条流的回声不对");
        })
    });
    for stream in streams.collect::<Vec<_>>() {
        tokio::time::timeout(3 * SOON, stream).await.expect("等流结束超时").unwrap();
    }
}
