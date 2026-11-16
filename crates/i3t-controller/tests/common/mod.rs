//! 集成测试共用的东西：一个跑在本机随机端口上的 controller，和一个用真的 TLS 连它的节点。

// 每个测试文件各自编译这个模块，各用其中的一部分。
#![allow(dead_code, reason = "不同的测试文件用到的辅助函数不一样")]
// clippy.toml 里的 allow-unwrap-in-tests 只认 `#[test]` 函数本身，认不出这里的辅助函数。
#![allow(clippy::unwrap_used, reason = "整个文件都是测试代码，出错就该直接失败")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use i3t_controller::frame::{read_frame, write_frame};
use i3t_controller::store::Store;
use i3t_controller::{ALPN, Controller, ServeOptions, Server, Timing};
use i3t_crypto::{NodeKey, tls};
use i3t_model::Netmap;
use i3t_model::admin::{CreateEnrollToken, CreateNetwork, Network, NetworkId};
use i3t_model::control::{Declared, EnrollToken, Hello, MAX_FRAME_LEN, ToController, ToNode};
use i3t_proto::{NodeId, PublicKey};
use rustls::pki_types::ServerName;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// 等一条消息最多等这么久。本机上正常只要几毫秒，放宽是为了在很忙的机器上不误报。
pub const PATIENCE: Duration = Duration::from_secs(5);

/// 测试里把各种等待都调短。
pub fn fast_timing() -> Timing {
    Timing { ack_coalesce: Duration::from_millis(5), report_coalesce: Duration::from_millis(20), ..Timing::default() }
}

/// 一个正在本机跑着的 controller。丢弃它就停止。
pub struct Running {
    pub controller: Arc<Controller>,
    pub node_addr: SocketAddr,
    pub admin_addr: SocketAddr,
    pub admin_token: String,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Running {
    pub async fn start(timing: Timing) -> Self {
        Self::start_with(timing, None).await
    }

    pub async fn start_with(timing: Timing, static_dir: Option<std::path::PathBuf>) -> Self {
        let store = Store::open_in_memory().unwrap();
        let controller = Controller::new(store, NodeKey::generate().unwrap(), "ctl.example.com:20331".to_owned(), timing).unwrap();
        let admin_token = controller.issue_admin_token().unwrap();
        let controller = Arc::new(controller);
        let options =
            ServeOptions { node_listen: "127.0.0.1:0".parse().unwrap(), admin_listen: "127.0.0.1:0".parse().unwrap(), static_dir };
        let server = Server::bind(Arc::clone(&controller), options).await.unwrap();
        let (node_addr, admin_addr) = (server.node_addr().unwrap(), server.admin_addr().unwrap());
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            server
                .run(async {
                    stopped.await.ok();
                })
                .await
                .unwrap();
        });
        Self { controller, node_addr, admin_addr, admin_token, stop: Some(stop), task }
    }

    pub fn network(&self, name: &str) -> Network {
        self.controller.create_network(&CreateNetwork { name: name.to_owned(), ..CreateNetwork::default() }).unwrap()
    }

    /// 生成一个注册令牌，返回里面的一次性密钥。
    pub fn secret(&self, network: NetworkId, tags: &[&str]) -> String {
        let request = CreateEnrollToken { tags: tags.iter().map(|&tag| tag.to_owned()).collect(), ..CreateEnrollToken::default() };
        let created = self.controller.create_token(network, &request).unwrap();
        let token: EnrollToken = created.token.parse().unwrap();
        assert_eq!(token.key, self.controller.public_key());
        token.secret
    }

    /// 一个新节点加入网络并保持连接。它的第一份 netmap 已经收下并确认。
    pub async fn join(&self, network: NetworkId, tags: &[&str]) -> Node {
        let secret = self.secret(network, tags);
        let mut node = Node::new();
        node.enroll(self, &secret).await.expect("注册应该成功");
        node
    }

    /// 停止服务并等它收尾。
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            stop.send(()).ok();
        }
        timeout(PATIENCE, &mut self.task).await.expect("controller 应该在限定时间内停下").unwrap();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn hello(declared: Declared) -> Hello {
    Hello { version: "0.1.0".to_owned(), platform: "linux-x86_64".to_owned(), declared, port: 20330 }
}

/// 一个测试用的节点：真的密钥，真的 TLS，手动收发消息。
pub struct Node {
    pub key: NodeKey,
    pub declared: Declared,
    link: Option<Link>,
    /// 最近收到的 netmap。
    pub netmap: Option<Netmap>,
}

pub struct Link {
    reader: ReadHalf<TlsStream<TcpStream>>,
    writer: WriteHalf<TlsStream<TcpStream>>,
}

impl Link {
    /// 用 `key` 连上 controller，校验对方的公钥是 `controller_key`。
    pub async fn open(addr: SocketAddr, key: &NodeKey, controller_key: PublicKey) -> std::io::Result<Self> {
        let config = tls::client_config(key, controller_key, ALPN).unwrap();
        let tcp = TcpStream::connect(addr).await?;
        let stream = TlsConnector::from(Arc::new(config)).connect(ServerName::try_from("i3t").unwrap(), tcp).await?;
        let (reader, writer) = tokio::io::split(stream);
        Ok(Self { reader, writer })
    }

