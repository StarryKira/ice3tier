//! 一个节点的数据面：把 UDP 端口、路径层、relay 角色和 QUIC 装在一起。

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use i3t_crypto::NodeKey;
use i3t_model::Netmap;
use i3t_model::control::{RouteEntry, Signal};
use i3t_model::netmap::Attachment;
use i3t_path::{PathConfig, Paths, RouteChange};
use i3t_proto::NodeId;
use i3t_relay::{Relay, RelayCounters};
use i3t_underlay::{PacketFilter, UdpPort};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::contract::{Acceptor, Connector, Incoming, OpenError};
use crate::manager::Manager;
use crate::quic::{self, QuicConfigError};
use crate::socket::PathSocket;
use crate::stream::{RecvHalf, SendHalf};

/// 对端开过来、上层还没有取走的流最多排多少条。满了之后那条连接上新的流先不接，压力传回对端。
const INCOMING_BACKLOG: usize = 256;

/// 建一个 [`Endpoint`] 要的东西。
#[derive(Debug)]
pub struct EndpointConfig {
    /// 节点密钥。
    pub key: Arc<NodeKey>,
    /// UDP 端口绑定在哪里。本节点启用为 relay 时，同一个地址和端口号上还会监听 TCP。
    pub bind: SocketAddr,
    /// 节点的配置。随时可以换成新的一份，立即生效。
    pub netmap: watch::Receiver<Arc<Netmap>>,
    /// 地址查询的服务器（通常是 controller 的那个 UDP 端口）。可以为空：有 relay 时反射地址也能从 relay 得到。
    pub addr_servers: Vec<SocketAddr>,
}

type SignalHook = Arc<dyn Fn(NodeId, &Signal) + Send + Sync>;

/// 一个节点的数据面。
///
/// 它实现 [`Connector`] 和 [`Acceptor`]：到某个节点开一条流，接受某个节点开过来的流。包走直连还是经 relay、
/// 什么时候换路、和对端之间有没有连接，上层都不用管。本节点在 netmap 里被启用为 relay 时，它同时转发别人的包。
///
/// 谁能访问谁（netmap 里的 `inbound`）不在这一层检查：netmap 里有的对端都能连进来、开流，
/// 上层收到流之后按发起节点自己查。
///
/// 丢弃这个值会关掉所有连接、中转会话和后台任务。
pub struct Endpoint {
    port: Arc<UdpPort>,
    paths: Arc<Paths>,
    relay: Arc<Relay>,
    manager: Arc<Manager>,
    incoming: tokio::sync::Mutex<mpsc::Receiver<Incoming<SendHalf, RecvHalf>>>,
    /// 装配方提供的信令出口。
    signal_hook: Arc<RwLock<Option<SignalHook>>>,
    tasks: [JoinHandle<()>; 2],
}

impl Endpoint {
    /// 绑定端口并启动。要在 tokio 运行时里调用。
    ///
    /// # Errors
    ///
    /// 端口绑不上，或者建不出 QUIC 的配置（节点密钥有问题）。
    pub fn bind(config: EndpointConfig) -> Result<Self, EndpointError> {
        let EndpointConfig { key, bind, netmap, addr_servers } = config;
        let me = netmap.borrow().me.id;
        let port = Arc::new(UdpPort::bind(bind).map_err(|source| EndpointError::Bind { addr: bind, source })?);
        let paths =
            Arc::new(Paths::new(PathConfig { key: Arc::clone(&key), port: Arc::clone(&port), netmap: netmap.clone(), addr_servers }));

        let relay = Arc::new(Relay::new(Arc::clone(&key), Arc::clone(&port), netmap.clone()));
        paths.set_local_relay(Arc::clone(&relay) as Arc<dyn i3t_path::LocalRelay>);
        // relay 只拿路径层的弱引用：路径层已经拿着 relay 了，两边都是强引用的话谁也释放不掉。
        let weak_paths = Arc::downgrade(&paths);
        relay.set_local_delivery(move |src, packet| {
            if let Some(paths) = weak_paths.upgrade() {
                paths.deliver_local(src, packet);
            }
        });

        // 接受连接时认不认对端的公钥，每次握手都按当时的 netmap 来。
        let allowed = netmap.clone();
        let server = quic::server(&key, move |key| allowed.borrow().peers.iter().any(|peer| peer.key == *key))?;
        let socket = PathSocket::new(Arc::clone(&paths), me);
        let quic = quinn::Endpoint::new_with_abstract_socket(quic::endpoint()?, Some(server), socket, Arc::new(quinn::TokioRuntime))
            .map_err(EndpointError::Quic)?;

        let (incoming_tx, incoming_rx) = mpsc::channel(INCOMING_BACKLOG);
        let manager = Manager::new(key, quic, Arc::clone(&paths), netmap, incoming_tx);

        // 信令两条路都走：已经有连接的话在连接里送（有 relay 的网络因此不依赖 controller），同时交给装配方的钩子。
        let signal_hook: Arc<RwLock<Option<SignalHook>>> = Arc::default();
        let (in_band, external) = (Arc::downgrade(&manager), Arc::clone(&signal_hook));
        paths.set_signal_hook(move |to, signal| {
            if let Some(manager) = in_band.upgrade() {
                manager.send_signal(to, signal);
            }
            let hook = external.read().expect("signal hook lock poisoned").clone();
            if let Some(hook) = hook {
                hook(to, signal);
            }
        });

        let tasks = [tokio::spawn(Arc::clone(&manager).accept_loop()), tokio::spawn(Arc::clone(&manager).follow_netmap())];
        tracing::info!(node = %me, addr = %port.local_addr(), "数据面启动");
        Ok(Self { port, paths, relay, manager, incoming: tokio::sync::Mutex::new(incoming_rx), signal_hook, tasks })
    }

