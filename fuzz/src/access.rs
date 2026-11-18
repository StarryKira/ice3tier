//! 一个跑在回环上的入口（[`i3t_access::Access`]），给 `access_*` 几个 fuzz 目标用。
//!
//! 入口里读客户端字节的代码（SOCKS5 的握手和 UDP 头、HTTP 的请求和请求体的分帧、用户名密码、两种协议都说的端口按第一个字节分、
//! 首批数据）都是 crate 私有的，只能从监听的端口进去：一个输入是客户端在一条 TCP 连接上发的全部字节，发完关掉写的一半，
//! 读到入口关掉连接为止。
//!
//! - 本节点是 1 号，对端只有 2 号 `gw`：可以访问，是出口，开了局域网。它是内存里的连接层（`i3t_flow::memory`）上的替身，
//!   接受每一条流，接到下面的"目标"上；目标端口是 [`REFUSED_PORT`] 的拒绝。
//! - 四个代理都经 `gw` 出去：一个只说 SOCKS5、一个只说 HTTP，都不认证；两个都说的（`mixed`）认用户名 [`USERNAME`] 和密码 [`PASSWORD`]。
//! - 目标一连上就写一个完整的 HTTP 应答、关掉写的一半，再把对面发来的读完。所以 CONNECT 隧道和明文转发都会走到头。
//!
//! 输入跑在一个单线程的运行时里，入口的任务只在输入进行的时候跑。入口在客户端关掉写的一半之后 [`PATIENCE`]
//! 还没有关连接，当作一个发现（panic）：那样的连接会一直占着入口的资源。

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use i3t_access::{Access, ListenState};
use i3t_conn::{RecvStream, SendStream};
use i3t_flow::memory::{MemoryConnector, MemoryMesh};
use i3t_flow::{Handler, Request};
use i3t_model::Netmap;
use i3t_model::netmap::Verifier;
use i3t_proto::NodeId;
use i3t_proto::flow::{Reply, Status, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::runtime::Runtime;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

/// 认证的代理认的用户名和密码。
pub const USERNAME: &str = "fuzz";
pub const PASSWORD: &str = "fuzz-secret";
/// 端口转发的编号。
pub const FORWARD_ID: u32 = 1;
/// 去这个端口的目标被拒绝（对端回 REPLY 说连不上）。
pub const REFUSED_PORT: u16 = 9;
/// 入口在客户端关掉写的一半之后，最多多久关掉连接。
pub const PATIENCE: Duration = Duration::from_secs(5);

/// 目标写回的应答。
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

/// 客户端连哪一个监听。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// 只说 SOCKS5、不认证的代理。
    Socks5,
    /// 两种协议都说、认用户名和密码的代理，SOCKS5 的输入从这里进。
    Socks5Credential,
    /// 只说 HTTP、不认证的代理。
    Http,
    /// 和 [`Entry::Socks5Credential`] 是同一个端口：HTTP 的输入从这里进。
    HttpCredential,
    /// 到 `gw` 的 TCP 端口转发。
    Forward,
}

pub struct AccessWorld {
    runtime: Runtime,
    /// 发送端丢了，入口就当作节点在关闭，关掉全部监听。
    _netmap: watch::Sender<Arc<Netmap>>,
    _mesh: MemoryMesh,
    _access: Arc<Access<MemoryConnector>>,
    listening: Vec<(Entry, SocketAddr)>,
}

