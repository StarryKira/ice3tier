//! 接受方在读 OPEN 之前按发起节点筛流：不该来的立刻重置，同时开着的太多的立刻拒绝，都不读它们发来的东西。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use i3t_conn::{Connector, RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_flow::memory::{MemoryConnector, MemoryMesh, MemoryRecv, MemorySend};
use i3t_flow::{FlowError, Handler, Request, Screening};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Open, Origin, Reason, Reply, Status, Target};
use tokio::sync::Semaphore;
use tokio::time::{sleep, timeout};

const EXIT: u32 = 1;
const ALLOWED: u32 = 2;
const OTHER: u32 = 3;
const STRANGER: u32 = 4;
const LIMIT: usize = 2;
const PATIENCE: Duration = Duration::from_secs(5);

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

fn request() -> Open {
    Open { target: Target::Node { port: 80 }, class: Class::Auto, origin: Origin::default() }
}

/// 一个筛流的处理者：`STRANGER` 一律重置；一个节点手上已经有 `LIMIT` 条流时拒绝。
/// 放行的流在这里一直占着，直到测试放走它（`release`），然后被拒绝掉。
struct Gate {
    screened: AtomicUsize,
    handled: AtomicUsize,
    release: Semaphore,
}

impl Handler for Gate {
    fn screen(&self, peer: NodeId, in_progress: usize) -> Screening {
        self.screened.fetch_add(1, Ordering::Relaxed);
        if peer == node(STRANGER) {
            Screening::Reset(Reason::AccessRevoked)
        } else if in_progress >= LIMIT {
            Screening::Refuse(Reply::new(Status::TooManyStreams, "同时开着的流太多"))
        } else {
            Screening::Admit
        }
    }

    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        self.handled.fetch_add(1, Ordering::Relaxed);
        self.release.acquire().await.unwrap().forget();
        request.refuse(&Reply::new(Status::NotEnabled, "放走了")).await;
    }
}

struct World {
    mesh: MemoryMesh,
    gate: Arc<Gate>,
}

impl World {
    fn new() -> Self {
        let mesh = MemoryMesh::new();
        let (_, acceptor) = mesh.join(node(EXIT));
        let gate = Arc::new(Gate { screened: AtomicUsize::new(0), handled: AtomicUsize::new(0), release: Semaphore::new(0) });
        let handler = Arc::clone(&gate);
        tokio::spawn(async move { i3t_flow::serve(&acceptor, handler).await });
        Self { mesh, gate }
    }

    fn connector(&self, raw: u32) -> MemoryConnector {
        self.mesh.join(node(raw)).0
    }

    /// 开一条流、发 OPEN，等接受方的结论。
    async fn ask(&self, connector: &MemoryConnector) -> Result<(), FlowError> {
        let pending = i3t_flow::open(connector, node(EXIT), &request(), b"first data").await?;
        timeout(PATIENCE, pending.established()).await.expect("接受方应该很快给出结论").map(drop)
    }

    /// 开一条流放在那里，让处理者占着它。
    fn hold(&self, connector: &MemoryConnector) {
        let connector = connector.clone();
        tokio::spawn(async move {
            let pending = i3t_flow::open(&connector, node(EXIT), &request(), b"").await.unwrap();
            let _ = pending.established().await;
        });
    }

