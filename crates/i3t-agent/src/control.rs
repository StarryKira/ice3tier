//! 连 controller 的那条长连接。
//!
//! 节点用它做四件事：第一次加入时注册（[`enroll`]）；收 netmap 并确认；定期上报；转发信令。
//! 连接断了就带退避地重连，这期间节点按手上最后一份 netmap 照常工作。
//! 被吊销，或者 controller 不认这个节点时，不再重连。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail, ensure};
use i3t_crypto::{NodeKey, tls};
use i3t_model::Netmap;
use i3t_model::control::{Hello, MAX_FRAME_LEN, Report, Signal, ToController, ToNode};
use i3t_proto::NodeId;
use rustls::pki_types::ServerName;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::state::{ControllerInfo, StateDir};

/// 节点协议在 TLS 上的 ALPN。
pub const ALPN: &[u8] = b"i3t-ctl/1";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 信令是尽力而为的：排队超过这个数说明另一头处理不过来，新的直接丢掉，发起方会重试。
const SIGNAL_QUEUE: usize = 64;

/// 用注册令牌里的一次性密钥加入网络，返回 controller 给这个节点的第一份 netmap。
///
/// # Errors
///
/// 连不上 controller、controller 的公钥不对、令牌被拒绝，或者回答不符合协议。
pub async fn enroll(controller: &ControllerInfo, key: &NodeKey, secret: &str, hello: Hello) -> anyhow::Result<Netmap> {
    let mut stream = connect(controller, key).await?;
    write_message(&mut stream, &ToController::Enroll { token: secret.to_owned(), hello }).await?;
    let first = timeout(CONNECT_TIMEOUT, read_message::<_, ToNode>(&mut stream)).await.context("等 controller 回答注册请求超时")??;
    match first {
        Some(ToNode::Snapshot(netmap)) => {
            write_message(&mut stream, &ToController::Ack { version: netmap.version }).await?;
            Ok(*netmap)
        }
        Some(ToNode::Rejected { reason }) => bail!("controller 拒绝了注册：{reason}"),
        Some(other) => bail!("controller 对注册请求的回答不对：{}", kind_of(&other)),
        None => bail!("controller 没有回答就关闭了连接"),
    }
}

/// 这条长连接现在的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// 还没连上，或者断了正在重连。
    Offline,
    Online,
    /// controller 吊销了这个节点。不再重连。
    Revoked,
    /// controller 不认这个节点（例如它被删除时本节点不在线）。不再重连，要重新加入。
    Rejected,
}

/// 长连接上的几个时间。默认值是协议约定的，测试里会改短。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// 多久上报一次。上报同时是节点一侧的心跳。
    pub report_interval: Duration,
    /// 这么久收不到 controller 的任何消息就认为连接已经死了。controller 空闲时每 20 秒发一次心跳。
    pub idle_timeout: Duration,
    /// 重连的退避从这个值起，每次翻倍，到 `backoff_max` 为止；连上之后重新从头算。
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            report_interval: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(90),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
        }
    }
}

/// 启动长连接所需要的东西。
pub struct LinkSetup {
    pub controller: ControllerInfo,
    pub key: Arc<NodeKey>,
    /// 启动时手上的 netmap（来自注册，或者上次存下来的）。
    pub initial: Arc<Netmap>,
    /// 当前的自我介绍。本地声明变了就往里放一个新值，长连接会把它发给 controller。
    pub hello: watch::Receiver<Hello>,
    /// 每次上报时调用，取当前要报的内容。
    pub report: Box<dyn Fn() -> Report + Send + Sync>,
    /// 收到的 netmap 存到这里，下次启动时用。
    pub state: StateDir,
    pub timing: Timing,
}

/// 一条正在后台运行的长连接。丢弃它就停止。
pub struct ControlLink {
    netmap: watch::Receiver<Arc<Netmap>>,
    state: watch::Receiver<LinkState>,
    signals: mpsc::Receiver<(NodeId, Signal)>,
    outgoing: mpsc::Sender<(NodeId, Signal)>,
    task: JoinHandle<()>,
}

