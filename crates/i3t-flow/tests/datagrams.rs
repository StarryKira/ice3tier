//! 数据报上下文的端到端行为：发起方 ↔ 内存里的连接 ↔ 接受方。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_conn::contract::{DatagramSendError, DatagramSender};
use i3t_conn::{Acceptor, Connector, RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_flow::datagram::{Context, ContextEnd, Opener, Received, Request, SendError};
use i3t_flow::memory::{MemoryConnector, MemoryDatagrams, MemoryMesh, MemoryRecv, MemorySend};
use i3t_flow::{FlowError, Handler, OPEN_DEADLINE};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, DatagramHeader, MAX_DATAGRAM_BODY_LEN, OpenDatagrams, Origin, Reason, Reply, SplitError, Status, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::timeout;

const LAPTOP: u32 = 1;
const GW: u32 = 2;
const PATIENCE: Duration = Duration::from_secs(5);
/// 断言"这件事不会发生"时观察多久。
const QUIET: Duration = Duration::from_millis(150);

type MemoryContext = Context<MemorySend, MemoryRecv, MemoryDatagrams>;

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

fn dns() -> Target {
    Target::Internet { host: "dns.example".parse().unwrap(), port: 53 }
}

fn from(addr: &str) -> Target {
    let addr: std::net::SocketAddr = addr.parse().unwrap();
    Target::Internet { host: i3t_proto::flow::Host::Ip(addr.ip()), port: addr.port() }
}

/// 接受方的处理者：答应每个上下文，把它交给测试；或者按设定拒绝。
struct Accepting {
    refuse: Option<Reply>,
    requests: mpsc::UnboundedSender<OpenDatagrams>,
    contexts: mpsc::UnboundedSender<Box<dyn std::any::Any + Send>>,
}

impl Handler for Accepting {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: i3t_flow::Request<S, R>) {
        request.refuse(&Reply::new(Status::NotEnabled, "这个替身只接受数据报上下文")).await;
    }

    async fn handle_datagrams<S: SendStream, R: RecvStream, T: DatagramSender>(&self, request: Request<S, R, T>) {
        self.requests.send(request.open().clone()).unwrap();
        match &self.refuse {
            Some(reply) => request.refuse(reply).await,
            None => {
                // 处理者对流的类型是泛型的，测试知道它们其实是内存里的那几种，取出来的时候再认回去。
                // 没接成（上下文号重复）时流已经被重置，没有东西可交。
                if let Ok(context) = request.accept().await {
                    self.contexts.send(Box::new(context)).unwrap();
                }
            }
        }
    }
}

/// 两个节点：laptop 开上下文，gw 接受。两边都把数据报的分发跑起来了。
struct World {
    mesh: MemoryMesh,
    opener: Arc<Opener<MemoryConnector>>,
    /// laptop 的开流端，用来在流上直接写东西。
    raw: MemoryConnector,
    /// gw 收到的 OPEN，按到达的顺序。
    requests: mpsc::UnboundedReceiver<OpenDatagrams>,
    accepted: mpsc::UnboundedReceiver<Box<dyn std::any::Any + Send>>,
}

impl World {
    fn new(refuse: Option<Reply>) -> Self {
        let mesh = MemoryMesh::new();
        let (connector, _) = mesh.join(node(LAPTOP));
        let (_, acceptor) = mesh.join(node(GW));
        let (requests_sender, requests) = mpsc::unbounded_channel();
        let (contexts, accepted) = mpsc::unbounded_channel();
        let handler = Arc::new(Accepting { refuse, requests: requests_sender, contexts });
        tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });

        let opener = Arc::new(Opener::new(Arc::new(connector.clone())));
        tokio::spawn({
            let opener = Arc::clone(&opener);
            async move { opener.run().await }
        });
        Self { mesh, opener, raw: connector, requests, accepted }
    }

    /// 开一个上下文，返回两头：(laptop 这一头, gw 那一头)。
    async fn context(&mut self, class: Class) -> (MemoryContext, MemoryContext) {
        let near = timeout(PATIENCE, self.opener.open(node(GW), class, Origin::default())).await.unwrap().unwrap();
        let far = timeout(PATIENCE, self.accepted.recv()).await.unwrap().unwrap();
        (near, *far.downcast::<MemoryContext>().unwrap())
    }

    /// 让内存里的网每 `nth` 个不保证送达的数据报丢一个，返回一共经过了多少个。
    fn lose_every(&self, nth: usize) -> Arc<AtomicUsize> {
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        self.mesh.set_datagram_filter(Some(Box::new(move |_, _, _| counter.fetch_add(1, Ordering::Relaxed) % nth != nth - 1)));
        seen
    }
}

