//! 路由层集成测试的公共部分：在一个进程里起几个节点，都绑在回环地址上，netmap 由测试直接给。
//!
//! 每个节点是"UDP 端口 + 路径层 + relay 角色"，上面跑一条真的 QUIC 连接，所以测到的是流在换路、多级中转时
//! 的实际表现。连接层（`i3t-conn`）依赖这个 crate，这里不能反过来用它，所以 QUIC 的接法在下面按同样的做法写了一份最小的：
//! 每个对端一个固定的虚拟地址，QUIC 只见得到虚拟地址。

// 这个文件里全是测试用的辅助代码。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]
// 每个测试文件各自编译这个模块，各用其中的一部分。
#![allow(dead_code)]

use std::fmt;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use i3t_crypto::{NodeKey, tls};
use i3t_model::Netmap;
use i3t_model::control::Route;
use i3t_path::{PacketMeta, Packets, PathConfig, Paths};
use i3t_proto::NodeId;
use i3t_relay::Relay;
use i3t_underlay::{Direction, Ecn, PacketFilter, UdpPort};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::udp::{EcnCodepoint, RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// 等一件"应该很快发生"的事最多等多久。
pub const SOON: Duration = Duration::from_secs(10);
const ALPN: &[u8] = b"i3t-test/1";
const VIRTUAL_PORT: u16 = 20330;

/// 设置环境变量 `I3T_TEST_LOG=debug`（或别的级别）可以看到节点的日志。
pub fn init_logging() {
    let Ok(level) = std::env::var("I3T_TEST_LOG") else { return };
    let level = level.parse().unwrap_or(tracing_subscriber::filter::LevelFilter::DEBUG);
    let _already_set = tracing_subscriber::fmt().with_max_level(level).with_test_writer().try_init();
}

/// 路径层的测试开关：骨干走 TCP 时在后台试 UDP 的间隔（秒），见 `i3t_path` 的 `timing::backbone_udp_retry`。
const BACKBONE_RETRY_SWITCH: &str = "I3T_TEST_BACKBONE_UDP_RETRY";

/// 在把骨干在后台试 UDP 的间隔改成 `retry` 的进程里跑测试 `name`（就是调用它的那个测试），测试的内容是 `test`。
///
/// 路径层在进程里头一次用到时读一次这个开关，同一个进程里别的测试不该跟着变，所以这里起一个只跑这一个测试的子进程，开关设在它的环境里；
/// 已经在这样的子进程里时就地跑 `test`。子进程的输出在它失败时整个带出来，成功时照常交给测试框架（`--nocapture` 看得到）。
pub fn with_backbone_udp_retry<F: Future<Output = ()>>(name: &str, retry: Duration, test: impl FnOnce() -> F) {
    if std::env::var_os(BACKBONE_RETRY_SWITCH).is_some() {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
        runtime.block_on(test());
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(BACKBONE_RETRY_SWITCH, retry.as_secs().to_string())
        .output()
        .unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "子进程里的测试失败了\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(stdout.contains("1 passed"), "子进程里没有跑到测试 {name}：\n{stdout}");
    println!("{stdout}");
}

/// 节点在 QUIC 库眼里的地址：一个不会和真实地址混淆的 IPv6 地址，后 32 位是节点编号。
fn virtual_addr(id: NodeId) -> SocketAddr {
    let [b0, b1, b2, b3] = id.to_be_bytes();
    let ip = Ipv6Addr::new(0xfd69, 0x3374, 0, 0, 0, 0, u16::from_be_bytes([b0, b1]), u16::from_be_bytes([b2, b3]));
    SocketAddr::new(IpAddr::V6(ip), VIRTUAL_PORT)
}

fn node_of(addr: SocketAddr) -> Option<NodeId> {
    let IpAddr::V6(ip) = addr.ip() else { return None };
    let [.., hi, lo] = ip.segments();
    let ([b0, b1], [b2, b3]) = (hi.to_be_bytes(), lo.to_be_bytes());
    NodeId::from_be_bytes([b0, b1, b2, b3])
}

/// 交给 QUIC 库的 socket：其实是路径层。
struct PathSocket {
    paths: Arc<Paths>,
    me: NodeId,
}

impl fmt::Debug for PathSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathSocket").field("me", &self.me).finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for PathSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(WritablePoller { paths: Arc::clone(&self.paths), waiting: None })
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let Some(peer) = node_of(transmit.destination) else { return Ok(()) };
        let ecn = transmit.ecn.map(|ecn| match ecn {
            EcnCodepoint::Ect0 => Ecn::Ect0,
            EcnCodepoint::Ect1 => Ecn::Ect1,
            EcnCodepoint::Ce => Ecn::Ce,
        });
        self.paths.try_send(peer, &Packets { contents: transmit.contents, segment_size: transmit.segment_size, ecn })
    }

    fn poll_recv(&self, cx: &mut Context<'_>, bufs: &mut [IoSliceMut<'_>], meta: &mut [RecvMeta]) -> Poll<io::Result<usize>> {
        let mut metas = [PacketMeta { peer: self.me, len: 0, stride: 0, ecn: None }; quinn::udp::BATCH_SIZE];
        let limit = bufs.len().min(meta.len()).min(metas.len());
        let count = ready!(self.paths.poll_recv(cx, &mut bufs[..limit], &mut metas[..limit]))?;
        for (out, packet) in meta.iter_mut().zip(&metas[..count]) {
            *out = RecvMeta { addr: virtual_addr(packet.peer), len: packet.len, stride: packet.stride, ecn: None, dst_ip: None };
        }
        Poll::Ready(Ok(count))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(virtual_addr(self.me))
    }

    fn max_transmit_segments(&self) -> usize {
        self.paths.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.paths.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.paths.may_fragment()
    }
}