impl ControlLink {
    #[must_use]
    pub fn spawn(setup: LinkSetup) -> Self {
        let (netmap_tx, netmap) = watch::channel(Arc::clone(&setup.initial));
        let (state_tx, state) = watch::channel(LinkState::Offline);
        let (signals_tx, signals) = mpsc::channel(SIGNAL_QUEUE);
        let (outgoing, outgoing_rx) = mpsc::channel(SIGNAL_QUEUE);
        let worker = Worker { setup, netmap: netmap_tx, state: state_tx, signals: signals_tx, outgoing: outgoing_rx };
        Self { netmap, state, signals, outgoing, task: tokio::spawn(worker.run()) }
    }

    /// 当前的 netmap，controller 每下发一份就更新一次。
    #[must_use]
    pub fn netmap(&self) -> watch::Receiver<Arc<Netmap>> {
        self.netmap.clone()
    }

    #[must_use]
    pub fn state(&self) -> watch::Receiver<LinkState> {
        self.state.clone()
    }

    /// 往这里放要经 controller 转给别的节点的信令。
    #[must_use]
    pub fn signal_sender(&self) -> mpsc::Sender<(NodeId, Signal)> {
        self.outgoing.clone()
    }

    /// 等下一条别的节点经 controller 转来的信令。长连接停止之后返回 `None`。
    pub async fn next_signal(&mut self) -> Option<(NodeId, Signal)> {
        self.signals.recv().await
    }
}

impl Drop for ControlLink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 一次会话是怎么结束的。出错的情况走 `Err`，会重连。
enum Ended {
    Revoked,
    Rejected(String),
}

struct Worker {
    setup: LinkSetup,
    netmap: watch::Sender<Arc<Netmap>>,
    state: watch::Sender<LinkState>,
    signals: mpsc::Sender<(NodeId, Signal)>,
    outgoing: mpsc::Receiver<(NodeId, Signal)>,
}

impl Worker {
    async fn run(mut self) {
        let timing = self.setup.timing;
        let mut backoff = timing.backoff_min;
        loop {
            let mut was_online = false;
            match self.session(&mut was_online).await {
                Ok(Ended::Revoked) => {
                    tracing::warn!("controller 吊销了这个节点，不再连接");
                    self.state.send_replace(LinkState::Revoked);
                    return;
                }
                Ok(Ended::Rejected(reason)) => {
                    tracing::warn!(%reason, "controller 不认这个节点，不再连接；要重新加入网络");
                    self.state.send_replace(LinkState::Rejected);
                    return;
                }
                Err(err) => tracing::warn!("和 controller 的连接断开：{err:#}"),
            }
            self.state.send_replace(LinkState::Offline);
            if was_online {
                backoff = timing.backoff_min;
            }
            tokio::time::sleep(jittered(backoff)).await;
            backoff = (backoff * 2).min(timing.backoff_max);
        }
    }

    async fn session(&mut self, was_online: &mut bool) -> anyhow::Result<Ended> {
        let stream = connect(&self.setup.controller, &self.setup.key).await?;
        let (reader, mut writer) = tokio::io::split(stream);
        let hello = self.setup.hello.borrow_and_update().clone();
        write_message(&mut writer, &ToController::Hello(hello)).await?;

        // 读一帧不能被打断（长度和内容分两次读），所以交给单独的任务，这里只从队列里取完整的消息。
        let mut inbound = spawn_reader(reader);
        let Timing { report_interval, idle_timeout, .. } = self.setup.timing;
        let mut report_at = tokio::time::interval_at(Instant::now() + report_interval, report_interval);
        let mut last_heard = Instant::now();
        loop {
            tokio::select! {
                message = inbound.messages.recv() => {
                    let message = message.ok_or_else(|| anyhow!("读取任务意外结束"))??;
                    last_heard = Instant::now();
                    match message {
                        ToNode::Snapshot(netmap) => {
                            let version = netmap.version;
                            self.apply(*netmap);
                            write_message(&mut writer, &ToController::Ack { version }).await?;
                            *was_online = true;
                            self.state.send_replace(LinkState::Online);
                        }
                        ToNode::Signal { from, signal } => {
                            if self.signals.try_send((from, signal)).is_err() {
                                tracing::debug!(%from, "信令队列满了，丢掉一条");
                            }
                        }
                        ToNode::Revoked => return Ok(Ended::Revoked),
                        ToNode::Rejected { reason } => return Ok(Ended::Rejected(reason)),
                        ToNode::Heartbeat => {}
                    }
                }
                Some((to, signal)) = self.outgoing.recv() => {
                    write_message(&mut writer, &ToController::Signal { to, signal }).await?;
                }
                _ = report_at.tick() => {
                    write_message(&mut writer, &ToController::Report((self.setup.report)())).await?;
                }
                Ok(()) = self.setup.hello.changed() => {
                    let hello = self.setup.hello.borrow_and_update().clone();
                    write_message(&mut writer, &ToController::Hello(hello)).await?;
                }
                () = tokio::time::sleep_until(last_heard + idle_timeout) => {
                    bail!("{} 秒没有收到 controller 的任何消息", idle_timeout.as_secs());
                }
            }
        }
    }

