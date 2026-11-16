//! 监听的管理：按 netmap 开关代理监听器和端口转发的监听。

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_conn::Connector;
use i3t_conn::contract::Datagrams;
use i3t_flow::datagram::Opener;
use i3t_model::Netmap;
use i3t_model::netmap::{ForwardProtocol, ListenerProtocol};
use ipnet::IpNet;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::Instrument;

use crate::egress::LocalEgress;
use crate::session::{self, Shared};
use crate::udp;
use crate::usage::{CredentialSnapshot, ExitSnapshot, Tally, Usage};

/// 绑定失败的监听隔多久重试一次。端口被别的程序占着时不去抢，等它让出来。
const RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// `accept` 出错之后歇多久再试。文件描述符用完这类错误不歇一下会空转。
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// UDP 端口转发的会话没有报文往来多久之后回收（白皮书 §9.6）。
const UDP_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 端口写 0 的端口转发挑定的端口记在哪里（白皮书 §9.6："挑定之后固定下来，重启不变"）。
///
/// 装配层把它存在节点的状态目录里。记着的端口下次启动时被别的程序占了，这条转发处于"端口被占用"的状态，
/// 不会换一个端口：换了的话，用它的人手里的端口号就悄悄失效了。
pub trait PortBook: Send + Sync + 'static {
    /// 这条转发上次挑定的端口。
    fn port(&self, forward: u32) -> Option<u16>;
    /// 这条转发挑定了 `port`。
    fn remember(&self, forward: u32, port: u16);
}

/// 不记：每次都让系统重新挑。静态模式和测试用。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPortBook;

impl PortBook for NoPortBook {
    fn port(&self, _forward: u32) -> Option<u16> {
        None
    }

    fn remember(&self, _forward: u32, _port: u16) {}
}

/// 一个节点的入口：代理监听器和端口转发。
///
/// 监听什么完全由 netmap 决定（`me.listeners` 和 `me.forwards`）。netmap 变了，只动变了的那几个监听；
/// 已经建立的连接各自跑在自己的任务里，监听被关掉也不受影响。
///
/// UDP 也从这里进来：SOCKS5 监听器上的 UDP ASSOCIATE，以及协议是 UDP 的端口转发。它们到对端节点走数据报上下文，
/// 所以 `C` 除了能开流，还要能收发数据报。
///
/// 节点排空时（[`Access::set_draining`]）监听照常开着，新的请求得到明确的拒绝：代理监听器回 SOCKS5 的 0x01、
/// HTTP 的 503；端口转发没有地方回错误，TCP 的连接被重置，UDP 不建新的会话。已经建立的连接照常跑完。
pub struct Access<C: Datagrams, E> {
    shared: Arc<Shared<C, E>>,
    published: Mutex<AccessStatus>,
}

impl<C: Connector + Datagrams, E: LocalEgress> Access<C, E> {
    /// `connector` 用来到对端节点开流和数据报上下文，`egress` 用来就地出站。这里还不开始监听，要调用 [`run`](Self::run)。
    ///
    /// `connector` 收的是 `Arc`：连接层的同一个端点通常既是开流的一方、又是接受流的一方，要和出口那一侧共用。
    pub fn new(connector: Arc<C>, egress: E, netmap: watch::Receiver<Arc<Netmap>>) -> Self {
        let opener = Arc::new(Opener::new(Arc::clone(&connector)));
        let shared = Shared {
            connector,
            opener,
            egress,
            netmap,
            usage: Usage::default(),
            ports: Arc::new(NoPortBook),
            draining: Mutex::new(None),
            drained: AtomicU64::new(0),
            udp_idle_timeout: UDP_IDLE_TIMEOUT,
        };
        Self { shared: Arc::new(shared), published: Mutex::default() }
    }

    /// UDP 端口转发的会话没有报文往来多久之后回收。默认 60 秒；测试里调短，不用真的等一分钟。
    ///
    /// # Panics
    ///
    /// 在 [`run`](Self::run) 已经开始之后调用。
    #[must_use]
    pub fn with_udp_idle_timeout(mut self, timeout: Duration) -> Self {
        self.shared_mut("with_udp_idle_timeout").udp_idle_timeout = timeout;
        self
    }

    /// 端口写 0 的端口转发挑定的端口记在哪里。不设时每次启动都重新挑。
    ///
    /// # Panics
    ///
    /// 在 [`run`](Self::run) 已经开始之后调用。
    #[must_use]
    pub fn with_port_book(mut self, ports: Arc<dyn PortBook>) -> Self {
        self.shared_mut("with_port_book").ports = ports;
        self
    }

