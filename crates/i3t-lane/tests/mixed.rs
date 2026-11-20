//! 共用的连接和 lane 装在一起（`LaneConnector`、`LaneAcceptor`）：哪条流走哪边；取不到 lane 时悄悄改走共用的连接、
//! 只记一笔；设置变了，跑着的流不换。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{Pair, read_to_end, wait_for};
use i3t_conn::{Acceptor as _, Connector as _, RecvStream as _, SendStream as _};
use i3t_crypto::NodeKey;
use i3t_flow::memory::{MemoryAcceptor, MemoryConnector, MemoryMesh};
use i3t_lane::{EitherRecv, EitherSend, LaneAcceptor, LaneConfig, LaneConnector, Lanes, Payload};
use i3t_proto::NodeId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::time::timeout;

type Send = EitherSend<i3t_flow::memory::MemorySend>;
type Recv = EitherRecv<i3t_flow::memory::MemoryRecv>;

/// 一边开流、一边接受：`from` 的连接层（共用的连接是内存里的）开到 `to`。
struct Side {
    choose: Arc<AtomicBool>,
    connector: LaneConnector<MemoryConnector>,
    acceptor: LaneAcceptor<MemoryAcceptor>,
    from: NodeId,
    to: NodeId,
    _unused: (MemoryAcceptor, MemoryConnector),
}

impl Side {
    fn new(mesh: &MemoryMesh, from: (NodeId, &Arc<Lanes>), to: (NodeId, &Arc<Lanes>)) -> Self {
        let (connector, unused_acceptor) = mesh.join(from.0);
        let (unused_connector, acceptor) = mesh.join(to.0);
        let choose = Arc::new(AtomicBool::new(true));
        let chosen = Arc::clone(&choose);
        Self {
            connector: LaneConnector::new(Arc::new(connector), Arc::clone(from.1), Arc::new(move |_| chosen.load(Ordering::Relaxed))),
            acceptor: LaneAcceptor::new(Arc::new(acceptor), Arc::clone(to.1)),
            choose,
            from: from.0,
            to: to.0,
            _unused: (unused_acceptor, unused_connector),
        }
    }

    /// 开一条 TCP 流，写下 `first`，对端接到它。返回 (这边的两个方向, 对端的两个方向)。
    async fn open(&self, first: &[u8]) -> ((Send, Recv), (Send, Recv)) {
        let (mut send, recv) = self.connector.open_tcp(self.to).await.unwrap();
        send.write_all(first).await.unwrap();
        send.end_header(Payload::Sealed);
        send.flush().await.unwrap();
        let incoming = timeout(Duration::from_secs(5), self.acceptor.accept()).await.expect("5 秒之内对端没有接到流").unwrap();
        assert_eq!(incoming.peer, self.from);
        assert_eq!(incoming.send.is_lane(), send.is_lane(), "两头在同一种传输上");
        ((send, recv), (incoming.send, incoming.recv))
    }

    /// 一条流来回一次，返回它是不是走的 lane。
    async fn round_trip(&self) -> bool {
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = self.open(b"ping").await;
        let on_lane = send.is_lane();
        send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"ping");
        far_send.write_all(b"pong").await.unwrap();
        far_send.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut recv).await.unwrap(), b"pong");
        on_lane
    }
}

fn quick_claims() -> LaneConfig {
    let mut config = common::quick();
    config.claim_wait = Duration::from_millis(300);
    config
}

fn fallbacks(lanes: &Lanes, peer: NodeId) -> u64 {
    lanes.status().peer(peer).map_or(0, |status| status.counts.fallbacks)
}

#[tokio::test]
async fn only_chosen_tcp_streams_take_a_lane_and_other_streams_stay_on_the_shared_connection() {
    let pair = Pair::new(common::quick()).await;
    pair.ready(2).await;
    let mesh = MemoryMesh::new();
    let side = Side::new(&mesh, (pair.entry.id, &pair.entry.lanes), (pair.exit.id, &pair.exit.lanes));

    assert!(side.round_trip().await, "选了 lane 的 TCP 流走 lane");
    // 没选 lane 的对端（例如两边都不能被拨）走共用的连接，不算"改走"。
    side.choose.store(false, Ordering::Relaxed);
    assert!(!side.round_trip().await);
    assert_eq!(fallbacks(&pair.entry.lanes, pair.exit.id), 0);
    // 别的流（例如数据报上下文）一律走共用的连接，选了 lane 也一样。
    side.choose.store(true, Ordering::Relaxed);
    let (send, _recv) = side.connector.open(pair.exit.id).await.unwrap();
    assert!(!send.is_lane());
    let incoming = timeout(Duration::from_secs(5), side.acceptor.accept()).await.unwrap().unwrap();
    assert!(!incoming.send.is_lane());
}

