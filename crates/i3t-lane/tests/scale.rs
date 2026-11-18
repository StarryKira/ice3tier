//! 几千条流同时开着、静默一段时间、再一起说话。报告每条静默的流占多少文件描述符和堆内存（两头合计，不含内核的 socket 缓冲）。
//!
//! 数字只在 release 下有意义：`cargo test --release -p i3t-lane --test scale -- --nocapture`。

// 量的是 Linux 上的数（线程 CPU 时间、/proc）。
#![cfg(target_os = "linux")]
// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]
// 只是把计数折成平均数打印出来，精度无所谓。
#![allow(clippy::cast_precision_loss)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Pair, accept_next, quick, wait_for};
use i3t_conn::SendStream as _;
use i3t_lane::{LaneRecv, LaneSend, Lanes, Payload};
use i3t_proto::NodeId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, sleep, timeout};

/// 数一数现在堆上还占着多少字节。比 RSS 准：分配器会把释放的内存留着不还给系统，RSS 看不出流到底占了多少。
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub static LIVE: AtomicUsize = AtomicUsize::new(0);

    pub struct Counting;

    // SAFETY: 每个方法都原样转给系统分配器，调用方对 GlobalAlloc 的约定原样传下去；这里只多做了一个原子计数。
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: 调用方保证 layout 合法（GlobalAlloc::alloc 的约定）。
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                LIVE.fetch_add(layout.size(), Ordering::Relaxed);
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            // SAFETY: ptr 是这个分配器用同一个 layout 分出来的（GlobalAlloc::dealloc 的约定）。
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: 同 alloc 和 dealloc：ptr 和 layout 来自这个分配器，new_size 由调用方保证合法。
            let moved = unsafe { System.realloc(ptr, layout, new_size) };
            if !moved.is_null() {
                LIVE.fetch_add(new_size, Ordering::Relaxed);
                LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            }
            moved
        }
    }
}

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

const STREAMS: usize = 2000;
const BATCH: usize = 250;
const MESSAGE: usize = 1024;

/// 本进程现在有多少个文件描述符、堆上占着多少字节。
fn resources() -> (usize, usize) {
    let fds = std::fs::read_dir("/proc/self/fd").map(Iterator::count).unwrap_or(0);
    (fds, counting::LIVE.load(std::sync::atomic::Ordering::Relaxed))
}

type Halves = (LaneSend, LaneRecv);

/// 入口开 `count` 条流到出口（一批一批地并发开），出口接住、回一个字节的 REPLY。返回配好对的两头。
///
/// 头是一条看上去像 ClientHello 的记录（这样 passthrough 才会被选上），最后三个字节是流的编号，出口靠它配对。
async fn open_many(entry: &Arc<Lanes>, peer: NodeId, exit: &Arc<Lanes>, count: usize, payload: Payload) -> Vec<(Halves, Halves)> {
    let mut near: Vec<Option<Halves>> = (0..count).map(|_| None).collect();
    let mut far: Vec<Option<Halves>> = (0..count).map(|_| None).collect();
    for start in (0..count).step_by(BATCH) {
        let batch = BATCH.min(count - start);
        let mut opened = tokio::task::JoinSet::new();
        for index in start..start + batch {
            let entry = Arc::clone(entry);
            opened.spawn(async move {
                let (mut send, mut recv) = entry.open_stream(peer).await.unwrap();
                let [_, a, b, c] = u32::try_from(index).unwrap().to_be_bytes();
                send.write_all(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, a, b, c]).await.unwrap();
                send.end_header(payload);
                let mut reply = [0; 1];
                recv.read_exact(&mut reply).await.unwrap();
                (index, (send, recv))
            });
        }
        for _ in 0..batch {
            let (mut send, mut recv) = accept_next(exit).await;
            let mut header = [0; 9];
            recv.read_exact(&mut header).await.unwrap();
            let index = usize::try_from(u32::from_be_bytes([0, header[6], header[7], header[8]])).unwrap();
            send.write_all(b"R").await.unwrap();
            send.end_header(Payload::Passthrough);
            far[index] = Some((send, recv));
        }
        while let Some(stream) = timeout(Duration::from_secs(30), opened.join_next()).await.unwrap() {
            let (index, halves) = stream.unwrap();
            near[index] = Some(halves);
        }
    }
    near.into_iter().zip(far).map(|(near, far)| (near.unwrap(), far.unwrap())).collect()
}

