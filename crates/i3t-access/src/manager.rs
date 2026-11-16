//! 监听的管理：按 netmap 开关代理监听器和端口转发的监听。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_conn::Connector;
use i3t_model::Netmap;
use i3t_model::netmap::{ForwardProtocol, ListenerProtocol};
use ipnet::IpNet;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::Instrument;

use crate::egress::LocalEgress;
use crate::session::{self, Shared};

/// 绑定失败的监听隔多久重试一次。端口被别的程序占着时不去抢，等它让出来。
const RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// `accept` 出错之后歇多久再试。文件描述符用完这类错误不歇一下会空转。
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// 一个节点的入口：代理监听器和端口转发。
///
/// 监听什么完全由 netmap 决定（`me.listeners` 和 `me.forwards`）。netmap 变了，只动变了的那几个监听；
/// 已经建立的连接各自跑在自己的任务里，监听被关掉也不受影响。
pub struct Access<C, E> {
    shared: Arc<Shared<C, E>>,
    published: Mutex<AccessStatus>,
}

impl<C: Connector, E: LocalEgress> Access<C, E> {
    /// `connector` 用来到对端节点开流，`egress` 用来就地出站。这里还不开始监听，要调用 [`run`](Self::run)。
    ///
    /// `connector` 收的是 `Arc`：连接层的同一个端点通常既是开流的一方、又是接受流的一方，要和出口那一侧共用。
    pub fn new(connector: Arc<C>, egress: E, netmap: watch::Receiver<Arc<Netmap>>) -> Self {
        Self { shared: Arc::new(Shared { connector, egress, netmap, slots: Arc::default() }), published: Mutex::default() }
    }

    /// 按 netmap 维护监听，直到 netmap 的发送端被丢弃（节点在关闭）。
    ///
    /// 只应该有一个在跑。这个 future 结束或者被丢弃时，所有监听都关掉；已经建立的连接继续跑到自己结束。
    pub async fn run(&self) {
        let mut netmap = self.shared.netmap.clone();
        let mut bound: Vec<Bound> = Vec::new();
        loop {
            let current = Arc::clone(&netmap.borrow_and_update());
            self.reconcile(&current, &mut bound).await;
            let any_failed = bound.iter().any(|bound| matches!(bound.state, ListenState::Failed { .. }));
            tokio::select! {
                changed = netmap.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                () = time::sleep(RETRY_INTERVAL), if any_failed => {}
            }
        }
    }

    /// 每个监听器和每条端口转发现在的状态。
    #[must_use]
    pub fn status(&self) -> AccessStatus {
        self.published.lock().expect("access status mutex poisoned").clone()
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
                    if let ListenState::Failed { reason } = &existing.state {
                        let previous = reason.clone();
                        (existing.state, existing.task) = self.listen(&key, &existing.active, Some(&previous)).await;
                    }
                }
                None => {
                    let active = Arc::new(AtomicUsize::new(0));
                    let (state, task) = match plan {
                        Plan::Bind => self.listen(&key, &active, None).await,
                        Plan::Virtual => {
                            tracing::info!(listen = %key, "这条端口转发监听在本节点的虚拟 IP 上，不占用操作系统的端口");
                            (ListenState::Virtual, None)
                        }
                        Plan::Unsupported => {
                            tracing::warn!(listen = %key, "UDP 的端口转发还没有实现，这一条不会生效");
                            (ListenState::Unsupported, None)
                        }
                    };
                    kept.push(Bound { key, state, active, task });
                }
            }
        }
        *bound = kept;

        let mut status = AccessStatus { version: netmap.version, listeners: Vec::new(), forwards: Vec::new() };
        for entry in bound.iter() {
            let (state, active) = (entry.state.clone(), Active(Arc::clone(&entry.active)));
            match &entry.key {
                Key::Listener { name, bind, .. } => {
                    status.listeners.push(ListenerStatus { name: name.clone(), bind: *bind, state, active })
                }
                Key::Forward { id, bind } => status.forwards.push(ForwardStatus { id: *id, bind: *bind, state, active }),
            }
        }
        *self.published.lock().expect("access status mutex poisoned") = status;
    }

    /// 绑定 `key` 的地址并开始接受连接。绑不上时不是错误，而是一个要报告的状态。
    ///
    /// `previous_failure` 是上一次失败的原因：重试时原因没变就不再记日志。
    async fn listen(&self, key: &Key, active: &Arc<AtomicUsize>, previous_failure: Option<&str>) -> (ListenState, Option<JoinHandle<()>>) {
        let bind = key.bind();
        let listener = match TcpListener::bind(bind).await {
            Ok(listener) => listener,
            Err(error) => {
                let reason = error.to_string();
                if previous_failure != Some(reason.as_str()) {
                    tracing::warn!(listen = %key, %reason, "绑定失败，这个监听处于失败状态，之后会重试");
                }
                return (ListenState::Failed { reason }, None);
            }
        };
        // 配置里端口写 0 时，实际的端口要问系统。
        let local = listener.local_addr().unwrap_or(bind);
        tracing::info!(listen = %key, %local, "开始监听");
        let task = tokio::spawn(accept_loop(Arc::clone(&self.shared), key.clone(), listener, Arc::clone(active)));
        (ListenState::Listening { local }, Some(task))
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
    pub state: ListenState,
    pub active: Active,
}