    /// 本节点开数据报上下文用的那一个。一个节点只该有一个（对端发回来的数据报由它分发，见 [`run`](Self::run)），
    /// 出口那一侧接力 UDP 端口转发时要用同一个。
    #[must_use]
    pub fn datagram_opener(&self) -> Arc<Opener<C>> {
        Arc::clone(&self.shared.opener)
    }

    fn shared_mut(&mut self, what: &str) -> &mut Shared<C, E> {
        Arc::get_mut(&mut self.shared).unwrap_or_else(|| panic!("{what}() 要在 run() 之前调用：那时还没有监听拿着这些设置"))
    }

    /// 按 netmap 维护监听，直到 netmap 的发送端被丢弃（节点在关闭）。
    ///
    /// 只应该有一个在跑。这个 future 结束或者被丢弃时，所有监听都关掉；已经建立的连接继续跑到自己结束。
    /// 对端节点在数据报上下文里发回来的数据报也是在这里面分发的。
    pub async fn run(&self) {
        let mut netmap = self.shared.netmap.clone();
        let mut bound: Vec<Bound> = Vec::new();
        let datagrams = self.shared.opener.run();
        tokio::pin!(datagrams);
        let mut pumping = true;
        loop {
            let current = Arc::clone(&netmap.borrow_and_update());
            self.reconcile(&current, &mut bound).await;
            let any_failed = bound.iter().any(|bound| matches!(bound.state, ListenState::Failed { .. } | ListenState::PortInUse { .. }));
            // 等到下一次要调整监听的时候；这期间一直在分发数据报。
            loop {
                tokio::select! {
                    changed = netmap.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        break;
                    }
                    () = time::sleep(RETRY_INTERVAL), if any_failed => break,
                    // 连接层关掉了：数据报不会再来，监听照常维护到节点关闭。
                    () = &mut datagrams, if pumping => pumping = false,
                }
            }
        }
    }

    /// 每个监听器和每条端口转发现在的状态。
    #[must_use]
    pub fn status(&self) -> AccessStatus {
        self.published.lock().expect("access status mutex poisoned").clone()
    }

    /// 开始或者结束排空。`Some(说明)` 时新的连接得到拒绝（见类型的文档），说明写进日志。
    pub fn set_draining(&self, draining: Option<String>) {
        *self.shared.draining.lock().expect("draining mutex poisoned") = draining;
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.shared.draining.lock().expect("draining mutex poisoned").is_some()
    }

    /// 启动以来因为排空而拒绝的连接（和 UDP 会话）。
    #[must_use]
    pub fn drained(&self) -> u64 {
        self.shared.drained.load(Ordering::Relaxed)
    }

    /// 这次运行以来每个用过的接入凭据的用量。
    #[must_use]
    pub fn credential_usage(&self) -> Vec<CredentialSnapshot> {
        self.shared.usage.credentials()
    }

    /// 这次运行以来经各个出口出去的流，本节点作为发起方看到的。
    #[must_use]
    pub fn exit_usage(&self) -> Vec<ExitSnapshot> {
        self.shared.usage.exits()
    }

    /// 让 `bound` 和 `netmap` 要求的监听一致：关掉不再需要的，绑定新增的，重试上次没绑上的。
    async fn reconcile(&self, netmap: &Netmap, bound: &mut Vec<Bound>) {
        let wanted = wanted(netmap);

        // 先关后开：监听器改了名字但地址没变时，新的要等旧的让出端口才绑得上。
        let mut kept = Vec::with_capacity(wanted.len());
        for existing in bound.drain(..) {
            if wanted.iter().any(|(key, _)| *key == existing.key) {
                kept.push(existing);
            } else {
                tracing::info!(listen = %existing.key, "netmap 里不再有这个监听，关掉它（已经建立的连接不受影响）");
                existing.close().await;
            }
        }

        for (key, plan) in wanted {
            match kept.iter_mut().find(|existing| existing.key == key) {
                Some(existing) => {
                    if let Some(previous) = existing.state.failure().map(str::to_owned) {
                        (existing.state, existing.task) = self.listen(&key, &existing.counters, Some(&previous)).await;
                    }
                }
                None => {
                    let counters = Counters::default();
                    let (state, task) = match plan {
                        Plan::Bind => self.listen(&key, &counters, None).await,
                        Plan::Virtual => {
                            tracing::info!(listen = %key, "这条端口转发监听在本节点的虚拟 IP 上，不占用操作系统的端口");
                            (ListenState::Virtual, None)
                        }
                        Plan::Unsupported(reason) => {
                            tracing::warn!(listen = %key, reason, "这条端口转发不能用");
                            (ListenState::Unsupported { reason: reason.to_owned() }, None)
                        }
                    };
                    kept.push(Bound { key, state, counters, task });
                }
            }
        }
        *bound = kept;

        let mut status = AccessStatus { version: netmap.version, listeners: Vec::new(), forwards: Vec::new() };
        for entry in bound.iter() {
            let (state, active, traffic) =
                (entry.state.clone(), Active(Arc::clone(&entry.counters.active)), Traffic(Arc::clone(&entry.counters.traffic)));
            match &entry.key {
                Key::Listener { name, bind, protocol } => {
                    status.listeners.push(ListenerStatus { name: name.clone(), bind: *bind, protocol: *protocol, state, active, traffic });
                }
                Key::Forward { id, bind, protocol } => {
                    status.forwards.push(ForwardStatus { id: *id, bind: *bind, protocol: *protocol, state, active, traffic });
                }
            }
        }
        *self.published.lock().expect("access status mutex poisoned") = status;
    }

    /// 绑定 `key` 的地址并开始接受连接。绑不上时不是错误，而是一个要报告的状态。
    ///
    /// `previous_failure` 是上一次失败的原因：重试时原因没变就不再记日志。
    async fn listen(&self, key: &Key, counters: &Counters, previous_failure: Option<&str>) -> (ListenState, Option<JoinHandle<()>>) {
        let configured = key.bind();
        // 端口写 0 的端口转发：用上次挑定的端口；还没挑过就让系统挑一个，记下来。
        let (bind, chosen_now) = match key {
            Key::Forward { id, .. } if configured.port() == 0 => match self.shared.ports.port(*id) {
                Some(port) => (SocketAddr::new(configured.ip(), port), false),
                None => (configured, true),
            },
            Key::Forward { .. } | Key::Listener { .. } => (configured, false),
        };
        let started = match key {
            Key::Forward { id, protocol: ForwardProtocol::Udp, .. } => UdpSocket::bind(bind).await.map(|socket| {
                let local = socket.local_addr().unwrap_or(bind);
                let span = tracing::info_span!("access", listen = %key);
                let sessions = udp::forward::serve(Arc::clone(&self.shared), *id, configured, socket, counters.clone());
                (local, tokio::spawn(sessions.instrument(span)))
            }),
            Key::Forward { protocol: ForwardProtocol::Tcp, .. } | Key::Listener { .. } => TcpListener::bind(bind).await.map(|listener| {
                let local = listener.local_addr().unwrap_or(bind);
                (local, tokio::spawn(accept_loop(Arc::clone(&self.shared), key.clone(), listener, counters.clone())))
            }),
        };
        match started {
            Ok((local, task)) => {
                if let (true, Key::Forward { id, .. }) = (chosen_now, key) {
                    tracing::info!(listen = %key, port = local.port(), "端口转发的端口写的是 0，挑定了这个端口，之后重启也用它");
                    self.shared.ports.remember(*id, local.port());
                }
                tracing::info!(listen = %key, %local, "开始监听");
                (ListenState::Listening { local }, Some(task))
            }
            Err(error) => {
                let reason = error.to_string();
                if previous_failure != Some(reason.as_str()) {
                    tracing::warn!(listen = %key, %bind, %reason, "绑定失败，这个监听处于失败状态，之后会重试");
                }
                let state = if error.kind() == io::ErrorKind::AddrInUse {
                    ListenState::PortInUse { reason }
                } else {
                    ListenState::Failed { reason }
                };
                (state, None)
            }
        }
    }
}