    pub async fn send(&mut self, message: &ToController) {
        write_frame(&mut self.writer, message).await.unwrap();
    }

    /// 原样写一段字节，用来发 controller 不认识的帧。
    pub async fn send_raw(&mut self, bytes: &[u8]) {
        use tokio::io::AsyncWriteExt;
        self.writer.write_all(bytes).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    /// 等下一条消息。连接被对方关闭时返回 `None`。
    pub async fn recv(&mut self) -> Option<ToNode> {
        timeout(PATIENCE, read_frame(&mut self.reader, MAX_FRAME_LEN)).await.expect("等 controller 的消息超时").unwrap_or(None)
    }

    /// 在 `within` 之内有没有消息来。
    pub async fn recv_within(&mut self, within: Duration) -> Option<ToNode> {
        timeout(within, read_frame(&mut self.reader, MAX_FRAME_LEN)).await.ok().and_then(|frame| frame.unwrap_or(None))
    }
}

impl Node {
    pub fn new() -> Self {
        Self::with_key(NodeKey::generate().unwrap())
    }

    pub fn with_key(key: NodeKey) -> Self {
        Self { key, declared: Declared::default(), link: None, netmap: None }
    }

    pub fn id(&self) -> NodeId {
        self.netmap().me.id
    }

    pub fn netmap(&self) -> &Netmap {
        self.netmap.as_ref().expect("节点应该已经收到过 netmap")
    }

    pub fn link(&mut self) -> &mut Link {
        self.link.as_mut().expect("节点应该连着")
    }

    /// 用一次性密钥注册。成功时收下并确认第一份 netmap；被拒绝时返回拒绝的原因。
    pub async fn enroll(&mut self, running: &Running, secret: &str) -> Result<(), String> {
        let first = ToController::Enroll { token: secret.to_owned(), hello: hello(self.declared) };
        self.open(running, first).await
    }

    /// 已经注册过的节点重新连上来。
    pub async fn connect(&mut self, running: &Running) -> Result<(), String> {
        self.open(running, ToController::Hello(hello(self.declared))).await
    }

    async fn open(&mut self, running: &Running, first: ToController) -> Result<(), String> {
        let mut link = Link::open(running.node_addr, &self.key, running.controller.public_key()).await.unwrap();
        link.send(&first).await;
        match link.recv().await {
            Some(ToNode::Snapshot(netmap)) => {
                link.send(&ToController::Ack { version: netmap.version }).await;
                self.netmap = Some(*netmap);
                self.link = Some(link);
                Ok(())
            }
            Some(ToNode::Rejected { reason }) => {
                assert!(link.recv().await.is_none(), "拒绝之后 controller 应该关闭连接");
                Err(reason)
            }
            other => panic!("报到之后的第一条消息应该是 snapshot 或者 rejected，收到的是 {other:?}"),
        }
    }

    pub async fn send(&mut self, message: ToController) {
        self.link().send(&message).await;
    }

    /// 等下一条不是心跳的消息；是 netmap 的话收下并确认。
    pub async fn next(&mut self) -> Option<ToNode> {
        loop {
            let message = self.link().recv().await?;
            match &message {
                ToNode::Heartbeat => continue,
                ToNode::Snapshot(netmap) => {
                    self.netmap = Some((**netmap).clone());
                    let version = netmap.version;
                    self.send(ToController::Ack { version }).await;
                }
                ToNode::Signal { .. } | ToNode::Revoked | ToNode::Rejected { .. } => {}
            }
            return Some(message);
        }
    }

    /// 一直收 netmap，直到手上的那一份满足条件。
    pub async fn until(&mut self, what: &str, satisfied: impl Fn(&Netmap) -> bool) {
        let waiting = async {
            while !satisfied(self.netmap()) {
                match self.next().await {
                    Some(_) => {}
                    None => panic!("还没等到「{what}」连接就断了"),
                }
            }
        };
        if timeout(PATIENCE, waiting).await.is_err() {
            panic!("没有等到「{what}」；手上的 netmap：{:#?}", self.netmap());
        }
    }

    /// 在 `within` 之内没有新的 netmap 或别的消息来（心跳不算）。
    pub async fn stays_quiet(&mut self, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.link().recv_within(left).await {
                Some(ToNode::Heartbeat) => continue,
                Some(_) => return false,
                None => return true,
            }
        }
    }

    pub fn disconnect(&mut self) {
        self.link = None;
    }
}

/// 让这些节点把发给它们的 netmap 都收下并确认，直到一轮下来谁也没有新消息。
///
/// 访问关系的变更要等对端确认才走下一步，所以测试里只盯着一个节点、别的节点不读消息的话，变更会停在半路。
pub async fn settle(nodes: &mut [&mut Node]) {
    let quiet = Duration::from_millis(100);
    loop {
        let mut active = false;
        for node in nodes.iter_mut() {
            while let Some(message) = node.link().recv_within(quiet).await {
                if let ToNode::Snapshot(netmap) = message {
                    let version = netmap.version;
                    node.netmap = Some(*netmap);
                    node.send(ToController::Ack { version }).await;
                    active = true;
                }
            }
        }
        if !active {
            return;
        }
    }
}

/// 反复查一个条件，直到它成立。给"controller 那边过一会儿才看得到"的状态用。
pub async fn eventually(what: &str, condition: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "没有等到「{what}」");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
