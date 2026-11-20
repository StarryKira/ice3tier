//! 流层的端到端行为：客户端 TCP ↔ 发起方 ↔ 内存里的连接 ↔ 接受方 ↔ 目标 TCP。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use i3t_conn::{Acceptor, Connector, OpenError, RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_flow::memory::{MemoryAcceptor, MemoryConnector, MemoryMesh, MemoryRecv, MemorySend};
use i3t_flow::{Carriage, End, FlowError, Handler, Request, Summary};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Open, Origin, Reason, Reply, Status, Target};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::time::timeout;

const INGRESS: u32 = 1;
const EXIT: u32 = 2;
const PATIENCE: Duration = Duration::from_secs(5);

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

fn open_node_port() -> Open {
    Open { target: Target::Node { port: 80 }, class: Class::Auto, origin: Origin::default() }
}

fn code(reason: Reason) -> StreamCode {
    StreamCode(reason.to_wire())
}

/// 一对连好的 TCP 连接：`near` 是测试扮演的那一头（客户端或者目标），`far` 交给被测的搬运。
struct TcpPair {
    near: TcpStream,
    far: TcpStream,
}

impl TcpPair {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (far, _) = listener.accept().await.unwrap();
        Self { near, far }
    }
}

/// 接受方的处理者：连到固定的目标，然后搬运；或者按设定拒绝。
struct Exit {
    target: SocketAddr,
    refuse: Option<Reply>,
    cancel: Arc<Notify>,
    summaries: mpsc::UnboundedSender<Summary>,
}

impl Handler for Exit {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        assert_eq!(request.peer(), node(INGRESS));
        if let Some(reply) = &self.refuse {
            request.refuse(reply).await;
            return;
        }
        let target = TcpStream::connect(self.target).await.unwrap();
        let flow = request.accept().await.unwrap();
        let cancel = Arc::clone(&self.cancel);
        let summary = flow
            .relay(target, async move {
                cancel.notified().await;
                Reason::AccessRevoked
            })
            .await;
        // 测试不关心这一侧的结果时，接收端已经丢弃了。
        let _ = self.summaries.send(summary);
    }
}

/// 两个节点、一个目标，接受方已经在跑。
struct World {
    mesh: MemoryMesh,
    connector: MemoryConnector,
    /// 目标一侧：测试从这里接到出口连过来的 TCP 连接。
    target: TcpListener,
    cancel: Arc<Notify>,
    exit_summaries: mpsc::UnboundedReceiver<Summary>,
}

impl World {
    async fn new() -> Self {
        Self::with_refusal(None).await
    }

    async fn with_refusal(refuse: Option<Reply>) -> Self {
        let mesh = MemoryMesh::new();
        let (connector, _) = mesh.join(node(INGRESS));
        let (_, acceptor) = mesh.join(node(EXIT));
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cancel = Arc::new(Notify::new());
        let (summaries, exit_summaries) = mpsc::unbounded_channel();
        let handler = Arc::new(Exit { target: target.local_addr().unwrap(), refuse, cancel: Arc::clone(&cancel), summaries });
        tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });
        Self { mesh, connector, target, cancel, exit_summaries }
    }

    async fn accept_target(&self) -> TcpStream {
        timeout(PATIENCE, self.target.accept()).await.expect("出口应该连到目标").unwrap().0
    }

    async fn exit_summary(&mut self) -> Summary {
        timeout(PATIENCE, self.exit_summaries.recv()).await.expect("出口一侧的搬运应该结束").unwrap()
    }
}

/// 把连接读到结束，返回读到的数据和结束的方式。
async fn drain(stream: &mut TcpStream) -> (Vec<u8>, io::Result<()>) {
    let mut data = Vec::new();
    let end = timeout(PATIENCE, stream.read_to_end(&mut data)).await.expect("连接应该结束");
    (data, end.map(drop))
}

fn assert_reset(end: &io::Result<()>) {
    let kind = end.as_ref().expect_err("连接应该是被重置的，而不是正常结束").kind();
    assert_eq!(kind, io::ErrorKind::ConnectionReset);
}