async fn next(context: &mut MemoryContext) -> Received {
    timeout(PATIENCE, context.recv()).await.expect("数据报应该很快就到").expect("上下文不应该结束")
}

async fn end_of(context: &mut MemoryContext) -> ContextEnd {
    timeout(PATIENCE, context.recv()).await.expect("上下文应该很快结束").expect_err("上下文应该结束了，而不是又来了数据报")
}

async fn assert_quiet(context: &mut MemoryContext) {
    assert!(timeout(QUIET, context.recv()).await.is_err(), "不应该有数据报，上下文也不应该结束");
}

fn payload(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|index| seed.wrapping_add(u8::try_from(index % 251).unwrap())).collect()
}

#[tokio::test]
async fn a_context_carries_datagrams_both_ways_each_with_its_own_target() {
    let mut world = World::new(None);
    let origin = Origin { credential: Some("gw-cred".into()), client: Some("192.0.2.9:5000".parse().unwrap()) };
    let mut near = world.opener.open(node(GW), Class::Interactive, origin.clone()).await.unwrap();
    let mut far = *world.accepted.recv().await.unwrap().downcast::<MemoryContext>().unwrap();

    // 接受方看到了等级和来源说明，也知道是谁开的。
    let request = world.requests.recv().await.unwrap();
    assert_eq!((request.class, request.origin), (Class::Interactive, origin));
    assert_eq!((far.peer(), far.class(), near.peer()), (node(LAPTOP), Class::Interactive, node(GW)));

    // 同一个上下文里的数据报各去各的目标。
    let lan = Target::Lan { addr: "192.168.1.10:161".parse().unwrap() };
    near.sender().send(&dns(), b"query").await.unwrap();
    near.sender().send(&lan, b"snmp").await.unwrap();
    near.sender().send(&Target::Node { port: 5353 }, b"").await.unwrap();
    assert_eq!(next(&mut far).await, Received { target: dns(), payload: "query".into() });
    assert_eq!(next(&mut far).await, Received { target: lan, payload: "snmp".into() });
    assert_eq!(next(&mut far).await, Received { target: Target::Node { port: 5353 }, payload: "".into() }, "空的报文也是报文");

    // 回来的数据报带的是它的来源。
    far.sender().send(&from("203.0.113.53:53"), b"answer").await.unwrap();
    assert_eq!(next(&mut near).await, Received { target: from("203.0.113.53:53"), payload: "answer".into() });
}

#[tokio::test]
async fn a_datagram_larger_than_the_connection_allows_is_fragmented_and_reassembled() {
    let mut world = World::new(None);
    world.mesh.set_max_datagram_len(300);
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&sizes);
    world.mesh.set_datagram_filter(Some(Box::new(move |_, _, frame| {
        seen.lock().unwrap().push(frame.len());
        true
    })));
    let (near, mut far) = world.context(Class::Interactive).await;

    // 一个小的不用切；一个 5000 字节的要切成 18 片，每片不超过连接允许的长度。
    near.sender().send(&dns(), b"small").await.unwrap();
    near.sender().send(&dns(), &payload(5000, 1)).await.unwrap();
    assert_eq!(next(&mut far).await.payload, &b"small"[..]);
    assert_eq!(next(&mut far).await, Received { target: dns(), payload: payload(5000, 1).into() });
    {
        let sizes = sizes.lock().unwrap();
        assert_eq!(sizes.len(), 1 + 18, "{sizes:?}");
        assert!(sizes.iter().all(|&len| len <= 300), "{sizes:?}");
        assert!(sizes[1..18].iter().all(|&len| len == 300), "除了最后一片都填满：{sizes:?}");
    }

    // 最长的内容也过得去，反方向也一样切。
    let mut near = near;
    let longest = payload(MAX_DATAGRAM_BODY_LEN - 7, 9);
    far.sender().send(&from("203.0.113.53:53"), &longest).await.unwrap();
    assert_eq!(next(&mut near).await, Received { target: from("203.0.113.53:53"), payload: longest.into() });
}

