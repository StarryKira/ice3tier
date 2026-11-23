//! 一条 lane 上的一条流：两种负载方式、两个开流方向、半关闭、异常结束。真实的回环 TCP。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::io;
use std::time::Duration;

use common::{End as HopEnd, Pair, accept_next, quick, read_to_end, wait_for};
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
    let ((send, mut recv), far) =
        if entry_initiates { pair.entry_opens(&header, payload).await } else { pair.exit_opens(&header, payload).await };
    // 这里接受方回的不是流层的 REPLY，发起方把收到的都当数据读：一开始就算读完了头。
    recv.end_header();
    ((send, recv), far)
}

/// 接受方回一点东西并切换（passthrough 时），发起方读到切换之后的第一个字节：这时发起方已经回了它的 SWITCH，
/// 两个方向都离开了 TLS（sealed 时都还在）。
#[cfg(target_os = "linux")]
async fn both_switch(recv: &mut i3t_lane::LaneRecv, far_send: &mut i3t_lane::LaneSend) {
    far_send.write_all(b"REPLY").await.unwrap();
    far_send.end_header(Payload::Passthrough);
    far_send.write_all(b"!").await.unwrap();
    let mut reply = [0; 6];
    recv.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"REPLY!");
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

/// 发了 ABORT 的一端丢弃流之后，不紧跟着用 RST 关掉 lane：Windows 收到 RST 时会扔掉还没读走的数据，对端就读不到 ABORT
/// 和它的原因码了。Linux 先交出 RST 之前到的数据，从对端看不出区别，所以在中间一跳上看那个方向是怎么结束的。
#[tokio::test]
async fn an_abort_is_followed_by_a_fin_while_the_peer_has_not_read_it_yet() {
    for entry_initiates in [true, false] {
        let pair = Pair::new(quick()).await;
        pair.ready(1).await;
        let ((mut send, recv), (_far_send, mut far_recv)) = open(&pair, entry_initiates, Payload::Sealed).await;
        send.write_all(b"partial").await.unwrap();
        send.reset(StreamCode(7));
        drop((send, recv));
        // 中间一跳上，从入口出来的方向是 backward（出口是拨号的一方）。池里多余的空闲 lane 这时也可能被关掉，所以不数条数，
        // 只看结束了的方向里没有 RST。
        let ended = |recording: &common::Recording| if entry_initiates { recording.backward_end } else { recording.forward_end };
        let ends = || -> Vec<HopEnd> { pair.hop.recordings.lock().unwrap().iter().filter_map(|r| ended(&r.lock().unwrap())).collect() };
        wait_for("重置的一端发完 ABORT 之后结束它的方向", Duration::from_secs(5), || !ends().is_empty()).await;
        let ends = ends();
        assert!(!ends.contains(&HopEnd::Reset), "对端还没读，ABORT 后面跟的是 FIN 而不是 RST（入口开流：{entry_initiates}）：{ends:?}");
        let error = assert_aborted(&mut far_recv, "发起方重置").await;
        assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(7))));
    }
}