    fn apply(&self, netmap: Netmap) {
        if let Err(err) = self.setup.state.save_netmap(&netmap) {
            // 存不下来不影响这次运行，只是下次启动拿不到这一版。
            tracing::warn!("保存 netmap 失败：{err:#}");
        }
        tracing::info!(version = netmap.version, peers = netmap.peers.len(), relays = netmap.relays.len(), "应用新的 netmap");
        self.netmap.send_replace(Arc::new(netmap));
    }
}

/// 一条连接上的读取任务，以及它读出来的消息。
///
/// 丢弃它时任务必须立刻停：任务平时停在"等对端发数据"上，握着连接的读半边。
/// 不停的话会话结束了连接也关不掉，controller 那边看不到这个节点已经断开。
struct Reader {
    messages: mpsc::Receiver<anyhow::Result<ToNode>>,
    task: JoinHandle<()>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 把完整的消息一条条放进队列。连接结束或出错时放进一个错误然后退出。
fn spawn_reader<R: AsyncRead + Unpin + Send + 'static>(mut reader: R) -> Reader {
    let (tx, messages) = mpsc::channel(16);
    let task = tokio::spawn(async move {
        loop {
            let message = match read_message(&mut reader).await {
                Ok(Some(message)) => Ok(message),
                Ok(None) => Err(anyhow!("controller 关闭了连接")),
                Err(err) => Err(err),
            };
            let failed = message.is_err();
            if tx.send(message).await.is_err() || failed {
                return;
            }
        }
    });
    Reader { messages, task }
}

async fn connect(controller: &ControllerInfo, key: &NodeKey) -> anyhow::Result<TlsStream<TcpStream>> {
    let config = tls::client_config(key, controller.key, ALPN).context("准备 TLS 配置")?;
    // 校验器只比对公钥，不看名字；这里的名字只是 TLS 接口要求有一个。
    let name = ServerName::try_from("i3t").context("构造 TLS 服务器名")?;
    let handshake = async {
        let tcp = TcpStream::connect(&controller.address).await.with_context(|| format!("连接 controller {}", controller.address))?;
        tcp.set_nodelay(true).context("设置 TCP_NODELAY")?;
        TlsConnector::from(Arc::new(config)).connect(name, tcp).await.context("和 controller 的 TLS 握手")
    };
    timeout(CONNECT_TIMEOUT, handshake).await.with_context(|| format!("连接 controller {} 超时", controller.address))?
}

/// 发一条消息：4 字节大端长度，后面是 JSON。
///
/// # Errors
///
/// 消息超过长度上限，或者写失败。
pub async fn write_message<W: AsyncWrite + Unpin, T: Serialize>(writer: &mut W, message: &T) -> anyhow::Result<()> {
    let body = serde_json::to_vec(message).context("序列化消息")?;
    ensure!(body.len() <= MAX_FRAME_LEN, "消息有 {} 字节，超过上限 {MAX_FRAME_LEN}", body.len());
    let len = u32::try_from(body.len()).context("消息长度放不进 4 个字节")?;
    let mut frame = Vec::with_capacity(size_of::<u32>() + body.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&body);
    writer.write_all(&frame).await.context("写消息")?;
    writer.flush().await.context("写消息")
}