#[tokio::test]
async fn a_datagram_that_cannot_be_carried_is_refused_and_the_context_goes_on() {
    let mut world = World::new(None);
    let (near, mut far) = world.context(Class::Interactive).await;
    // 目标占 7 字节，再加 65529 字节的载荷就超过了内容的上限。
    let too_long = vec![0; MAX_DATAGRAM_BODY_LEN - 6];
    let error = near.sender().send(&from("203.0.113.53:53"), &too_long).await.unwrap_err();
    assert_eq!(error, SendError::Split(SplitError::BodyTooLarge { len: MAX_DATAGRAM_BODY_LEN + 1 }));

    // 连接一次只能发很短的数据报时，切成 255 片也装不下的报文发不了。
    world.mesh.set_max_datagram_len(DatagramHeader::FRAGMENT_LEN + 10);
    assert!(matches!(near.sender().send(&dns(), &[0; 4000]).await, Err(SendError::Split(SplitError::FrameTooSmall { .. }))));
    near.sender().send(&dns(), &payload(2000, 3)).await.unwrap();
    assert_eq!(next(&mut far).await.payload, payload(2000, 3));
}

#[tokio::test]
async fn losing_one_fragment_loses_that_datagram_only() {
    let mut world = World::new(None);
    world.mesh.set_max_datagram_len(300);
    let (near, mut far) = world.context(Class::Interactive).await;

    // 第一个报文的第三片丢了。
    let seen = world.lose_every(1000);
    seen.store(997, Ordering::Relaxed);
    near.sender().send(&dns(), &payload(2000, 1)).await.unwrap();
    assert_quiet(&mut far).await;

    // 后面的报文照常到，前一个报文留下的那几片不会混进来。
    near.sender().send(&dns(), &payload(2000, 2)).await.unwrap();
    near.sender().send(&dns(), b"whole").await.unwrap();
    assert_eq!(next(&mut far).await.payload, payload(2000, 2));
    assert_eq!(next(&mut far).await.payload, &b"whole"[..]);
}

#[tokio::test]
async fn every_class_but_interactive_delivers_every_datagram_under_loss() {
    const COUNT: u32 = 200;
    let mut world = World::new(None);
    world.mesh.set_max_datagram_len(300);
    let (fresh, mut fresh_far) = world.context(Class::Interactive).await;
    let mut kept = Vec::new();
    for class in [Class::Auto, Class::Reliable, Class::Bulk] {
        kept.push((class, world.context(class).await));
    }
    world.lose_every(3);

    for index in 0..COUNT {
        // 每隔几个发一个要切片的大报文。
        let len = if index % 10 == 0 { 1500 } else { 40 };
        let mut datagram = index.to_be_bytes().to_vec();
        datagram.resize(len, 0x55);
        fresh.sender().send(&dns(), &datagram).await.unwrap();
        for (_, (near, _)) in &kept {
            near.sender().send(&dns(), &datagram).await.unwrap();
        }
    }

    // 默认的等级（auto）、reliable 和 bulk：一个不少，内容原样，大的也没有被切坏。
    for (class, (_, far)) in &mut kept {
        let mut arrived = Vec::new();
        for _ in 0..COUNT {
            let received = next(far).await;
            let index = u32::from_be_bytes(received.payload[..4].try_into().unwrap());
            assert_eq!(received.payload.len(), if index % 10 == 0 { 1500 } else { 40 }, "{class:?}");
            arrived.push(index);
        }
        arrived.sort_unstable();
        assert_eq!(arrived, (0..COUNT).collect::<Vec<_>>(), "{class:?}");
        assert_quiet(far).await;
    }

    // interactive：丢了一部分。切成六片的大报文只要丢一片就整个没了。
    let mut survived = 0;
    while let Ok(Ok(received)) = timeout(QUIET, fresh_far.recv()).await {
        assert!(u32::from_be_bytes(received.payload[..4].try_into().unwrap()) < COUNT);
        survived += 1;
    }
    assert!((1..COUNT).contains(&survived), "不保证送达的数据报到了 {survived} 个");
}