/// 一条端口转发（监听一侧）的状态。
#[derive(Debug, Clone)]
pub struct ForwardStatus {
    pub id: u32,
    /// 配置里写的监听地址。
    pub bind: SocketAddr,
    pub state: ListenState,
    pub active: Active,
}

/// 一个监听现在的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListenState {
    /// 正在监听。`local` 是实际绑定的地址：配置里端口写 0 时，这里是系统挑的端口。
    Listening { local: SocketAddr },
    /// 没有绑上，`reason` 是系统给的原因（比如端口已经被占用）。每隔一段时间和每次 netmap 变化时重试。
    Failed { reason: String },
    /// 监听在本节点的虚拟 IP 上的端口转发：不占用操作系统的端口，由出口模块在别的节点访问这个端口时接住。
    Virtual,
    /// UDP 的端口转发，还没有实现。
    Unsupported,
}

/// 一个监听上现在有多少条连接。读到的是调用那一刻的值，不是状态生成时的。
#[derive(Debug, Clone)]
pub struct Active(Arc<AtomicUsize>);

impl Active {
    #[must_use]
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

/// 一个监听是谁。地址或协议变了就是另一个监听（要重新绑定）；别的设置变了还是同一个，新的连接按新的设置处理。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Listener { name: String, bind: SocketAddr, protocol: ListenerProtocol },
    Forward { id: u32, bind: SocketAddr },
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
            Self::Forward { id, bind } => write!(f, "端口转发 {id}（{bind}）"),
        }
    }
}

/// netmap 要求的一个监听怎么落实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Bind,
    Virtual,
    Unsupported,
}

fn wanted(netmap: &Netmap) -> Vec<(Key, Plan)> {
    let listeners = netmap
        .me
        .listeners
        .iter()
        .map(|listener| (Key::Listener { name: listener.name.clone(), bind: listener.bind, protocol: listener.protocol }, Plan::Bind));
    let forwards = netmap.me.forwards.iter().map(|forward| {
        let plan = match forward.protocol {
            ForwardProtocol::Udp => Plan::Unsupported,
            ForwardProtocol::Tcp if forward.bind.ip() == netmap.me.virtual_ip => Plan::Virtual,
            ForwardProtocol::Tcp => Plan::Bind,
        };
        (Key::Forward { id: forward.id, bind: forward.bind }, plan)
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
    active: Arc<AtomicUsize>,
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

async fn accept_loop<C: Connector, E: LocalEgress>(shared: Arc<Shared<C, E>>, key: Key, listener: TcpListener, active: Arc<AtomicUsize>) {
    loop {
        match listener.accept().await {
            Ok((stream, from)) => {
                let connection = Connection { active: Arc::clone(&active) };
                connection.active.fetch_add(1, Ordering::Relaxed);
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

/// 一条连接占着的计数，连接的任务结束时减回去。
struct Connection {
    active: Arc<AtomicUsize>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 处理一条刚接受的连接。监听器的设置每次都从当前的 netmap 里取，所以改设置不需要重新绑定。
async fn serve<C: Connector, E: LocalEgress>(
    shared: Arc<Shared<C, E>>,
    key: Key,
    stream: TcpStream,
    from: SocketAddr,
    _connection: Connection,
) {
    let netmap = Arc::clone(&shared.netmap.borrow());
    if let Err(error) = stream.set_nodelay(true) {
        tracing::debug!(%error, "设置不了 TCP_NODELAY");
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
            session::proxy(&shared, &netmap, listener, stream, from).await;
        }
        Key::Forward { id, bind } => {
            let Some(forward) = netmap.me.forwards.iter().find(|forward| forward.id == id && forward.bind == bind) else { return };
            if !allowed(&forward.allow_from, from) {
                return tracing::info!("拒绝了一条连接：来源不在这条端口转发的白名单里");
            }
            session::forward(&shared, &netmap, forward, stream, from).await;
        }
    }
}

/// 来源白名单。空的表示不限制。
fn allowed(allow_from: &[IpNet], from: SocketAddr) -> bool {
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
