//! 测量，不是测试：握手的开销、每 GB 的 CPU、池里有热 lane 时从取用到首字节的时间。
//!
//! `cargo test --release -p i3t-lane --test measure -- --ignored --nocapture --test-threads 1`

// 量的是 Linux 上的数（线程 CPU 时间、/proc）。
#![cfg(target_os = "linux")]
// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]
// 这里只是把测出来的数折成秒和比例打印出来，精度无所谓。
#![allow(clippy::cast_precision_loss)]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use common::{id, quick};
use i3t_conn::Acceptor;
use i3t_conn::{RecvStream as _, SendStream as _};
use i3t_crypto::NodeKey;
use i3t_flow::engine::Splicer;
use i3t_lane::{LanePeer, LaneRecv, LaneSend, Lanes, Payload};
use rustix::time::{ClockId, clock_gettime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 这个线程到现在用了多少 CPU（用户态加内核态）。
fn thread_cpu() -> Duration {
    let now = clock_gettime(ClockId::ThreadCPUTime);
    Duration::new(u64::try_from(now.tv_sec).unwrap(), u32::try_from(now.tv_nsec).unwrap())
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").unwrap_or_default().split_whitespace().take(3).collect::<Vec<_>>().join(" ")
}

#[test]
#[ignore = "测量，release 下手动跑"]
fn handshake_cost_on_each_side() {
    use bytes::BytesMut;
    use i3t_crypto::tls::{self, Identity};
    use rustls::pki_types::ServerName;
    use rustls::{ClientConnection, ServerConnection};

    let (dialler, acceptor) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
    let dialler_public = dialler.public();
    let mut client_config = tls::client_config_with(&Identity::new(&dialler).unwrap(), acceptor.public(), i3t_lane::ALPN).unwrap();
    client_config.resumption = rustls::client::Resumption::disabled();
    let client_config = Arc::new(client_config);
    let mut server_config =
        tls::server_config_with(&Identity::new(&acceptor).unwrap(), move |key| *key == dialler_public, i3t_lane::ALPN).unwrap();
    server_config.send_tls13_tickets = 0;
    let server_config = Arc::new(server_config);

    const ROUNDS: u32 = 3000;
    let (mut client_time, mut server_time) = (Duration::ZERO, Duration::ZERO);
    for _ in 0..ROUNDS {
        let started = Instant::now();
        let mut client = ClientConnection::new(Arc::clone(&client_config), ServerName::try_from("i3t").unwrap()).unwrap();
        let mut to_server = BytesMut::new();
        client.write_tls(&mut (&mut to_server).writer_compat()).unwrap();
        client_time += started.elapsed();
        let started = Instant::now();
        let mut server = ServerConnection::new(Arc::clone(&server_config)).unwrap();
        server.read_tls(&mut &to_server[..]).unwrap();
        server.process_new_packets().unwrap();
        let mut to_client = Vec::new();
        while server.wants_write() {
            server.write_tls(&mut to_client).unwrap();
        }
        server_time += started.elapsed();
        let started = Instant::now();
        let mut rest = &to_client[..];
        while !rest.is_empty() {
            client.read_tls(&mut rest).unwrap();
            client.process_new_packets().unwrap();
        }
        let mut finished = Vec::new();
        while client.wants_write() {
            client.write_tls(&mut finished).unwrap();
        }
        client_time += started.elapsed();
        let started = Instant::now();
        let mut rest = &finished[..];
        while !rest.is_empty() {
            server.read_tls(&mut rest).unwrap();
            server.process_new_packets().unwrap();
        }
        assert!(!server.is_handshaking());
        server_time += started.elapsed();
    }
    let per = |total: Duration| total / ROUNDS;
    eprintln!(
        "握手（负载 {}）：拨号方每次 {:?}，每核每秒 {:.0} 次；接受方每次 {:?}，每核每秒 {:.0} 次",
        load(),
        per(client_time),
        f64::from(ROUNDS) / client_time.as_secs_f64(),
        per(server_time),
        f64::from(ROUNDS) / server_time.as_secs_f64()
    );
}

/// `BytesMut` 当 `io::Write` 用。
trait WriterCompat {
    fn writer_compat(self) -> bytes::buf::Writer<Self>
    where
        Self: Sized + bytes::BufMut;
}

impl WriterCompat for &mut bytes::BytesMut {
    fn writer_compat(self) -> bytes::buf::Writer<Self> {
        bytes::BufMut::writer(self)
    }
}

/// 交给一边的线程去跑的事。
type Job = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send>;

/// 一种搬运方式：`None` 是经流的两个方向在用户态拷贝（sealed 只能这样）。
type Engine = Option<fn() -> Splicer>;

/// 一个节点跑在自己的单线程运行时里，这个线程的 CPU 时间就是它花的。
struct Side {
    commands: std_mpsc::Sender<Job>,
    _thread: std::thread::JoinHandle<()>,
}

impl Side {
    fn start() -> Self {
        let (commands, receiver) = std_mpsc::channel::<Job>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            while let Ok(command) = receiver.recv() {
                command(&runtime);
            }
        });
        Self { commands, _thread: thread }
    }

    /// 在这一边的线程上跑 `job`，等它返回。
    fn run<T: Send + 'static>(&self, job: impl FnOnce(&tokio::runtime::Runtime) -> T + Send + 'static) -> T {
        let (sender, receiver) = std_mpsc::channel();
        self.commands.send(Box::new(move |runtime| sender.send(job(runtime)).unwrap())).unwrap();
        receiver.recv().unwrap()
    }
}

