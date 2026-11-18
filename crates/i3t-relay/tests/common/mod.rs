//! relay 测试的公共部分：一个真的 relay（回环上的 UDP 端口和 TCP 监听），和用最原始的 socket 手写握手的节点。
//! 节点一侧不用路径层的实现，这样测到的是线上的字节，不是另一份实现的理解。

// 这个文件里全是测试用的辅助代码。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]
// 每个测试文件各自编译这个模块，各用其中的一部分。
#![allow(dead_code)]

use std::io::IoSliceMut;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use i3t_crypto::{NodeKey, Signature};
use i3t_model::Netmap;
use i3t_proto::backbone::Row;
use i3t_proto::datagram::{RelayHeader, RelayPath, RouteHeader};
use i3t_proto::session::{Accepted, Auth, Challenge, Frame, Init};
use i3t_proto::{NodeId, PublicKey, tcp_frame};
use i3t_relay::Relay;
use i3t_underlay::tcp_leg::FrameReader;
use i3t_underlay::{BATCH_SIZE, RecvMeta, UdpPort};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub const RELAY_ID: u32 = 100;
pub const WAIT: Duration = Duration::from_secs(3);
/// 断言"没有回应"时等多久。回环上应答在一毫秒之内就到，这个数留了很大的余量。
pub const SILENCE: Duration = Duration::from_millis(200);

pub fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// netmap 里本节点的 relay 角色。
#[derive(Clone, Copy)]
pub enum Role {
    Off,
    On,
    /// 带宽上限，Mbit/s。
    Capped(u32),
}

/// relay 的 netmap 里除了它自己之外的内容。
#[derive(Clone, Copy)]
pub struct Net<'a> {
    pub role: Role,
    /// 允许建会话的节点。
    pub clients: &'a [&'a Client],
    /// 网络里别的 relay（测试里也是用原始 socket 扮演的）。
    pub relays: &'a [&'a Client],
    pub max_relay_hops: u8,
}

impl<'a> Net<'a> {
    pub fn clients(role: Role, clients: &'a [&'a Client]) -> Self {
        Self { role, clients, relays: &[], max_relay_hops: 2 }
    }
}

fn netmap(net: &Net<'_>) -> Arc<Netmap> {
    build(net.role, &identities(net.clients), &identities(net.relays), net.max_relay_hops)
}

fn build(role: Role, clients: &[(NodeId, PublicKey)], relays: &[(NodeId, PublicKey)], max_relay_hops: u8) -> Arc<Netmap> {
    let clients: Vec<_> = clients.iter().map(|(id, key)| serde_json::json!({ "id": id, "key": key })).collect();
    let relays: Vec<_> =
        relays.iter().map(|(id, key)| serde_json::json!({ "id": id, "key": key, "name": format!("relay-{id}") })).collect();
    let role = match role {
        Role::Off => serde_json::Value::Null,
        Role::On => serde_json::json!({}),
        Role::Capped(mbps) => serde_json::json!({ "bandwidth_mbps": mbps }),
    };
    let netmap = serde_json::json!({
        "version": 1,
        "network": { "name": "test", "domain": "test.i3t", "cidr": "10.66.0.0/16", "max_relay_hops": max_relay_hops },
        "me": { "id": RELAY_ID, "name": "relay", "virtual_ip": "10.66.0.100", "relay": role },
        "relays": relays,
        "relay_clients": clients,
    });
    Arc::new(serde_json::from_value(netmap).unwrap())
}

pub struct TestRelay {
    /// 测试结束之前把它取走丢掉，就是"relay 进程退出"。
    pub relay: Option<Arc<Relay>>,
    pub key: Arc<NodeKey>,
    pub addr: SocketAddr,
    netmap: watch::Sender<Arc<Netmap>>,
    pump: JoinHandle<()>,
}

impl Drop for TestRelay {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

impl TestRelay {
    /// 起一个启用了的 relay，允许 `clients` 里的节点。
    pub fn start(clients: &[&Client]) -> Self {
        Self::start_with(Role::On, clients)
    }

    pub fn start_with(role: Role, clients: &[&Client]) -> Self {
        Self::start_in(&Net::clients(role, clients))
    }

    pub fn start_in(net: &Net<'_>) -> Self {
        let key = Arc::new(NodeKey::generate().unwrap());
        let port = Arc::new(port_free_for_tcp_too());
        let addr = port.local_addr();
        let (netmap, netmap_rx) = watch::channel(netmap(net));
        let relay = Arc::new(Relay::new(Arc::clone(&key), Arc::clone(&port), netmap_rx));
        let pump = tokio::spawn(pump(port, Arc::downgrade(&relay)));
        Self { relay: Some(relay), key, addr, netmap, pump }
    }

    pub fn relay(&self) -> &Relay {
        self.relay.as_ref().expect("relay 已经被这个测试丢掉了")
    }