#[tokio::test]
async fn request_and_response_cross_with_half_close_from_the_client_first() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;

    let pending = i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"GET / ", Carriage::Shared).await.unwrap();
    let flow = pending.established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));

    // 客户端发完请求就关掉写的一半，然后等回复。
    client.write_all(b"HTTP/1.0\r\n\r\n").await.unwrap();
    client.shutdown().await.unwrap();

    let mut target = world.accept_target().await;
    let (request, end) = drain(&mut target).await;
    assert_eq!(request, b"GET / HTTP/1.0\r\n\r\n");
    end.expect("客户端半关闭，目标应该读到正常结束");

    // 目标这时才回复：另一个方向还开着。
    target.write_all(b"200 OK").await.unwrap();
    drop(target);

    let (response, end) = drain(&mut client).await;
    assert_eq!(response, b"200 OK");
    end.expect("回复完整，客户端应该读到正常结束");

    let ingress = ingress.await.unwrap();
    assert!(ingress.end.is_clean(), "{:?}", ingress.end);
    assert_eq!((ingress.from_tcp.bytes, ingress.from_flow.bytes), (12, 6));
    let exit = world.exit_summary().await;
    assert!(exit.end.is_clean(), "{:?}", exit.end);
    // 首批数据是随 OPEN 来的，也算进流到 TCP 的字节里。
    assert_eq!((exit.from_flow.bytes, exit.from_tcp.bytes), (18, 6));
    assert!(exit.from_tcp.first_byte.is_some(), "出口应该记下目标的首字节时间");
}

/// 划分：只有前 `left` 个字节属于这条流，内容随便。
struct FirstBytes {
    left: usize,
}

impl i3t_flow::Framing for FirstBytes {
    fn take(&mut self, bytes: &[u8]) -> Result<usize, Reason> {
        let taken = bytes.len().min(self.left);
        self.left -= taken;
        Ok(taken)
    }

    fn is_complete(&self) -> bool {
        self.left == 0
    }
}

#[tokio::test]
async fn a_framed_relay_sends_only_its_part_and_keeps_the_flow_open_until_the_answer_is_done() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow = i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"HEAD ", Carriage::Shared)
        .await
        .unwrap()
        .established()
        .await
        .unwrap();
    let ingress =
        tokio::spawn(async move { flow.relay_framed(far, future::pending(), &i3t_flow::Unmetered, &mut FirstBytes { left: 4 }).await });

    // 四个字节之后的是下一个请求，不属于这条流。
    client.write_all(b"bodyNEXT REQUEST").await.unwrap();
    let mut target = world.accept_target().await;
    let mut request = [0; 9];
    timeout(PATIENCE, target.read_exact(&mut request)).await.unwrap().unwrap();
    assert_eq!(&request, b"HEAD body");
    let mut more = [0; 1];
    let early = timeout(Duration::from_millis(300), target.read(&mut more)).await;
    assert!(early.is_err(), "这一段完整之后目标既不该再收到字节，也不该看到结束：{early:?}");

    target.write_all(b"answer").await.unwrap();
    target.shutdown().await.unwrap();
    let mut response = [0; 6];
    timeout(PATIENCE, client.read_exact(&mut response)).await.unwrap().unwrap();
    assert_eq!(&response, b"answer");
    let (rest, end) = drain(&mut target).await;
    assert!(rest.is_empty(), "{rest:?}");
    end.expect("应答发完之后流正常结束，目标读到正常结束");

    let ingress = ingress.await.unwrap();
    assert!(ingress.end.is_clean(), "{:?}", ingress.end);
    assert_eq!((ingress.from_tcp.bytes, ingress.from_flow.bytes), (4, 6));
    assert!(world.exit_summary().await.end.is_clean());
}

#[tokio::test]
async fn a_framed_part_cut_short_by_the_client_reaches_the_target_as_a_reset() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow =
        i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap().established().await.unwrap();
    let ingress =
        tokio::spawn(async move { flow.relay_framed(far, future::pending(), &i3t_flow::Unmetered, &mut FirstBytes { left: 100 }).await });

    client.write_all(b"only part").await.unwrap();
    client.shutdown().await.unwrap();
    let mut target = world.accept_target().await;
    let (received, end) = drain(&mut target).await;
    assert_eq!(received, b"only part");
    assert_reset(&end);
    assert!(matches!(ingress.await.unwrap().end, End::Tcp(ref error) if error.kind() == io::ErrorKind::UnexpectedEof));
    assert!(!world.exit_summary().await.end.is_clean());
}