#[tokio::test]
async fn closing_a_context_is_a_clean_end_on_the_other_side_after_the_datagrams_already_there() {
    let mut world = World::new(None);
    let (near, mut far) = world.context(Class::Auto).await;
    near.sender().send(&dns(), b"last words").await.unwrap();
    // 数据报和流各走各的，没有先后：还在路上的数据报可能输给紧跟着的"结束"。等它到了再结束。
    tokio::time::sleep(QUIET).await;
    near.close().await;
    // 已经到了的数据报先交完，然后才是结束。
    assert_eq!(next(&mut far).await.payload, &b"last words"[..]);
    assert!(matches!(end_of(&mut far).await, ContextEnd::Finished));

    // 接受方先结束也一样（它那边空闲回收时就是这样）。
    let (mut near, far) = world.context(Class::Auto).await;
    far.close().await;
    assert!(matches!(end_of(&mut near).await, ContextEnd::Finished));
}

#[tokio::test]
async fn aborting_or_dropping_a_context_is_an_abnormal_end_with_the_reason_code() {
    let mut world = World::new(None);
    let (mut near, far) = world.context(Class::Auto).await;
    far.abort(Reason::AccessRevoked);
    let ContextEnd::Aborted(aborted) = end_of(&mut near).await else { panic!("应该是异常结束") };
    assert_eq!(aborted, StreamAborted::Reset(StreamCode(Reason::AccessRevoked.to_wire())));

    // 没有正常结束就丢弃：对面看到的是重置，不是正常结束。
    let (near, mut far) = world.context(Class::Auto).await;
    drop(near);
    assert!(matches!(end_of(&mut far).await, ContextEnd::Aborted(StreamAborted::Reset(_))));
}

#[tokio::test]
async fn losing_the_connection_ends_the_context_and_later_sends_fail() {
    let mut world = World::new(None);
    let (mut near, mut far) = world.context(Class::Auto).await;
    let (sender, reliable) = (near.sender(), world.context(Class::Reliable).await.0);
    world.mesh.partition(node(LAPTOP), node(GW));

    assert!(matches!(end_of(&mut near).await, ContextEnd::Lost(_)));
    assert!(matches!(end_of(&mut far).await, ContextEnd::Lost(_)));
    let not_connected = SendError::Transport(DatagramSendError::NotConnected(node(GW)));
    assert_eq!(sender.send(&dns(), b"x").await, Err(not_connected.clone()));
    assert_eq!(reliable.sender().send(&dns(), b"x").await, Err(not_connected));
    // 连不上的时候也开不出新的上下文。
    assert!(matches!(world.opener.open(node(GW), Class::Auto, Origin::default()).await, Err(FlowError::Connect(_))));
}

#[tokio::test]
async fn a_refused_context_reports_what_the_other_side_said() {
    let world = World::new(Some(Reply::new(Status::AccessDenied, "访问规则不允许")));
    let refused = world.opener.open(node(GW), Class::Auto, Origin::default()).await.unwrap_err();
    assert!(matches!(refused, FlowError::Refused(reply) if reply == Reply::new(Status::AccessDenied, "访问规则不允许")));
}

/// 只会处理流的处理者，没有写 `handle_datagrams`。
struct StreamsOnly;

impl Handler for StreamsOnly {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: i3t_flow::Request<S, R>) {
        request.refuse(&Reply::new(Status::InternalError, "用不到")).await;
    }
}

#[tokio::test]
async fn a_handler_that_only_knows_streams_refuses_datagram_contexts_by_default() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(LAPTOP));
    let (_, acceptor) = mesh.join(node(GW));
    tokio::spawn(async move { i3t_flow::serve(&acceptor, Arc::new(StreamsOnly)).await });
    let opener = Opener::new(Arc::new(connector));
    let refused = opener.open(node(GW), Class::Auto, Origin::default()).await.unwrap_err();
    assert!(matches!(refused, FlowError::Refused(reply) if reply.status == Status::NotEnabled));
}

#[tokio::test]
async fn datagrams_for_a_context_that_has_ended_reach_nobody_and_contexts_do_not_mix() {
    let mut world = World::new(None);
    let (first, mut first_far) = world.context(Class::Auto).await;
    let (second, mut second_far) = world.context(Class::Auto).await;
    let stale = first.sender();

    first.sender().send(&dns(), b"first").await.unwrap();
    second.sender().send(&dns(), b"second").await.unwrap();
    assert_eq!(next(&mut second_far).await.payload, &b"second"[..]);
    assert_eq!(next(&mut first_far).await.payload, &b"first"[..]);

    // 第一个上下文结束之后，拿着旧句柄发的数据报没有人收，也不会跑到别的上下文里去。
    first.close().await;
    assert!(matches!(end_of(&mut first_far).await, ContextEnd::Finished));
    drop(first_far);
    stale.send(&dns(), b"too late").await.unwrap();
    second.sender().send(&dns(), b"still here").await.unwrap();
    assert_eq!(next(&mut second_far).await.payload, &b"still here"[..]);
    assert_quiet(&mut second_far).await;
}

