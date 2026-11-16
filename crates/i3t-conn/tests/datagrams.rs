//! 一对节点之间的数据报：给哪一侧的就到哪一侧，直连和经 relay 都通；不保证送达的会丢，保证送达的一个不少。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use common::{Node, Peer, SOON, View, echo_once};
use i3t_conn::Direction;
use i3t_conn::contract::{Datagram, DatagramSendError, DatagramSender, Datagrams};
use i3t_model::control::Route;
use i3t_proto::flow::{DatagramHeader, MAX_DATAGRAM_BODY_LEN, Recipient};

/// 一帧：头后面跟着 `body`。连接层只看头的首字节。
fn frame(recipient: Recipient, body: &[u8]) -> Bytes {
    let mut frame = Vec::new();
    DatagramHeader { recipient, context: 1, fragment: None }.encode(&mut frame);
    frame.extend_from_slice(body);
    frame.into()
}

/// 帧里头后面的部分。
fn body(datagram: &Datagram) -> &[u8] {
    DatagramHeader::decode(&datagram.payload).unwrap().1
}

async fn next(node: &Node, recipient: Recipient) -> Datagram {
    tokio::time::timeout(SOON, node.endpoint.recv_datagram(recipient)).await.expect("等数据报超时").expect("本端没有关闭")
}

