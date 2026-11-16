//! 端到端测试的搭建：几个节点跑在一张内存里的网上，每个节点都有完整的入口和出口。
//!
//! ```text
//! 测试扮演的客户端 ──TCP──▶ 入口（Access）──内存里的连接──▶ 出口（Exit）──TCP──▶ 测试扮演的目标
//! ```
//!
//! 出口和操作系统之间换成了 [`FakeSystem`]：网卡、域名解析由测试给出，"公网地址"被引到本机的测试服务上。
//! 出站策略的检查在它之上，照常进行。

// 每个测试文件各用到这里的一部分，没用到的在那个文件里会被报成死代码。
#![allow(dead_code)]
// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到这些辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use i3t_access::{Access, AccessStatus, ListenState};
use i3t_exit::{Exit, Os, Outbound, ResolveError, System};
use i3t_flow::memory::{MemoryConnector, MemoryMesh};
use i3t_model::Netmap;
use i3t_model::netmap::{Credential, ExitRole, Listener, ListenerProtocol, LocalNode, Network, Peer, PeerExit, TokenHash};
use i3t_proto::{NodeId, PublicKey};
use ipnet::IpNet;
use ring::digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

/// 入口所在的节点。
pub const LAPTOP: u32 = 1;
/// 出口节点。
pub const GW: u32 = 2;
/// 一个不是出口的节点，上面有服务。
pub const NAS: u32 = 3;

/// 测试里等一件事发生最多等多久。
pub const PATIENCE: Duration = Duration::from_secs(5);

/// 一个"公网地址"：它过得了出站策略，测试把它引到本机的测试服务上。
pub const PUBLIC_IP: &str = "93.184.216.34";

pub fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// 节点的虚拟 IP：`10.66.0.<编号>`。
pub fn virtual_ip(raw: u32) -> Ipv4Addr {
    Ipv4Addr::new(10, 66, 0, u8::try_from(raw).unwrap())
}

pub fn name_of(raw: u32) -> &'static str {
    match raw {
        LAPTOP => "laptop",
        GW => "gw",
        NAS => "nas",
        other => panic!("测试里没有 {other} 号节点"),
    }
}

/// 一个互相都能访问的对端。
pub fn peer(raw: u32, exit: Option<PeerExit>) -> Peer {
    Peer {
        id: node(raw),
        key: PublicKey::from_bytes([u8::try_from(raw).unwrap(); 32]),
        name: name_of(raw).into(),
        virtual_ip: virtual_ip(raw),
        inbound: true,
        outbound: true,
        endpoints: Vec::new(),
        attached: Vec::new(),
        exit,
        dial_out_only: false,
        keep_alive: false,
    }
}

/// `me` 号节点的 netmap：另外两个节点都是它的对端，互相都能访问。gw 是出口（局域网开关关着），默认只放行 80 和 443。
pub fn netmap(me: u32) -> Netmap {
    let exit_of = |raw: u32| (raw == GW).then_some(PeerExit { lan: false });
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
            id: node(me),
            name: name_of(me).into(),
            virtual_ip: virtual_ip(me),
            routing: None,
            reach: None,
            dial_out_only: false,
            relay: None,
            exit: (me == GW).then(ExitRole::default),
            listeners: Vec::new(),
            forwards: Vec::new(),
            forward_targets: Vec::new(),
            pinned_relays: Vec::new(),
        },
        peers: [LAPTOP, GW, NAS].into_iter().filter(|&raw| raw != me).map(|raw| peer(raw, exit_of(raw))).collect(),
        relays: Vec::new(),
        relay_clients: Vec::new(),
        credentials: Vec::new(),
    }
}

