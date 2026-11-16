//! 搬运时的关卡（计数、叫停），和把两条流接起来。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::future;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use i3t_conn::{RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_flow::memory::{MemoryConnector, MemoryMesh};
use i3t_flow::{Direction, End, Handler, Meter, Request};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Open, Origin, Reason, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

const PATIENCE: Duration = Duration::from_secs(5);

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

fn open(target: Target) -> Open {
    Open { target, class: Class::Auto, origin: Origin::default() }
}

/// 数两个方向的字节；累计超过 `limit` 时叫停。
struct Counting {
    from_tcp: AtomicU64,
    from_flow: AtomicU64,
    limit: u64,
}

impl Counting {
    fn new(limit: u64) -> Self {
        Self { from_tcp: AtomicU64::new(0), from_flow: AtomicU64::new(0), limit }
    }
}

impl Meter for Counting {
    async fn pass(&self, direction: Direction, len: usize) -> Result<(), Reason> {
        let counter = match direction {
            Direction::FromTcp => &self.from_tcp,
            Direction::FromFlow => &self.from_flow,
        };
        counter.fetch_add(u64::try_from(len).unwrap(), Ordering::Relaxed);
        let total = self.from_tcp.load(Ordering::Relaxed) + self.from_flow.load(Ordering::Relaxed);
        if total > self.limit { Err(Reason::LimitReached) } else { Ok(()) }
    }
}

/// 出口：接受每条流，连到 `target`，过 `meter` 搬运，把结局报出来。
struct MeteredExit {
    target: std::net::SocketAddr,
    meter: Arc<Counting>,
    ends: mpsc::UnboundedSender<End>,
}

impl Handler for MeteredExit {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        let target = TcpStream::connect(self.target).await.unwrap();
        let flow = request.accept().await.unwrap();
        let summary = flow.relay_metered(target, future::pending(), &*self.meter).await;
        let _ = self.ends.send(summary.end);
    }
}

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let near = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (far, _) = listener.accept().await.unwrap();
    (near, far)
}

#[tokio::test]
async fn a_meter_sees_every_byte_in_both_directions() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(1));
    let (_, acceptor) = mesh.join(node(2));
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let meter = Arc::new(Counting::new(u64::MAX));
    let (ends, mut ended) = mpsc::unbounded_channel();
    let handler = Arc::new(MeteredExit { target: target.local_addr().unwrap(), meter: Arc::clone(&meter), ends });
    tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });

    let (mut client, far) = tcp_pair().await;
    let ingress_meter = Counting::new(u64::MAX);
    let pending = i3t_flow::open(&connector, node(2), &open(Target::Node { port: 1 }), b"").await.unwrap();
    let ingress = async { pending.relay_metered(far, future::pending(), &ingress_meter).await };
    let exchange = async {
        let (mut server, _) = target.accept().await.unwrap();
        client.write_all(&[7; 3000]).await.unwrap();
        client.shutdown().await.unwrap();
        let mut request = Vec::new();
        server.read_to_end(&mut request).await.unwrap();
        server.write_all(&[9; 5000]).await.unwrap();
        drop(server);
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        (request.len(), response.len())
    };
    let (summary, lengths) = timeout(PATIENCE, async { tokio::join!(ingress, exchange) }).await.unwrap();
    assert_eq!(lengths, (3000, 5000));
    assert!(summary.end.is_clean(), "{:?}", summary.end);
    assert_eq!((ingress_meter.from_tcp.load(Ordering::Relaxed), ingress_meter.from_flow.load(Ordering::Relaxed)), (3000, 5000));
    assert!(timeout(PATIENCE, ended.recv()).await.unwrap().unwrap().is_clean());
    assert_eq!((meter.from_flow.load(Ordering::Relaxed), meter.from_tcp.load(Ordering::Relaxed)), (3000, 5000), "出口那一侧方向反过来");
}

#[tokio::test]
async fn a_meter_that_says_stop_ends_the_flow_abnormally_with_its_reason_on_both_sides() {
    let mesh = MemoryMesh::new();
    let (connector, _) = mesh.join(node(1));
    let (_, acceptor) = mesh.join(node(2));
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let meter = Arc::new(Counting::new(10_000));
    let (ends, mut ended) = mpsc::unbounded_channel();
    let handler = Arc::new(MeteredExit { target: target.local_addr().unwrap(), meter, ends });
    tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });

    let (mut client, far) = tcp_pair().await;
    let pending = i3t_flow::open(&connector, node(2), &open(Target::Node { port: 1 }), b"").await.unwrap();
    let ingress = tokio::spawn(pending.relay(far));
    let (mut server, _) = timeout(PATIENCE, target.accept()).await.unwrap().unwrap();
    // 目标一直往回写，写满上限之后出口叫停。
    let writer = tokio::spawn(async move {
        let chunk = [1; 4096];
        while server.write_all(&chunk).await.is_ok() {}
    });
    let mut received = Vec::new();
    let end = timeout(PATIENCE, client.read_to_end(&mut received)).await.unwrap();
    assert_eq!(end.unwrap_err().kind(), io::ErrorKind::ConnectionReset, "被叫停的流在客户端看来是重置，不是正常结束");
    assert!(received.len() <= 10_000 + 16 * 1024, "叫停之后不再往下搬：收到 {}", received.len());

    let exit_end = timeout(PATIENCE, ended.recv()).await.unwrap().unwrap();
    assert!(matches!(exit_end, End::Cancelled(Reason::LimitReached)), "{exit_end:?}");
    let summary = timeout(PATIENCE, ingress).await.unwrap().unwrap();
    let End::Aborted(aborted) = summary.end else { panic!("入口应该看到流被对端重置：{:?}", summary.end) };
    assert_eq!(aborted.code(), StreamCode(Reason::LimitReached.to_wire()));
    writer.abort();
}

