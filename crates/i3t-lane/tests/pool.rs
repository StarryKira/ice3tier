//! 停车的 lane：取用、抽干、补拨、等待和放弃；控制 lane 断了又建；两边重启；不认识的公钥；赖着不握手的连接。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Node, Pair, accept_next, id, quick, read_to_end, wait_for};
use i3t_conn::SendStream as _;
use i3t_lane::{OpenLaneError, Payload};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};

/// 入口开一条流到出口，出口回一句话，确认这条 lane 是通的。
async fn round_trip(pair: &Pair) {
    let ((mut send, mut recv), (mut far_send, mut far_recv)) = pair.entry_opens(b"hello", Payload::Sealed).await;
    far_send.write_all(b"world").await.unwrap();
    far_send.shutdown().await.unwrap();
    send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b"world");
    assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"");
}

#[tokio::test]
async fn a_drained_pool_asks_for_a_burst_and_waiting_claims_are_served() {
    let pair = Pair::new(quick()).await;
    pair.ready(4).await;
    let before = pair.entry.lanes.status();
    // 一下子要 60 条，池里只有 4 条：其余的要等对方补拨。每条取到就写头，和流层一样。
    let mut claims = tokio::task::JoinSet::new();
    for _ in 0..60 {
        let lanes = Arc::clone(&pair.entry.lanes);
        let peer = pair.exit.id;
        claims.spawn(async move {
            let (mut send, recv) = lanes.open_stream(peer).await?;
            send.write_all(b"header").await.unwrap();
            send.end_header(Payload::Sealed);
            Ok::<_, OpenLaneError>((send, recv))
        });
    }
    let mut held = Vec::new();
    while let Some(claim) = timeout(Duration::from_secs(10), claims.join_next()).await.unwrap() {
        held.push(claim.unwrap().unwrap());
    }
    let status = pair.entry.lanes.status();
    assert_eq!(status.total.claims - before.total.claims, 60);
    assert!(status.total.claims_waited > before.total.claims_waited, "池被抽干时有人要等：{status:?}");
    assert_eq!(status.total.claims_failed, 0);
    assert_eq!(status.total.in_use, 60, "取走的 lane 都在用：{status:?}");
    // 补拨之后池又满了，而且刚才那一阵让目标比最小值大。
    wait_for("池补回来", Duration::from_secs(5), || pair.entry.lanes.status().idle() > 4).await;
    for _ in 0..60 {
        drop(accept_next(&pair.exit.lanes).await);
    }
    drop(held);
}

#[tokio::test]
async fn a_claim_waits_two_seconds_on_an_empty_pool_and_then_fails_distinctly() {
    let pair = Pair::new(quick()).await;
    pair.ready(4).await;
    // 出口从此拨不进来：池用完了就补不上。
    pair.hop.refuse_new(true);
    let mut held = Vec::new();
    while pair.entry.lanes.status().idle() > 0 {
        held.push(pair.entry.lanes.open_stream(pair.exit.id).await.unwrap());
    }
    let started = Instant::now();
    let error = pair.entry.lanes.open_stream(pair.exit.id).await.unwrap_err();
    let waited = started.elapsed();
    assert!(matches!(error, OpenLaneError::PoolDry { .. }), "{error}");
    assert!((Duration::from_millis(1900)..Duration::from_millis(3000)).contains(&waited), "等了 {waited:?}");
    assert!(matches!(error.to_open_error(), i3t_conn::OpenError::Timeout(_)), "通过连接层契约看到的是超时");
    assert_eq!(pair.entry.lanes.status().total.claims_failed, 1);
    // 出口又能拨进来之后，池补上，取用照常。
    pair.hop.refuse_new(false);
    pair.ready(1).await;
    round_trip(&pair).await;
}

#[tokio::test]
async fn a_lost_control_lane_takes_the_idle_lanes_with_it_and_is_reestablished() {
    let pair = Pair::new(quick()).await;
    pair.ready(4).await;
    let before = pair.hop.connections();
    // 现有的连接（控制 lane 和停着的 lane）全部装死：不转发，也不关。
    pair.hop.freeze_all();
    // 两边都在三个保活周期（600 毫秒）之后发现；出口随即换新的 epoch 重拨，入口扔掉旧的空闲 lane。
    wait_for("出口重拨控制 lane", Duration::from_secs(3), || pair.hop.connections() > before).await;
    pair.ready(4).await;
    // 入口取到的必须是新的 lane：旧的装死，在上面开流会一直挂着。
    for _ in 0..8 {
        timeout(Duration::from_secs(3), round_trip(&pair)).await.expect("取到了一条装死的旧 lane");
    }
}

