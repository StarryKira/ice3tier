//! 一个启用了的 relay，上面已经有三个会话：节点 2、节点 3，和另一个 relay 4（骨干）。
//!
//! 每个会话的主人是一个回环上的 UDP socket，各用一个不同的回环地址：relay 按来源 IP 给握手限速，
//! fuzz 输入里冒充的来源挤不掉它们的名额。输入从这几个地址之一，或者一个陌生的地址，交给 [`Relay::handle_udp`]，
//! 和路径层把端口上收到的一批数据报交给它一样。会话每两秒保活一次，掉了就重新握手。

use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use i3t_crypto::NodeKey;
use i3t_model::Netmap;
use i3t_proto::NodeId;
use i3t_proto::session::{Auth, Frame, Init, Ping};
use i3t_relay::Relay;
use i3t_underlay::{Ecn, UdpPort};
use tokio::runtime::Runtime;
use tokio::sync::watch;

/// relay 自己的节点编号。
pub const RELAY_ID: u32 = 100;
/// 挂在 relay 上的两个节点，和网络里另一个 relay。
pub const MEMBER_IDS: [u32; 3] = [2, 3, 4];

const KEEPALIVE: Duration = Duration::from_secs(2);
/// 等 relay 的应答等多久。都在回环上，这个数留了很大的余量。
const ANSWER_WAIT: Duration = Duration::from_secs(1);

/// 输入的数据报从哪里来。
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// [`MEMBER_IDS`] 里的第几个。
    Member(usize),
    /// 没有会话的地址。
    Stranger,
}

pub struct RelayWorld {
    /// relay 的后台任务挂在这个运行时上；它从不被驱动，`handle_udp` 是同步的，用不到它们。
    _runtime: Runtime,
    /// 发送端丢了，relay 会当作节点在关闭。
    _netmap: watch::Sender<Arc<Netmap>>,
    relay: Relay,
    members: Vec<Member>,
    stranger: SocketAddr,
    last_keepalive: Mutex<Instant>,
}

struct Member {
    id: NodeId,
    key: NodeKey,
    socket: UdpSocket,
}

impl RelayWorld {
    /// 整个进程共用的一个。第一次调用时建好、握好手。
    ///
    /// # Panics
    ///
    /// 回环上的 socket 绑不上，或者 relay 不肯给节点开会话：环境本身坏了，fuzz 没有意义。
    pub fn get() -> &'static Self {
        static WORLD: OnceLock<RelayWorld> = OnceLock::new();
        WORLD.get_or_init(Self::start)
    }

    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("建不了 tokio 运行时");
        let entered = runtime.enter();
        let members: Vec<Member> = MEMBER_IDS
            .iter()
            .zip(2u8..)
            .map(|(&raw, host)| Member {
                id: node(raw),
                key: NodeKey::generate().expect("生成不了节点密钥"),
                socket: bind(SocketAddr::from(([127, 0, 0, host], 0))),
            })
            .collect();
        let port = UdpPort::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("relay 的 UDP 端口绑不上");
        let key = Arc::new(NodeKey::generate().expect("生成不了 relay 的密钥"));
        let (netmap_sender, netmap) = watch::channel(Arc::new(netmap(&members)));
        let relay = Relay::new(key, Arc::new(port), netmap);
        relay.set_local_delivery(|_, _| {});
        assert!(relay.is_enabled(), "netmap 里本节点是 relay，relay 却没有启用");
        drop(entered);
        let world = Self {
            _runtime: runtime,
            _netmap: netmap_sender,
            relay,
            members,
            stranger: SocketAddr::from(([127, 0, 0, 9], 40_000)),
            last_keepalive: Mutex::new(Instant::now()),
        };
        for member in &world.members {
            assert!(world.attach(member), "节点 {} 和 relay 握不上手", member.id);
        }
        world
    }

    /// 把 `data`（每 `stride` 字节一个数据报）当作从 `source` 收到的交给 relay。
    pub fn deliver(&self, source: Source, ecn: Option<Ecn>, data: &[u8], stride: usize) {
        self.keep_alive();
        let from = match source {
            Source::Member(index) => self.members[index % self.members.len()].addr(),
            Source::Stranger => self.stranger,
        };
        self.relay.handle_udp(from, ecn, data, stride);
    }

    fn keep_alive(&self) {
        let mut last = self.last_keepalive.lock().expect("relay keepalive mutex poisoned");
        if last.elapsed() < KEEPALIVE {
            return;
        }
        *last = Instant::now();
        for member in &self.members {
            drain(&member.socket);
            if !self.relay.has_session(member.id) {
                assert!(self.attach(member), "节点 {} 的会话掉了，重新握手也握不上", member.id);
                continue;
            }
            let ping = Ping { node: member.id, nonce: i3t_underlay::random_bytes() };
            self.relay.handle_udp(member.addr(), None, &ping.to_bytes(), Ping::LEN);
        }
    }

    /// 走一遍握手：INIT、CHALLENGE、AUTH、OK。
    fn attach(&self, member: &Member) -> bool {
        drain(&member.socket);
        let nonce = i3t_underlay::random_bytes();
        self.relay.handle_udp(member.addr(), None, &Init { node: member.id, nonce }.to_bytes(), Init::LEN);
        let Some(Frame::Challenge(challenge)) = answer(&member.socket) else { return false };
        let message = Auth::signed_message(node(RELAY_ID), member.id, &nonce, &challenge.cookie);
        let signature = *member.key.sign(Auth::PURPOSE, &message).as_bytes();
        let auth = Auth { node: member.id, nonce, cookie: challenge.cookie, signature };
        self.relay.handle_udp(member.addr(), None, &auth.to_bytes(), Auth::LEN);
        matches!(answer(&member.socket), Some(Frame::Ok(_)))
    }
}