type Writable = Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>;

struct WritablePoller {
    paths: Arc<Paths>,
    waiting: Option<Writable>,
}

impl fmt::Debug for WritablePoller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WritablePoller").finish_non_exhaustive()
    }
}

impl UdpPoller for WritablePoller {
    fn poll_writable(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let waiting = this.waiting.get_or_insert_with(|| {
            let paths = Arc::clone(&this.paths);
            Box::pin(async move { paths.writable().await })
        });
        let result = ready!(waiting.as_mut().poll(cx));
        this.waiting = None;
        Poll::Ready(result)
    }
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut mtu = quinn::MtuDiscoveryConfig::default();
    mtu.upper_bound(i3t_proto::MAX_CONN_PACKET_LEN);
    let mut transport = quinn::TransportConfig::default();
    transport
        // 保活比产品里勤：测试里靠它让对端一直算"活跃"，路径层才会持续为它选路、探测。
        .keep_alive_interval(Some(Duration::from_secs(2)))
        .max_idle_timeout(Some(quinn::IdleTimeout::from(quinn::VarInt::from_u32(30_000))))
        .mtu_discovery_config(Some(mtu));
    Arc::new(transport)
}

/// 某个节点的 netmap 里怎么写一个对端。
#[derive(Clone, Copy)]
pub struct Peer<'a> {
    pub node: &'a Node,
    /// 把对端真实的地址写进它的候选端点。
    pub endpoints: bool,
    /// 对端挂靠的 relay，和它到每个 relay 的延迟（毫秒）。
    pub attached: &'a [(&'a Node, u32)],
}

impl<'a> Peer<'a> {
    pub fn direct(node: &'a Node) -> Self {
        Self { node, endpoints: true, attached: &[] }
    }

    /// 不知道地址、挂靠在 `relays` 上的对端。
    pub fn via(node: &'a Node, attached: &'a [(&'a Node, u32)]) -> Self {
        Self { node, endpoints: false, attached }
    }

    /// 不知道地址，也没有挂靠任何 relay。
    pub fn unknown(node: &'a Node) -> Self {
        Self { node, endpoints: false, attached: &[] }
    }
}

/// 一个节点的 netmap 里除了它自己之外的内容。
#[derive(Clone, Copy, Default)]
pub struct View<'a> {
    pub peers: &'a [Peer<'a>],
    /// 网络里的 relay。
    pub relays: &'a [&'a Node],
    /// 有值时本节点启用为 relay，值是允许建中转会话的节点。
    pub relay_clients: Option<&'a [&'a Node]>,
    /// 本节点的选路模式：`"lowest_cost"`，不写就是默认模式。
    pub routing: Option<&'static str>,
    /// 本节点能用哪些路：`"relay_only"`、`"direct_only"`，不写就是都能用。
    pub reach: Option<&'static str>,
    /// 网络的最大中转级数，不写是 2。
    pub max_relay_hops: Option<u8>,
    /// relay 的带宽上限，Mbit/s。
    pub bandwidth_mbps: Option<u32>,
    /// netmap 里不写 relay 的 TCP 地址。这样用过滤器挡掉 UDP 之后，这个节点就真的连不上那个 relay，不会退到 TCP 中转。
    pub udp_only: bool,
    /// 这些 relay 的 TCP 地址写成给定的地址（测试在那里放一个转发 TCP 的中间人），而不是 relay 自己的。
    pub tcp_via: &'a [(&'a Node, SocketAddr)],
}

