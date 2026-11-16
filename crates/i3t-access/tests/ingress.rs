//! 入口在真的 socket 上的行为。对端节点是一个记下建流请求、然后原样回显的替身，就地出站的拨号器也是替身。
//!
//! 入口加真的出口的端到端测试在 `i3t-exit` 的 `tests/` 里。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use i3t_access::{Access, AccessStatus, EgressPolicy, ListenState, LocalEgress};
use i3t_conn::{RecvStream, SendStream};
use i3t_flow::memory::{MemoryConnector, MemoryMesh};
use i3t_flow::{Handler, Request};
use i3t_model::Netmap;
use i3t_model::netmap::{Credential, Forward, ForwardProtocol, Listener, ListenerProtocol, LocalNode, Network, Peer, PeerExit, TokenHash};
use i3t_proto::flow::{Class, Host, Open, Origin, Reply, Status, Target};
use i3t_proto::{NodeId, PublicKey};
use ring::digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const ME: u32 = 1;
const GW: u32 = 2;
const PATIENCE: Duration = Duration::from_secs(5);

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// 本节点是 1 号，对端只有 2 号 `gw`：可以访问，是出口，开了局域网。
fn netmap() -> Netmap {
    Netmap {
        version: 1,
        network: Network {
            name: "home".into(),
            domain: "home.i3t".into(),
            cidr: "10.66.0.0/16".parse().unwrap(),
            routing: Default::default(),
            max_relay_hops: 2,
            reach: Default::default(),
        },
        me: LocalNode {
            id: node(ME),
            name: "laptop".into(),
            virtual_ip: [10, 66, 0, 1].into(),
            routing: None,
            reach: None,
            dial_out_only: false,
            relay: None,
            exit: None,
            listeners: Vec::new(),
            forwards: Vec::new(),
            forward_targets: Vec::new(),
            pinned_relays: Vec::new(),
        },
        peers: vec![Peer {
            id: node(GW),
            key: PublicKey::from_bytes([2; 32]),
            name: "gw".into(),
            virtual_ip: [10, 66, 0, 2].into(),
            inbound: true,
            outbound: true,
            endpoints: Vec::new(),
            attached: Vec::new(),
            exit: Some(PeerExit { lan: true }),
            dial_out_only: false,
            keep_alive: false,
        }],
        relays: Vec::new(),
        relay_clients: Vec::new(),
        credentials: Vec::new(),
    }
}

fn listener(name: &str, protocol: ListenerProtocol, bind: &str) -> Listener {
    Listener {
        name: name.into(),
        bind: bind.parse().unwrap(),
        protocol,
        allow_from: Vec::new(),
        require_credential: false,
        default_action: None,
        fixed_exit: None,
    }
}

fn forward(id: u32, protocol: ForwardProtocol, bind: &str) -> Forward {
    Forward { id, protocol, bind: bind.parse().unwrap(), target: node(GW), allow_from: Vec::new() }
}

async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    addr
}

/// 对端节点的替身：记下每个建流请求，接受它，然后把流接到一个回显服务上。
struct EchoPeer {
    opens: mpsc::UnboundedSender<Open>,
    echo: SocketAddr,
}

impl Handler for EchoPeer {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        assert_eq!(request.peer(), node(ME));
        self.opens.send(request.open().clone()).unwrap();
        let target = TcpStream::connect(self.echo).await.unwrap();
        let flow = request.accept().await.unwrap();
        flow.relay(target, future::pending()).await;
    }
}

/// 就地出站的替身：只连写成地址的目标，记下每次是按什么策略来的。克隆出来的是同一个。
#[derive(Clone, Default)]
struct DirectEgress {
    calls: Arc<Mutex<Vec<(String, u16, EgressPolicy)>>>,
}