    /// UDP 端口实际绑定的地址。
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.port.local_addr()
    }

    /// 到每个对端现在走哪条路，延迟多少。
    #[must_use]
    pub fn routes(&self) -> Vec<RouteEntry> {
        self.paths.routes()
    }

    /// 本节点的候选端点，上报给 controller 用。
    #[must_use]
    pub fn endpoints(&self) -> Vec<SocketAddr> {
        self.paths.endpoints()
    }

    /// 映射出来的端口是不是随对端变化。要有两个以上的观察点才知道。
    #[must_use]
    pub fn mapping_varies(&self) -> Option<bool> {
        self.paths.mapping_varies()
    }

    /// 本节点挂靠的 relay 和到它们的延迟，按延迟从低到高。上报给 controller 用。
    #[must_use]
    pub fn attached(&self) -> Vec<Attachment> {
        self.paths.attached()
    }

    /// 订阅换路事件。每一次换路都有一条，带着给人看的原因。
    #[must_use]
    pub fn route_changes(&self) -> broadcast::Receiver<RouteChange> {
        self.paths.subscribe()
    }

    /// 本节点作为 relay 的计数。没有启用 relay 时除了"因为没启用而丢掉"的那一项都是 0。
    #[must_use]
    pub fn relay_counters(&self) -> RelayCounters {
        self.relay.counters()
    }

    /// 打洞信令从哪里送出去：`hook(收信令的节点, 信令)`，装配方把它经 controller 的通道转给那个节点。
    /// 钩子在后台任务里被调用，不能阻塞。
    ///
    /// # Panics
    ///
    /// 之前设的钩子自己 panic 过（锁中毒）。
    pub fn set_signal_hook(&self, hook: impl Fn(NodeId, &Signal) + Send + Sync + 'static) {
        *self.signal_hook.write().expect("signal hook lock poisoned") = Some(Arc::new(hook));
    }

    /// controller 转来了节点 `from` 发的一条信令。
    pub fn deliver_signal(&self, from: NodeId, signal: Signal) {
        self.paths.handle_signal(from, signal);
    }

    /// 装上或者卸掉 UDP 端口上的丢包过滤器。只给测试用，用来模拟某条路不通。
    pub fn set_packet_filter(&self, filter: Option<PacketFilter>) {
        self.port.set_filter(filter);
    }

    /// 关闭：所有连接带着关闭帧结束，[`Acceptor::accept`] 返回 `None`，之后的 [`Connector::open`] 失败。
    pub async fn close(&self) {
        self.manager.close().await;
    }
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint").field("local_addr", &self.port.local_addr()).finish_non_exhaustive()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.manager.abort();
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Connector for Endpoint {
    type Send = SendHalf;
    type Recv = RecvHalf;

    async fn open(&self, peer: NodeId) -> Result<(SendHalf, RecvHalf), OpenError> {
        self.manager.open(peer).await
    }
}

impl Acceptor for Endpoint {
    type Send = SendHalf;
    type Recv = RecvHalf;

    async fn accept(&self) -> Option<Incoming<SendHalf, RecvHalf>> {
        let mut closed = self.manager.subscribe_closed();
        let mut incoming = self.incoming.lock().await;
        tokio::select! {
            biased;
            stream = incoming.recv() => stream,
            _ = closed.wait_for(|closed| *closed) => None,
        }
    }
}

/// [`Endpoint::bind`] 失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("绑定 UDP {addr} 失败")]
    Bind { addr: SocketAddr, source: io::Error },
    #[error("建不出连接层的配置")]
    Config(#[from] QuicConfigError),
    #[error("启动连接层失败")]
    Quic(#[source] io::Error),
}