impl Member {
    fn addr(&self) -> SocketAddr {
        self.socket.local_addr().expect("绑好的 socket 一定有本地地址")
    }
}

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).expect("编号不是 0")
}

fn bind(addr: SocketAddr) -> UdpSocket {
    let socket = UdpSocket::bind(addr).unwrap_or_else(|error| panic!("{addr} 绑不上：{error}"));
    socket.set_read_timeout(Some(ANSWER_WAIT)).expect("设不了读的时限");
    socket
}

/// relay 发来的下一个中转会话帧。中间夹着的别的数据报（转发过来的帧、矩阵行）跳过去。
fn answer(socket: &UdpSocket) -> Option<Frame> {
    let deadline = Instant::now() + ANSWER_WAIT;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        let len = socket.recv(&mut buf).ok()?;
        if let Some(frame @ (Frame::Challenge(_) | Frame::Ok(_))) = Frame::parse(&buf[..len]) {
            return Some(frame);
        }
    }
    None
}

/// 丢掉 socket 里攒着的数据报：relay 转给会话的东西没有人要。
fn drain(socket: &UdpSocket) {
    socket.set_nonblocking(true).expect("设不了非阻塞");
    let mut buf = [0u8; 2048];
    while socket.recv(&mut buf).is_ok() {}
    socket.set_nonblocking(false).expect("设不了阻塞");
}

fn netmap(members: &[Member]) -> Netmap {
    let [client_a, client_b, other_relay] = members else { panic!("正好三个会话的主人") };
    let identity = |member: &Member| serde_json::json!({ "id": member.id, "key": member.key.public() });
    let netmap = serde_json::json!({
        "version": 1,
        "network": { "name": "fuzz", "domain": "fuzz.i3t", "cidr": "10.66.0.0/16", "max_relay_hops": 4 },
        "me": { "id": RELAY_ID, "name": "relay", "virtual_ip": "10.66.0.100", "relay": {} },
        "relays": [{ "id": other_relay.id, "key": other_relay.key.public(), "name": "relay-4" }],
        "relay_clients": [identity(client_a), identity(client_b)],
    });
    serde_json::from_value(netmap).expect("手写的 netmap 应该读得进来")
}
