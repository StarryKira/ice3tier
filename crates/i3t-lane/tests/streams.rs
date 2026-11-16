//! 一条 lane 上的一条流：两种负载方式、两个开流方向、半关闭、异常结束。真实的回环 TCP。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::io;
use std::time::Duration;

use common::{Pair, quick, read_to_end};
use i3t_conn::{RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_lane::Payload;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CLIENT_HELLO: &[u8] = &[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0x00, 0x00, 0x00];

/// 各种开法：(谁开, 负载方式)。
async fn both_ways_both_modes(mut check: impl AsyncFnMut(&Pair, bool, Payload)) {
    let pair = Pair::new(quick()).await;
    pair.ready(2).await;
    for entry_initiates in [true, false] {
        for payload in [Payload::Sealed, Payload::Passthrough] {
            check(&pair, entry_initiates, payload).await;
        }
    }
}

async fn open(
    pair: &Pair,
    entry_initiates: bool,
    payload: Payload,
) -> ((i3t_lane::LaneSend, i3t_lane::LaneRecv), (i3t_lane::LaneSend, i3t_lane::LaneRecv)) {
    let mut header = b"OPEN".to_vec();
    header.extend_from_slice(CLIENT_HELLO);
    if entry_initiates { pair.entry_opens(&header, payload).await } else { pair.exit_opens(&header, payload).await }
}

#[tokio::test]
async fn a_stream_carries_data_both_ways_in_each_mode_and_direction() {
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        assert_eq!(far_recv.payload(), Some(payload));
        far_send.write_all(b"REPLY").await.unwrap();
        far_send.end_header(Payload::Passthrough);
        let mut reply = [0; 5];
        recv.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"REPLY");

        let upload: Vec<u8> = (0..300_000u32).map(|i| u8::try_from(i % 253).unwrap()).collect();
        let download: Vec<u8> = upload.iter().rev().copied().collect();
        let (up, down) = tokio::join!(
            async {
                send.write_all(&upload).await.unwrap();
                send.shutdown().await.unwrap();
                read_to_end(&mut far_recv).await.unwrap()
            },
            async {
                far_send.write_all(&download).await.unwrap();
                far_send.shutdown().await.unwrap();
                read_to_end(&mut recv).await.unwrap()
            }
        );
        assert_eq!(up, upload, "上行 {payload:?}，入口开流：{entry_initiates}");
        assert_eq!(down, download, "下行 {payload:?}，入口开流：{entry_initiates}");
    })
    .await;
}

#[tokio::test]
async fn each_direction_can_end_first_while_the_other_keeps_going() {
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        far_send.end_header(Payload::Passthrough);
        // 发起方先说完、关掉自己的方向；接受方读到结束之后才开始回话。
        send.write_all(b"request").await.unwrap();
        send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"request");
        far_send.write_all(b"answer after the request ended").await.unwrap();
        far_send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut recv).await.unwrap(), b"answer after the request ended");
    })
    .await;

    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        // 反过来：接受方先结束，发起方之后还在发。
        far_send.write_all(b"done").await.unwrap();
        far_send.end_header(Payload::Passthrough);
        far_send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut recv).await.unwrap(), b"done");
        send.write_all(b"still talking").await.unwrap();
        send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"still talking");
    })
    .await;
}

/// 读的结果不能是正常结束（EOF），必须是错误。
async fn assert_aborted(recv: &mut (impl AsyncReadExt + Unpin), what: &str) -> io::Error {
    let mut sink = Vec::new();
    match tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(&mut sink)).await {
        Ok(Ok(_)) => panic!("{what}：读到了正常结束，应该是异常结束"),
        Ok(Err(error)) => error,
        Err(_) => panic!("{what}：5 秒之内没有看到异常结束"),
    }
}

#[tokio::test]
async fn a_reset_reaches_the_other_side_as_an_abort_never_as_a_clean_end() {
    // 和流层一样：重置之后两个方向都丢弃。原始字节的方向上，Linux 之外 RST 要到这时才发（见下一个测试）。
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        // 发起方重置。
        let ((mut send, recv), (_far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        send.write_all(b"partial").await.unwrap();
        send.reset(StreamCode(7));
        drop((send, recv));
        let error = assert_aborted(&mut far_recv, "发起方重置").await;
        if payload == Payload::Sealed {
            assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(7))), "sealed 时原因码送得到");
        }

        // 接受方回了一半之后重置。
        let ((_send, mut recv), (mut far_send, far_recv)) = open(pair, entry_initiates, payload).await;
        far_send.write_all(b"half an ans").await.unwrap();
        far_send.end_header(Payload::Passthrough);
        far_send.write_all(b"wer").await.unwrap();
        far_send.reset(StreamCode(2));
        drop((far_send, far_recv));
        let error = assert_aborted(&mut recv, "接受方中途重置").await;
        if payload == Payload::Sealed {
            assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(2))));
        }

        // 接受方连 REPLY 都没回就以访问规则为由重置：这时它的方向还在 TLS 里，两种模式都送得到原因码，不用等丢弃。
        let ((_send, mut recv), (mut far_send, _far_recv)) = open(pair, entry_initiates, payload).await;
        far_send.reset(StreamCode(5));
        let error = assert_aborted(&mut recv, "接受方开头就重置").await;
        assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(5))), "{payload:?}");
    })
    .await;
}