impl LocalEgress for DirectEgress {
    async fn connect(&self, host: &Host, port: u16, policy: EgressPolicy) -> Result<TcpStream, Reply> {
        self.calls.lock().unwrap().push((host.to_string(), port, policy));
        match host {
            Host::Ip(ip) => TcpStream::connect((*ip, port)).await.map_err(|error| Reply::new(Status::ConnectionRefused, error.to_string())),
            Host::Name(_) => Err(Reply::new(Status::NameNotFound, "替身不解析域名")),
        }
    }
}

/// 一个跑着的入口，对面是 [`EchoPeer`]。
struct World {
    netmap: watch::Sender<Arc<Netmap>>,
    access: Arc<Access<MemoryConnector, DirectEgress>>,
    egress: DirectEgress,
    /// 对端收到的建流请求，按到达的顺序。
    opens: mpsc::UnboundedReceiver<Open>,
    manager: JoinHandle<()>,
}

impl World {
    async fn start(netmap: Netmap) -> Self {
        let mesh = MemoryMesh::new();
        let (connector, _) = mesh.join(node(ME));
        let (_, acceptor) = mesh.join(node(GW));
        let (opens_sender, opens) = mpsc::unbounded_channel();
        let peer = Arc::new(EchoPeer { opens: opens_sender, echo: echo_server().await });
        tokio::spawn(async move { i3t_flow::serve(&acceptor, peer).await });

        let (sender, receiver) = watch::channel(Arc::new(netmap));
        let egress = DirectEgress::default();
        let access = Arc::new(Access::new(Arc::new(connector), egress.clone(), receiver));
        let manager = tokio::spawn({
            let access = Arc::clone(&access);
            async move { access.run().await }
        });
        let world = Self { netmap: sender, access, egress, opens, manager };
        world.settled().await;
        world
    }

    async fn update(&self, change: impl FnOnce(&mut Netmap)) -> AccessStatus {
        let mut netmap = Netmap::clone(&self.netmap.borrow());
        change(&mut netmap);
        netmap.version += 1;
        self.netmap.send(Arc::new(netmap)).unwrap();
        self.settled().await
    }