impl AccessWorld {
    /// 整个进程共用的一个。第一次调用时建好，等到所有监听都起来。
    ///
    /// # Panics
    ///
    /// 回环上绑不上端口，或者监听起不来：环境本身坏了，fuzz 没有意义。
    pub fn get() -> &'static Self {
        static WORLD: OnceLock<AccessWorld> = OnceLock::new();
        WORLD.get_or_init(Self::start)
    }

    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("建不了 tokio 运行时");
        let (netmap_sender, netmap) = watch::channel(Arc::new(netmap()));
        let (mesh, access, listening) = runtime.block_on(async {
            let target = start_target().await;
            let mesh = MemoryMesh::new();
            let (connector, _) = mesh.join(node(1));
            let (_, acceptor) = mesh.join(node(2));
            tokio::spawn(async move { i3t_flow::serve(&acceptor, Arc::new(Gateway { target })).await });
            let access = Arc::new(Access::new(Arc::new(connector), netmap));
            tokio::spawn({
                let access = Arc::clone(&access);
                async move { access.run().await }
            });
            let listening = timeout(PATIENCE, listening(&access)).await.expect("入口的监听一直没有全部起来");
            (mesh, access, listening)
        });
        Self { runtime, _netmap: netmap_sender, _mesh: mesh, _access: access, listening }
    }

    fn addr(&self, entry: Entry) -> SocketAddr {
        self.listening.iter().find(|(listening, _)| *listening == entry).map(|(_, addr)| *addr).expect("每个入口都在监听")
    }

    /// 客户端连上 `entry`，发 `bytes`（`split` 不是 0 时分两次发，中间停一下，让入口先读到前一半），
    /// 关掉写的一半，读到入口关掉连接为止。
    ///
    /// # Panics
    ///
    /// 入口在 [`PATIENCE`] 之内没有关掉连接。
    pub fn exchange(&self, entry: Entry, bytes: &[u8], split: usize) {
        let addr = self.addr(entry);
        self.runtime.block_on(async {
            let Ok(mut stream) = TcpStream::connect(addr).await else { return };
            // 客户端这一侧用 RST 关，不留 TIME_WAIT：每秒上千条连接，回环上的端口会被占满。
            let _ = stream.set_zero_linger();
            let (first, rest) = bytes.split_at(split.min(bytes.len()));
            if stream.write_all(first).await.is_ok() && !rest.is_empty() {
                sleep(Duration::from_millis(1)).await;
                let _ = stream.write_all(rest).await;
            }
            let _ = stream.shutdown().await;
            wait_closed(&mut stream, entry, bytes).await;
        });
    }

    /// 在不认证的 SOCKS5 代理上做一次 UDP ASSOCIATE，把 `datagrams` 发到入口给的中继端口，然后关掉控制连接。
    ///
    /// # Panics
    ///
    /// 入口不接受一个合法的 UDP ASSOCIATE，或者在 [`PATIENCE`] 之内没有关掉控制连接。
    pub fn associate(&self, datagrams: &[&[u8]]) {
        let addr = self.addr(Entry::Socks5);
        self.runtime.block_on(async {
            let mut control = TcpStream::connect(addr).await.expect("连不上 SOCKS5 代理");
            let _ = control.set_zero_linger();
            control.write_all(&[5, 1, 0]).await.expect("写不了问候");
            let mut method = [0; 2];
            control.read_exact(&mut method).await.expect("读不到选中的认证方法");
            assert_eq!(method, [5, 0], "不认证的代理应该选中不认证");
            control.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).await.expect("写不了 UDP ASSOCIATE");
            let mut reply = [0; 10];
            control.read_exact(&mut reply).await.expect("读不到 UDP ASSOCIATE 的应答");
            let [5, 0, 0, 1, a, b, c, d, hi, lo] = reply else { panic!("UDP ASSOCIATE 被拒绝：{reply:?}") };
            let relay = SocketAddr::from(([a, b, c, d], u16::from_be_bytes([hi, lo])));
            let udp = UdpSocket::bind("127.0.0.1:0").await.expect("客户端的 UDP 端口绑不上");
            for datagram in datagrams {
                let _ = udp.send_to(datagram, relay).await;
            }
            // 让关联的任务把这些报文读走、处理完。
            sleep(Duration::from_millis(2)).await;
            let _ = control.shutdown().await;
            wait_closed(&mut control, Entry::Socks5, &[]).await;
        });
    }
}

async fn wait_closed(stream: &mut TcpStream, entry: Entry, sent: &[u8]) {
    let mut sink = [0; 4096];
    let closed = timeout(PATIENCE, async { while matches!(stream.read(&mut sink).await, Ok(read) if read > 0) {} }).await;
    assert!(closed.is_ok(), "{entry:?}：客户端关掉写的一半之后，入口 {PATIENCE:?} 还没有关连接。客户端发的是 {sent:02x?}");
}