#[tokio::test]
async fn half_close_from_the_target_first_leaves_the_other_direction_open() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow =
        i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap().established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));

    // 目标先说话，说完关掉写的一半。
    let mut target = world.accept_target().await;
    target.write_all(b"banner").await.unwrap();
    target.shutdown().await.unwrap();

    let (banner, end) = drain(&mut client).await;
    assert_eq!(banner, b"banner");
    end.expect("目标半关闭，客户端应该读到正常结束");

    // 客户端在这之后还能发。
    client.write_all(b"late upload").await.unwrap();
    client.shutdown().await.unwrap();
    let (upload, end) = drain(&mut target).await;
    assert_eq!(upload, b"late upload");
    end.unwrap();

    assert!(ingress.await.unwrap().end.is_clean());
    assert!(world.exit_summary().await.end.is_clean());
}

#[tokio::test]
async fn a_reset_by_the_target_reaches_the_client_as_a_reset_not_a_clean_end() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow = i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"request", Carriage::Shared)
        .await
        .unwrap()
        .established()
        .await
        .unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));

    let mut target = world.accept_target().await;
    let mut request = [0; 7];
    target.read_exact(&mut request).await.unwrap();
    target.write_all(b"half of the resp").await.unwrap();
    target.set_zero_linger().unwrap();
    drop(target);

    let (_partial, end) = drain(&mut client).await;
    assert_reset(&end);

    let exit = world.exit_summary().await;
    assert!(matches!(&exit.end, End::Tcp(error) if error.kind() == io::ErrorKind::ConnectionReset), "{:?}", exit.end);
    let ingress = ingress.await.unwrap();
    assert!(matches!(ingress.end, End::Aborted(StreamAborted::Reset(c)) if c == code(Reason::TargetReset)), "{:?}", ingress.end);
}

#[tokio::test]
async fn a_reset_by_the_client_reaches_the_target_as_a_reset() {
    let mut world = World::new().await;
    let TcpPair { near: client, far } = TcpPair::new().await;
    let flow = i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"half a requ", Carriage::Shared)
        .await
        .unwrap()
        .established()
        .await
        .unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));

    let mut target = world.accept_target().await;
    client.set_zero_linger().unwrap();
    drop(client);

    let (_partial, end) = drain(&mut target).await;
    assert_reset(&end);

    let ingress = ingress.await.unwrap();
    assert!(matches!(&ingress.end, End::Tcp(error) if error.kind() == io::ErrorKind::ConnectionReset), "{:?}", ingress.end);
    let exit = world.exit_summary().await;
    assert!(matches!(exit.end, End::Aborted(StreamAborted::Reset(c)) if c == code(Reason::ClientReset)), "{:?}", exit.end);
}

#[tokio::test]
async fn waiting_for_the_reply_reports_a_refusal_with_its_status() {
    let refusal = Reply::new(Status::PolicyDenied, "端口不允许");
    let world = World::with_refusal(Some(refusal.clone())).await;
    let pending = i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"first data", Carriage::Shared).await.unwrap();
    match pending.established().await {
        Err(FlowError::Refused(reply)) => assert_eq!(reply, refusal),
        other => panic!("应该被拒绝，实际是 {other:?}"),
    }
}

#[tokio::test]
async fn without_waiting_a_refusal_resets_the_client_and_is_still_reported_precisely() {
    let refusal = Reply::new(Status::AccessDenied, "不允许访问");
    let world = World::with_refusal(Some(refusal.clone())).await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let pending = i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap();
    let ingress = tokio::spawn(pending.relay(far));

    // 客户端以为已经连上，一直在发：对端的"别发了"会先于 REPLY 被这个方向看到，结果仍然应该是 REPLY 里的状态。
    let upload = vec![0x5a; 1 << 20];
    let _ = client.write_all(&upload).await;
    let (data, end) = drain(&mut client).await;
    assert!(data.is_empty());
    assert_reset(&end);

    match ingress.await.unwrap().end {
        End::Refused(reply) => assert_eq!(reply, refusal),
        other => panic!("应该报告对端的拒绝，实际是 {other:?}"),
    }
}