/// 一个测试节点。
pub struct Node {
    pub id: NodeId,
    pub key: Arc<NodeKey>,
    pub port: Arc<UdpPort>,
    pub paths: Arc<Paths>,
    /// 取走丢掉就是"这个节点的 relay 进程退出"。
    pub relay: Option<Arc<Relay>>,
    quic: quinn::Endpoint,
    netmap: watch::Sender<Arc<Netmap>>,
}

impl Node {
    /// 起一个节点，绑在回环地址的一个空闲端口上。一开始它的 netmap 里只有自己。
    pub fn start(id: u32) -> Self {
        Self::start_on(id, "127.0.0.1").expect("找不到 TCP 和 UDP 都空闲的端口")
    }

    /// 同 [`Node::start`]，绑在给定的本机地址上。绑不上（比如这台机器没有 IPv6）时返回 `None`。
    pub fn start_on(id: u32, ip: &str) -> Option<Self> {
        init_logging();
        let id = NodeId::new(id).unwrap();
        let key = Arc::new(NodeKey::generate().unwrap());
        let (netmap, netmap_rx) = watch::channel(build_netmap(id, &View::default()));
        // 节点启用为 relay 时要在同一个端口号上监听 TCP，所以挑一个 TCP 和 UDP 都空着的端口。
        let port = (0..50).find_map(|_| {
            let probe = std::net::TcpListener::bind((ip, 0)).ok()?;
            UdpPort::bind(probe.local_addr().ok()?).ok()
        })?;
        let port = Arc::new(port);
        let paths = Arc::new(Paths::new(PathConfig {
            key: Arc::clone(&key),
            port: Arc::clone(&port),
            netmap: netmap_rx.clone(),
            addr_servers: Vec::new(),
        }));
        let relay = Arc::new(Relay::new(Arc::clone(&key), Arc::clone(&port), netmap_rx.clone()));
        paths.set_local_relay(Arc::clone(&relay) as Arc<dyn i3t_path::LocalRelay>);
        relay.set_backbone_inbox(paths.backbone_inbox());
        let weak_paths = Arc::downgrade(&paths);
        relay.set_local_delivery(move |src, packet| {
            if let Some(paths) = weak_paths.upgrade() {
                paths.deliver_local(src, packet);
            }
        });

        let allowed = netmap_rx;
        let server = tls::server_config(&key, move |key| allowed.borrow().peers.iter().any(|peer| peer.key == *key), ALPN).unwrap();
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server).unwrap()));
        server.transport_config(transport());
        server.migration(false);
        let mut endpoint = quinn::EndpointConfig::default();
        endpoint.grease_quic_bit(false);
        endpoint.max_udp_payload_size(u16::try_from(Paths::max_datagram_len()).unwrap()).unwrap();
        let socket = Arc::new(PathSocket { paths: Arc::clone(&paths), me: id });
        let quic = quinn::Endpoint::new_with_abstract_socket(endpoint, Some(server), socket, Arc::new(quinn::TokioRuntime)).unwrap();
        Some(Self { id, key, port, paths, relay: Some(relay), quic, netmap })
    }

    pub fn addr(&self) -> SocketAddr {
        self.port.local_addr()
    }

    pub fn relay(&self) -> &Relay {
        self.relay.as_ref().expect("这个节点的 relay 已经被测试丢掉了")
    }

    /// 给这个节点换一份 netmap。
    pub fn apply(&self, view: &View<'_>) {
        self.netmap.send(build_netmap(self.id, view)).unwrap();
    }

    /// 到 `peer` 现在走哪条路。
    pub fn route_to(&self, peer: &Node) -> Route {
        self.paths.routes().into_iter().find(|entry| entry.peer == peer.id).map_or(Route::None, |entry| entry.route)
    }

    /// 等到 `peer` 的路满足 `wanted` 为止。
    pub async fn wait_route(&self, peer: &Node, what: &str, wanted: impl Fn(&Route) -> bool) -> Route {
        let deadline = Instant::now() + 3 * SOON;
        loop {
            let route = self.route_to(peer);
            if wanted(&route) {
                return route;
            }
            assert!(Instant::now() < deadline, "节点 {} 到节点 {} 的路一直没有变成{what}，现在是 {route:?}", self.id, peer.id);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 等这个节点的 relay 角色把 TCP 监听起来，也就是 netmap 已经生效。
    pub async fn wait_relay_enabled(&self) {
        let deadline = Instant::now() + SOON;
        while !self.relay().is_enabled() {
            assert!(Instant::now() < deadline, "节点 {} 一直没有启用为 relay", self.id);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// 丢掉这个节点和 `others` 之间所有的 UDP 包（两个方向），模拟它们之间的路不通。
    pub fn block(&self, others: &[&Node]) {
        let blocked: Vec<SocketAddr> = others.iter().map(|node| node.addr()).collect();
        let filter: PacketFilter = Arc::new(move |_: Direction, peer, _: &[u8]| !blocked.contains(&peer));
        self.port.set_filter(Some(filter));
    }

    pub fn unblock(&self) {
        self.port.set_filter(None);
    }

    /// 到 `peer` 开一条流。没有连接时先建连接，等路出现最多等一小会儿。
    pub async fn open(&self, peer: &Node) -> (quinn::SendStream, quinn::RecvStream) {
        assert!(self.paths.wait_for_route(peer.id, 3 * SOON).await, "节点 {} 到节点 {} 一直没有路", self.id, peer.id);
        let crypto = QuicClientConfig::try_from(tls::client_config(&self.key, peer.key.public(), ALPN).unwrap()).unwrap();
        let mut client = quinn::ClientConfig::new(Arc::new(crypto));
        client.transport_config(transport());
        let connecting = self.quic.connect_with(client, virtual_addr(peer.id), "i3t").unwrap();
        let connection = tokio::time::timeout(SOON, connecting).await.expect("建立连接超时").expect("建立连接失败");
        connection.open_bi().await.expect("开流失败")
    }

    /// 起一个回声服务：对端开过来的每条流，读到什么写回什么，对端结束时也跟着正常结束。
    pub fn spawn_echo(&self) -> JoinHandle<()> {
        let quic = self.quic.clone();
        tokio::spawn(async move {
            while let Some(incoming) = quic.accept().await {
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else { return };
                    while let Ok((send, recv)) = connection.accept_bi().await {
                        tokio::spawn(echo(send, recv));
                    }
                });
            }
        })
    }
}

async fn echo(mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
    let mut buf = vec![0u8; 64 * 1024];
    while let Ok(Some(len)) = recv.read(&mut buf).await {
        if send.write_all(&buf[..len]).await.is_err() {
            return;
        }
    }
    let _finished = send.finish();
}

fn build_netmap(me: NodeId, view: &View<'_>) -> Arc<Netmap> {
    let virtual_ip = |id: NodeId| format!("10.66.{}.{}", id.get() / 256 % 256, id.get() % 256);
    let peers: Vec<_> = view
        .peers
        .iter()
        .map(|peer| {
            let attached: Vec<_> =
                peer.attached.iter().map(|(relay, rtt_ms)| serde_json::json!({ "relay": relay.id, "rtt_ms": rtt_ms })).collect();
            serde_json::json!({
                "id": peer.node.id,
                "key": peer.node.key.public(),
                "name": format!("node-{}", peer.node.id),
                "virtual_ip": virtual_ip(peer.node.id),
                "inbound": true,
                "outbound": true,
                "endpoints": if peer.endpoints { vec![peer.node.addr()] } else { Vec::new() },
                "attached": attached,
            })
        })
        .collect();
    let relays: Vec<_> = view
        .relays
        .iter()
        .map(|relay| {
            serde_json::json!({
                "id": relay.id,
                "key": relay.key.public(),
                "name": format!("relay-{}", relay.id),
                "udp": [relay.addr()],
                "tcp": match view.tcp_via.iter().find(|(via, _)| via.id == relay.id) {
                    Some((_, addr)) => vec![*addr],
                    None if view.udp_only => Vec::new(),
                    None => vec![relay.addr()],
                },
            })
        })
        .collect();
    let relay_clients: Vec<_> = view
        .relay_clients
        .unwrap_or_default()
        .iter()
        .map(|client| serde_json::json!({ "id": client.id, "key": client.key.public() }))
        .collect();
    let netmap = serde_json::json!({
        "version": 1,
        "network": {
            "name": "test", "domain": "test.i3t", "cidr": "10.66.0.0/16",
            "max_relay_hops": view.max_relay_hops.unwrap_or(2),
        },
        "me": {
            "id": me,
            "name": format!("node-{me}"),
            "virtual_ip": virtual_ip(me),
            "routing": view.routing,
            "reach": view.reach,
            "relay": view.relay_clients.map(|_| serde_json::json!({ "bandwidth_mbps": view.bandwidth_mbps })),
        },
        "peers": peers,
        "relays": relays,
        "relay_clients": relay_clients,
    });
    Arc::new(serde_json::from_value(netmap).unwrap())
}

/// 在一条新的流上做一次回声往返，核对内容。
pub async fn echo_once(from: &Node, to: &Node, payload: &[u8]) {
    let (mut send, mut recv) = from.open(to).await;
    send.write_all(payload).await.unwrap();
    send.finish().unwrap();
    let echoed = tokio::time::timeout(SOON, recv.read_to_end(payload.len() + 1)).await.expect("等回声超时").expect("读回声失败");
    assert!(echoed == payload, "回声的内容和发出去的不一样（{} 字节对 {} 字节）", echoed.len(), payload.len());
}

/// 每一块的长度。块里是它的序号反复写，所以丢了、重了、乱了都看得出来。
const BLOCK: usize = 1024;

/// 一条一直在传数据的回声流：一头不停地写编了号的块，另一头核对回来的块一个不少、一个不乱。
pub struct Transfer {
    verified: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    writer: JoinHandle<u64>,
    reader: JoinHandle<Result<u64, String>>,
}

impl Transfer {
    /// 在 `from` 到 `to` 的一条新流上开始传。`to` 上要有回声服务。每秒大约传 `blocks_per_sec` 块。
    pub async fn start(from: &Node, to: &Node, blocks_per_sec: u32) -> Self {
        let (mut send, mut recv) = from.open(to).await;
        let verified = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let stopping = Arc::clone(&stop);
        let writer = tokio::spawn(async move {
            let mut pace = tokio::time::interval(Duration::from_secs(1) / blocks_per_sec);
            let mut index = 0u64;
            while !stopping.load(Ordering::Relaxed) {
                pace.tick().await;
                send.write_all(&block(index)).await.expect("写流失败");
                index += 1;
            }
            send.finish().expect("正常结束流失败");
            index
        });

        let progress = Arc::clone(&verified);
        let reader = tokio::spawn(async move {
            let mut received = [0u8; BLOCK];
            let mut index = 0u64;
            loop {
                match recv.read_exact(&mut received).await {
                    Ok(()) if received == block(index) => {
                        index += 1;
                        progress.store(index, Ordering::Relaxed);
                    }
                    Ok(()) => return Err(format!("第 {index} 块的内容不对：数据丢了、重了或者乱了")),
                    Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(index),
                    Err(error) => return Err(format!("读第 {index} 块时出错：{error}")),
                }
            }
        });
        Self { verified, stop, writer, reader }
    }

    /// 已经核对过多少块。
    pub fn verified(&self) -> u64 {
        self.verified.load(Ordering::Relaxed)
    }

    /// 等到又核对了 `more` 块为止，返回等了多久。数据不再流动的话会在这里超时失败。
    pub async fn expect_progress(&self, more: u64) -> Duration {
        let started = Instant::now();
        let target = self.verified() + more;
        while self.verified() < target {
            assert!(!self.reader.is_finished(), "流提前结束了");
            assert!(started.elapsed() < 3 * SOON, "数据不再流动：只核对到第 {} 块，要到第 {target} 块", self.verified());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        started.elapsed()
    }

    /// 正常结束这条流，核对发出去的每一块都原样回来了。返回一共传了多少块。
    pub async fn finish(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        let sent = tokio::time::timeout(3 * SOON, self.writer).await.expect("写的一头迟迟不结束").unwrap();
        let received = tokio::time::timeout(3 * SOON, self.reader).await.expect("读的一头迟迟不结束").unwrap().unwrap();
        assert_eq!(received, sent, "发出去 {sent} 块，回来 {received} 块");
        sent
    }
}

fn block(index: u64) -> [u8; BLOCK] {
    let mut bytes = [0u8; BLOCK];
    for chunk in bytes.as_chunks_mut::<8>().0 {
        *chunk = index.to_be_bytes();
    }
    bytes
}

/// 订阅之后到 `peer` 的换路原因，按发生的顺序。
pub fn drain_reasons(changes: &mut tokio::sync::broadcast::Receiver<i3t_path::RouteChange>, peer: &Node) -> Vec<String> {
    let mut reasons = Vec::new();
    while let Ok(change) = changes.try_recv() {
        if change.peer == peer.id {
            reasons.push(change.reason);
        }
    }
    reasons
}
