//! 流的发送优先级在真的连接上有没有用：一条慢的路上有很多条流在传大块数据时，一条小消息要等多久。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Node, Peer, SOON, View};
use i3t_conn::contract::Priority;
use i3t_conn::{Acceptor, Connector, Direction, RecvHalf, SendHalf, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 大块数据一块多大。收的一方每收满一块回一个字节。
const CHUNK: usize = 64 * 1024;
/// 小消息多大。
const MESSAGE: usize = 64;

const MODE_ECHO: u8 = b'E';
const MODE_SINK: u8 = b'S';

/// B 上的服务：流的第一个字节说要什么。回显，或者只收不回（每收满一块回一个字节，写的一方靠它知道对面收到哪里了）。
fn spawn_service(node: &Node) {
    let endpoint = Arc::clone(&node.endpoint);
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            tokio::spawn(serve(incoming.send, incoming.recv));
        }
    });
}

async fn serve(mut send: SendHalf, mut recv: RecvHalf) {
    let Ok(mode) = recv.read_u8().await else { return };
    let mut buf = vec![0; CHUNK];
    loop {
        let read = if mode == MODE_ECHO { recv.read(&mut buf).await } else { recv.read_exact(&mut buf).await };
        let reply: &[u8] = match (read, mode) {
            (Ok(0) | Err(_), _) => return,
            (Ok(len), MODE_ECHO) => &buf[..len],
            (Ok(_), _) => &[1],
        };
        if send.write_all(reply).await.is_err() {
            return;
        }
    }
}

/// 一条不停地往 B 写大块数据的流。`window` 是写出去、对面还没有确认收到的最多几块：
/// 它决定这条流在连接里压着多少数据。`None` 是不管对面，能写多少写多少。
fn spawn_bulk(mut send: SendHalf, mut recv: RecvHalf, window: Option<usize>) {
    tokio::spawn(async move {
        let chunk = vec![0x42; CHUNK];
        let mut unconfirmed = 0;
        loop {
            if send.write_all(&chunk).await.is_err() {
                return;
            }
            unconfirmed += 1;
            // 不管对面的时候也要把确认读走，不然回程的流量控制会把对面卡住。
            while window.is_none_or(|window| unconfirmed >= window) {
                match (window, tokio::time::timeout(Duration::ZERO, recv.read_u8()).await) {
                    (_, Ok(Ok(_))) => unconfirmed -= 1,
                    (_, Ok(Err(_))) => return,
                    (None, Err(_)) => break,
                    (Some(_), Err(_)) => match recv.read_u8().await {
                        Ok(_) => unconfirmed -= 1,
                        Err(_) => return,
                    },
                }
            }
        }
    });
}

async fn open(from: &Node, to: &Node, mode: u8, priority: Priority) -> (SendHalf, RecvHalf) {
    let (mut send, recv) = tokio::time::timeout(SOON, from.endpoint.open(to.id)).await.expect("开流超时").unwrap();
    send.set_priority(priority);
    send.write_all(&[mode]).await.unwrap();
    (send, recv)
}

/// 在一条回显流上发一条小消息，等它回来，返回用了多久。
async fn round_trip(send: &mut SendHalf, recv: &mut RecvHalf) -> Duration {
    let started = Instant::now();
    send.write_all(&[0x69; MESSAGE]).await.unwrap();
    let mut echoed = [0; MESSAGE];
    tokio::time::timeout(3 * SOON, recv.read_exact(&mut echoed)).await.expect("小消息迟迟没有回来").unwrap();
    started.elapsed()
}

fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// 测一种情形：`bulk_streams` 条低优先级的流在传大块数据，同时在一条高优先级的流和一条低优先级的流上轮流发小消息。
/// 返回两者往返时间的中位数：(高优先级, 低优先级)。
async fn measure(bulk_streams: usize, window: Option<usize>, per_batch: Duration) -> (Duration, Duration) {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    spawn_service(&b);

    let mut urgent = open(&a, &b, MODE_ECHO, Priority::High).await;
    let mut patient = open(&a, &b, MODE_ECHO, Priority::Low).await;
    round_trip(&mut urgent.0, &mut urgent.1).await;
    round_trip(&mut patient.0, &mut patient.1).await;

    // 一条慢的路：A 每往外发一批包（最多 10 个）都要花这么久，像一条窄的上行链路。
    // 过滤器只会丢包不会拖延，所以在里面睡；包一个都不丢，连接不会因为丢包而降速，积压全在连接自己的发送缓冲里。
    a.set_filter(Some(Arc::new(move |direction, _, _: &[u8]| {
        if direction == Direction::Outbound {
            std::thread::sleep(per_batch);
        }
        true
    })));
    for _ in 0..bulk_streams {
        let (send, recv) = open(&a, &b, MODE_SINK, Priority::Low).await;
        spawn_bulk(send, recv, window);
    }
    // 等大块数据把发送缓冲填起来。
    tokio::time::sleep(Duration::from_millis(500)).await;

    let (mut high, mut low) = (Vec::new(), Vec::new());
    for _ in 0..15 {
        high.push(round_trip(&mut urgent.0, &mut urgent.1).await);
        low.push(round_trip(&mut patient.0, &mut patient.1).await);
    }
    a.set_filter(None);
    (median(&mut high), median(&mut low))
}

/// 路有多慢：每批包 10 毫秒，也就是每秒大约一千个包、十兆比特。
const PER_BATCH: Duration = Duration::from_millis(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_small_message_on_a_high_priority_stream_is_not_stuck_behind_bulk_streams_on_a_slow_path() {
    // 160 条流各压着一块数据等着发。同一优先级的流是轮流发的，一个包只带一条流的数据，
    // 所以排在它们中间的小消息要等上一百多个包；高优先级的小消息下一个包就走。
    let (high, low) = measure(160, Some(1), PER_BATCH).await;
    assert!(low > 2 * high, "高优先级的小消息往返 {high:?}，低优先级的 {low:?}：应该差出好几倍");
}

/// 不是断言，是量出来给人看的表：`cargo test -p i3t-conn --test priority -- --ignored --nocapture`。
///
/// `window` 是 `None` 的几行里，写的一方不管对面收到哪里，能写多少写多少，每条流在连接里压着一个流窗口（2MB）的数据。
/// 压着的总量到了连接的发送窗口之后，高优先级的小消息也要先等别人腾出位置才写得进去，优先级就帮不上了。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "打印一张延迟的表，要半分钟，不做断言"]
async fn print_the_round_trip_of_a_small_message_under_different_bulk_loads() {
    for (bulk_streams, window) in [(0, Some(1)), (4, Some(1)), (32, Some(1)), (160, Some(1)), (4, None), (24, None), (64, None)] {
        let (high, low) = measure(bulk_streams, window, PER_BATCH).await;
        eprintln!("bulk_streams={bulk_streams:<4} window={window:<8?} high={high:<14?} low={low:?}");
    }
}