/// 不经过 [`Request`]，直接在流上扮演对端：读走 OPEN，然后由测试决定以什么顺序做什么。
async fn raw_peer(
    mesh: &MemoryMesh,
) -> (MemoryConnector, i3t_conn::Incoming<MemorySend, MemoryRecv>, i3t_flow::Pending<MemorySend, MemoryRecv>) {
    let (connector, _) = mesh.join(node(INGRESS));
    let (_, acceptor) = mesh.join(node(EXIT));
    let pending = i3t_flow::open(&connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap();
    let mut incoming = acceptor.accept().await.unwrap();
    // 目标是"节点端口"的 OPEN 正好 5 字节。
    incoming.recv.read_exact(&mut [0; 5]).await.unwrap();
    (connector, incoming, pending)
}

fn encoded(reply: &Reply) -> Vec<u8> {
    let mut bytes = Vec::new();
    reply.encode(&mut bytes);
    bytes
}

#[tokio::test]
async fn a_stop_that_overtakes_the_reply_does_not_hide_the_refusal() {
    let mesh = MemoryMesh::new();
    let (_connector, mut peer, pending) = raw_peer(&mesh).await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let ingress = tokio::spawn(pending.relay(far));

    // 对端的"别发了"先到，REPLY 过一会儿才到：真的连接上这两样东西可以分在两个包里。
    let refusal = Reply::new(Status::PolicyDenied, "端口不允许");
    peer.recv.stop(code(Reason::Cancelled));
    client.write_all(&[0x5a; 4096]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!ingress.is_finished(), "被叫停只说明对端不读了，原因要等另一个方向");
    peer.send.write_all(&encoded(&refusal)).await.unwrap();
    peer.send.shutdown().await.unwrap();

    match ingress.await.unwrap().end {
        End::Refused(reply) => assert_eq!(reply, refusal),
        other => panic!("应该报告对端的拒绝，实际是 {other:?}"),
    }
    assert_reset(&drain(&mut client).await.1);
}

#[tokio::test]
async fn a_peer_that_stops_reading_ends_the_relay_abnormally_even_after_a_complete_response() {
    let mesh = MemoryMesh::new();
    let (_connector, mut peer, pending) = raw_peer(&mesh).await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let ingress = tokio::spawn(pending.relay(far));

    // 回复完整地发完并且正常结束了，但对端不肯再读客户端的数据：客户端的上传没有送到，不能当作一切正常。
    peer.send.write_all(&encoded(&Reply::ok())).await.unwrap();
    peer.send.write_all(b"complete response").await.unwrap();
    peer.send.shutdown().await.unwrap();
    let mut response = [0; 17];
    client.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"complete response");
    peer.recv.stop(code(Reason::TargetError));
    client.write_all(b"upload that goes nowhere").await.unwrap();

    let summary = ingress.await.unwrap();
    assert!(matches!(summary.end, End::Aborted(StreamAborted::Stopped(c)) if c == code(Reason::TargetError)), "{:?}", summary.end);
    assert_eq!(summary.from_flow.bytes, 17);

    // 回复那个方向在这之前已经正常结束，客户端读到的就是正常结束；被重置的是它的上传，再写就会出错。
    let (rest, end) = drain(&mut client).await;
    assert!(rest.is_empty());
    end.expect("回复是完整的");
    let broken = timeout(PATIENCE, async {
        loop {
            if let Err(error) = client.write_all(b"more upload").await {
                break error;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("客户端的上传方向应该已经被重置");
    assert!(matches!(broken.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset), "{broken:?}");
}

#[tokio::test]
async fn a_reply_that_cannot_be_parsed_ends_the_relay_with_a_protocol_error() {
    let mesh = MemoryMesh::new();
    let (_connector, mut peer, pending) = raw_peer(&mesh).await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let ingress = tokio::spawn(pending.relay(far));

    peer.send.write_all(&[0xee, 0]).await.unwrap();
    assert!(matches!(ingress.await.unwrap().end, End::BadReply(FlowError::Malformed(_))));
    assert_reset(&drain(&mut client).await.1);
    let error = peer.recv.read(&mut [0; 8]).await.unwrap_err();
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(code(Reason::ProtocolError))));
}

#[tokio::test]
async fn cancelling_the_relay_resets_the_flow_with_the_given_reason_and_the_tcp_side() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow =
        i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap().established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));
    let mut target = world.accept_target().await;
    client.write_all(b"x").await.unwrap();
    target.read_exact(&mut [0; 1]).await.unwrap();

    world.cancel.notify_one();

    assert_reset(&drain(&mut target).await.1);
    assert_reset(&drain(&mut client).await.1);
    let exit = world.exit_summary().await;
    assert!(matches!(exit.end, End::Cancelled(Reason::AccessRevoked)), "{:?}", exit.end);
    let ingress = ingress.await.unwrap();
    assert!(matches!(ingress.end, End::Aborted(StreamAborted::Reset(c)) if c == code(Reason::AccessRevoked)), "{:?}", ingress.end);
}

