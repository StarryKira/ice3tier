//! 服务等级怎么变成流的发送优先级：建流时两端各按等级设好，`auto` 的流发得多了降级。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::future;
use std::sync::Arc;
use std::time::Duration;

use i3t_conn::contract::Priority;
use i3t_conn::{RecvStream, SendStream};
use i3t_flow::memory::MemoryMesh;
use i3t_flow::{Handler, Request};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Open, Origin, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const INGRESS: u32 = 1;
const EXIT: u32 = 2;
const PATIENCE: Duration = Duration::from_secs(5);
const MEGABYTE: usize = 1024 * 1024;

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// 接受方：每条流都连到同一个目标，然后搬运。
struct ToTarget(std::net::SocketAddr);

impl Handler for ToTarget {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        let target = TcpStream::connect(self.0).await.unwrap();
        request.accept().await.unwrap().relay(target, future::pending()).await;
    }
}

/// 一条从客户端到目标、经过一对节点的流。测试扮演客户端和目标。
struct Path {
    mesh: MemoryMesh,
    client: TcpStream,
    target: TcpStream,
}

impl Path {
    async fn open(class: Class) -> Self {
        let mesh = MemoryMesh::new();
        let (connector, _) = mesh.join(node(INGRESS));
        let (_, acceptor) = mesh.join(node(EXIT));
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let handler = Arc::new(ToTarget(target_listener.local_addr().unwrap()));
        tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });

        let client_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(client_listener.local_addr().unwrap()).await.unwrap();
        let (client_far, _) = client_listener.accept().await.unwrap();
        let request = Open { target: Target::Node { port: 80 }, class, origin: Origin::default() };
        tokio::spawn(async move {
            let flow = i3t_flow::open(&connector, node(EXIT), &request, b"").await.unwrap().established().await.unwrap();
            flow.relay(client_far, future::pending()).await;
        });
        let (target, _) = timeout(PATIENCE, target_listener.accept()).await.unwrap().unwrap();
        let mut path = Self { mesh, client, target };
        // 来回各走一个字节，确认两端都已经在搬运了。
        path.upload(1).await;
        path.download(1).await;
        path
    }

    /// 两端现在各用什么优先级往对面发：(入口, 出口)。
    fn priorities(&self) -> (Priority, Priority) {
        let of = |raw| match self.mesh.send_priorities(node(raw))[..] {
            [priority] => priority,
            ref other => panic!("{raw} 号节点应该正好有一条流的一个发送方向，实际是 {other:?}"),
        };
        (of(INGRESS), of(EXIT))
    }

    async fn upload(&mut self, len: usize) {
        Self::transfer(&mut self.client, &mut self.target, len).await;
    }

    async fn download(&mut self, len: usize) {
        Self::transfer(&mut self.target, &mut self.client, len).await;
    }

    async fn transfer(from: &mut TcpStream, to: &mut TcpStream, len: usize) {
        let data = vec![0x61; len];
        let received = async {
            let mut buf = vec![0; len];
            to.read_exact(&mut buf).await.unwrap();
        };
        let (written, ()) = timeout(PATIENCE, async { tokio::join!(from.write_all(&data), received) }).await.expect("数据应该很快搬完");
        written.unwrap();
    }
}

#[tokio::test]
async fn each_end_sends_at_the_priority_of_the_class_in_the_open() {
    for (class, priority) in [
        (Class::Interactive, Priority::High),
        (Class::Auto, Priority::High),
        (Class::Reliable, Priority::Normal),
        (Class::Bulk, Priority::Low),
    ] {
        let path = Path::open(class).await;
        assert_eq!(path.priorities(), (priority, priority), "{class:?}");
    }
}

#[tokio::test]
async fn an_auto_stream_drops_to_bulk_priority_in_a_direction_once_that_direction_has_carried_a_megabyte() {
    let mut path = Path::open(Class::Auto).await;

    // 刚好一兆字节还不降（建流时试的那一个字节也算在里面，所以这里少传一个）。
    path.download(MEGABYTE - 1).await;
    assert_eq!(path.priorities(), (Priority::High, Priority::High));

    // 下载超过了一兆：出口往回发的方向降级。入口往上发的还是交互的优先级，请求和确认照样先走。
    path.download(1).await;
    assert_eq!(path.priorities(), (Priority::High, Priority::Low));

    // 上传也超过了一兆：入口这一端也降。
    path.upload(MEGABYTE).await;
    assert_eq!(path.priorities(), (Priority::Low, Priority::Low));
}

#[tokio::test]
async fn streams_with_an_explicit_class_keep_their_priority_however_much_they_carry() {
    for (class, priority) in [(Class::Interactive, Priority::High), (Class::Reliable, Priority::Normal), (Class::Bulk, Priority::Low)] {
        let mut path = Path::open(class).await;
        path.download(2 * MEGABYTE).await;
        path.upload(2 * MEGABYTE).await;
        assert_eq!(path.priorities(), (priority, priority), "{class:?}");
    }
}