#[tokio::test]
async fn a_restarted_dialler_replaces_its_lanes() {
    let mut pair = Pair::new(quick()).await;
    pair.ready(4).await;
    let before = pair.hop.connections();
    pair.exit.restart(quick());
    pair.introduce();
    // 旧的出口关掉了它所有的 lane：入口看到控制 lane 断开，新的出口拨来新的。
    wait_for("新的出口拨进来", Duration::from_secs(3), || pair.hop.connections() > before).await;
    pair.ready(4).await;
    for _ in 0..8 {
        timeout(Duration::from_secs(3), round_trip(&pair)).await.expect("取到了旧出口的 lane");
    }
}

#[tokio::test]
async fn a_dialler_that_came_back_on_a_new_address_makes_its_old_idle_lanes_go_at_once() {
    // 保活周期很长：旧的 lane 被扔掉只能是因为新的 epoch，不是因为保活超时。
    let mut slow = quick();
    slow.ping_interval = Duration::from_secs(60);
    let mut pair = Pair::new(slow.clone()).await;
    pair.ready(4).await;
    let old_connections = pair.hop.connections();
    // 家宽重拨：旧地址上的连接全部无声无息地死了（不转发，也收不到重置），出口从新地址重新拨。
    pair.hop.freeze_all();
    pair.exit.restart(slow);
    pair.introduce();
    wait_for("新的控制 lane 到达", Duration::from_secs(5), || pair.hop.connections() > old_connections).await;
    // 等出口那一头的控制 lane 算建立了再往下：它要先收到入口的第一个帧，而入口是登记了新的 epoch、扔掉旧的空闲 lane 之后才发的。
    // 只等"中间那一跳看到了新连接"不够：那时入口可能还没读到它，手里仍是那四条旧的（它没法知道它们死了），`ready(4)` 马上就满足，
    // 接着取到的就是旧的。机器忙的时候这个测试因此失败过，和被测的行为无关。
    let entry = pair.entry.id;
    wait_for("出口的新控制 lane 建立", Duration::from_secs(5), || {
        pair.exit.lanes.status().peer(entry).is_some_and(|peer| peer.control)
    })
    .await;
    pair.ready(4).await;
    // 入口取到的必须是新 epoch 的 lane：旧的装死，在上面开流会一直挂着。
    for _ in 0..8 {
        timeout(Duration::from_secs(3), round_trip(&pair)).await.expect("取到了一条旧 epoch 的死 lane");
    }
}

#[tokio::test]
async fn a_restarted_accepting_side_gets_its_lanes_back() {
    let mut pair = Pair::new(quick()).await;
    pair.ready(4).await;
    let addr = pair.entry.addr.unwrap();
    pair.entry.restart(quick());
    // 端口空着的这段时间里出口拨不通，按退避重试。
    sleep(Duration::from_millis(300)).await;
    pair.entry.listen(addr).await;
    pair.introduce();
    timeout(Duration::from_secs(15), pair.ready(4)).await.expect("入口重启之后出口要重新停 lane 过来");
    round_trip(&pair).await;
}

#[tokio::test]
async fn a_node_whose_key_is_not_known_is_refused_in_the_handshake() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let stranger = Node::new(id(9), quick());
    stranger.lanes.set_peers([pair.entry.peer(vec![pair.hop.addr])]).unwrap();
    wait_for("陌生人的握手被拒绝", Duration::from_secs(5), || pair.entry.lanes.status().handshake_failures.unknown_key >= 1).await;
    assert!(!pair.entry.lanes.can_open(stranger.id));
    wait_for("陌生人知道自己被拒绝", Duration::from_secs(5), || stranger.lanes.status().total.dial_failures >= 1).await;
    // 它的控制 lane 没有被认可：不在 lane 上开流，开流请求一个字节都没发出去，调用方改走共用的连接。
    assert!(!stranger.lanes.can_open(pair.entry.id));
    assert!(matches!(stranger.lanes.open_stream(pair.entry.id).await, Err(OpenLaneError::NoControl(_))));

    // 吊销：入口不再认出口，出口的控制 lane 断了以后重拨也被拒绝。
    pair.entry.lanes.set_peers(Vec::new()).unwrap();
    let unknown = pair.entry.lanes.status().handshake_failures.unknown_key;
    wait_for("被吊销的出口的握手被拒绝", Duration::from_secs(5), || {
        pair.entry.lanes.status().handshake_failures.unknown_key > unknown
    })
    .await;
    assert_eq!(pair.entry.lanes.status().idle(), 0);
}