#[tokio::test]
async fn losing_the_connection_resets_both_tcp_sides() {
    let mut world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow =
        i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap().established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));
    let mut target = world.accept_target().await;

    world.mesh.partition(node(INGRESS), node(EXIT));

    assert_reset(&drain(&mut client).await.1);
    assert_reset(&drain(&mut target).await.1);
    assert!(matches!(ingress.await.unwrap().end, End::Lost(_)));
    assert!(matches!(world.exit_summary().await.end, End::Lost(_)));
}

#[tokio::test]
async fn dropping_the_relay_midway_resets_the_tcp_side_instead_of_closing_it_cleanly() {
    let world = World::new().await;
    let TcpPair { near: mut client, far } = TcpPair::new().await;
    let flow =
        i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap().established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));
    let mut target = world.accept_target().await;
    target.write_all(b"partial").await.unwrap();
    let mut partial = [0; 7];
    client.read_exact(&mut partial).await.unwrap();

    // 任务被取消（进程退出时也是这样）：没有人来得及做任何收尾。
    ingress.abort();

    assert_reset(&drain(&mut client).await.1);
    assert_reset(&drain(&mut target).await.1);
}

/// 数一数发送方向上有几次写。
struct CountingSend {
    inner: MemorySend,
    writes: Arc<AtomicUsize>,
}

impl AsyncWrite for CountingSend {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, data);
        if poll.is_ready() {
            self.writes.fetch_add(1, Ordering::Relaxed);
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl SendStream for CountingSend {
    fn reset(&mut self, code: StreamCode) {
        self.inner.reset(code);
    }
}

/// 数一数开了几条流（其中几条是问连接层要可以走 ice3proxy 的那一种）、每条流上写了几次。
struct CountingConnector {
    inner: MemoryConnector,
    opens: AtomicUsize,
    tcp_opens: AtomicUsize,
    writes: Arc<AtomicUsize>,
}

impl Connector for CountingConnector {
    type Send = CountingSend;
    type Recv = MemoryRecv;

    async fn open(&self, peer: NodeId) -> Result<(Self::Send, Self::Recv), OpenError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        let (send, recv) = self.inner.open(peer).await?;
        Ok((CountingSend { inner: send, writes: Arc::clone(&self.writes) }, recv))
    }

    async fn open_tcp(&self, peer: NodeId) -> Result<(Self::Send, Self::Recv), OpenError> {
        self.tcp_opens.fetch_add(1, Ordering::Relaxed);
        let (send, recv) = self.inner.open(peer).await?;
        Ok((CountingSend { inner: send, writes: Arc::clone(&self.writes) }, recv))
    }
}

fn counting(mesh: &MemoryMesh) -> (CountingConnector, MemoryAcceptor) {
    let (inner, _) = mesh.join(node(INGRESS));
    let (_, acceptor) = mesh.join(node(EXIT));
    (CountingConnector { inner, opens: AtomicUsize::new(0), tcp_opens: AtomicUsize::new(0), writes: Arc::default() }, acceptor)
}