/// 全部监听的状态，对应某一版 netmap。
#[derive(Debug, Clone, Default)]
pub struct AccessStatus {
    /// 这些状态是按哪一版 netmap 算的。
    pub version: u64,
    pub listeners: Vec<ListenerStatus>,
    pub forwards: Vec<ForwardStatus>,
}

/// 一个代理监听器的状态。
#[derive(Debug, Clone)]
pub struct ListenerStatus {
    pub name: String,
    /// 配置里写的绑定地址。
    pub bind: SocketAddr,
    pub protocol: ListenerProtocol,
    pub state: ListenState,
    pub active: Active,
    pub traffic: Traffic,
}

/// 一条端口转发（监听一侧）的状态。
#[derive(Debug, Clone)]
pub struct ForwardStatus {
    pub id: u32,
    /// 配置里写的监听地址。端口写 0 时实际的端口在 [`ListenState::Listening`] 里。
    pub bind: SocketAddr,
    pub protocol: ForwardProtocol,
    pub state: ListenState,
    pub active: Active,
    pub traffic: Traffic,
}

/// 一个监听现在的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListenState {
    /// 正在监听。`local` 是实际绑定的地址：配置里端口写 0 时，这里是挑定的端口。
    Listening { local: SocketAddr },
    /// 端口已经被别的程序占用。不去抢，每隔一段时间和每次 netmap 变化时重试。
    PortInUse { reason: String },
    /// 别的原因没有绑上，`reason` 是系统给的原因。同样会重试。
    Failed { reason: String },
    /// 监听在本节点的虚拟 IP 上的端口转发：不占用操作系统的端口，由出口模块在别的节点访问这个端口时接住。
    Virtual,
    /// 这种配置节点不支持，`reason` 说明为什么。不重试，配置改了才重新判断。
    Unsupported { reason: String },
}