#[tokio::test]
async fn both_nodes_may_open_contexts_to_each_other_with_the_same_numbers() {
    // 两个节点各自既开上下文、又接受上下文：上下文号各数各的，都从 0 开始，靠"给哪一侧"分开。
    let mesh = MemoryMesh::new();
    let mut sides = Vec::new();
    let (requests, _requests) = mpsc::unbounded_channel();
    for raw in [LAPTOP, GW] {
        let (connector, acceptor) = mesh.join(node(raw));
        let (contexts, accepted) = mpsc::unbounded_channel();
        let handler = Arc::new(Accepting { refuse: None, requests: requests.clone(), contexts });
        tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });
        let opener = Arc::new(Opener::new(Arc::new(connector)));
        tokio::spawn({
            let opener = Arc::clone(&opener);
            async move { opener.run().await }
        });
        sides.push((opener, accepted));
    }
    let (laptop, mut laptop_accepted) = sides.remove(0);
    let (gw, mut gw_accepted) = sides.remove(0);

    let mut from_laptop = laptop.open(node(GW), Class::Auto, Origin::default()).await.unwrap();
    let mut at_gw = *gw_accepted.recv().await.unwrap().downcast::<MemoryContext>().unwrap();
    let mut from_gw = gw.open(node(LAPTOP), Class::Auto, Origin::default()).await.unwrap();
    let mut at_laptop = *laptop_accepted.recv().await.unwrap().downcast::<MemoryContext>().unwrap();

    from_laptop.sender().send(&dns(), b"laptop asks").await.unwrap();
    from_gw.sender().send(&dns(), b"gw asks").await.unwrap();
    assert_eq!(next(&mut at_gw).await.payload, &b"laptop asks"[..]);
    assert_eq!(next(&mut at_laptop).await.payload, &b"gw asks"[..]);
    at_gw.sender().send(&dns(), b"gw answers").await.unwrap();
    at_laptop.sender().send(&dns(), b"laptop answers").await.unwrap();
    assert_eq!(next(&mut from_laptop).await.payload, &b"gw answers"[..]);
    assert_eq!(next(&mut from_gw).await.payload, &b"laptop answers"[..]);
    for context in [&mut from_laptop, &mut from_gw, &mut at_gw, &mut at_laptop] {
        assert_quiet(context).await;
    }
}

/// 在一条新的流上直接写一个数据报上下文的 OPEN，返回流的两个方向。
async fn raw_open(connector: &MemoryConnector, context: u32) -> (MemorySend, MemoryRecv) {
    let (mut send, recv) = connector.open(node(GW)).await.unwrap();
    let mut bytes = Vec::new();
    OpenDatagrams { context, class: Class::Auto, origin: Origin::default() }.encode(&mut bytes);
    send.write_all(&bytes).await.unwrap();
    (send, recv)
}

#[tokio::test]
async fn a_context_number_that_is_still_in_use_is_refused_with_a_protocol_error() {
    let mut world = World::new(None);
    let (_first_send, mut first_recv) = raw_open(&world.raw, 7).await;
    let mut reply = [0xff; 2];
    first_recv.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [0x00, 0], "第一个用 7 号的被答应了");
    let _first = world.accepted.recv().await.unwrap();

    // 第二个用同一个号的：流被重置，原因是协议错误。
    let (_second_send, mut second_recv) = raw_open(&world.raw, 7).await;
    let error = timeout(PATIENCE, second_recv.read(&mut reply)).await.unwrap().unwrap_err();
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(Reason::ProtocolError.to_wire()))));
}

#[tokio::test]
async fn reading_a_stream_request_rejects_a_datagram_open() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(LAPTOP));
    let (_, acceptor) = mesh.join(node(GW));
    let (_send, mut recv) = raw_open(&connector, 1).await;
    let incoming = acceptor.accept().await.unwrap();
    assert!(matches!(i3t_flow::Request::read(incoming, OPEN_DEADLINE).await, Err(FlowError::NotAStream)));
    let error = recv.read(&mut [0; 2]).await.unwrap_err();
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(Reason::ProtocolError.to_wire()))));
}