    async fn settled(&self) -> AccessStatus {
        let version = self.netmap.borrow().version;
        timeout(PATIENCE, async {
            loop {
                let status = self.access.status();
                if status.version == version {
                    return status;
                }
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("入口应该很快应用完新的 netmap")
    }

    /// 一个监听器实际监听的地址（监听在 0.0.0.0 上时给回环地址）。
    async fn listener(&self, name: &str) -> SocketAddr {
        let status = self.settled().await;
        local(&status.listeners.iter().find(|listener| listener.name == name).unwrap().state)
    }

    async fn forward(&self, id: u32) -> SocketAddr {
        let status = self.settled().await;
        local(&status.forwards.iter().find(|forward| forward.id == id).unwrap().state)
    }

    async fn next_open(&mut self) -> Open {
        timeout(PATIENCE, self.opens.recv()).await.expect("对端应该收到一个建流请求").unwrap()
    }

    fn assert_no_open(&mut self) {
        assert!(self.opens.try_recv().is_err(), "对端不应该收到建流请求");
    }
}

fn local(state: &ListenState) -> SocketAddr {
    match state {
        ListenState::Listening { local } if local.ip().is_unspecified() => SocketAddr::new([127, 0, 0, 1].into(), local.port()),
        ListenState::Listening { local } => *local,
        other => panic!("监听没有起来：{other:?}"),
    }
}

async fn assert_echo(stream: &mut TcpStream) {
    stream.write_all(b"ping through the mesh").await.unwrap();
    let mut back = [0; 21];
    timeout(PATIENCE, stream.read_exact(&mut back)).await.expect("回显应该回来").unwrap();
    assert_eq!(&back, b"ping through the mesh");
}

/// 把 `request` 发给代理，读回 `expected.len()` 字节，确认和 `expected` 一样。
async fn exchange(stream: &mut TcpStream, request: &[u8], expected: &[u8]) {
    stream.write_all(request).await.unwrap();
    let mut answer = vec![0; expected.len()];
    timeout(PATIENCE, stream.read_exact(&mut answer)).await.expect("代理应该应答").unwrap();
    assert_eq!(answer, expected, "发的是 {request:02x?}");
}

/// 代理接下来是关连接（正常关或者重置都算），而不是再发数据。
async fn assert_closed(stream: &mut TcpStream) {
    let mut rest = Vec::new();
    let _ = timeout(PATIENCE, stream.read_to_end(&mut rest)).await.expect("代理应该关掉连接");
    assert!(rest.is_empty(), "代理不应该再发东西：{rest:02x?}");
}

/// 用户名密码认证的 SOCKS5 握手，一直做到 CONNECT 一个域名，返回应答码。
async fn socks5_connect(proxy: SocketAddr, username: &str, password: &str, host: &str, port: u16) -> (u8, TcpStream) {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    exchange(&mut stream, &[5, 2, 0, 2], &[5, 2]).await;
    let auth = [
        &[1, u8::try_from(username.len()).unwrap()][..],
        username.as_bytes(),
        &[u8::try_from(password.len()).unwrap()],
        password.as_bytes(),
    ]
    .concat();
    exchange(&mut stream, &auth, &[1, 0]).await;
    let request = [&[5, 1, 0, 3, u8::try_from(host.len()).unwrap()][..], host.as_bytes(), &port.to_be_bytes()].concat();
    stream.write_all(&request).await.unwrap();
    let mut reply = [0; 10];
    timeout(PATIENCE, stream.read_exact(&mut reply)).await.expect("代理应该应答").unwrap();
    (reply[1], stream)
}

fn internet(host: &str, port: u16) -> Target {
    Target::Internet { host: host.parse().unwrap(), port }
}

#[tokio::test]
async fn proxy_requests_become_flows_carrying_target_class_and_origin() {
    let mut netmap = netmap();
    netmap.me.listeners =
        vec![listener("socks", ListenerProtocol::Socks5, "127.0.0.1:0"), listener("http", ListenerProtocol::Http, "127.0.0.1:0")];
    let mut world = World::start(netmap).await;
    let socks = world.listener("socks").await;

    // 用户名写出口和等级，域名原样交给出口去解析。
    let (code, mut stream) = socks5_connect(socks, "gw+c-i", "", "example.com", 443).await;
    assert_eq!(code, 0);
    assert_echo(&mut stream).await;
    let open = world.next_open().await;
    assert_eq!(open, Open { target: internet("example.com", 443), class: Class::Interactive, origin: Origin::default() });

    // 私有地址按"出口的局域网"发。
    let (code, mut stream) = socks5_connect(socks, "10.66.0.2", "", "192.168.1.10", 8080).await;
    assert_eq!(code, 0);
    assert_echo(&mut stream).await;
    assert_eq!(world.next_open().await.target, Target::Lan { addr: "192.168.1.10:8080".parse().unwrap() });

    // 组网地址是那个节点自己的端口，不需要用户名。
    let mut stream = TcpStream::connect(socks).await.unwrap();
    exchange(&mut stream, &[5, 1, 0], &[5, 0]).await;
    exchange(&mut stream, &[5, 1, 0, 1, 10, 66, 0, 2, 0, 22], &[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await;
    assert_echo(&mut stream).await;
    assert_eq!(world.next_open().await, Open { target: Target::Node { port: 22 }, class: Class::Auto, origin: Origin::default() });

    // HTTP 代理的用户名是同一个意思。
    let mut stream = TcpStream::connect(world.listener("http").await).await.unwrap();
    let request = format!("CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n", STANDARD.encode("gw+c-b:"));
    exchange(&mut stream, request.as_bytes(), b"HTTP/1.1 200 Connection established\r\n\r\n").await;
    assert_echo(&mut stream).await;
    assert_eq!(world.next_open().await, Open { target: internet("example.com", 443), class: Class::Bulk, origin: Origin::default() });
}

#[tokio::test]
async fn the_credential_name_and_the_client_address_travel_in_the_origin() {
    let token = "s3cret-token";
    let hash = digest::digest(&digest::SHA256, token.as_bytes());
    let mut netmap = netmap();
    netmap.me.listeners = vec![listener("lan", ListenerProtocol::Socks5, "0.0.0.0:0")];
    netmap.credentials =
        vec![Credential { name: "gateway".into(), token: TokenHash::from_bytes(hash.as_ref().try_into().unwrap()), max_concurrent: None }];
    let mut world = World::start(netmap).await;
    let port = world.listener("lan").await.port();

    // 从回环连过去：凭据的名字带上，回环地址不带。
    let (code, _stream) = socks5_connect(SocketAddr::new([127, 0, 0, 1].into(), port), "gw", token, "example.com", 443).await;
    assert_eq!(code, 0);
    assert_eq!(world.next_open().await.origin, Origin { credential: Some("gateway".into()), client: None });

    // 从这台机器的局域网地址连过去（有的话）：入口看到的客户端地址也带上。
    let Some(lan_ip) = non_loopback_address() else {
        eprintln!("跳过后半段：这台机器只有回环网卡");
        return;
    };
    let (code, stream) = socks5_connect(SocketAddr::new(lan_ip, port), "gw", token, "example.com", 443).await;
    assert_eq!(code, 0);
    let origin = world.next_open().await.origin;
    assert_eq!(origin, Origin { credential: Some("gateway".into()), client: Some(stream.local_addr().unwrap()) });
}

/// 这台机器的一个非回环 IPv4 地址：往外"连"一个 UDP socket（不发任何包），看系统给它挑的本地地址。
fn non_loopback_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

#[tokio::test]
async fn a_port_forward_turns_each_connection_into_a_flow_for_its_id() {
    let mut netmap = netmap();
    netmap.me.forwards = vec![forward(7, ForwardProtocol::Tcp, "127.0.0.1:0")];
    let mut world = World::start(netmap).await;

    let mut stream = TcpStream::connect(world.forward(7).await).await.unwrap();
    assert_echo(&mut stream).await;
    assert_eq!(world.next_open().await, Open { target: Target::Forward { id: 7 }, class: Class::Auto, origin: Origin::default() });
    assert_eq!(world.settled().await.forwards[0].active.get(), 1);
    drop(stream);
    let active = world.settled().await.forwards[0].active.clone();
    timeout(PATIENCE, async {
        while active.get() != 0 {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("连接结束之后计数应该归零");
}

#[tokio::test]
async fn forwards_that_take_no_socket_are_reported_as_such() {
    let mut netmap = netmap();
    netmap.me.forwards = vec![
        forward(1, ForwardProtocol::Udp, "127.0.0.1:0"),
        // 监听在本节点自己的虚拟 IP 上：由出口模块在别的节点访问这个端口时接住。
        forward(2, ForwardProtocol::Tcp, "10.66.0.1:8080"),
        forward(3, ForwardProtocol::Tcp, "127.0.0.1:0"),
    ];
    let world = World::start(netmap).await;
    let status = world.settled().await;
    assert_eq!(status.forwards.iter().map(|forward| forward.id).collect::<Vec<u32>>(), [1, 2, 3]);
    assert_eq!(status.forwards[0].state, ListenState::Unsupported);
    assert_eq!(status.forwards[1].state, ListenState::Virtual);
    assert!(matches!(status.forwards[2].state, ListenState::Listening { .. }));
}

#[tokio::test]
async fn the_source_allow_list_is_checked_before_anything_else() {
    let mut netmap = netmap();
    let mut guarded = listener("socks", ListenerProtocol::Socks5, "127.0.0.1:0");
    guarded.allow_from = vec!["10.0.0.0/8".parse().unwrap()];
    netmap.me.listeners = vec![guarded];
    let mut guarded_forward = forward(7, ForwardProtocol::Tcp, "127.0.0.1:0");
    guarded_forward.allow_from = vec!["192.168.0.0/16".parse().unwrap()];
    netmap.me.forwards = vec![guarded_forward];
    let mut world = World::start(netmap).await;
    let (socks, forwarded) = (world.listener("socks").await, world.forward(7).await);

    // 来源（127.0.0.1）不在白名单里：连接直接被关掉，连握手都不做。
    let mut stream = TcpStream::connect(socks).await.unwrap();
    let _ = stream.write_all(&[5, 1, 0]).await;
    assert_closed(&mut stream).await;
    let mut stream = TcpStream::connect(forwarded).await.unwrap();
    assert_closed(&mut stream).await;
    world.assert_no_open();

    // 白名单改了之后新的连接就按新的来，不需要重新绑定。
    world
        .update(|netmap| {
            netmap.me.listeners[0].allow_from.push("127.0.0.0/8".parse().unwrap());
            netmap.me.forwards[0].allow_from.push("127.0.0.1/32".parse().unwrap());
        })
        .await;
    assert_eq!((world.listener("socks").await, world.forward(7).await), (socks, forwarded));
    let (code, _stream) = socks5_connect(socks, "gw", "", "example.com", 443).await;
    assert_eq!(code, 0);
    let mut stream = TcpStream::connect(forwarded).await.unwrap();
    assert_echo(&mut stream).await;
}

#[tokio::test]
async fn unsupported_and_malformed_socks5_requests_get_the_right_answer() {
    let mut netmap = netmap();
    netmap.me.listeners = vec![listener("socks", ListenerProtocol::Socks5, "127.0.0.1:0")];
    let mut world = World::start(netmap).await;
    let socks = world.listener("socks").await;

    let after_greeting: [(&[u8], u8); 6] = [
        // BIND
        (&[5, 2, 0, 1, 1, 2, 3, 4, 0, 80], 7),
        // UDP ASSOCIATE：数据报还没有实现
        (&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0], 7),
        // 不认识的地址类型
        (&[5, 1, 0, 2, 1, 2, 3, 4, 0, 80], 8),
        // 保留字节不是 0
        (&[5, 1, 7, 1, 1, 2, 3, 4, 0, 80], 1),
        // 域名里有空格
        (&[5, 1, 0, 3, 3, b'a', b' ', b'b', 0, 80], 1),
        // 空域名
        (&[5, 1, 0, 3, 0, 0, 80], 1),
    ];
    for (request, code) in after_greeting {
        let mut stream = TcpStream::connect(socks).await.unwrap();
        exchange(&mut stream, &[5, 1, 0], &[5, 0]).await;
        exchange(&mut stream, request, &[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        assert_closed(&mut stream).await;
    }

    // 客户端只会 GSSAPI：没有可用的认证方法。
    let mut stream = TcpStream::connect(socks).await.unwrap();
    exchange(&mut stream, &[5, 1, 1], &[5, 0xff]).await;
    assert_closed(&mut stream).await;

    // 根本不是 SOCKS5：什么都不回。
    for garbage in [&[4, 1, 0, 80, 1, 2, 3, 4, 0][..], b"GET / HTTP/1.1\r\n\r\n", &[5, 0]] {
        let mut stream = TcpStream::connect(socks).await.unwrap();
        stream.write_all(garbage).await.unwrap();
        assert_closed(&mut stream).await;
    }

    // 用户名的写法不对，和节点不存在是同一个应答。
    for username in ["gw+c-zzz", "fd00::2", "no-such-node"] {
        assert_eq!(socks5_connect(socks, username, "", "example.com", 443).await.0, 2, "{username}");
    }
    world.assert_no_open();
}

#[tokio::test]
async fn unsupported_and_malformed_http_requests_get_the_right_status() {
    let mut netmap = netmap();
    netmap.me.listeners = vec![listener("http", ListenerProtocol::Http, "127.0.0.1:0")];
    let mut world = World::start(netmap).await;
    let http = world.listener("http").await;

    let oversized = format!("CONNECT example.com:443 HTTP/1.1\r\nX-Padding: {}\r\n\r\n", "a".repeat(9000));
    let unknown_exit = format!("CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n", STANDARD.encode("nobody:"));
    let cases = [
        ("GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n", "405 Method Not Allowed"),
        ("CONNECT example.com HTTP/1.1\r\n\r\n", "400 Bad Request"),
        ("CONNECT example.com:443 HTTP/3\r\n\r\n", "400 Bad Request"),
        ("\x05\x01\x00 this is not http\r\n\r\n", "400 Bad Request"),
        (oversized.as_str(), "431 Request Header Fields Too Large"),
        // 没有写出口、目标也不是组网地址，而替身的就地出站不解析域名。
        ("CONNECT example.com:443 HTTP/1.1\r\n\r\n", "502 Bad Gateway"),
        // 出口节点不存在。
        (unknown_exit.as_str(), "403 Forbidden"),
    ];
    for (request, status) in cases {
        let mut stream = TcpStream::connect(http).await.unwrap();
        // 超长的请求头写到一半代理就可能已经应答并关掉连接了。
        let _ = stream.write_all(request.as_bytes()).await;
        let mut answer = Vec::new();
        let _ = timeout(PATIENCE, stream.read_to_end(&mut answer)).await.expect("代理应该应答并关掉连接");
        let answer = String::from_utf8(answer).unwrap();
        assert!(answer.starts_with(&format!("HTTP/1.1 {status}\r\n")), "{request:?} → {answer:?}");
    }
    world.assert_no_open();
}

#[tokio::test]
async fn local_egress_is_asked_with_the_policy_that_fits_the_listener() {
    let mut netmap = netmap();
    netmap.me.listeners = vec![listener("socks", ListenerProtocol::Socks5, "127.0.0.1:0")];
    let mut world = World::start(netmap).await;
    let echo = echo_server().await;

    // 回环监听器，没有写出口，目标不是组网地址：就地出站，不加限制。
    let mut stream = TcpStream::connect(world.listener("socks").await).await.unwrap();
    let [hi, lo] = echo.port().to_be_bytes();
    // 握手的三步和紧跟着的数据一次写出去：多读进来的数据不能丢。
    stream.write_all(&[5, 1, 0, 5, 1, 0, 1, 127, 0, 0, 1, hi, lo]).await.unwrap();
    stream.write_all(b"early").await.unwrap();
    let mut answer = [0; 12 + 5];
    timeout(PATIENCE, stream.read_exact(&mut answer)).await.unwrap().unwrap();
    assert_eq!(answer[..12], [5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&answer[12..], b"early");
    assert_echo(&mut stream).await;

    assert_eq!(*world.egress.calls.lock().unwrap(), [("127.0.0.1".to_owned(), echo.port(), EgressPolicy::Unrestricted)]);
    world.assert_no_open();
}

#[tokio::test]
async fn stopping_the_manager_closes_the_listeners_and_leaves_connections_alone() {
    let mut netmap = netmap();
    netmap.me.listeners = vec![listener("socks", ListenerProtocol::Socks5, "127.0.0.1:0")];
    let world = World::start(netmap).await;
    let socks = world.listener("socks").await;
    let (code, mut established) = socks5_connect(socks, "gw", "", "example.com", 443).await;
    assert_eq!(code, 0);
    assert_echo(&mut established).await;

    let manager = world.manager;
    manager.abort();
    // 任务是被取消的，它的结果只会是"已取消"。
    let _ = manager.await;

    // 接受连接的任务随后被取消，监听的 socket 跟着关掉。
    let refused = timeout(PATIENCE, async {
        loop {
            match TcpStream::connect(socks).await {
                Ok(_) => sleep(Duration::from_millis(2)).await,
                Err(error) => break error,
            }
        }
    })
    .await
    .expect("监听应该很快关掉");
    assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
    assert_echo(&mut established).await;
}