    pub fn set_netmap(&self, role: Role, clients: &[&Client]) {
        self.set_net(&Net::clients(role, clients));
    }

    pub fn set_net(&self, net: &Net<'_>) {
        self.netmap.send(netmap(net)).unwrap();
    }

    /// 直接给出允许的节点的编号和公钥（节点还没有连上来、手上只有它的身份时用）。
    pub fn set_identities(&self, role: Role, clients: &[(NodeId, PublicKey)]) {
        self.netmap.send(build(role, clients, &[], 2)).unwrap();
    }
}

/// 产品里这件事由路径层做：从端口收一批，把归 relay 管的交给它。这里全部都交。
async fn pump(port: Arc<UdpPort>, relay: std::sync::Weak<Relay>) {
    let buf_len = 2048 * port.gro_segments();
    let mut storage = vec![0u8; buf_len * BATCH_SIZE];
    let mut metas = [RecvMeta::default(); BATCH_SIZE];
    loop {
        let mut bufs: Vec<IoSliceMut<'_>> = storage.chunks_mut(buf_len).map(IoSliceMut::new).collect();
        let Ok(count) = port.recv(&mut bufs, &mut metas).await else { return };
        let Some(relay) = relay.upgrade() else { return };
        for (meta, buf) in metas.iter().zip(&bufs).take(count) {
            relay.handle_udp(meta.addr, meta.ecn, &buf[..meta.len], meta.stride);
        }
    }
}

/// 一个用原始 UDP socket 扮演的节点（或者别的 relay）。
pub struct Client {
    pub id: NodeId,
    pub key: NodeKey,
    pub socket: UdpSocket,
}

impl Client {
    pub async fn new(id: u32) -> Self {
        Self { id: node(id), key: NodeKey::generate().unwrap(), socket: UdpSocket::bind("127.0.0.1:0").await.unwrap() }
    }

    pub fn addr(&self) -> SocketAddr {
        self.socket.local_addr().unwrap()
    }

    pub async fn send(&self, datagram: &[u8], relay: &TestRelay) {
        self.socket.send_to(datagram, relay.addr).await.unwrap();
    }

    pub async fn recv(&self) -> Vec<u8> {
        let mut buf = [0u8; 2048];
        let (len, _from) = tokio::time::timeout(WAIT, self.socket.recv_from(&mut buf)).await.expect("等 relay 的数据报超时").unwrap();
        buf[..len].to_vec()
    }

    pub async fn expect_silence(&self) {
        let mut buf = [0u8; 2048];
        if let Ok(received) = tokio::time::timeout(SILENCE, self.socket.recv_from(&mut buf)).await {
            let (len, from) = received.unwrap();
            panic!("不该有回应，却从 {from} 收到了 {:?}", &buf[..len]);
        }
    }

    pub fn auth(&self, relay_id: NodeId, nonce: [u8; 8], cookie: [u8; 16]) -> Auth {
        let message = Auth::signed_message(relay_id, self.id, &nonce, &cookie);
        Auth { node: self.id, nonce, cookie, signature: *self.key.sign(Auth::PURPOSE, &message).as_bytes() }
    }

    pub async fn challenge(&self, relay: &TestRelay, nonce: [u8; 8]) -> Challenge {
        self.send(&Init { node: self.id, nonce }.to_bytes(), relay).await;
        let reply = self.recv().await;
        assert!(reply.len() <= Init::LEN, "CHALLENGE 不能比 INIT 大");
        let Some(Frame::Challenge(challenge)) = Frame::parse(&reply) else { panic!("不是 CHALLENGE：{reply:?}") };
        assert_eq!(challenge.nonce, nonce);
        challenge
    }