pub fn listener(name: &str, protocol: ListenerProtocol, bind: &str) -> Listener {
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

pub fn credential(name: &str, token: &str, max_concurrent: Option<u32>) -> Credential {
    let hash = digest::digest(&digest::SHA256, token.as_bytes());
    Credential { name: name.into(), token: TokenHash::from_bytes(hash.as_ref().try_into().unwrap()), max_concurrent }
}

/// 找到 netmap 里的一个对端来改。
pub fn peer_mut(netmap: &mut Netmap, raw: u32) -> &mut Peer {
    netmap.peers.iter_mut().find(|peer| peer.id == node(raw)).unwrap()
}

/// 假的操作系统。克隆出来的是同一个。
#[derive(Clone)]
pub struct FakeSystem {
    state: Arc<Mutex<FakeState>>,
}

struct FakeState {
    /// `None` 时用真的网卡。
    interfaces: Option<Vec<IpNet>>,
    names: HashMap<String, Result<Vec<IpAddr>, ResolveError>>,
    redirects: HashMap<SocketAddr, SocketAddr>,
    resolved: Vec<String>,
    connected: Vec<SocketAddr>,
}

impl Default for FakeSystem {
    /// 默认有一块回环网卡和一块局域网网卡 `192.168.77.5/24`。
    fn default() -> Self {
        let interfaces = vec!["127.0.0.1/8".parse().unwrap(), "192.168.77.5/24".parse().unwrap()];
        let state = FakeState {
            interfaces: Some(interfaces),
            names: HashMap::new(),
            redirects: HashMap::new(),
            resolved: Vec::new(),
            connected: Vec::new(),
        };
        Self { state: Arc::new(Mutex::new(state)) }
    }
}

impl FakeSystem {
    /// 用这台机器真的网卡：测"本节点自己的服务"时要真的连到本机的非回环地址上。
    pub fn use_real_interfaces(&self) {
        self.state.lock().unwrap().interfaces = None;
    }

    pub fn set_interfaces(&self, interfaces: &[&str]) {
        self.state.lock().unwrap().interfaces = Some(interfaces.iter().map(|net| net.parse().unwrap()).collect());
    }

    /// 让 `name` 解析成这些地址。没有安排过的域名一律"不存在"。
    pub fn name(&self, name: &str, addrs: &[&str]) {
        let addrs = addrs.iter().map(|ip| ip.parse().unwrap()).collect();
        self.state.lock().unwrap().names.insert(name.into(), Ok(addrs));
    }

    pub fn name_fails(&self, name: &str, error: ResolveError) {
        self.state.lock().unwrap().names.insert(name.into(), Err(error));
    }

    /// 把去 `target` 的连接引到本机的 `real` 上。
    pub fn redirect(&self, target: &str, real: SocketAddr) {
        self.state.lock().unwrap().redirects.insert(target.parse().unwrap(), real);
    }

    /// 出口到现在为止试着连过哪些地址（引到别处之前的）。
    pub fn connected(&self) -> Vec<SocketAddr> {
        self.state.lock().unwrap().connected.clone()
    }

    /// 出口到现在为止问过解析器哪些域名。
    pub fn resolved(&self) -> Vec<String> {
        self.state.lock().unwrap().resolved.clone()
    }
}

impl System for FakeSystem {
    fn interfaces(&self) -> io::Result<Vec<IpNet>> {
        match self.state.lock().unwrap().interfaces.clone() {
            Some(interfaces) => Ok(interfaces),
            None => Os.interfaces(),
        }
    }

    async fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, ResolveError> {
        let mut state = self.state.lock().unwrap();
        state.resolved.push(name.to_owned());
        state.names.get(name).cloned().unwrap_or(Err(ResolveError::NotFound))
    }

    async fn connect(&self, addr: SocketAddr) -> io::Result<TcpStream> {
        let redirect = {
            let mut state = self.state.lock().unwrap();
            state.connected.push(addr);
            state.redirects.get(&addr).copied()
        };
        let real = match redirect {
            Some(real) => real,
            None if is_this_machine(addr.ip()) => addr,
            // 没有安排过的地址绝不真的去连：测试不应该碰到外面的网络。
            None => return Err(io::Error::new(io::ErrorKind::NetworkUnreachable, "测试里没有安排这个地址")),
        };
        TcpStream::connect(real).await
    }
}

fn is_this_machine(ip: IpAddr) -> bool {
    ip.is_loopback() || Os.interfaces().unwrap().iter().any(|net| net.addr() == ip)
}

/// 这台机器的一个非回环 IPv4 地址；没有（只有回环网卡的环境）时是 `None`。
pub fn non_loopback_address() -> Option<IpAddr> {
    let interfaces = Os.interfaces().unwrap();
    interfaces.iter().map(IpNet::addr).find(|ip| ip.is_ipv4() && !ip.is_loopback() && !matches!(ip, IpAddr::V4(v4) if v4.is_link_local()))
}

/// 一个跑着的节点。
pub struct Node {
    pub id: NodeId,
    pub system: FakeSystem,
    pub access: Arc<Access<MemoryConnector, Outbound<FakeSystem>>>,
    pub exit: Arc<Exit<FakeSystem>>,
    netmap: watch::Sender<Arc<Netmap>>,
}

impl Node {
    pub fn start(mesh: &MemoryMesh, netmap: Netmap) -> Self {
        let id = netmap.me.id;
        let (connector, acceptor) = mesh.join(id);
        let (sender, receiver) = watch::channel(Arc::new(netmap));
        let system = FakeSystem::default();
        let outbound = Outbound::new(receiver.clone(), system.clone());
        let exit = Arc::new(Exit::new(receiver.clone(), outbound.clone()));
        let access = Arc::new(Access::new(Arc::new(connector), outbound, receiver));
        tokio::spawn({
            let access = Arc::clone(&access);
            async move { access.run().await }
        });
        tokio::spawn({
            let exit = Arc::clone(&exit);
            async move { i3t_flow::serve(&acceptor, exit).await }
        });
        Self { id, system, access, exit, netmap: sender }
    }

    /// 改这个节点的 netmap 并下发（版本号加一），等入口把监听调整完。
    pub async fn update(&self, change: impl FnOnce(&mut Netmap)) -> AccessStatus {
        let mut netmap = Netmap::clone(&self.netmap.borrow());
        change(&mut netmap);
        netmap.version += 1;
        self.netmap.send(Arc::new(netmap)).unwrap();
        self.settled().await
    }

    /// 等入口应用完当前这一版 netmap，返回它的监听状态。
    pub async fn settled(&self) -> AccessStatus {
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

    /// 一个代理监听器实际监听的地址。
    pub async fn listener(&self, name: &str) -> SocketAddr {
        let status = self.settled().await;
        let listener = status.listeners.iter().find(|listener| listener.name == name).unwrap_or_else(|| panic!("没有叫 {name} 的监听器"));
        local_addr(&listener.state)
    }

    /// 一条端口转发实际监听的地址。
    pub async fn forward(&self, id: u32) -> SocketAddr {
        let status = self.settled().await;
        local_addr(&status.forwards.iter().find(|forward| forward.id == id).unwrap_or_else(|| panic!("没有 {id} 号端口转发")).state)
    }
}

fn local_addr(state: &ListenState) -> SocketAddr {
    match state {
        // 监听在 0.0.0.0 上时从回环连过去。
        ListenState::Listening { local } if local.ip().is_unspecified() => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), local.port()),
        ListenState::Listening { local } => *local,
        other => panic!("监听没有起来：{other:?}"),
    }
}