/// 中间的节点：把开过来的流和它替对方开到 `far` 的流接起来。
struct Middle {
    connector: MemoryConnector,
    far: NodeId,
    done: mpsc::UnboundedSender<i3t_flow::Spliced>,
}

impl Handler for Middle {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        let onward = i3t_flow::open(&self.connector, self.far, &open(Target::Forward { id: 1 }), b"").await.unwrap();
        let onward = onward.established().await.unwrap();
        let near = request.accept().await.unwrap();
        let _ = self.done.send(near.splice(onward, future::pending()).await);
    }
}

/// 最远的节点：把每条流交给测试，由测试扮演目标。
struct Far {
    flows: mpsc::UnboundedSender<TcpStream>,
}

impl Handler for Far {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        assert_eq!(request.open().target, Target::Forward { id: 1 });
        let (near, far) = tcp_pair().await;
        let flow = request.accept().await.unwrap();
        let _ = self.flows.send(near);
        flow.relay(far, future::pending()).await;
    }
}

struct Chain {
    client: MemoryConnector,
    targets: mpsc::UnboundedReceiver<TcpStream>,
    spliced: mpsc::UnboundedReceiver<i3t_flow::Spliced>,
}

fn chain() -> Chain {
    let mesh = MemoryMesh::new();
    let (client, _) = mesh.join(node(1));
    let (middle_connector, middle_acceptor) = mesh.join(node(2));
    let (_, far_acceptor) = mesh.join(node(3));
    let (done, spliced) = mpsc::unbounded_channel();
    let middle = Arc::new(Middle { connector: middle_connector, far: node(3), done });
    tokio::spawn(async move { i3t_flow::serve(&middle_acceptor, middle).await });
    let (flows, targets) = mpsc::unbounded_channel();
    let far = Arc::new(Far { flows });
    tokio::spawn(async move { i3t_flow::serve(&far_acceptor, far).await });
    Chain { client, targets, spliced }
}

#[tokio::test]
async fn two_spliced_flows_carry_data_and_half_close_end_to_end() {
    let mut chain = chain();
    let (mut client, far) = tcp_pair().await;
    let pending = i3t_flow::open(&chain.client, node(2), &open(Target::Node { port: 9100 }), b"hello ").await.unwrap();
    let flow = pending.established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));
    let mut target = timeout(PATIENCE, chain.targets.recv()).await.unwrap().unwrap();

    client.write_all(b"world").await.unwrap();
    client.shutdown().await.unwrap();
    let mut request = Vec::new();
    timeout(PATIENCE, target.read_to_end(&mut request)).await.unwrap().unwrap();
    assert_eq!(request, b"hello world");
    target.write_all(b"response").await.unwrap();
    target.shutdown().await.unwrap();
    let mut response = Vec::new();
    timeout(PATIENCE, client.read_to_end(&mut response)).await.unwrap().unwrap();
    assert_eq!(response, b"response");

    let spliced = timeout(PATIENCE, chain.spliced.recv()).await.unwrap().unwrap();
    assert!(spliced.end.is_clean(), "{:?}", spliced.end);
    assert_eq!((spliced.near_to_far, spliced.far_to_near), (11, 8));
    assert!(timeout(PATIENCE, ingress).await.unwrap().unwrap().end.is_clean());
}

#[tokio::test]
async fn a_reset_at_the_far_end_reaches_the_client_as_a_reset_with_the_same_reason() {
    let mut chain = chain();
    let (mut client, far) = tcp_pair().await;
    let pending = i3t_flow::open(&chain.client, node(2), &open(Target::Node { port: 9100 }), b"").await.unwrap();
    let flow = pending.established().await.unwrap();
    let ingress = tokio::spawn(flow.relay(far, future::pending()));
    let target = timeout(PATIENCE, chain.targets.recv()).await.unwrap().unwrap();

    target.set_zero_linger().unwrap();
    drop(target);
    let end = timeout(PATIENCE, client.read_to_end(&mut Vec::new())).await.unwrap();
    assert_eq!(end.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
    let spliced = timeout(PATIENCE, chain.spliced.recv()).await.unwrap().unwrap();
    let End::Aborted(aborted) = spliced.end else { panic!("中间节点应该看到异常结束：{:?}", spliced.end) };
    assert_eq!(aborted.code(), StreamCode(Reason::TargetReset.to_wire()));
    let summary = timeout(PATIENCE, ingress).await.unwrap().unwrap();
    assert!(
        matches!(summary.end, End::Aborted(StreamAborted::Reset(code)) if code == StreamCode(Reason::TargetReset.to_wire())),
        "入口看到的原因码就是目标那一侧的：{:?}",
        summary.end
    );
}