impl ListenState {
    /// 绑定失败（会重试）时的原因。
    fn failure(&self) -> Option<&str> {
        match self {
            Self::PortInUse { reason } | Self::Failed { reason } => Some(reason),
            Self::Listening { .. } | Self::Virtual | Self::Unsupported { .. } => None,
        }
    }
}

/// 一个监听上现在有多少条连接（UDP 的端口转发是多少个会话）。读到的是调用那一刻的值，不是状态生成时的。
#[derive(Debug, Clone)]
pub struct Active(Arc<AtomicUsize>);

impl Active {
    #[must_use]
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

/// 一个监听上这次运行以来的流量。读到的是调用那一刻的值。
#[derive(Debug, Clone)]
pub struct Traffic(Arc<Tally>);

impl Traffic {
    /// 客户端发进来的字节数。
    #[must_use]
    pub fn up(&self) -> u64 {
        self.0.up()
    }

    /// 发回给客户端的字节数。
    #[must_use]
    pub fn down(&self) -> u64 {
        self.0.down()
    }
}

/// 一个监听上的计数：连接数和流量。监听重新绑定时沿用，所以流量是这次运行以来的。
#[derive(Debug, Clone, Default)]
pub(crate) struct Counters {
    pub(crate) active: Arc<AtomicUsize>,
    pub(crate) traffic: Arc<Tally>,
}

/// 一个监听是谁。地址或协议变了就是另一个监听（要重新绑定）；别的设置变了还是同一个，新的连接按新的设置处理。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Listener { name: String, bind: SocketAddr, protocol: ListenerProtocol },
    Forward { id: u32, bind: SocketAddr, protocol: ForwardProtocol },
}

impl Key {
    const fn bind(&self) -> SocketAddr {
        match self {
            Self::Listener { bind, .. } | Self::Forward { bind, .. } => *bind,
        }
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listener { name, bind, protocol } => write!(f, "监听器 {name}（{protocol:?}，{bind}）"),
            Self::Forward { id, bind, protocol } => write!(f, "端口转发 {id}（{protocol:?}，{bind}）"),
        }
    }
}

/// netmap 要求的一个监听怎么落实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Bind,
    Virtual,
    Unsupported(&'static str),
}

fn wanted(netmap: &Netmap) -> Vec<(Key, Plan)> {
    let listeners = netmap
        .me
        .listeners
        .iter()
        .map(|listener| (Key::Listener { name: listener.name.clone(), bind: listener.bind, protocol: listener.protocol }, Plan::Bind));
    let forwards = netmap.me.forwards.iter().map(|forward| {
        let plan = if forward.bind.ip() != netmap.me.virtual_ip {
            Plan::Bind
        } else if forward.bind.port() == 0 {
            Plan::Unsupported("监听在虚拟 IP 上的端口转发不占操作系统的端口，要写明端口号")
        } else {
            Plan::Virtual
        };
        (Key::Forward { id: forward.id, bind: forward.bind, protocol: forward.protocol }, plan)
    });
    let mut wanted: Vec<(Key, Plan)> = Vec::new();
    // 配置里重复的条目只留第一个：同一个地址本来也只能绑一次。
    for entry in listeners.chain(forwards) {
        if !wanted.iter().any(|(key, _)| *key == entry.0) {
            wanted.push(entry);
        }
    }
    wanted
}

/// 一个已经落实（或者落实失败）的监听。
struct Bound {
    key: Key,
    state: ListenState,
    counters: Counters,
    /// 接受连接的任务，监听的 socket 在它手里。
    task: Option<JoinHandle<()>>,
}