/// Linux 上原始字节的方向一重置就发 RST，不等另一个方向也被丢弃。
#[cfg(target_os = "linux")]
#[tokio::test]
async fn on_linux_a_raw_reset_goes_out_at_once() {
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((mut send, _recv), (_far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        send.write_all(b"partial").await.unwrap();
        send.reset(StreamCode(7));
        assert_aborted(&mut far_recv, "发起方重置，接收方向还留着").await;
    })
    .await;
}

#[tokio::test]
async fn dropping_a_stream_without_shutdown_is_an_abort() {
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((send, recv), (mut far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        far_send.end_header(Payload::Passthrough);
        drop((send, recv));
        assert_aborted(&mut far_recv, "发起方丢弃").await;

        let ((_send, mut recv), (mut far_send, far_recv)) = open(pair, entry_initiates, payload).await;
        far_send.write_all(b"some").await.unwrap();
        far_send.end_header(Payload::Passthrough);
        drop((far_send, far_recv));
        assert_aborted(&mut recv, "接受方丢弃").await;
    })
    .await;
}

#[tokio::test]
async fn a_refusal_gets_its_reply_through_and_stops_the_initiator() {
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        // 流层的拒绝：先叫停对方，再写 REPLY 并正常结束。
        far_recv.stop(StreamCode(0));
        far_send.write_all(b"REFUSED").await.unwrap();
        far_send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut recv).await.unwrap(), b"REFUSED");
        // 叫停送到之后，发起方的写会出错。
        let mut stopped = None;
        for _ in 0..100 {
            if let Err(error) = send.write_all(b"more data").await {
                stopped = Some(error);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let error = stopped.expect("被叫停之后发起方的写要出错");
        assert_eq!(
            StreamAborted::of(&error),
            Some(StreamAborted::Stopped(StreamCode(0))),
            "{payload:?}，入口开流：{entry_initiates}：{error}"
        );
        drop((far_send, far_recv));
    })
    .await;
}

#[tokio::test]
async fn raw_halves_can_be_taken_once_both_directions_left_tls() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let ((mut send, mut recv), (mut far_send, mut far_recv)) = pair.entry_opens(b"OPEN", Payload::Passthrough).await;
    assert!(recv.take_raw().is_none(), "回复还没读到时这个方向还在 TLS 里");
    let raw_send = send.take_raw().expect("发起方的方向在 OPENING 之后就离开了 TLS");
    far_send.write_all(b"REPLY").await.unwrap();
    far_send.end_header(Payload::Passthrough);
    far_send.write_all(b"raw after switch").await.unwrap();
    let mut reply = [0; 5];
    recv.read_exact(&mut reply).await.unwrap();
    // SWITCH 跟在 REPLY 后面发出；读到 REPLY 时它多半已经到了，没到的话稍等一下再要。
    let mut raw_recv = None;
    for _ in 0..100 {
        raw_recv = recv.take_raw();
        if raw_recv.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let raw_recv = raw_recv.expect("SWITCH 到了之后接收方向可以交出去");
    let far_raw_recv = far_recv.take_raw().expect("接受方读到 passthrough 的 OPENING 之后它的接收方向就是原始字节");
    let (socket, unsent) = raw_send.into_parts();
    assert!(unsent.is_empty());
    socket.writable().await.unwrap();
    socket.try_write(b"spliced up").unwrap();
    let (far_socket, already) = far_raw_recv.into_parts();
    let mut got = already.to_vec();
    while got.len() < b"spliced up".len() {
        far_socket.readable().await.unwrap();
        let mut buf = [0; 64];
        if let Ok(len) = far_socket.try_read(&mut buf) {
            got.extend_from_slice(&buf[..len]);
        }
    }
    assert_eq!(got, b"spliced up");
    let (socket, already) = raw_recv.into_parts();
    let mut got = already.to_vec();
    while got.len() < b"raw after switch".len() {
        socket.readable().await.unwrap();
        let mut buf = [0; 64];
        if let Ok(len) = socket.try_read(&mut buf) {
            got.extend_from_slice(&buf[..len]);
        }
    }
    assert_eq!(got, b"raw after switch");
}