/// 开 [`STREAMS`] 条流，像搬运那样每条两头各有一个任务守着（入口那头等着读，出口那头等着有话可说），静默一阵，量资源；
/// 然后一起说话。`report` 为假时只是热身。
async fn silent_then_active(pair: &Pair, payload: Payload, report: bool) {
    sleep(Duration::from_millis(300)).await;
    let (fds_before, heap_before) = resources();
    let before = pair.entry.lanes.status();
    let started = Instant::now();
    let streams = open_many(&pair.entry.lanes, pair.exit.id, &pair.exit.lanes, STREAMS, payload).await;
    let opening = started.elapsed();

    let (go, _) = tokio::sync::watch::channel(false);
    let mut talkers = tokio::task::JoinSet::new();
    for ((mut near_send, mut near_recv), (mut far_send, mut far_recv)) in streams {
        talkers.spawn(async move {
            let mut got = Vec::new();
            near_recv.read_to_end(&mut got).await.unwrap();
            assert_eq!(got.len(), MESSAGE);
            // 两个方向都正常结束，两头都不重置。
            near_send.shutdown().await.unwrap();
        });
        let mut go = go.subscribe();
        talkers.spawn(async move {
            go.wait_for(|go| *go).await.unwrap();
            far_send.write_all(&[7; MESSAGE]).await.unwrap();
            far_send.shutdown().await.unwrap();
            let mut rest = Vec::new();
            assert_eq!(far_recv.read_to_end(&mut rest).await.unwrap(), 0);
        });
    }
    // 静默一阵：缓冲应该都还回去了。
    sleep(Duration::from_secs(2)).await;
    let (fds_after, heap_after) = resources();
    let status = pair.entry.lanes.status();
    assert_eq!(status.total.in_use, u64::try_from(STREAMS).unwrap());
    let per_stream_fds = (fds_after.saturating_sub(fds_before)) as f64 / STREAMS as f64;
    let per_stream_kb = (heap_after.saturating_sub(heap_before)) as f64 / 1024.0 / STREAMS as f64;
    if report {
        eprintln!(
            "{payload:?}: {STREAMS} 条流用了 {opening:?} 开好；两头合计每条静默的流 {per_stream_fds:.2} 个文件描述符、{per_stream_kb:.1} KB 堆内存（{} → {} KB，含两头守着它的任务；量的时候池里另有 {} 条空闲的）；取用 {} 次，其中等待 {} 次，共等 {:?}",
            heap_before / 1024,
            heap_after / 1024,
            status.idle(),
            status.total.claims - before.total.claims,
            status.total.claims_waited - before.total.claims_waited,
            status.total.claim_wait - before.total.claim_wait
        );
        assert!(per_stream_fds < 2.5, "每条流两头各一个 socket，多出来的是池里的空闲 lane：{per_stream_fds}");
    }

    let started = Instant::now();
    go.send_replace(true);
    while let Some(talked) = timeout(Duration::from_secs(60), talkers.join_next()).await.unwrap() {
        talked.unwrap();
    }
    if report {
        eprintln!("{payload:?}: {STREAMS} 条流一起各收 {MESSAGE} 字节、两头正常结束，用了 {:?}", started.elapsed());
    }
    wait_for("流都结束了", Duration::from_secs(10), || pair.entry.lanes.status().total.in_use == 0).await;
}

/// 同时开着的文件描述符要能有这么多：每条流两头各一个 socket，再留出池里空闲的 lane 和测试自己的零碎。
const FDS_NEEDED: u64 = 2 * STREAMS as u64 + 1024;

/// 把能同时打开的文件数的软上限抬到 [`FDS_NEEDED`]（不超过硬上限）。抬不到那么高时返回现在的上限：
/// 很多桌面和容器的软上限只有 1024，几千条流开到一半就开不出 socket，看起来像是 lane 的池子干了。
fn enough_open_files() -> Result<(), u64> {
    let limit = rlimit::increase_nofile_limit(FDS_NEEDED).unwrap();
    if limit >= FDS_NEEDED { Ok(()) } else { Err(limit) }
}

/// 两种负载方式依次跑：同一个进程里的数字才能比，所以放在一个测试里，不和别的测试并行。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousands_of_streams_stay_silent_and_then_all_talk() {
    if let Err(limit) = enough_open_files() {
        eprintln!("跳过：能同时打开的文件数最多只能是 {limit}，这个测试要 {FDS_NEEDED}（ulimit -Hn 太低）");
        return;
    }
    let pair = Pair::direct(quick()).await;
    pair.ready(4).await;
    silent_then_active(&pair, Payload::Sealed, false).await;
    silent_then_active(&pair, Payload::Passthrough, true).await;
    silent_then_active(&pair, Payload::Sealed, true).await;
}