    /// 反复开流，直到有一条被放行（处理者占着它，迟迟没有结论）。
    async fn until_admitted(&self, connector: &MemoryConnector) {
        let admitted = timeout(PATIENCE, async {
            loop {
                match timeout(Duration::from_millis(300), self.ask(connector)).await {
                    Err(_) => return,
                    Ok(outcome) => {
                        assert_eq!(status_of(outcome), Status::TooManyStreams);
                        sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        });
        admitted.await.expect("名额空出来之后应该又能开流");
    }

    async fn wait_handled(&self, count: usize) {
        let waited = timeout(PATIENCE, async {
            while self.gate.handled.load(Ordering::Relaxed) < count {
                sleep(Duration::from_millis(2)).await;
            }
        });
        waited.await.unwrap_or_else(|_| panic!("处理者应该收到 {count} 条流"));
    }
}

fn status_of(outcome: Result<(), FlowError>) -> Status {
    match outcome {
        Err(FlowError::Refused(reply)) => reply.status,
        other => panic!("应该被拒绝，实际是 {other:?}"),
    }
}

#[tokio::test]
async fn streams_from_a_peer_that_is_screened_out_are_reset_before_their_open_is_read() {
    let world = World::new();
    let stranger = world.connector(STRANGER);

    // 发起方看到的和对方读了 OPEN 再拒绝一样：访问规则不允许。
    assert_eq!(status_of(world.ask(&stranger).await), Status::AccessDenied);
    assert_eq!(world.gate.handled.load(Ordering::Relaxed), 0, "被筛掉的流不交给处理者");

    // 流本身是被重置的，没有 REPLY。
    let (_send, mut recv) = stranger.open(node(EXIT)).await.unwrap();
    let mut byte = [0; 1];
    let error = timeout(PATIENCE, tokio::io::AsyncReadExt::read(&mut recv, &mut byte)).await.unwrap().expect_err("流应该被重置");
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Reset(StreamCode(Reason::AccessRevoked.to_wire()))));

    // 发起方写的东西，接受方一个字节也没读：它那边的接收方向是被叫停的。
    let (mut send, _recv) = stranger.open(node(EXIT)).await.unwrap();
    let written = timeout(PATIENCE, async {
        loop {
            if let Err(error) = tokio::io::AsyncWriteExt::write_all(&mut send, &[0; 4096]).await {
                return error;
            }
        }
    });
    let error = written.await.expect("接受方叫停之后写应该出错");
    assert_eq!(StreamAborted::of(&error), Some(StreamAborted::Stopped(StreamCode(Reason::AccessRevoked.to_wire()))));
}

#[tokio::test]
async fn a_peer_with_too_many_streams_in_progress_is_refused_until_one_of_them_ends() {
    let world = World::new();
    let allowed = world.connector(ALLOWED);
    for _ in 0..LIMIT {
        world.hold(&allowed);
    }
    world.wait_handled(LIMIT).await;

    assert_eq!(status_of(world.ask(&allowed).await), Status::TooManyStreams);
    assert_eq!(world.gate.handled.load(Ordering::Relaxed), LIMIT, "被拒绝的流不交给处理者");

    // 别的节点各算各的。
    world.hold(&world.connector(OTHER));
    world.wait_handled(LIMIT + 1).await;

    // 放走一条（先到的先放），就又能开一条。
    world.gate.release.add_permits(1);
    world.until_admitted(&allowed).await;
}

#[tokio::test]
async fn streams_that_have_not_sent_their_open_yet_count_as_in_progress() {
    let world = World::new();
    let allowed = world.connector(ALLOWED);
    // 只开流、不写 OPEN：接受方还在等它的 OPEN，这也算占着。
    let silent = open_silently(&allowed, LIMIT).await;
    let screened = timeout(PATIENCE, async {
        while world.gate.screened.load(Ordering::Relaxed) < LIMIT {
            sleep(Duration::from_millis(2)).await;
        }
    });
    screened.await.expect("接受方应该看到这几条流");
    assert_eq!(status_of(world.ask(&allowed).await), Status::TooManyStreams);
    assert_eq!(world.gate.handled.load(Ordering::Relaxed), 0);

    // 不写 OPEN 的流被丢掉（重置）之后名额空出来。
    drop(silent);
    world.until_admitted(&allowed).await;
}

/// 到出口开 `count` 条流，什么都不写。
async fn open_silently(connector: &MemoryConnector, count: usize) -> Vec<(MemorySend, MemoryRecv)> {
    let mut streams = Vec::new();
    for _ in 0..count {
        streams.push(connector.open(node(EXIT)).await.unwrap());
    }
    streams
}

#[tokio::test]
async fn a_ping_is_answered_after_the_same_screening_without_reaching_the_handler() {
    let world = World::new();
    let rtt = i3t_flow::ping(&world.connector(ALLOWED), node(EXIT)).await.unwrap();
    assert!(rtt < PATIENCE, "{rtt:?}");
    let refused = i3t_flow::ping(&world.connector(STRANGER), node(EXIT)).await.unwrap_err();
    assert!(matches!(refused, FlowError::Refused(ref reply) if reply.status == Status::AccessDenied), "{refused:?}");
    assert_eq!(world.gate.screened.load(Ordering::Relaxed), 2, "往返测量照样先筛");
    assert_eq!(world.gate.handled.load(Ordering::Relaxed), 0, "处理者看不到往返测量");
}