/// 三个节点。laptop 上有一个 SOCKS5 监听器 `socks` 和一个 HTTP 监听器 `http`，都在回环上。
pub struct World {
    pub mesh: MemoryMesh,
    pub laptop: Node,
    pub gw: Node,
    pub nas: Node,
}

impl World {
    pub async fn new() -> Self {
        let mesh = MemoryMesh::new();
        let mut laptop = netmap(LAPTOP);
        laptop.me.listeners =
            vec![listener("socks", ListenerProtocol::Socks5, "127.0.0.1:0"), listener("http", ListenerProtocol::Http, "127.0.0.1:0")];
        let world =
            Self { laptop: Node::start(&mesh, laptop), gw: Node::start(&mesh, netmap(GW)), nas: Node::start(&mesh, netmap(NAS)), mesh };
        world.laptop.settled().await;
        world
    }

    pub async fn socks(&self) -> SocketAddr {
        self.laptop.listener("socks").await
    }

    pub async fn http(&self) -> SocketAddr {
        self.laptop.listener("http").await
    }
}

/// 测试扮演的目标：一个监听着的端口，测试从它接到出口连过来的连接。
pub struct Target {
    listener: TcpListener,
}

impl Target {
    pub async fn bind(ip: &str) -> Self {
        Self { listener: TcpListener::bind((ip.parse::<IpAddr>().unwrap(), 0)).await.unwrap() }
    }