/// 读一条消息。对端在两条消息之间正常关闭连接时返回 `None`。
///
/// # Errors
///
/// 读失败、长度超过上限，或者内容不是期望的 JSON。
pub async fn read_message<R: AsyncRead + Unpin, T: DeserializeOwned>(reader: &mut R) -> anyhow::Result<Option<T>> {
    let len = match reader.read_u32().await {
        Ok(len) => usize::try_from(len).context("消息长度超出了这台机器的地址范围")?,
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err).context("读消息长度"),
    };
    ensure!(len <= MAX_FRAME_LEN, "对端声称的消息长度 {len} 超过上限 {MAX_FRAME_LEN}");
    let mut body = vec![0; len];
    reader.read_exact(&mut body).await.context("读消息内容")?;
    serde_json::from_slice(&body).map(Some).context("解析消息")
}

/// 退避时间上下浮动一半，免得一批节点在 controller 重启之后同时连回来。
fn jittered(base: Duration) -> Duration {
    base.mul_f64(0.5 + rand::random::<f64>())
}

fn kind_of(message: &ToNode) -> &'static str {
    match message {
        ToNode::Snapshot(_) => "snapshot",
        ToNode::Signal { .. } => "signal",
        ToNode::Revoked => "revoked",
        ToNode::Rejected { .. } => "rejected",
        ToNode::Heartbeat => "heartbeat",
    }
}

/// 测试用的假 controller，本 crate 里别的模块的测试也用它。
#[cfg(test)]
pub(crate) mod testing {
    use i3t_proto::PublicKey;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;
    pub(crate) use tokio_rustls::server::TlsStream as ServerStream;

    use super::*;

    pub(crate) fn netmap(version: u64) -> Netmap {
        serde_json::from_value(serde_json::json!({
            "version": version,
            "network": { "name": "home", "domain": "home.i3t", "cidr": "10.66.0.0/16" },
            "me": { "id": 1, "name": "laptop", "virtual_ip": "10.66.0.1" }
        }))
        .unwrap()
    }

    /// 一个只会接受连接的假 controller。测试里拿到每条连接之后自己决定说什么。
    pub(crate) struct FakeController {
        pub(crate) listener: TcpListener,
        pub(crate) acceptor: TlsAcceptor,
        pub(crate) info: ControllerInfo,
    }

    impl FakeController {
        pub(crate) async fn start() -> Self {
            let key = NodeKey::generate().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let info = ControllerInfo { address: listener.local_addr().unwrap().to_string(), key: key.public() };
            let config = tls::server_config(&key, |_| true, ALPN).unwrap();
            Self { listener, acceptor: TlsAcceptor::from(Arc::new(config)), info }
        }

        /// 接受下一条连接，返回它和节点在握手里出示的公钥。
        pub(crate) async fn accept(&self) -> (ServerStream<TcpStream>, PublicKey) {
            let (tcp, _) = self.listener.accept().await.unwrap();
            let stream = self.acceptor.accept(tcp).await.unwrap();
            let certs = stream.get_ref().1.peer_certificates().unwrap();
            let key = tls::peer_key(&certs[0]).unwrap();
            (stream, key)
        }
    }