#[tokio::test]
async fn a_dry_pool_sends_the_stream_over_the_shared_connection_and_counts_it() {
    let pair = Pair::new(quick_claims()).await;
    pair.ready(2).await;
    // 出口不能再补拨：停着的用完之后池就干了。
    pair.hop.refuse_new(true);
    let mesh = MemoryMesh::new();
    let side = Side::new(&mesh, (pair.entry.id, &pair.entry.lanes), (pair.exit.id, &pair.exit.lanes));

    // 一直开流并且拿着不放，直到有一条开不出 lane。
    let mut held = Vec::new();
    let fell_back = loop {
        let opened = side.open(b"hold").await;
        if !opened.0.0.is_lane() {
            break opened;
        }
        held.push(opened);
        assert!(held.len() <= 64, "池一直没干");
    };
    assert!(!held.is_empty(), "干之前停着的 lane 都用上了");
    assert_eq!(fallbacks(&pair.entry.lanes, pair.exit.id), 1, "改走共用连接的流记一笔");
    let counts = pair.entry.lanes.status().peer(pair.exit.id).unwrap().counts;
    assert!(counts.claims_failed >= 1 && counts.claims_waited >= 1, "{counts:?}");

    // 改走的那一条照常把数据送到。
    let ((mut send, mut recv), (mut far_send, mut far_recv)) = fell_back;
    send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"hold");
    far_send.write_all(b"done").await.unwrap();
    far_send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b"done");
}

#[tokio::test]
async fn when_the_peer_refuses_our_lanes_streams_quietly_use_the_shared_connection() {
    let pair = Pair::new(common::quick()).await;
    // 入口不认识出口的公钥：出口拨的 lane 在握手时就被拒绝，控制 lane 建不起来。
    pair.entry.lanes.set_peers([]).unwrap();
    wait_for("入口拒绝了出口的握手", Duration::from_secs(5), || pair.entry.lanes.status().handshake_failures.unknown_key > 0).await;
    // 出口要经中间一跳读到入口的拒绝，才知道这次拨失败了，比入口记下拒绝晚一些。
    wait_for("出口记下拨控制 lane 失败", Duration::from_secs(5), || {
        pair.exit.lanes.status().peer(pair.entry.id).is_some_and(|status| status.counts.dial_failures > 0)
    })
    .await;
    let mesh = MemoryMesh::new();
    // 出口自己开流：用它自己拨的 lane，开不出来。
    let side = Side::new(&mesh, (pair.exit.id, &pair.exit.lanes), (pair.entry.id, &pair.entry.lanes));
    assert!(!side.round_trip().await);
    assert!(!side.round_trip().await);
    assert_eq!(fallbacks(&pair.exit.lanes, pair.entry.id), 2);
}

#[tokio::test]
async fn turning_lanes_off_and_on_never_moves_a_running_stream() {
    let pair = Pair::new(common::quick()).await;
    pair.ready(2).await;
    let mesh = MemoryMesh::new();
    let side = Side::new(&mesh, (pair.entry.id, &pair.entry.lanes), (pair.exit.id, &pair.exit.lanes));

    let ((mut send, mut recv), (mut far_send, mut far_recv)) = side.open(b"first half, ").await;
    assert!(send.is_lane());
    // 管理员把这一对改成 off：两边都不再为对方开 lane。
    side.choose.store(false, Ordering::Relaxed);
    pair.entry.lanes.set_peers_with_identities([], [(pair.exit.id, pair.exit.key.public())]).unwrap();
    pair.exit.lanes.set_peers_with_identities([], [(pair.entry.id, pair.entry.key.public())]).unwrap();
    assert!(!side.round_trip().await, "新的流走共用的连接");
    let ((mut shared_send, mut shared_recv), (mut shared_far_send, mut shared_far_recv)) = side.open(b"shared ").await;
    assert!(!shared_send.is_lane());

    // lane 上跑着的那一条不受影响，跑到自己结束。
    send.write_all(b"second half").await.unwrap();
    send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"first half, second half");
    far_send.write_all(b"answer").await.unwrap();
    far_send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b"answer");
    drop((send, recv, far_send, far_recv));
    wait_for("lane 用完就关", Duration::from_secs(3), || pair.entry.lanes.status().total.in_use == 0).await;

    // 再打开：新的流回到 lane 上，共用连接上跑着的那一条也不动。
    pair.introduce();
    side.choose.store(true, Ordering::Relaxed);
    pair.ready(2).await;
    assert!(side.round_trip().await);
    shared_send.write_all(b"still").await.unwrap();
    shared_send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut shared_far_recv).await.unwrap(), b"shared still");
    shared_far_send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut shared_recv).await.unwrap(), b"");
    assert_eq!(fallbacks(&pair.entry.lanes, pair.exit.id), 0);
}