/// Linux 上原始字节的方向一重置就发 RST，不等另一个方向也被丢弃。
#[cfg(target_os = "linux")]
#[tokio::test]
async fn on_linux_a_raw_reset_goes_out_at_once() {
    both_ways_both_modes(async |pair, entry_initiates, payload| {
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = open(pair, entry_initiates, payload).await;
        // 发起方的方向要等接受方切换了才是原始字节。
        both_switch(&mut recv, &mut far_send).await;
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

/// 流层的头只能来自 TLS 里面：接受方在 REPLY 写完之前就发了 SWITCH、REPLY 的另一半跟在后面的原始字节里时，发起方读出错，
/// 不会把后面的原始字节当作头交出来（issue #17）。反方向（OPEN 的另一半不在 TLS 里）只有没升级的发起方做得出来：
/// 升级了的发起方在接受方切换之前不离开 TLS，见 `i3t-lane` 里流的单元测试。
#[tokio::test]
async fn a_header_cannot_be_completed_with_bytes_from_outside_tls() {
    let pair = Pair::new(quick()).await;
    pair.ready(2).await;
    for entry_initiates in [true, false] {
        let (from, to) = if entry_initiates { (&pair.entry, &pair.exit) } else { (&pair.exit, &pair.entry) };
        let (mut send, mut recv) = from.lanes.open_stream(to.id).await.unwrap();
        send.write_all(b"OPEN").await.unwrap();
        send.end_header(Payload::Passthrough);
        let (mut far_send, mut far_recv) = accept_next(&to.lanes).await;
        let mut open = [0; 4];
        far_recv.read_exact(&mut open).await.unwrap();
        far_recv.end_header();

        far_send.write_all(b"RE").await.unwrap();
        far_send.end_header(Payload::Passthrough);
        far_send.write_all(b"PLY, then data").await.unwrap();
        let mut got = [0; 2];
        recv.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"RE");
        let error = recv.read_exact(&mut got).await.expect_err("REPLY 的另一半不在 TLS 里，不能交出来");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }
}

/// 头读完之后，后面的字节按顺序接着交出来：先是还在 TLS 里的（OPENING 里头后面的首批数据、切换之前写的），再是切换之后的原始字节。
#[tokio::test]
async fn once_the_header_is_read_the_bytes_behind_it_follow_in_order() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let (mut send, mut recv) = pair.entry.lanes.open_stream(pair.exit.id).await.unwrap();
    send.write_all(b"OPEN first data").await.unwrap();
    send.end_header(Payload::Passthrough);
    send.write_all(b", sealed data").await.unwrap();
    let (mut far_send, mut far_recv) = accept_next(&pair.exit.lanes).await;
    let mut open = [0; 4];
    far_recv.read_exact(&mut open).await.unwrap();
    assert_eq!(&open, b"OPEN");
    far_recv.end_header();

    far_send.write_all(b"REPLY").await.unwrap();
    far_send.end_header(Payload::Passthrough);
    far_send.write_all(b" raw answer").await.unwrap();
    far_send.shutdown().await.unwrap();
    let mut reply = [0; 5];
    recv.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"REPLY");
    recv.end_header();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b" raw answer");
    // 发起方读到接受方的 SWITCH 时已经回了自己的：这之后写的是原始字节。
    send.write_all(b", raw data").await.unwrap();
    send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b" first data, sealed data, raw data");
}

#[tokio::test]
async fn raw_halves_can_be_taken_once_both_directions_left_tls() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let ((mut send, mut recv), (mut far_send, mut far_recv)) = pair.entry_opens(b"OPEN", Payload::Passthrough).await;
    assert!(recv.take_raw().is_none(), "回复还没读到时这个方向还在 TLS 里");
    assert!(send.take_raw().is_none(), "接受方切换之前发起方的方向也还在 TLS 里");
    assert!(far_recv.take_raw().is_none(), "发起方还没回 SWITCH");
    far_send.write_all(b"REPLY").await.unwrap();
    far_send.end_header(Payload::Passthrough);
    far_send.write_all(b"raw after switch").await.unwrap();
    let mut reply = [0; 5];
    recv.read_exact(&mut reply).await.unwrap();
    // SWITCH 跟在 REPLY 后面发出，读到 REPLY 时可能还在路上。发起方收到它时回一个 SWITCH，自己的方向跟着离开 TLS。
    let patience = Duration::from_secs(5);
    let raw_recv = tokio::time::timeout(patience, recv.wait_raw()).await.unwrap().expect("SWITCH 到了之后接收方向可以交出去");
    let raw_send = send.take_raw().expect("回了 SWITCH 之后发起方的方向也离开了 TLS");
    let far_raw_recv =
        tokio::time::timeout(patience, far_recv.wait_raw()).await.unwrap().expect("发起方的 SWITCH 之后接受方的接收方向是原始字节");
    let (socket, unsent) = raw_send.into_parts();
    // 发起方的 SWITCH 可能还没全部交给内核：交出来的 socket 上先写它。
    let mut out = [&unsent[..], b"spliced up"].concat();
    while !out.is_empty() {
        socket.writable().await.unwrap();
        if let Ok(len) = socket.try_write(&out) {
            out.drain(..len);
        }
    }
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
