//! 集成测试共用的东西：一对打开了独占 TCP 的节点，和一个可以录音、断线、装死的中间一跳。

#![allow(dead_code, reason = "每个测试文件只用到其中一部分")]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_conn::SendStream as _;
use i3t_crypto::NodeKey;
use i3t_lane::{LaneConfig, LanePeer, LaneRecv, LaneSend, Lanes, Payload};
use i3t_proto::NodeId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

pub fn id(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// 测试用的设置：保活和握手的时限调短，不用真的等几十秒。
pub fn quick() -> LaneConfig {
    let mut config = LaneConfig::default();
    config.ping_interval = Duration::from_millis(200);
    config.handshake_deadline = Duration::from_secs(2);
    config
}

pub struct Node {
    pub id: NodeId,
    pub key: NodeKey,
    pub lanes: Arc<Lanes>,
    /// 接受方在这里听。
    pub addr: Option<SocketAddr>,
    listener: Option<JoinHandle<()>>,
}

impl Node {
    pub fn new(id: NodeId, config: LaneConfig) -> Self {
        let key = NodeKey::generate().unwrap();
        let lanes = Arc::new(Lanes::new(&key, config).unwrap());
        Self { id, key, lanes, addr: None, listener: None }
    }

    /// 用同一把密钥重新起一个（模拟重启）。
    pub fn restart(&mut self, config: LaneConfig) {
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
        let pem = self.key.to_pem();
        self.key = NodeKey::from_pem(&pem).unwrap();
        self.lanes = Arc::new(Lanes::new(&self.key, config).unwrap());
    }

    pub async fn listen(&mut self, addr: SocketAddr) -> SocketAddr {
        let listener = TcpListener::bind(addr).await.unwrap();
        let bound = listener.local_addr().unwrap();
        let lanes = Arc::clone(&self.lanes);
        self.listener = Some(tokio::spawn(async move { lanes.serve(listener).await }));
        self.addr = Some(bound);
        bound
    }

    pub fn peer(&self, dial: Vec<SocketAddr>) -> LanePeer {
        LanePeer { id: self.id, key: self.key.public(), dial }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.abort();
        }
    }
}

/// 入口（有公网端口，接受 lane）和出口（在 NAT 后面，拨 lane）。出口经 `hop` 拨入口。
pub struct Pair {
    pub entry: Node,
    pub exit: Node,
    pub hop: Hop,
    direct: bool,
}

impl Pair {
    pub async fn new(config: LaneConfig) -> Self {
        let mut entry = Node::new(id(1), config.clone());
        let exit = Node::new(id(2), config);
        let entry_addr = entry.listen("127.0.0.1:0".parse().unwrap()).await;
        let hop = Hop::start(entry_addr).await;
        let pair = Self { entry, exit, hop, direct: false };
        pair.introduce();
        pair
    }

    /// 出口直接拨入口，不经中间一跳（量资源占用时用：中间一跳自己也占文件描述符）。
    pub async fn direct(config: LaneConfig) -> Self {
        let mut pair = Self::new(config).await;
        let entry = pair.entry.addr.unwrap();
        pair.hop.retarget(entry);
        pair.exit.lanes.set_peers([pair.entry.peer(vec![entry])]).unwrap();
        pair.direct = true;
        pair
    }

    /// 两边互相登记：出口经中间一跳拨入口。
    pub fn introduce(&self) {
        let via = if self.direct { self.entry.addr.unwrap() } else { self.hop.addr };
        self.entry.lanes.set_peers([self.exit.peer(Vec::new())]).unwrap();
        self.exit.lanes.set_peers([self.entry.peer(vec![via])]).unwrap();
    }

    /// 等到入口能在出口停过来的 lane 上开流，并且至少停了 `idle` 条。
    pub async fn ready(&self, idle: u64) {
        wait_for("入口那里停好 lane", Duration::from_secs(10), || {
            self.entry.lanes.can_open(self.exit.id) && self.entry.lanes.status().idle() >= idle
        })
        .await;
    }

    /// 入口在出口停过来的 lane 上开流：写头，选负载方式，出口接到它。返回 (入口的两个方向, 出口的两个方向)。
    pub async fn entry_opens(&self, header: &[u8], payload: Payload) -> ((LaneSend, LaneRecv), (LaneSend, LaneRecv)) {
        open(&self.entry.lanes, self.exit.id, &self.exit.lanes, header, payload).await
    }