/// 等到 netmap 里的监听全部起来，返回各自实际的地址。认证的那个代理两种输入共用。
async fn listening(access: &Access<MemoryConnector>) -> Vec<(Entry, SocketAddr)> {
    let ids = [(1, &[Entry::Socks5][..]), (2, &[Entry::Http]), (3, &[Entry::Socks5Credential, Entry::HttpCredential])];
    loop {
        let status = access.status();
        let mut found: Vec<(Entry, SocketAddr)> = status
            .proxies
            .iter()
            .filter_map(|proxy| {
                let (_, entries) = ids.iter().find(|(id, _)| *id == proxy.id)?;
                let at = listening_at(&proxy.state)?;
                Some(entries.iter().map(move |entry| (*entry, at)))
            })
            .flatten()
            .collect();
        found.extend(status.forwards.iter().filter_map(|forward| Some((Entry::Forward, listening_at(&forward.state)?))));
        if found.len() == 5 {
            return found;
        }
        sleep(Duration::from_millis(5)).await;
    }
}

fn listening_at(state: &ListenState) -> Option<SocketAddr> {
    match state {
        ListenState::Listening { local } => Some(*local),
        _ => None,
    }
}

/// 回环上的"目标"：一连上就写 [`RESPONSE`]、关掉写的一半，然后把对面发来的读完。
async fn start_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("目标绑不上");
    let addr = listener.local_addr().expect("绑好的监听一定有本地地址");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                if stream.write_all(RESPONSE).await.is_ok() && stream.shutdown().await.is_ok() {
                    let mut sink = [0; 4096];
                    let _ = timeout(PATIENCE, async { while matches!(stream.read(&mut sink).await, Ok(read) if read > 0) {} }).await;
                }
            });
        }
    });
    addr
}

/// 对端节点 `gw` 的替身。
struct Gateway {
    target: SocketAddr,
}

impl Handler for Gateway {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        let refusal = match &request.open().target {
            Target::Forward { id } if *id != FORWARD_ID => Some(Status::UnknownForward),
            target if port_of(target) == Some(REFUSED_PORT) => Some(Status::ConnectionRefused),
            _ => None,
        };
        if let Some(status) = refusal {
            return request.refuse(&Reply::new(status, "fuzz")).await;
        }
        let Ok(target) = TcpStream::connect(self.target).await else {
            return request.refuse(&Reply::new(Status::InternalError, "目标连不上")).await;
        };
        if let Ok(flow) = request.accept().await {
            flow.relay(target, std::future::pending()).await;
        }
    }
}

fn port_of(target: &Target) -> Option<u16> {
    match target {
        Target::Internet { port, .. } | Target::Node { port } => Some(*port),
        Target::Lan { addr } => Some(addr.port()),
        Target::Forward { .. } => None,
    }
}

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).expect("编号不是 0")
}

/// 本节点 1 号 `laptop`：三个经 `gw` 出去的代理（见模块的说明），一条到 `gw` 的 TCP 端口转发。
fn netmap() -> Netmap {
    let auth = Verifier::derive([7; Verifier::SALT_LEN], USERNAME.as_bytes(), PASSWORD.as_bytes());
    let proxy = |id: u32, protocol: &str| serde_json::json!({ "id": id, "name": format!("p{id}"), "bind": "127.0.0.1:0", "protocol": protocol, "exit": 2 });
    let mut guarded = proxy(3, "mixed");
    guarded["auth"] = serde_json::to_value(auth).expect("校验值写得成 JSON");
    let netmap = serde_json::json!({
        "version": 1,
        "network": { "name": "home", "domain": "home.i3t", "cidr": "10.66.0.0/16" },
        "me": {
            "id": 1, "name": "laptop", "virtual_ip": "10.66.0.1",
            "proxies": [
                proxy(1, "socks5"),
                proxy(2, "http"),
                guarded,
            ],
            "forwards": [{ "id": FORWARD_ID, "protocol": "tcp", "bind": "127.0.0.1:0", "target": 2 }],
        },
        "peers": [{
            "id": 2, "key": i3t_proto::PublicKey::from_bytes([2; 32]), "name": "gw", "virtual_ip": "10.66.0.2",
            "inbound": true, "outbound": true, "exit": { "lan": true },
        }],
    });
    serde_json::from_value(netmap).expect("手写的 netmap 应该读得进来")
}