    pub fn addr(&self) -> SocketAddr {
        self.listener.local_addr().unwrap()
    }

    pub async fn accept(&self) -> TcpStream {
        timeout(PATIENCE, self.listener.accept()).await.expect("应该有连接连到目标").unwrap().0
    }

    /// 在 `within` 之内没有任何连接连过来。
    pub async fn assert_untouched(&self, within: Duration) {
        assert!(timeout(within, self.listener.accept()).await.is_err(), "不应该有连接连到这个目标");
    }

    /// 之后每条连接都原样回显，直到对方关掉发送方向。
    pub fn echo(self) -> SocketAddr {
        let addr = self.addr();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = self.listener.accept().await {
                tokio::spawn(async move {
                    let (mut read, mut write) = stream.split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
            }
        });
        addr
    }
}

/// 在本机回环上起一个回显服务。
pub async fn echo_server() -> SocketAddr {
    Target::bind("127.0.0.1").await.echo()
}

/// 一个没有人监听的本机端口。
pub async fn closed_port() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap()
}

/// [`assert_echo`] 每次发出去多少字节。
pub const ECHO_LEN: u64 = 50;

/// 发一段数据过去，确认原样回来。
///
/// 这段数据长得像一条完整的 TLS 握手记录，所以经过代理入口时，它会被当作首批数据随建流请求一起发出去。
pub async fn assert_echo(stream: &mut TcpStream) {
    let mut message = vec![0x16, 0x03, 0x01, 0x00, 45];
    message.extend(b"client hello ".iter().cycle().take(45));
    assert_eq!(u64::try_from(message.len()).unwrap(), ECHO_LEN);
    stream.write_all(&message).await.unwrap();
    let mut back = vec![0; message.len()];
    timeout(PATIENCE, stream.read_exact(&mut back)).await.expect("回显应该回来").unwrap();
    assert_eq!(back, message);
}

/// 把连接读到结束，返回读到的数据和结束的方式。
pub async fn drain(stream: &mut TcpStream) -> (Vec<u8>, io::Result<()>) {
    let mut data = Vec::new();
    let end = timeout(PATIENCE, stream.read_to_end(&mut data)).await.expect("连接应该结束");
    (data, end.map(drop))
}

pub fn assert_reset(end: &io::Result<()>) {
    let kind = end.as_ref().expect_err("连接应该是被重置的，而不是正常结束").kind();
    assert_eq!(kind, io::ErrorKind::ConnectionReset);
}

/// SOCKS5 握手的结果。
#[derive(Debug)]
pub enum Socks5 {
    /// 代理说连上了。
    Connected(TcpStream),
    /// 代理对 CONNECT 回了一个失败的应答码。
    Failed(u8),
    /// 用户名密码认证没有通过。
    AuthRejected,
    /// 代理不接受客户端提供的认证方法。
    NoAcceptableMethod,
}