impl Bound {
    /// 关掉监听，等到 socket 真的关上再返回，这样紧接着绑同一个地址不会撞上自己。
    async fn close(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            // 任务是被取消的，它的结果只会是"已取消"。
            let _ = task.await;
        }
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn accept_loop<C: Connector + Datagrams, E: LocalEgress>(
    shared: Arc<Shared<C, E>>,
    key: Key,
    listener: TcpListener,
    counters: Counters,
) {
    loop {
        match listener.accept().await {
            Ok((stream, from)) => {
                let connection = Connection::enter(&counters);
                let span = tracing::info_span!("access", listen = %key, %from);
                tokio::spawn(serve(Arc::clone(&shared), key.clone(), stream, from, connection).instrument(span));
            }
            Err(error) => {
                tracing::warn!(listen = %key, %error, "接受连接出错");
                time::sleep(ACCEPT_BACKOFF).await;
            }
        }
    }
}

/// 一条连接占着的计数，连接的任务结束时减回去。它也带着这个监听的流量计数，搬运时往里记。
pub(crate) struct Connection {
    active: Arc<AtomicUsize>,
    pub(crate) traffic: Arc<Tally>,
}

impl Connection {
    pub(crate) fn enter(counters: &Counters) -> Self {
        counters.active.fetch_add(1, Ordering::Relaxed);
        Self { active: Arc::clone(&counters.active), traffic: Arc::clone(&counters.traffic) }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 处理一条刚接受的连接。监听器的设置每次都从当前的 netmap 里取，所以改设置不需要重新绑定。
async fn serve<C: Connector + Datagrams, E: LocalEgress>(
    shared: Arc<Shared<C, E>>,
    key: Key,
    stream: TcpStream,
    from: SocketAddr,
    connection: Connection,
) {
    let netmap = Arc::clone(&shared.netmap.borrow());
    if let Err(error) = stream.set_nodelay(true) {
        tracing::debug!(%error, "设置不了 TCP_NODELAY");
    }
    // 流没有空闲时限，客户端悄悄消失（断电、换了网络）时靠内核保活发现，这条连接经过的流和出口上的连接才会释放。
    if let Err(error) = i3t_flow::keep_alive(&stream) {
        tracing::debug!(%error, "客户端连接打不开内核保活，客户端悄悄消失时这条连接要等到有数据要发才会发现");
    }
    match key {
        Key::Listener { name, bind, protocol } => {
            let current =
                netmap.me.listeners.iter().find(|listener| listener.name == name && listener.bind == bind && listener.protocol == protocol);
            // 找不到说明它刚从 netmap 里删掉，监听马上就会关。
            let Some(listener) = current else { return };
            if !allowed(&listener.allow_from, from) {
                return tracing::info!("拒绝了一条连接：来源不在这个监听器的白名单里");
            }
            session::proxy(&shared, &netmap, listener, stream, from, &connection).await;
        }
        Key::Forward { id, bind, .. } => {
            let Some(forward) = netmap.me.forwards.iter().find(|forward| forward.id == id && forward.bind == bind) else { return };
            if !allowed(&forward.allow_from, from) {
                return tracing::info!("拒绝了一条连接：来源不在这条端口转发的白名单里");
            }
            session::forward(&shared, &netmap, forward, stream, from, &connection).await;
        }
    }
}

/// 来源白名单。空的表示不限制。
pub(crate) fn allowed(allow_from: &[IpNet], from: SocketAddr) -> bool {
    let ip = from.ip().to_canonical();
    allow_from.is_empty() || allow_from.iter().any(|net| net.contains(&ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_allow_list_admits_everyone_and_a_filled_one_only_its_networks() {
        let from = |text: &str| text.parse::<SocketAddr>().unwrap();
        assert!(allowed(&[], from("203.0.113.9:1")));

        let list: Vec<IpNet> = vec!["192.168.1.0/24".parse().unwrap(), "127.0.0.1/32".parse().unwrap(), "fd00::/8".parse().unwrap()];
        assert!(allowed(&list, from("192.168.1.77:1")));
        assert!(allowed(&list, from("127.0.0.1:1")));
        assert!(allowed(&list, from("[fd00::7]:1")));
        assert!(allowed(&list, from("[::ffff:192.168.1.77]:1")), "双栈 socket 上的 IPv4 客户端按 IPv4 比对");
        assert!(!allowed(&list, from("192.168.2.1:1")));
        assert!(!allowed(&list, from("127.0.0.2:1")));
        assert!(!allowed(&list, from("[2001:db8::1]:1")));
    }
}