    /// 走完握手，核对 OK 的签名，返回 OK。
    pub async fn attach(&self, relay: &TestRelay) -> Accepted {
        let nonce = i3t_underlay::random_bytes();
        let challenge = self.challenge(relay, nonce).await;
        self.send(&self.auth(node(RELAY_ID), nonce, challenge.cookie).to_bytes(), relay).await;
        let reply = self.recv().await;
        assert!(reply.len() <= Auth::LEN, "OK 不能比 AUTH 大");
        let Some(Frame::Ok(ok)) = Frame::parse(&reply) else { panic!("不是 OK：{reply:?}") };
        verify_ok(&ok, relay, self.id, nonce);
        ok
    }
}

pub fn identities(clients: &[&Client]) -> Vec<(NodeId, PublicKey)> {
    clients.iter().map(|client| (client.id, client.key.public())).collect()
}

pub fn verify_ok(ok: &Accepted, relay: &TestRelay, client: NodeId, nonce: [u8; 8]) {
    assert_eq!(ok.nonce, nonce);
    let message = Accepted::signed_message(node(RELAY_ID), client, &nonce, ok.valid_for, ok.observed);
    i3t_crypto::verify(&relay.key.public(), Accepted::PURPOSE, &message, &Signature::from_bytes(ok.signature))
        .expect("OK 的签名应该是 relay 的");
}

/// 一帧必须是 relay 签了名的矩阵行，返回它的内容。
pub fn verified_row(frame: &[u8], relay: &TestRelay) -> Row {
    let (row, signature) = Row::parse(frame).unwrap_or_else(|| panic!("不是矩阵行：{frame:?}"));
    let signed = Row::signed_part(frame).unwrap();
    i3t_crypto::verify(&relay.key.public(), Row::PURPOSE, signed, &Signature::from_bytes(signature)).expect("矩阵行的签名应该是 relay 的");
    assert_eq!(row.origin, node(RELAY_ID));
    row
}

/// 经一个 relay 的中转帧。
pub fn relayed(dst: NodeId, src: NodeId, packet: &[u8]) -> Vec<u8> {
    let mut datagram = RelayHeader { dst, src }.to_bytes().to_vec();
    datagram.extend_from_slice(packet);
    datagram
}

/// 经 `relays` 这几个 relay 的多级中转帧。
pub fn routed(dst: NodeId, src: NodeId, relays: &[NodeId], packet: &[u8]) -> Vec<u8> {
    let mut datagram = RouteHeader { dst, src, relays: RelayPath::new(relays).unwrap() }.to_bytes();
    datagram.extend_from_slice(packet);
    datagram
}

/// 一条到 relay 的 TCP 中转腿，节点一侧。
pub struct TcpClient {
    pub id: NodeId,
    pub key: NodeKey,
    reader: FrameReader,
    writer: OwnedWriteHalf,
    pub local: SocketAddr,
}

impl TcpClient {
    pub async fn connect(id: u32, relay: &TestRelay) -> Self {
        Self::connect_as(node(id), NodeKey::generate().unwrap(), relay).await
    }

    /// 用给定的身份连上去。
    pub async fn connect_as(id: NodeId, key: NodeKey, relay: &TestRelay) -> Self {
        // relay 的后台任务要先把 TCP 监听起来。
        let stream = tokio::time::timeout(WAIT, async {
            loop {
                match TcpStream::connect(relay.addr).await {
                    Ok(stream) => break stream,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .expect("relay 的 TCP 端口一直连不上");
        let local = stream.local_addr().unwrap();
        let (reader, writer) = stream.into_split();
        Self { id, key, reader: FrameReader::new(reader), writer, local }
    }

    pub async fn send(&mut self, datagram: &[u8]) {
        let mut framed = bytes::BytesMut::new();
        tcp_frame::put_frame(&mut framed, datagram).unwrap();
        self.writer.write_all(&framed).await.unwrap();
    }

    pub async fn recv(&mut self) -> Bytes {
        tokio::time::timeout(WAIT, self.reader.next()).await.expect("等 relay 的帧超时").unwrap().expect("relay 关掉了这条 TCP")
    }

    /// 这条 TCP 上下一帧是什么；对方关闭了就是 `None`。
    pub async fn next(&mut self) -> Option<Bytes> {
        tokio::time::timeout(WAIT, self.reader.next()).await.expect("等 relay 的帧超时").ok().flatten()
    }

    pub async fn attach(&mut self, relay: &TestRelay) -> Accepted {
        let nonce = i3t_underlay::random_bytes();
        self.send(&Init { node: self.id, nonce }.to_bytes()).await;
        let reply = self.recv().await;
        let Some(Frame::Challenge(challenge)) = Frame::parse(&reply) else { panic!("不是 CHALLENGE：{reply:?}") };
        let message = Auth::signed_message(node(RELAY_ID), self.id, &nonce, &challenge.cookie);
        let signature = *self.key.sign(Auth::PURPOSE, &message).as_bytes();
        self.send(&Auth { node: self.id, nonce, cookie: challenge.cookie, signature }.to_bytes()).await;
        let reply = self.recv().await;
        let Some(Frame::Ok(ok)) = Frame::parse(&reply) else { panic!("不是 OK：{reply:?}") };
        verify_ok(&ok, relay, self.id, nonce);
        ok
    }
}

/// 一个 UDP 端口，同一个端口号在 TCP 上也空着：relay 在同一个端口号上听 TCP。
///
/// 随便挑的 UDP 端口号可能正被一条 TIME_WAIT 的 TCP 连接占着（同一次 `cargo test` 里前面的测试留下的，
/// 例如 ice3proxy 的测试一下子开关几千条连接），那样 relay 的 TCP 监听一直绑不上。先让内核挑一个空着的 TCP 端口号。
fn port_free_for_tcp_too() -> UdpPort {
    loop {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        if let Ok(port) = UdpPort::bind(addr) {
            return port;
        }
    }
}