/// 两个能直连的节点，之间已经有连接：数据报不会自己去建连接，所以先开一条流。
async fn connected() -> (Node, Node) {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    b.spawn_echo();
    echo_once(&a, &b, b"connect").await;
    (a, b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_reaches_the_side_it_is_addressed_to_and_names_its_sender() {
    let (a, b) = connected().await;
    let (from_a, from_b) = (a.endpoint.datagram_sender(), b.endpoint.datagram_sender());

    from_a.send_datagram(b.id, frame(Recipient::Acceptor, b"to the acceptor side")).unwrap();
    from_a.send_datagram(b.id, frame(Recipient::Initiator, b"to the initiator side")).unwrap();
    let to_acceptor = next(&b, Recipient::Acceptor).await;
    let to_initiator = next(&b, Recipient::Initiator).await;
    assert_eq!((to_acceptor.peer, body(&to_acceptor)), (a.id, &b"to the acceptor side"[..]));
    assert_eq!((to_initiator.peer, body(&to_initiator)), (a.id, &b"to the initiator side"[..]));

    // 反方向用的是同一条连接，不用另外握手。
    from_b.send_datagram(a.id, frame(Recipient::Initiator, b"back")).unwrap();
    let back = next(&a, Recipient::Initiator).await;
    assert_eq!((back.peer, body(&back)), (b.id, &b"back"[..]));

    // 连接从 1200 字节的包起步，探测到的上限是 1400：一个数据报能带的比包略小。
    let max = from_a.max_datagram_len(b.id).expect("有连接就有上限");
    assert!((1100..1400).contains(&max), "{max}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_largest_datagram_goes_through_and_a_larger_one_is_refused_instead_of_truncated() {
    let (a, b) = connected().await;
    let sender = a.endpoint.datagram_sender();
    let max = sender.max_datagram_len(b.id).unwrap();

    let largest = vec![0x5a; max - DatagramHeader::LEN];
    sender.send_datagram(b.id, frame(Recipient::Acceptor, &largest)).unwrap();
    assert_eq!(body(&next(&b, Recipient::Acceptor).await), largest);

    // 上限会随着路径 MTU 的探测往上走，但到不了连接层的最大包长（1400）。
    let too_large = frame(Recipient::Acceptor, &[0; 1400]);
    let refused = sender.send_datagram(b.id, too_large).unwrap_err();
    assert!(matches!(refused, DatagramSendError::TooLarge { len: 1405, max } if (1100..1400).contains(&max)), "{refused:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datagrams_are_not_sent_to_a_peer_without_a_connection() {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let sender = a.endpoint.datagram_sender();

    // 数据报不会为自己去建连接：上层总是先开流。
    assert_eq!(sender.max_datagram_len(b.id), None);
    assert_eq!(sender.send_datagram(b.id, frame(Recipient::Acceptor, b"x")), Err(DatagramSendError::NotConnected(b.id)));
    assert_eq!(sender.send_reliable(b.id, frame(Recipient::Acceptor, b"x")).await, Err(DatagramSendError::NotConnected(b.id)));

    // 本端关掉之后，留在别人手里的句柄也发不出去，等数据报的人被叫醒。
    b.spawn_echo();
    echo_once(&a, &b, b"connect").await;
    assert!(sender.max_datagram_len(b.id).is_some());
    let waiting = tokio::spawn({
        let endpoint = Arc::clone(&a.endpoint);
        async move { endpoint.recv_datagram(Recipient::Acceptor).await }
    });
    a.endpoint.close().await;
    assert_eq!(tokio::time::timeout(SOON, waiting).await.expect("关闭之后等数据报的人应该被叫醒").unwrap(), None);
    assert!(sender.send_datagram(b.id, frame(Recipient::Acceptor, b"x")).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datagrams_cross_a_relay_when_there_is_no_direct_path() {
    let (a, r, b) = (Node::start(1), Node::start(9), Node::start(2));
    a.block(&[&b]);
    r.apply(&View { relay_clients: Some(&[&a, &b]), ..View::default() });
    r.wait_tcp_listening().await;
    a.apply(&View { peers: &[Peer::both(&b, &[&r])], relays: &[&r], ..View::default() });
    b.apply(&View { peers: &[Peer::both(&a, &[&r])], relays: &[&r], ..View::default() });
    b.spawn_echo();
    echo_once(&a, &b, b"connect").await;
    assert_eq!(a.route_to(&b), Route::Relay { relays: vec![r.id] });
    let forwarded_before = r.endpoint.relay_counters().forwarded_bytes;

    let (from_a, from_b) = (a.endpoint.datagram_sender(), b.endpoint.datagram_sender());
    from_a.send_datagram(b.id, frame(Recipient::Acceptor, &[1; 1000])).unwrap();
    assert_eq!(body(&next(&b, Recipient::Acceptor).await), [1; 1000]);
    from_b.send_datagram(a.id, frame(Recipient::Initiator, &[2; 1000])).unwrap();
    assert_eq!(body(&next(&a, Recipient::Initiator).await), [2; 1000]);
    from_a.send_reliable(b.id, frame(Recipient::Acceptor, &[3; 20_000])).await.unwrap();
    assert_eq!(body(&next(&b, Recipient::Acceptor).await), vec![3; 20_000]);

    // 数据报是连接的一部分：relay 转发的还是连接的包，看不到里面是流还是数据报。
    // relay 是先把包转出去、再记数的，所以对面收到的那一刻它可能还没记完，等一下。
    let forwarded = || r.endpoint.relay_counters().forwarded_bytes - forwarded_before;
    let deadline = std::time::Instant::now() + SOON;
    while forwarded() <= 22_000 {
        assert!(std::time::Instant::now() < deadline, "两个数据报和一个走单向流的数据报都应该经过 relay：{}", forwarded());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reliable_datagram_may_be_as_large_as_a_body_can_be() {
    let (a, b) = connected().await;
    let sender = a.endpoint.datagram_sender();
    let largest: Vec<u8> = (0..MAX_DATAGRAM_BODY_LEN).map(|index| u8::try_from(index % 251).unwrap()).collect();
    sender.send_reliable(b.id, frame(Recipient::Initiator, &largest)).await.unwrap();
    assert_eq!(body(&next(&b, Recipient::Initiator).await), largest);

    // 再长就不是一个合法的数据报了，对端读到上限就不要了。之后的照常收。
    sender.send_reliable(b.id, frame(Recipient::Initiator, &vec![0; MAX_DATAGRAM_BODY_LEN + 1])).await.unwrap();
    sender.send_reliable(b.id, frame(Recipient::Initiator, b"after")).await.unwrap();
    assert_eq!(body(&next(&b, Recipient::Initiator).await), b"after");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reliable_datagrams_all_arrive_under_packet_loss_while_plain_ones_may_be_lost() {
    const COUNT: u32 = 300;
    let (a, b) = connected().await;
    let sender = a.endpoint.datagram_sender();

    // A 发出去的包每五个丢一个。
    let sent = Arc::new(AtomicU64::new(0));
    a.set_filter(Some(Arc::new(move |direction, _, _: &[u8]| {
        direction == Direction::Inbound || sent.fetch_add(1, Ordering::Relaxed) % 5 != 4
    })));

    for index in 0..COUNT {
        sender.send_datagram(b.id, frame(Recipient::Acceptor, &index.to_be_bytes())).unwrap();
        // 一口气塞进去的话它们会挤在同一批包里，要丢一起丢；隔开一点，丢的就是其中一部分。
        if index % 10 == 9 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    for index in 0..COUNT {
        tokio::time::timeout(SOON, sender.send_reliable(b.id, frame(Recipient::Initiator, &index.to_be_bytes()))).await.unwrap().unwrap();
    }

    let mut reliable = BTreeSet::new();
    while reliable.len() < usize::try_from(COUNT).unwrap() {
        let datagram = tokio::time::timeout(3 * SOON, b.endpoint.recv_datagram(Recipient::Initiator)).await;
        let datagram = datagram.unwrap_or_else(|_| panic!("保证送达的数据报只到了 {} 个", reliable.len())).unwrap();
        assert!(reliable.insert(u32::from_be_bytes(body(&datagram).try_into().unwrap())), "同一个数据报到了两次");
    }
    assert_eq!(reliable, (0..COUNT).collect());

    // 不保证送达的：到了的都是原样的，但不是每个都到。等保证送达的那批收齐，它们该到的早就到了。
    let mut plain = BTreeSet::new();
    while let Ok(Some(datagram)) = tokio::time::timeout(Duration::from_millis(200), b.endpoint.recv_datagram(Recipient::Acceptor)).await {
        assert!(plain.insert(u32::from_be_bytes(body(&datagram).try_into().unwrap())));
    }
    assert!(plain.iter().all(|index| *index < COUNT));
    assert!(!plain.is_empty() && plain.len() < usize::try_from(COUNT).unwrap(), "丢了一部分，不是全丢也不是全到：到了 {} 个", plain.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datagrams_nobody_collects_are_dropped_at_a_bound_and_never_block_the_connection() {
    let (a, b) = connected().await;
    let sender = a.endpoint.datagram_sender();
    // B 上没有人取给接受方的数据报。发的比队列能排的多得多。
    for index in 0..5_000u32 {
        sender.send_datagram(b.id, frame(Recipient::Acceptor, &index.to_be_bytes())).unwrap();
        if index % 100 == 99 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    // 连接照常工作：流通，给另一侧的数据报也通。
    echo_once(&a, &b, &[9; 100_000]).await;
    sender.send_datagram(b.id, frame(Recipient::Initiator, b"other side")).unwrap();
    assert_eq!(body(&next(&b, Recipient::Initiator).await), b"other side");

    // 排着的不超过队列的上限，而且是先到的那些。
    let mut queued = 0;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(100), b.endpoint.recv_datagram(Recipient::Acceptor)).await {
        queued += 1;
    }
    assert!((1..=1024).contains(&queued), "{queued}");
}