    pub(crate) async fn expect<T: DeserializeOwned>(stream: &mut ServerStream<TcpStream>) -> T {
        timeout(Duration::from_secs(5), read_message(stream)).await.expect("等节点的消息超时").unwrap().expect("节点关闭了连接")
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{FakeController, expect, netmap};
    use super::*;

    fn link(controller: &FakeController, key: Arc<NodeKey>, state: StateDir) -> (ControlLink, watch::Sender<Hello>) {
        let hello = Hello { version: "0.1.0".into(), platform: "test".into(), ..Hello::default() };
        let (hello_tx, hello_rx) = watch::channel(hello);
        let setup = LinkSetup {
            controller: controller.info.clone(),
            key,
            initial: Arc::new(netmap(1)),
            hello: hello_rx,
            report: Box::new(|| Report { flows: 3, ..Report::default() }),
            state,
            timing: Timing {
                report_interval: Duration::from_millis(50),
                idle_timeout: Duration::from_millis(400),
                backoff_min: Duration::from_millis(10),
                backoff_max: Duration::from_millis(40),
            },
        };
        (ControlLink::spawn(setup), hello_tx)
    }

    fn temp_state() -> (tempfile::TempDir, StateDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        (dir, state)
    }

    #[tokio::test]
    async fn enroll_sends_the_secret_and_returns_the_first_netmap() {
        let controller = FakeController::start().await;
        let key = NodeKey::generate().unwrap();
        let public = key.public();
        let info = controller.info.clone();
        let server = tokio::spawn(async move {
            let (mut stream, seen_key) = controller.accept().await;
            assert_eq!(seen_key, public, "controller 从握手里认出节点的公钥");
            let ToController::Enroll { token, hello } = expect(&mut stream).await else { panic!("第一条消息不是 enroll") };
            assert_eq!((token.as_str(), hello.platform.as_str()), ("s3cret", "test"));
            write_message(&mut stream, &ToNode::Snapshot(Box::new(netmap(7)))).await.unwrap();
            assert_eq!(expect::<ToController>(&mut stream).await, ToController::Ack { version: 7 });
        });
        let hello = Hello { platform: "test".into(), ..Hello::default() };
        let received = enroll(&info, &key, "s3cret", hello).await.unwrap();
        assert_eq!(received.version, 7);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn enroll_reports_the_reason_when_rejected() {
        let controller = FakeController::start().await;
        let info = controller.info.clone();
        tokio::spawn(async move {
            let (mut stream, _) = controller.accept().await;
            let _: ToController = expect(&mut stream).await;
            write_message(&mut stream, &ToNode::Rejected { reason: "令牌已经用过".into() }).await.unwrap();
        });
        let err = enroll(&info, &NodeKey::generate().unwrap(), "x", Hello::default()).await.unwrap_err();
        assert!(format!("{err:#}").contains("令牌已经用过"), "{err:#}");
    }

    #[tokio::test]
    async fn a_controller_with_another_key_is_not_trusted() {
        let controller = FakeController::start().await;
        let mut info = controller.info.clone();
        info.key = NodeKey::generate().unwrap().public();
        tokio::spawn(async move {
            let (tcp, _) = controller.listener.accept().await.unwrap();
            let _ = controller.acceptor.accept(tcp).await;
        });
        let err = enroll(&info, &NodeKey::generate().unwrap(), "x", Hello::default()).await.unwrap_err();
        assert!(format!("{err:#}").contains("TLS"), "{err:#}");
    }

    #[tokio::test]
    async fn snapshots_are_applied_saved_and_acknowledged() {
        let controller = FakeController::start().await;
        let (_dir, state) = temp_state();
        let (link, _hello) = link(&controller, Arc::new(NodeKey::generate().unwrap()), state.clone());
        let mut netmaps = link.netmap();
        assert_eq!(netmaps.borrow().version, 1);

        let (mut stream, _) = controller.accept().await;
        assert!(matches!(expect::<ToController>(&mut stream).await, ToController::Hello(_)));
        write_message(&mut stream, &ToNode::Snapshot(Box::new(netmap(2)))).await.unwrap();
        assert_eq!(expect::<ToController>(&mut stream).await, ToController::Ack { version: 2 });

        netmaps.changed().await.unwrap();
        assert_eq!(netmaps.borrow().version, 2);
        assert_eq!(state.netmap().unwrap().unwrap().version, 2);
        assert_eq!(*link.state().borrow(), LinkState::Online);
    }

    #[tokio::test]
    async fn signals_travel_both_ways_and_a_changed_hello_is_resent() {
        let controller = FakeController::start().await;
        let (_dir, state) = temp_state();
        let (mut link, hello) = link(&controller, Arc::new(NodeKey::generate().unwrap()), state);
        let (mut stream, _) = controller.accept().await;
        let _: ToController = expect(&mut stream).await;

        let peer = NodeId::new(2).unwrap();
        let request = Signal::PunchRequest { endpoints: vec!["203.0.113.5:20330".parse().unwrap()] };
        write_message(&mut stream, &ToNode::Signal { from: peer, signal: request.clone() }).await.unwrap();
        assert_eq!(link.next_signal().await, Some((peer, request)));

        let reply = Signal::PunchReply { endpoints: Vec::new() };
        link.signal_sender().send((peer, reply.clone())).await.unwrap();
        assert_eq!(expect::<ToController>(&mut stream).await, ToController::Signal { to: peer, signal: reply });

        hello.send_modify(|hello| hello.declared.exit = true);
        let ToController::Hello(resent) = expect(&mut stream).await else { panic!("声明变了之后应该重发 hello") };
        assert!(resent.declared.exit);
    }

    #[tokio::test]
    async fn reports_are_sent_periodically() {
        let controller = FakeController::start().await;
        let (_dir, state) = temp_state();
        let (_link, _hello) = link(&controller, Arc::new(NodeKey::generate().unwrap()), state);
        let (mut stream, _) = controller.accept().await;
        let _: ToController = expect(&mut stream).await;
        let ToController::Report(report) = expect(&mut stream).await else { panic!("应该收到上报") };
        assert_eq!(report.flows, 3);
    }

    #[tokio::test]
    async fn the_link_reconnects_after_the_connection_drops() {
        let controller = FakeController::start().await;
        let (_dir, state) = temp_state();
        let (link, _hello) = link(&controller, Arc::new(NodeKey::generate().unwrap()), state);
        let (mut first, _) = controller.accept().await;
        let _: ToController = expect(&mut first).await;
        drop(first);

        let (mut second, _) = controller.accept().await;
        assert!(matches!(expect::<ToController>(&mut second).await, ToController::Hello(_)));
        write_message(&mut second, &ToNode::Snapshot(Box::new(netmap(5)))).await.unwrap();
        let mut netmaps = link.netmap();
        timeout(Duration::from_secs(5), netmaps.wait_for(|netmap| netmap.version == 5)).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_silent_controller_is_dropped_and_dialled_again() {
        let controller = FakeController::start().await;
        let (_dir, state) = temp_state();
        let (_link, _hello) = link(&controller, Arc::new(NodeKey::generate().unwrap()), state);
        // 第一条连接上 controller 一句话都不说，但也不关。
        let (_silent, _) = controller.accept().await;
        let (mut second, _) = timeout(Duration::from_secs(5), controller.accept()).await.expect("空闲超时之后应该重连");
        assert!(matches!(expect::<ToController>(&mut second).await, ToController::Hello(_)));
    }

    #[tokio::test]
    async fn revocation_and_rejection_stop_the_link_for_good() {
        for (message, expected) in [
            (ToNode::Revoked, LinkState::Revoked),
            (ToNode::Rejected { reason: "不认识这个公钥".into() }, LinkState::Rejected),
        ] {
            let controller = FakeController::start().await;
            let (_dir, state) = temp_state();
            let (link, _hello) = link(&controller, Arc::new(NodeKey::generate().unwrap()), state);
            let (mut stream, _) = controller.accept().await;
            let _: ToController = expect(&mut stream).await;
            write_message(&mut stream, &message).await.unwrap();
            // 节点这时可能正好在发上报，把它读掉，直到连接被节点关闭。
            while read_message::<_, ToController>(&mut stream).await.is_ok_and(|message| message.is_some()) {}

            let mut states = link.state();
            timeout(Duration::from_secs(5), states.wait_for(|state| *state == expected)).await.unwrap().unwrap();
            // 之后不应该再有新的连接进来。
            assert!(timeout(Duration::from_millis(300), controller.listener.accept()).await.is_err());
        }
    }

    #[tokio::test]
    async fn oversized_and_truncated_frames_are_errors() {
        let mut oversized: &[u8] = &u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes();
        assert!(read_message::<_, ToNode>(&mut oversized).await.is_err());
        let mut truncated: &[u8] = &[0, 0, 0, 9, b'{'];
        assert!(read_message::<_, ToNode>(&mut truncated).await.is_err());
        let mut empty: &[u8] = &[];
        assert!(read_message::<_, ToNode>(&mut empty).await.unwrap().is_none());
    }
}