    /// 出口在自己拨的 lane 上开流，入口接到它。
    pub async fn exit_opens(&self, header: &[u8], payload: Payload) -> ((LaneSend, LaneRecv), (LaneSend, LaneRecv)) {
        open(&self.exit.lanes, self.entry.id, &self.entry.lanes, header, payload).await
    }
}

/// `from` 到 `peer` 开一条流，写下 `header` 并结束头；`to` 那边接到它，读出 `header`。
pub async fn open(from: &Lanes, peer: NodeId, to: &Lanes, header: &[u8], payload: Payload) -> ((LaneSend, LaneRecv), (LaneSend, LaneRecv)) {
    let (mut send, recv) = from.open_stream(peer).await.unwrap();
    send.write_all(header).await.unwrap();
    send.end_header(payload);
    let (far_send, mut far_recv) = accept_next(to).await;
    let mut got = vec![0; header.len()];
    far_recv.read_exact(&mut got).await.unwrap();
    assert_eq!(got, header, "对端读到的头");
    ((send, recv), (far_send, far_recv))
}

/// 等对端开过来的下一条流，最多等 5 秒。
pub async fn accept_next(lanes: &Lanes) -> (LaneSend, LaneRecv) {
    use i3t_conn::Acceptor;
    let incoming = tokio::time::timeout(Duration::from_secs(5), lanes.accept()).await.expect("5 秒之内没有等到对端开流").unwrap();
    (incoming.send, incoming.recv)
}

/// 一直查 `done`，直到它成立；`limit` 之内不成立就让测试失败。
pub async fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < until, "{limit:?} 之内没有等到：{what}");
        sleep(Duration::from_millis(10)).await;
    }
}

/// 读到 EOF 为止。
pub async fn read_to_end(recv: &mut (impl AsyncReadExt + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut all = Vec::new();
    recv.read_to_end(&mut all).await?;
    Ok(all)
}

/// 一条经过中间一跳的连接，两个方向各录下了什么。
#[derive(Debug, Default)]
pub struct Recording {
    /// 从拨号的一方发往被拨的一方的字节。
    pub forward: Vec<u8>,
    /// 反方向。
    pub backward: Vec<u8>,
}

/// 中间一跳：把连上来的连接转发到 `target`，录下两个方向的字节；可以把已有的连接全部掐断（RST）或者让它们装死
/// （不再转发、也不关闭，像对端的地址换了之后旧连接的样子），可以让新连接连不上，可以把字节切成小段转发。
pub struct Hop {
    pub addr: SocketAddr,
    target: Arc<Mutex<SocketAddr>>,
    pub recordings: Arc<Mutex<Vec<Arc<Mutex<Recording>>>>>,
    /// 改成 true 时现有的连接全部掐断。
    cut: watch::Sender<u64>,
    /// 装死的代数：连接记下自己出生时的代数，代数变了就停止转发。
    freeze: watch::Sender<u64>,
    refuse_new: Arc<AtomicBool>,
    /// 转发时每段最多多少字节；0 表示不切。
    pub chunk: Arc<Mutex<usize>>,
    task: JoinHandle<()>,
}

impl Hop {
    pub async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let target = Arc::new(Mutex::new(target));
        let recordings: Arc<Mutex<Vec<Arc<Mutex<Recording>>>>> = Arc::default();
        let (cut, _) = watch::channel(0);
        let (freeze, _) = watch::channel(0);
        let refuse_new = Arc::new(AtomicBool::new(false));
        let chunk = Arc::new(Mutex::new(0));
        let task = tokio::spawn({
            let (target, recordings, cut, freeze, refuse_new, chunk) = (
                Arc::clone(&target),
                Arc::clone(&recordings),
                cut.subscribe(),
                freeze.subscribe(),
                Arc::clone(&refuse_new),
                Arc::clone(&chunk),
            );
            async move {
                loop {
                    let Ok((inbound, _)) = listener.accept().await else { return };
                    if refuse_new.load(Ordering::Relaxed) {
                        inbound.set_zero_linger().unwrap();
                        drop(inbound);
                        continue;
                    }
                    let to = *target.lock().unwrap();
                    let Ok(outbound) = TcpStream::connect(to).await else { continue };
                    inbound.set_nodelay(true).unwrap();
                    outbound.set_nodelay(true).unwrap();
                    let recording = Arc::new(Mutex::new(Recording::default()));
                    recordings.lock().unwrap().push(Arc::clone(&recording));
                    let born = *freeze.borrow();
                    tokio::spawn(forward(inbound, outbound, recording, cut.clone(), freeze.clone(), born, Arc::clone(&chunk)));
                }
            }
        });
        Self { addr, target, recordings, cut, freeze, refuse_new, chunk, task }
    }

    pub fn retarget(&self, target: SocketAddr) {
        *self.target.lock().unwrap() = target;
    }

    /// 现有的连接全部用 RST 掐断。
    pub fn cut_all(&self) {
        self.cut.send_modify(|generation| *generation += 1);
    }

    /// 现有的连接全部装死：不再转发，也不关闭。之后的新连接照常。
    pub fn freeze_all(&self) {
        self.freeze.send_modify(|generation| *generation += 1);
    }

    pub fn refuse_new(&self, refuse: bool) {
        self.refuse_new.store(refuse, Ordering::Relaxed);
    }

    pub fn connections(&self) -> usize {
        self.recordings.lock().unwrap().len()
    }
}