#[tokio::test]
async fn connections_that_never_finish_the_handshake_are_cut_at_the_deadline() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let entry = pair.entry.addr.unwrap();
    let mut silent = Vec::new();
    for _ in 0..20 {
        silent.push(TcpStream::connect(entry).await.unwrap());
    }
    // 半个 ClientHello，然后不动了。
    let mut partial = Vec::new();
    for _ in 0..5 {
        let mut stream = TcpStream::connect(entry).await.unwrap();
        stream.write_all(&[0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00]).await.unwrap();
        partial.push(stream);
    }
    let started = Instant::now();
    for mut stream in silent.into_iter().chain(partial) {
        let mut buf = [0; 16];
        let outcome = timeout(Duration::from_secs(5), stream.read(&mut buf)).await.expect("没握手的连接要在时限到了时被关掉");
        assert!(matches!(outcome, Ok(0) | Err(_)), "{outcome:?}");
    }
    let waited = started.elapsed();
    assert!(waited < Duration::from_secs(4), "等了 {waited:?}");
    assert!(pair.entry.lanes.status().handshake_failures.deadline >= 25);
    // 这期间真正的对端照常工作。
    round_trip(&pair).await;
}

#[tokio::test]
async fn a_connection_that_goes_away_in_the_middle_of_the_handshake_counts_as_a_protocol_failure() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let before = pair.entry.lanes.status().handshake_failures;
    let mut stream = TcpStream::connect(pair.entry.addr.unwrap()).await.unwrap();
    stream.write_all(&[0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00]).await.unwrap();
    drop(stream);
    wait_for("半截握手就断了的连接记在 protocol 下", Duration::from_secs(5), || {
        pair.entry.lanes.status().handshake_failures.protocol > before.protocol
    })
    .await;
    assert_eq!(pair.entry.lanes.status().handshake_failures.deadline, before.deadline, "不是等到时限才断的");
}

#[tokio::test]
async fn a_crowd_of_idle_connections_from_one_source_cannot_lock_the_dialler_out() {
    let mut config = quick();
    config.handshake_deadline = Duration::from_secs(30);
    let pair = Pair::new(config).await;
    pair.ready(1).await;
    let entry = pair.entry.addr.unwrap();
    // 同一个来源（127.0.0.1，和出口一样）开的连接比它的名额多 50 条，都不握手。
    let mut crowd = Vec::new();
    for _ in 0..306 {
        crowd.push(TcpStream::connect(entry).await.unwrap());
    }
    wait_for("最老的被挤掉", Duration::from_secs(5), || pair.entry.lanes.status().handshake_failures.busy >= 50).await;
    // 出口这时拨的新 lane 照样握手成功：挤掉的是赖着的老连接，不是新来的。连开十条，池里原来的不够用，一定有新拨的。
    let dial_failures = pair.exit.lanes.status().total.dial_failures;
    for _ in 0..10 {
        timeout(Duration::from_secs(3), round_trip(&pair)).await.unwrap();
    }
    assert_eq!(pair.exit.lanes.status().total.dial_failures, dial_failures);
    drop(crowd);
}

#[tokio::test]
async fn the_dialler_streams_go_over_its_warm_lanes_or_a_lane_dialled_on_the_spot() {
    let mut config = quick();
    config.min_idle = 1;
    let pair = Pair::new(config).await;
    pair.ready(1).await;
    sleep(Duration::from_millis(300)).await;
    assert_eq!(pair.exit.lanes.status().ready(), 0, "还没有往入口开过流时不备自己的热 lane");
    // 第一条当场拨，之后开始备热的。
    let first = pair.exit_opens(b"first", Payload::Sealed).await;
    drop(first);
    wait_for("出口的热 lane", Duration::from_secs(5), || pair.exit.lanes.status().ready() >= 1).await;
    // 第一条用热的，后面几条热的来不及补，当场拨。
    let mut streams = Vec::new();
    for _ in 0..5 {
        streams.push(pair.exit_opens(b"from exit", Payload::Sealed).await);
    }
    for ((mut send, mut recv), (mut far_send, mut far_recv)) in streams {
        send.shutdown().await.unwrap();
        far_send.write_all(b"ok").await.unwrap();
        far_send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut recv).await.unwrap(), b"ok");
        assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"");
    }
}

#[tokio::test]
async fn going_away_stops_new_streams_on_lanes_of_that_pair() {
    let pair = Pair::new(quick()).await;
    pair.ready(2).await;
    pair.exit.lanes.go_away();
    wait_for("入口知道出口要走", Duration::from_secs(3), || !pair.entry.lanes.can_open(pair.exit.id)).await;
    assert_eq!(pair.entry.lanes.status().idle(), 0);
    assert!(matches!(pair.entry.lanes.open_stream(pair.exit.id).await, Err(OpenLaneError::GoingAway(_))));
    assert!(matches!(pair.exit.lanes.open_stream(pair.entry.id).await, Err(OpenLaneError::GoingAway(_))));
}

