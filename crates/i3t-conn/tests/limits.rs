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

/// `from` 到 `to` 开一条流，来回一趟，`to` 接到它。返回 (发起的那一头, 接受的那一头)，各是 (发送方向, 接收方向)。
///
/// 重启回来的节点要等 `to` 发现它的旧地址不通、验证过新地址才连得上，所以开流失败时接着试。
///
/// 来回一趟之后 `to` 发过的东西都得到了确认，四个方向又都交给调用者拿着（丢掉哪一个，那一头就要在连接上发帧）：
/// `from` 这时崩溃的话，`to` 在这条连接上没有要重发的，下一个包是保活。不这样的话 `to` 会一直重发，
/// `from` 带着同一把密钥换了地址回来之后，重发的包到了它手上，它不认得这条连接，回一个无状态重置，
/// 连接什么时候断就看重发赶在什么时候了。
async fn stream(from: &Node, to: &Node) -> ((SendHalf, RecvHalf), (SendHalf, RecvHalf)) {
    let opened = timeout(3 * SOON, async {
        loop {
            match from.endpoint.open(to.id).await {
                Ok(opened) => return opened,
                Err(error) => eprintln!("开流失败，再试：{error}"),
            }
        }
    });
    let (mut send, mut recv) = opened.await.expect("一直开不了流");
    send.write_all(b"hello").await.unwrap();
    let mut incoming = timeout(SOON, to.endpoint.accept()).await.expect("等对端开过来的流超时").unwrap();
    let mut word = [0; 5];
    timeout(SOON, incoming.recv.read_exact(&mut word)).await.expect("读流超时").unwrap();
    incoming.send.write_all(b"olleh").await.unwrap();
    timeout(SOON, recv.read_exact(&mut word)).await.expect("读回答超时").unwrap();
    // 带着这几个字节的包也带着对 "olleh" 的确认。
    send.write_all(b"again").await.unwrap();
    timeout(SOON, incoming.recv.read_exact(&mut word)).await.expect("读流超时").unwrap();
    ((send, recv), (incoming.send, incoming.recv))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_keeps_coming_back_cannot_pile_up_connections() {
    let a = Node::start(1);
    let first = Node::start(2);
    let key = Arc::clone(&first.key);
    a.apply(&View { peers: &[Peer::direct(&first)], ..View::default() });
    first.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let (_first_opener, (_unsent, mut held)) = stream(&first, &a).await;

    // 它崩溃了：一个包也不再发，连关闭帧也没有。a 上的那条连接要等空闲超时（30 秒）才会自己断。
    let crashed = Instant::now();
    first.block_all();
    drop(first);

    // 它带着同一把密钥换了端口回来，又崩溃了一次，再回来。a 上于是先后有三条它的连接，前两条在 a 看来还活着。
    let back = Node::start_with_key(2, Arc::clone(&key));
    a.apply(&View { peers: &[Peer::direct(&back)], ..View::default() });
    back.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let (_second_opener, (_second_unsent, mut second_held)) = stream(&back, &a).await;
    back.block_all();
    drop(back);

    let back = Node::start_with_key(2, Arc::clone(&key));
    a.apply(&View { peers: &[Peer::direct(&back)], ..View::default() });
    back.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    let ((mut send, _unread), (_unsent_third, mut recv)) = stream(&back, &a).await;

    // a 的路换到回来的它那里时，会把留底的最后几个包（路径层的 stash，最多留 20 秒）从新路补发一遍，里面常有前两条连接的包。
    // 回来的它不认得那些连接，回无状态重置，它们就在第三条握完手之前断了。所以第三条进来时 a 上多半只剩一两条，
    // 偶尔才是三条。上限本身（三条都活着时关最旧的）由 manager 模块里的单元测试用真的连接管着。
    let mut buf = [0; 16];
    let second_alive = timeout(Duration::ZERO, second_held.read(&mut buf)).await.is_err();
    let read = if second_alive {
        // 三条：上限关掉了最旧的那条，上面的流马上就知道。
        timeout(Duration::from_secs(5), held.read(&mut buf)).await.expect("最旧的连接应该马上被关掉")
    } else {
        // 轮不到上限。最旧的那条要么已经这样被重置了，要么最晚在它下一次保活（15 秒）时被回来的它重置掉。
        timeout(Duration::from_secs(25).saturating_sub(crashed.elapsed()), held.read(&mut buf))
            .await
            .expect("最旧的连接应该在保活时被重置掉")
    };
    assert!(read.is_err(), "被关掉的连接上的流应该读出错，而不是读到 {read:?}");
    assert!(crashed.elapsed() < Duration::from_secs(25), "要在那条连接自己空闲超时之前，才说明不是空闲超时关掉的它");

    // 现在的这条照常工作。
    send.write_all(b"still").await.unwrap();
    let mut still = [0; 5];
    timeout(SOON, recv.read_exact(&mut still)).await.expect("现在的连接应该照常工作").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_crashed_and_came_back_ends_its_old_connection_at_once() {
    let (a, r) = (Node::start(1), Node::start(9));
    let first = Node::start(2);
    let key = Arc::clone(&first.key);
    // A 和 B 只经 relay R 来往：B 重启换了端口之后，A 在旧连接上发的包照样经 R 送到新的 B 手上。
    let via_relay = |b: &Node| {
        r.apply(&View { relay_clients: Some(&[&a, b]), ..View::default() });
        a.apply(&View { peers: &[Peer::via(b, &[&r])], relays: &[&r], reach: Some("relay_only"), ..View::default() });
        b.apply(&View { peers: &[Peer::via(&a, &[&r])], relays: &[&r], reach: Some("relay_only"), ..View::default() });
    };
    via_relay(&first);
    r.wait_tcp_listening().await;
    let ((mut send, _unread), _far) = stream(&a, &first).await;

    // B 崩溃了：一个包也不再发，连关闭帧也没有。然后带着同一把密钥回来。
    first.block_all();
    drop(first);
    let crashed = Instant::now();
    let back = Node::start_with_key(2, key);
    via_relay(&back);

    // A 接着在旧连接上发。新的 B 认得出这是自己发出去过的连接编号，却没有这条连接，于是回一个无状态重置；
    // A 凭它立刻知道连接没了，不用等 30 秒的空闲超时。
    let ended = timeout(Duration::from_secs(10), async {
        while send.write_all(&[0; 1024]).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    ended.await.expect("旧连接应该在几秒之内结束，而不是等到空闲超时");

    // 新开的流马上可用。
    let ((mut send, _unread), (_unsent, mut recv)) = stream(&a, &back).await;
    send.write_all(b"again").await.unwrap();
    let mut again = [0; 5];
    timeout(SOON, recv.read_exact(&mut again)).await.expect("新的连接应该照常工作").unwrap();
    assert!(crashed.elapsed() < Duration::from_secs(20), "整个恢复用了 {:?}", crashed.elapsed());
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