impl Drop for Hop {
    fn drop(&mut self) {
        self.task.abort();
        self.cut_all();
    }
}

async fn forward(
    inbound: TcpStream,
    outbound: TcpStream,
    recording: Arc<Mutex<Recording>>,
    mut cut: watch::Receiver<u64>,
    mut freeze: watch::Receiver<u64>,
    born: u64,
    chunk: Arc<Mutex<usize>>,
) {
    let cut_at = *cut.borrow_and_update();
    let both = async {
        let there = pump(&inbound, &outbound, |bytes| recording.lock().unwrap().forward.extend_from_slice(bytes), &chunk);
        let back = pump(&outbound, &inbound, |bytes| recording.lock().unwrap().backward.extend_from_slice(bytes), &chunk);
        tokio::pin!(there, back);
        let (mut there_done, mut back_done) = (false, false);
        // 一个方向正常结束时另一个继续；一个方向被重置时两头都重置，不能把重置变成 FIN。
        while !(there_done && back_done) {
            let end = tokio::select! {
                end = &mut there, if !there_done => { there_done = true; end }
                end = &mut back, if !back_done => { back_done = true; end }
            };
            if end == End::Reset {
                return true;
            }
        }
        false
    };
    let mut cut_later = cut.clone();
    let cut_wanted = async move { cut.wait_for(|generation| *generation != cut_at).await.is_ok() };
    let frozen = async move { freeze.wait_for(|generation| *generation != born).await.is_ok() };
    let reset = tokio::select! {
        reset = both => reset,
        _ = cut_wanted => true,
        _ = frozen => {
            // 装死：不再转发，也不关，连接一直挂着。
            let _ = async move { cut_later.wait_for(|generation| *generation != cut_at).await.is_ok() }.await;
            true
        }
    };
    if reset {
        // 关闭时发 RST。两个 socket 在这个函数返回时被丢弃。
        let _ = inbound.set_zero_linger();
        let _ = outbound.set_zero_linger();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum End {
    Clean,
    Reset,
}

/// 单向转发。来源 EOF 时给目标发 FIN；来源出错（被重置）时报出来，由调用方把两头都重置。
async fn pump(from: &TcpStream, to: &TcpStream, mut record: impl FnMut(&[u8]), chunk: &Mutex<usize>) -> End {
    let mut buf = vec![0; 64 * 1024];
    loop {
        if from.readable().await.is_err() {
            return End::Reset;
        }
        let len = match from.try_read(&mut buf) {
            Ok(0) => {
                let _ = socket2::SockRef::from(to).shutdown(std::net::Shutdown::Write);
                return End::Clean;
            }
            Ok(len) => len,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return End::Reset,
        };
        record(&buf[..len]);
        let size = *chunk.lock().unwrap();
        let size = if size == 0 { len } else { size };
        for piece in buf[..len].chunks(size) {
            let mut piece = piece;
            while !piece.is_empty() {
                if to.writable().await.is_err() {
                    return End::Reset;
                }
                match to.try_write(piece) {
                    Ok(written) => piece = &piece[written..],
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return End::Reset,
                }
            }
            if size < len {
                tokio::task::yield_now().await;
            }
        }
    }
}