#[tokio::test]
async fn removing_or_rotating_a_peer_closes_its_draining_lane() {
    for replacement in [None, Some(NodeKey::generate().unwrap().public())] {
        let pair = Pair::direct(common::quick()).await;
        pair.ready(2).await;
        let ((send, mut recv), (mut far_send, mut far_recv)) = pair.entry_opens(b"request", Payload::Sealed).await;

        // 只关闭 ice3proxy，存量流还可以传数据。
        pair.entry.lanes.set_peers_with_identities([], [(pair.exit.id, pair.exit.key.public())]).unwrap();
        far_send.write_all(b"r").await.unwrap();
        far_send.flush().await.unwrap();
        assert_eq!(timeout(Duration::from_secs(3), recv.read_u8()).await.unwrap().unwrap(), b'r');

        // 在撤销前让本端开始等待读取，验证吊销信号能唤醒一个已经挂起的任务。
        let (waiting, ready) = oneshot::channel();
        let local = tokio::spawn(async move {
            let end = recv.read_u8();
            tokio::pin!(end);
            tokio::select! {
                biased;
                result = &mut end => panic!("撤销前读取就结束了：{result:?}"),
                () = tokio::task::yield_now() => {}
            }
            waiting.send(()).unwrap();
            let result = end.await;
            drop(send);
            result
        });
        ready.await.unwrap();
        pair.entry.lanes.set_peers_with_identities([], replacement.map(|key| (pair.exit.id, key))).unwrap();

        let local_end = timeout(Duration::from_secs(3), local).await.expect("本端读取未被唤醒").unwrap();
        assert!(local_end.is_err(), "撤销对端后本端应读到错误，读到了 {local_end:?}");
        let remote_end = timeout(Duration::from_secs(3), far_recv.read(&mut [0; 1])).await.expect("远端没有看到连接断开");
        assert!(remote_end.is_err(), "撤销对端后远端应读到错误，读到了 {remote_end:?}");
    }
}

#[tokio::test]
async fn revocation_signals_both_raw_halves_and_the_peer_reads_a_reset() {
    for replacement in [None, Some(NodeKey::generate().unwrap().public())] {
        let pair = Pair::direct(common::quick()).await;
        pair.ready(2).await;
        let ((mut send, mut recv), (mut far_send, mut far_recv)) = pair.entry_opens(b"request", Payload::Passthrough).await;
        far_send.end_header(Payload::Passthrough);
        let write = send.take_raw().unwrap();
        let read = timeout(Duration::from_secs(3), recv.wait_raw()).await.unwrap().unwrap();
        drop((send, recv));

        pair.entry.lanes.set_peers_with_identities([], [(pair.exit.id, pair.exit.key.public())]).unwrap();
        assert!(!write.is_revoked());
        assert!(!read.is_revoked());
        pair.entry.lanes.set_peers_with_identities([], replacement.map(|key| (pair.exit.id, key))).unwrap();

        timeout(Duration::from_secs(3), async { tokio::join!(write.revoked(), read.revoked()) })
            .await
            .expect("两个原始方向都应收到吊销信号");
        assert!(write.is_revoked() && read.is_revoked());
        let socket = read.socket_handle();
        drop((read, write));
        i3t_underlay::tcp_reset::close(socket);
        let remote_end = timeout(Duration::from_secs(3), far_recv.read(&mut [0; 1])).await.expect("远端没有看到连接断开");
        assert!(remote_end.is_err(), "撤销后原始方向应读到重置，读到了 {remote_end:?}");
    }
}

#[tokio::test]
async fn accept_skips_a_queued_stream_whose_peer_has_been_revoked() {
    let pair = Pair::direct(common::quick()).await;
    pair.ready(2).await;
    let (mut send, _recv) = pair.entry.lanes.open_stream(pair.exit.id).await.unwrap();
    send.write_all(b"request").await.unwrap();
    send.end_header(Payload::Sealed);
    wait_for("出口登记了流", Duration::from_secs(3), || pair.exit.lanes.status().total.in_use == 1).await;

    pair.exit.lanes.set_peers([]).unwrap();
    // 两边都清掉旧连接池，再等新的一组空闲 lane。
    pair.entry.lanes.set_peers([]).unwrap();
    pair.introduce();
    pair.ready(2).await;
    let (_local, _remote) = pair.entry_opens(b"new request", Payload::Sealed).await;
}