impl Socks5 {
    pub fn connected(self) -> TcpStream {
        match self {
            Self::Connected(stream) => stream,
            other => panic!("代理应该说连上了，实际是 {other:?}"),
        }
    }

    pub fn failed(self) -> u8 {
        match self {
            Self::Failed(code) => code,
            other => panic!("代理应该回一个失败的应答码，实际是 {other:?}"),
        }
    }
}

/// 做一次 SOCKS5 握手。`auth` 是用户名和密码；没有时客户端只提供"不认证"。
pub async fn socks5(proxy: SocketAddr, auth: Option<(&str, &str)>, host: &str, port: u16) -> Socks5 {
    timeout(PATIENCE, async {
        let mut stream = TcpStream::connect(proxy).await.unwrap();
        stream.write_all(if auth.is_some() { &[5, 2, 0, 2] } else { &[5, 1, 0] }).await.unwrap();
        let mut selection = [0; 2];
        stream.read_exact(&mut selection).await.unwrap();
        match (selection, auth) {
            ([5, 0xff], _) => return Socks5::NoAcceptableMethod,
            ([5, 2], Some((username, password))) => {
                let mut message = vec![1, u8::try_from(username.len()).unwrap()];
                message.extend_from_slice(username.as_bytes());
                message.push(u8::try_from(password.len()).unwrap());
                message.extend_from_slice(password.as_bytes());
                stream.write_all(&message).await.unwrap();
                let mut status = [0; 2];
                stream.read_exact(&mut status).await.unwrap();
                if status != [1, 0] {
                    return Socks5::AuthRejected;
                }
            }
            ([5, 0], _) => {}
            other => panic!("代理选的认证方法不对：{other:?}"),
        }

        let mut request = vec![5, 1, 0];
        match host.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => {
                request.push(1);
                request.extend_from_slice(&ip.octets());
            }
            Ok(IpAddr::V6(ip)) => {
                request.push(4);
                request.extend_from_slice(&ip.octets());
            }
            Err(_) => {
                request.push(3);
                request.push(u8::try_from(host.len()).unwrap());
                request.extend_from_slice(host.as_bytes());
            }
        }
        request.extend_from_slice(&port.to_be_bytes());
        stream.write_all(&request).await.unwrap();

        let mut reply = [0; 10];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!((reply[0], reply[2], &reply[3..]), (5, 0, &[1, 0, 0, 0, 0, 0, 0][..]), "应答的格式不对");
        match reply[1] {
            0 => Socks5::Connected(stream),
            code => Socks5::Failed(code),
        }
    })
    .await
    .expect("SOCKS5 握手应该很快完成")
}

/// 发一个 HTTP CONNECT，返回状态码和连接。`auth` 是 Basic 认证的用户名和密码。
pub async fn http_connect(proxy: SocketAddr, authority: &str, auth: Option<(&str, &str)>) -> (u16, TcpStream) {
    timeout(PATIENCE, async {
        let mut stream = TcpStream::connect(proxy).await.unwrap();
        let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
        if let Some((username, password)) = auth {
            request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", STANDARD.encode(format!("{username}:{password}"))));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();

        // 一个字节一个字节读到空行为止，不碰后面的隧道数据。
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.unwrap());
        }
        let head = String::from_utf8(head).unwrap();
        let status = head.strip_prefix("HTTP/1.1 ").and_then(|rest| rest.get(..3)).unwrap_or_else(|| panic!("不是 HTTP 应答：{head:?}"));
        (status.parse().unwrap(), stream)
    })
    .await
    .expect("HTTP CONNECT 应该很快得到应答")
}

/// 等一个条件成立。用在"另一头的任务很快就会做完某件事"的地方。
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let waited = timeout(PATIENCE, async {
        while !condition() {
            sleep(Duration::from_millis(2)).await;
        }
    });
    waited.await.unwrap_or_else(|_| panic!("等了 {PATIENCE:?} 也没有等到：{what}"));
}