/// 出口：接受流、读 `header_len` 字节的头、连 `sink`、回 REPLY、结束头，然后搬运。
async fn exit_side(lanes: Arc<Lanes>, sink: SocketAddr, splicer: Option<Arc<Splicer>>, header_len: usize) {
    while let Some(incoming) = lanes.accept().await {
        let (mut send, mut recv) = (incoming.send, incoming.recv);
        let mut header = vec![0; header_len];
        recv.read_exact(&mut header).await.unwrap();
        recv.end_header();
        let target = TcpStream::connect(sink).await.unwrap();
        target.set_nodelay(true).unwrap();
        send.write_all(b"R").await.unwrap();
        send.end_header(Payload::Passthrough);
        relay(send, recv, target, splicer.as_deref()).await;
    }
}

/// 一条流和一条 TCP 之间的双向搬运：两个方向都离开了 TLS 就交给搬运引擎，否则经流的两个方向在用户态拷贝。
async fn relay(mut send: LaneSend, mut recv: LaneRecv, user: TcpStream, splicer: Option<&Splicer>) {
    if let Some(splicer) = splicer {
        // 先等接收方向：发起方收到接受方的 SWITCH 时，自己的方向才跟着离开 TLS。
        let raw_recv = recv.wait_raw().await.expect("对端切换之后它的方向离开 TLS");
        let raw_send = send.take_raw().expect("passthrough 的这个方向已经离开 TLS");
        let (lane, unsent) = raw_send.into_parts();
        let (_, received) = raw_recv.into_parts();
        let relayed = splicer.relay(&user, received, &lane, unsent).await;
        assert!(relayed.error.is_none(), "{:?}", relayed.error);
        return;
    }
    let (mut user_read, mut user_write) = user.into_split();
    let down = async {
        let mut buf = vec![0; 16 * 1024];
        loop {
            let len = recv.read(&mut buf).await.unwrap();
            if len == 0 {
                user_write.shutdown().await.unwrap();
                return;
            }
            user_write.write_all(&buf[..len]).await.unwrap();
        }
    };
    let up = async {
        let mut buf = vec![0; 16 * 1024];
        loop {
            let len = user_read.read(&mut buf).await.unwrap();
            if len == 0 {
                send.shutdown().await.unwrap();
                return;
            }
            send.write_all(&buf[..len]).await.unwrap();
        }
    };
    tokio::join!(down, up);
}

