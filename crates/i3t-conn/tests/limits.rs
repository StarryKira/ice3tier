//! 一个对端在本节点上能占多少：连接的条数有上限，多出来的关掉最旧的。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{Node, Peer, SOON, View};
use i3t_conn::{Acceptor, Connector, RecvHalf, SendHalf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

/// `from` 到 `to` 开一条流写几个字节，`to` 接到它。返回两头：发起的发送方向，和接受的接收方向。
///
/// 重启回来的节点要等 `to` 发现它的旧地址不通、验证过新地址才连得上，所以开流失败时接着试。
async fn stream(from: &Node, to: &Node) -> (SendHalf, RecvHalf) {
    let opened = timeout(3 * SOON, async {
        loop {
            match from.endpoint.open(to.id).await {
                Ok(opened) => return opened,
                Err(error) => eprintln!("开流失败，再试：{error}"),
            }
        }
    });
    let (mut send, _recv) = opened.await.expect("一直开不了流");
    send.write_all(b"hello").await.unwrap();
    let mut incoming = timeout(SOON, to.endpoint.accept()).await.expect("等对端开过来的流超时").unwrap();
    let mut hello = [0; 5];
    timeout(SOON, incoming.recv.read_exact(&mut hello)).await.expect("读流超时").unwrap();
    (send, incoming.recv)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_keeps_coming_back_cannot_pile_up_connections() {
    let a = Node::start(1);
    let first = Node::start(2);
    let key = Arc::clone(&first.key);
    a.apply(&View { peers: &[Peer::direct(&first)], ..View::default() });
    first.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let (_send, mut held) = stream(&first, &a).await;

    // 它崩溃了：一个包也不再发，连关闭帧也没有。a 上的那条连接要等空闲超时（30 秒）才会自己断。
    let crashed = Instant::now();
    first.block_all();
    drop(first);

    // 它带着同一把密钥换了端口回来，又崩溃了一次，再回来。a 上于是先后有三条它的连接，前两条在 a 看来还活着。
    let back = Node::start_with_key(2, Arc::clone(&key));
    a.apply(&View { peers: &[Peer::direct(&back)], ..View::default() });
    back.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let second = stream(&back, &a).await;
    back.block_all();
    drop((back, second));

    let back = Node::start_with_key(2, Arc::clone(&key));
    a.apply(&View { peers: &[Peer::direct(&back)], ..View::default() });
    back.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let (mut send, mut recv) = stream(&back, &a).await;

    // 第三条连接进来时，a 关掉了最旧的那条，上面的流马上就知道，不用等到空闲超时。
    let mut buf = [0; 16];
    let read = timeout(Duration::from_secs(5), held.read(&mut buf)).await.expect("最旧的连接应该马上被关掉");
    assert!(read.is_err(), "被关掉的连接上的流应该读出错，而不是读到 {read:?}");
    assert!(crashed.elapsed() < Duration::from_secs(25), "要在那条连接自己空闲超时之前，才说明是上限关掉的它");

    // 现在的这条照常工作。
    send.write_all(b"still").await.unwrap();
    let mut still = [0; 5];
    timeout(SOON, recv.read_exact(&mut still)).await.expect("现在的连接应该照常工作").unwrap();
}

/// 一条连接的接收窗口：对端发过来、本端还没有读走的数据，所有的流加起来最多这么多。
const RECEIVE_WINDOW: u64 = 64 * 1024 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_cannot_park_more_than_one_receive_window_on_a_connection_that_is_not_read() {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });

    // a 接下每条流，却一个字节也不读：就像出口在等一个不说话的目标，或者对端在流的开头留了空洞。
    let acceptor = Arc::clone(&a.endpoint);
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Some(incoming) = acceptor.accept().await {
            held.push(incoming);
        }
    });

    // b 开 48 条流，每条想写满一个流窗口（2MB），一共 96MB，比接收窗口多。
    let written = Arc::new(AtomicU64::new(0));
    let chunk = vec![7u8; 64 * 1024];
    for _ in 0..48 {
        let (mut send, recv) = timeout(SOON, b.endpoint.open(a.id)).await.expect("开流超时").expect("开流失败");
        let (written, chunk) = (Arc::clone(&written), chunk.clone());
        tokio::spawn(async move {
            let _recv = recv;
            for _ in 0..32 {
                if send.write_all(&chunk).await.is_err() {
                    return;
                }
                written.fetch_add(64 * 1024, Ordering::Relaxed);
            }
            std::future::pending::<()>().await;
        });
    }

    // 等写不动了为止。
    let mut last = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = written.load(Ordering::Relaxed);
        if now == last {
            break;
        }
        last = now;
    }
    assert!(last > RECEIVE_WINDOW / 2, "数据应该是流动过的，只写进去了 {last} 字节");
    assert!(last <= RECEIVE_WINDOW + 2 * 1024 * 1024, "对端压住的数据不该超过一个接收窗口，实际写进去了 {last} 字节");
}