#[tokio::test]
async fn only_a_stream_that_may_take_ice3proxy_asks_the_connection_layer_for_one() {
    let mesh = MemoryMesh::new();
    let (connector, acceptor) = counting(&mesh);
    let _shared = i3t_flow::open(&connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap();
    assert_eq!(
        (connector.opens.load(Ordering::Relaxed), connector.tcp_opens.load(Ordering::Relaxed)),
        (1, 0),
        "端口转发这样的流只走共用的连接"
    );
    let _lane = i3t_flow::open(&connector, node(EXIT), &open_node_port(), b"", Carriage::Lane).await.unwrap();
    assert_eq!((connector.opens.load(Ordering::Relaxed), connector.tcp_opens.load(Ordering::Relaxed)), (1, 1), "选了 ice3proxy 的代理的流");
    for _ in 0..2 {
        acceptor.accept().await.unwrap();
    }
}

#[tokio::test]
async fn open_and_the_first_data_leave_in_a_single_write() {
    let mesh = MemoryMesh::new();
    let (connector, acceptor) = counting(&mesh);
    let request = Open {
        target: Target::Forward { id: 7 },
        class: Class::Interactive,
        origin: Origin { credential: Some("gw".into()), client: Some("192.0.2.1:4000".parse().unwrap()) },
    };
    let _pending = i3t_flow::open(&connector, node(EXIT), &request, b"\x16\x03\x01 client hello", Carriage::Shared).await.unwrap();
    assert_eq!(connector.writes.load(Ordering::Relaxed), 1);

    let incoming = acceptor.accept().await.unwrap();
    let received = Request::read(incoming, PATIENCE).await.unwrap();
    assert_eq!(received.open(), &request);
    assert_eq!(received.peer(), node(INGRESS));
}

#[tokio::test]
async fn an_open_is_never_replayed_after_the_connection_is_lost() {
    let mesh = MemoryMesh::new();
    let (connector, acceptor) = counting(&mesh);
    let pending = i3t_flow::open(&connector, node(EXIT), &open_node_port(), b"POST /charge", Carriage::Shared).await.unwrap();
    let incoming = acceptor.accept().await.unwrap();

    // OPEN 已经到了对端，REPLY 还没回来，连接断了。
    mesh.partition(node(INGRESS), node(EXIT));
    mesh.heal(node(INGRESS), node(EXIT));

    assert!(matches!(pending.established().await, Err(FlowError::Read(_))));
    assert_eq!(connector.opens.load(Ordering::Relaxed), 1, "失败之后不应该自己再开一条流重发");
    assert!(timeout(Duration::from_millis(50), acceptor.accept()).await.is_err(), "对端不应该再收到第二个 OPEN");
    drop(incoming);
}

#[tokio::test]
async fn opening_towards_an_unreachable_peer_fails_without_sending_anything() {
    let mesh = MemoryMesh::new();
    let (connector, _acceptor) = counting(&mesh);
    mesh.partition(node(INGRESS), node(EXIT));
    let error = i3t_flow::open(&connector, node(EXIT), &open_node_port(), b"data", Carriage::Shared).await.unwrap_err();
    assert!(matches!(error, FlowError::Connect(OpenError::NoRoute(peer)) if peer == node(EXIT)));
    assert_eq!(connector.writes.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn a_stream_that_never_sends_its_open_is_reset_at_the_deadline() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(INGRESS));
    let (_, acceptor) = mesh.join(node(EXIT));
    let (_send, mut recv) = connector.open(node(EXIT)).await.unwrap();
    let incoming = acceptor.accept().await.unwrap();

    let deadline = Duration::from_millis(30);
    assert!(matches!(Request::read(incoming, deadline).await, Err(FlowError::Deadline(d)) if d == deadline));
    let error = recv.read(&mut [0; 1]).await.unwrap_err();
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(code(Reason::ProtocolError))));
}

#[tokio::test]
async fn a_malformed_open_is_answered_with_a_protocol_error_reset() {
    let world = World::new().await;
    let (mut send, mut recv) = world.connector.open(node(EXIT)).await.unwrap();
    send.write_all(&[0x7f, 0, 0]).await.unwrap();
    let error = timeout(PATIENCE, recv.read(&mut [0; 1])).await.unwrap().unwrap_err();
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(code(Reason::ProtocolError))));
    let error = send.write_all(b"more").await.unwrap_err();
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Stopped(code(Reason::ProtocolError))));
}

#[tokio::test]
async fn a_large_transfer_arrives_intact_in_both_directions() {
    let mut world = World::new().await;
    let TcpPair { near: client, far } = TcpPair::new().await;
    let flow =
        i3t_flow::open(&world.connector, node(EXIT), &open_node_port(), b"", Carriage::Shared).await.unwrap().established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));
    let target = world.accept_target().await;

    let upload: Vec<u8> = (0..=u8::MAX).cycle().take(3_000_001).collect();
    let download: Vec<u8> = (0..=u8::MAX).rev().cycle().take(2_000_003).collect();

    async fn exchange(mut stream: TcpStream, outgoing: Vec<u8>) -> Vec<u8> {
        let (mut read, mut write) = stream.split();
        let send = async {
            write.write_all(&outgoing).await.unwrap();
            write.shutdown().await.unwrap();
        };
        let receive = async {
            let mut incoming = Vec::new();
            read.read_to_end(&mut incoming).await.unwrap();
            incoming
        };
        tokio::join!(send, receive).1
    }

    let client_side = tokio::spawn(exchange(client, upload.clone()));
    let target_side = tokio::spawn(exchange(target, download.clone()));
    assert_eq!(timeout(PATIENCE, target_side).await.unwrap().unwrap(), upload);
    assert_eq!(timeout(PATIENCE, client_side).await.unwrap().unwrap(), download);

    let ingress = ingress.await.unwrap();
    assert!(ingress.end.is_clean());
    assert_eq!((ingress.from_tcp.bytes, ingress.from_flow.bytes), (3_000_001, 2_000_003));
    let exit = world.exit_summary().await;
    assert_eq!((exit.from_flow.bytes, exit.from_tcp.bytes), (3_000_001, 2_000_003));
}