/// 入口：接受客户端、读首批数据、开流、读 REPLY，然后搬运。
async fn entry_side(lanes: Arc<Lanes>, listener: TcpListener, peer: i3t_proto::NodeId, splicer: Option<Arc<Splicer>>, first_len: usize) {
    let (mut client, _) = listener.accept().await.unwrap();
    client.set_nodelay(true).unwrap();
    let mut first = vec![0; first_len];
    client.read_exact(&mut first).await.unwrap();
    let (mut send, mut recv) = lanes.open_stream(peer).await.unwrap();
    send.write_all(&first).await.unwrap();
    send.end_header(Payload::for_first_data(&first));
    let mut reply = [0; 1];
    recv.read_exact(&mut reply).await.unwrap();
    recv.end_header();
    relay(send, recv, client, splicer.as_deref()).await;
}

/// 经入口、lane、出口把 `total` 字节从客户端送到目标，返回两边各用了多少 CPU。
fn cpu_for(total: usize, first: &[u8], engine: Engine) -> (Duration, Duration, Duration) {
    let first = first.to_vec();
    let first_len = first.len();
    let entry = Side::start();
    let exit = Side::start();
    let entry_key = NodeKey::generate().unwrap();
    let exit_key = NodeKey::generate().unwrap();
    let entry_public = entry_key.public();
    let exit_public = exit_key.public();
    let entry_lanes = Arc::new(Lanes::new(&entry_key, quick()).unwrap());
    let exit_lanes = Arc::new(Lanes::new(&exit_key, quick()).unwrap());

    // 目标在主线程自己的运行时里：只读、计数。
    let main = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let sink = main.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let sink_addr = sink.local_addr().unwrap();
    let received = main.spawn(async move {
        let (mut stream, _) = sink.accept().await.unwrap();
        let mut buf = vec![0; 256 * 1024];
        let mut count = 0;
        loop {
            let len = stream.read(&mut buf).await.unwrap();
            if len == 0 {
                return count;
            }
            count += len;
        }
    });

    let (lanes, splicer) = (Arc::clone(&entry_lanes), engine.map(|make| Arc::new(make())));
    let (entry_addr, user_addr) = entry.run(move |runtime| {
        runtime.block_on(async move {
            let lane_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let lane_addr = lane_listener.local_addr().unwrap();
            let user = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let user_addr = user.local_addr().unwrap();
            lanes.set_peers([LanePeer { id: id(2), key: exit_public, dial: Vec::new() }]).unwrap();
            let serving = Arc::clone(&lanes);
            tokio::spawn(async move { serving.serve(lane_listener).await });
            tokio::spawn(entry_side(lanes, user, id(2), splicer, first_len));
            (lane_addr, user_addr)
        })
    });
    let (lanes, splicer) = (Arc::clone(&exit_lanes), engine.map(|make| Arc::new(make())));
    exit.run(move |runtime| {
        runtime.block_on(async move {
            lanes.set_peers([LanePeer { id: id(1), key: entry_public, dial: vec![entry_addr] }]).unwrap();
            tokio::spawn(exit_side(lanes, sink_addr, splicer, first_len));
        });
    });
    // 让两边各自的运行时转起来：它们的线程在等下一个命令时不跑任务，所以给它们各一个一直跑的命令。
    let (entry_done, entry_wait) = std_mpsc::channel();
    let (exit_done, exit_wait) = std_mpsc::channel::<Duration>();
    let entry_stop = Arc::new(tokio::sync::Notify::new());
    let exit_stop = Arc::new(tokio::sync::Notify::new());
    {
        let stop = Arc::clone(&entry_stop);
        entry
            .commands
            .send(Box::new(move |runtime| {
                let started = thread_cpu();
                runtime.block_on(stop.notified());
                entry_done.send(thread_cpu() - started).unwrap();
            }))
            .unwrap();
        let stop = Arc::clone(&exit_stop);
        exit.commands
            .send(Box::new(move |runtime| {
                let started = thread_cpu();
                runtime.block_on(stop.notified());
                exit_done.send(thread_cpu() - started).unwrap();
            }))
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(entry_lanes.can_open(id(2)), "池没有准备好");

    let started = Instant::now();
    main.block_on(async {
        let mut client = TcpStream::connect(user_addr).await.unwrap();
        client.set_nodelay(true).unwrap();
        client.write_all(&first).await.unwrap();
        let chunk = vec![0x5a; 1 << 20];
        let mut sent = 0;
        while sent < total {
            client.write_all(&chunk).await.unwrap();
            sent += chunk.len();
        }
        client.shutdown().await.unwrap();
        let mut rest = Vec::new();
        let _ = client.read_to_end(&mut rest).await;
    });
    let count = main.block_on(received).unwrap();
    let wall = started.elapsed();
    assert_eq!(count, total);
    entry_stop.notify_one();
    exit_stop.notify_one();
    let entry_cpu = entry_wait.recv().unwrap();
    let exit_cpu = exit_wait.recv().unwrap();
    drop((entry, exit));
    (entry_cpu, exit_cpu, wall)
}

#[test]
#[ignore = "测量，release 下手动跑"]
fn cpu_seconds_per_gigabyte() {
    const TOTAL: usize = 2 << 30;
    let gigabytes = TOTAL as f64 / f64::from(1 << 30);
    let hello = i3t_flow::testing::client_hello(b"");
    let plain = b"GET / HTTP/1.1\r\n\r\n".to_vec();
    let cases: [(&str, &[u8], Engine); 3] = [
        ("sealed", &plain, None),
        ("passthrough + splice", &hello, Some(Splicer::new)),
        ("passthrough + 用户态拷贝", &hello, Some(Splicer::copying)),
    ];
    for (name, first, engine) in cases {
        let (entry, exit, wall) = cpu_for(TOTAL, first, engine);
        eprintln!(
            "{name}（负载 {}）：入口 {:.3} s/GB，出口 {:.3} s/GB，墙钟 {:.2} GB/s",
            load(),
            entry.as_secs_f64() / gigabytes,
            exit.as_secs_f64() / gigabytes,
            gigabytes / wall.as_secs_f64()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "测量，release 下手动跑"]
async fn time_from_claim_to_first_byte_with_a_warm_pool() {
    let pair = common::Pair::direct(quick()).await;
    pair.ready(4).await;
    let exit = Arc::clone(&pair.exit.lanes);
    tokio::spawn(async move {
        while let Some(incoming) = exit.accept().await {
            tokio::spawn(async move {
                let (mut send, mut recv) = (incoming.send, incoming.recv);
                let mut header = [0; 9];
                recv.read_exact(&mut header).await.unwrap();
                send.write_all(b"R").await.unwrap();
                send.end_header(Payload::Passthrough);
                // 取用的一方读到首字节就丢弃了流（重置），这里结束不了也无所谓。
                let _ = send.shutdown().await;
            });
        }
    });
    for payload in [Payload::Sealed, Payload::Passthrough] {
        let mut samples = Vec::new();
        for _ in 0..2000 {
            // 池总是热的：每次取用之后等一下，让补拨补上。
            tokio::time::sleep(Duration::from_millis(2)).await;
            let started = Instant::now();
            let (mut send, mut recv) = pair.entry.lanes.open_stream(pair.exit.id).await.unwrap();
            send.write_all(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0, 0, 0]).await.unwrap();
            send.end_header(payload);
            let mut first = [0; 1];
            recv.read_exact(&mut first).await.unwrap();
            samples.push(started.elapsed());
        }
        samples.sort();
        let at = |q: usize| samples[samples.len() * q / 100];
        let status = pair.entry.lanes.status();
        eprintln!(
            "{payload:?}（负载 {}）：取用到首字节 p50 {:?}，p90 {:?}，p99 {:?}；取用 {} 次，其中要等的 {} 次",
            load(),
            at(50),
            at(90),
            at(99),
            status.total.claims,
            status.total.claims_waited
        );
    }
}