/// 出口自己开一条流到入口，入口回一句话。
async fn round_trip_from_exit(pair: &Pair) {
    let ((mut send, mut recv), (mut far_send, mut far_recv)) = pair.exit_opens(b"hello", Payload::Sealed).await;
    far_send.write_all(b"world").await.unwrap();
    far_send.shutdown().await.unwrap();
    send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b"world");
    assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"");
}

#[tokio::test]
async fn lanes_come_back_after_a_drain_is_called_off_or_the_draining_node_restarts() {
    let mut pair = Pair::new(quick()).await;
    pair.ready(2).await;

    // 入口（接受方）排空：出口不再在 lane 上往它开流。撤销之后两边重新建立控制 lane，照常开流。
    pair.entry.lanes.go_away();
    wait_for("出口知道入口要走", Duration::from_secs(3), || !pair.exit.lanes.can_open(pair.entry.id)).await;
    pair.entry.lanes.come_back();
    timeout(Duration::from_secs(10), pair.ready(2)).await.expect("撤销排空之后入口这边的 lane 要回来");
    wait_for("出口重新能开流", Duration::from_secs(10), || pair.exit.lanes.can_open(pair.entry.id)).await;
    round_trip(&pair).await;
    round_trip_from_exit(&pair).await;

    // 出口（拨号方）排空再撤销。
    pair.exit.lanes.go_away();
    wait_for("入口知道出口要走", Duration::from_secs(3), || !pair.entry.lanes.can_open(pair.exit.id)).await;
    pair.exit.lanes.come_back();
    timeout(Duration::from_secs(10), pair.ready(2)).await.expect("撤销排空之后出口要重新停 lane 过来");
    round_trip(&pair).await;

    // 入口排空着的时候出口重启：新拨来的控制 lane 上马上听到"要走"。
    pair.entry.lanes.go_away();
    pair.exit.restart(quick());
    pair.introduce();
    wait_for("出口的控制 lane 建立", Duration::from_secs(10), || {
        pair.exit.lanes.status().peer(pair.entry.id).is_some_and(|peer| peer.control)
    })
    .await;
    sleep(Duration::from_millis(200)).await;
    assert!(!pair.exit.lanes.can_open(pair.entry.id), "排空中的入口要在新的控制 lane 上也说要走");

    // 入口更新完重启：出口对新的入口照常开流。
    let addr = pair.entry.addr.unwrap();
    pair.entry.restart(quick());
    // 旧的监听任务被取消之后才放开端口。
    sleep(Duration::from_millis(300)).await;
    pair.entry.listen(addr).await;
    pair.introduce();
    timeout(Duration::from_secs(15), pair.ready(2)).await.expect("入口重启之后出口要重新停 lane 过来");
    wait_for("出口对重启之后的入口照常开流", Duration::from_secs(10), || pair.exit.lanes.can_open(pair.entry.id)).await;
    round_trip_from_exit(&pair).await;
}

#[tokio::test]
async fn an_address_that_never_answers_does_not_hold_up_the_dial() {
    // relay 的地址有好几个，只有一部分连得上。排在前面的这个不回答握手。Linux 和 Windows 上：监听着、却从不接受连接，
    // 等着接受的队列已经满了，再来的 SYN 内核直接丢掉（只绑定、不监听的 socket 在那里会回 RST）。macOS 上队列满了照样
    // 回应握手，只绑定、不监听的 socket 却什么都不回，就用它。
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let dead = socket.local_addr().unwrap();
    let _silence: Box<dyn std::any::Any + Send> = if cfg!(target_os = "macos") {
        Box::new(socket)
    } else {
        let listener = socket.listen(0).unwrap();
        Box::new((listener, TcpStream::connect(dead).await.unwrap()))
    };
    assert!(timeout(Duration::from_millis(300), TcpStream::connect(dead)).await.is_err(), "这个地址应该不回答");

    let mut entry = Node::new(id(1), quick());
    let exit = Node::new(id(2), quick());
    let entry_addr = entry.listen("127.0.0.1:0".parse().unwrap()).await;
    entry.lanes.set_peers([exit.peer(Vec::new())]).unwrap();
    let started = Instant::now();
    exit.lanes.set_peers([entry.peer(vec![dead, entry_addr])]).unwrap();
    wait_for("控制 lane 建好", Duration::from_secs(10), || entry.lanes.can_open(exit.id)).await;
    // 原来一个接一个地连，要先等那个地址超时（3 秒）。现在和到 relay 的 TCP 中转一样错开 250 毫秒一起连，先连上的为准。
    assert!(started.elapsed() < Duration::from_millis(1500), "等了 {:?}", started.elapsed());
}
