//! 没有认证过的来源能从一个节点那里得到什么：什么都得不到。
//! 这里用一个最原始的 UDP socket 往节点的端口上发各种帧，看它回不回。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::{Node, Peer, View, echo_once};
use i3t_conn::DropReason;
use i3t_crypto::Signature;
use i3t_proto::datagram::RelayHeader;
use i3t_proto::session::{Auth, Frame, Init, RefusalReason, Refused};
use i3t_proto::{NodeId, addr, probe, session};
use tokio::net::UdpSocket;

/// 断言"没有回应"时等多久。回环上应答在一毫秒之内就到。
const SILENCE: Duration = Duration::from_millis(300);

struct Stranger {
    socket: UdpSocket,
}

impl Stranger {
    async fn new() -> Self {
        Self { socket: UdpSocket::bind("127.0.0.1:0").await.unwrap() }
    }

    fn addr(&self) -> SocketAddr {
        self.socket.local_addr().unwrap()
    }

    async fn send(&self, datagram: &[u8], to: &Node) {
        self.socket.send_to(datagram, to.addr()).await.unwrap();
    }

    async fn recv(&self) -> Vec<u8> {
        let mut buf = [0u8; 2048];
        let (len, _from) =
            tokio::time::timeout(Duration::from_secs(3), self.socket.recv_from(&mut buf)).await.expect("等节点的回应超时").unwrap();
        buf[..len].to_vec()
    }

    async fn expect_silence(&self) {
        let mut buf = [0u8; 2048];
        if let Ok(received) = tokio::time::timeout(SILENCE, self.socket.recv_from(&mut buf)).await {
            let (len, from) = received.unwrap();
            panic!("不该有回应，却从 {from} 收到了 {:?}", &buf[..len]);
        }
    }
}

fn ping(src: &Node, dst: NodeId, nonce: [u8; 8]) -> probe::Ping {
    let signature = *src.key.sign(probe::Ping::PURPOSE, &probe::Ping::signed_message(src.id, dst, &nonce)).as_bytes();
    probe::Ping { src: src.id, dst, nonce, signature }
}

fn relayed(dst: NodeId, src: NodeId, packet: &[u8]) -> Vec<u8> {
    let mut datagram = RelayHeader { dst, src }.to_bytes().to_vec();
    datagram.extend_from_slice(packet);
    datagram
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_that_are_malformed_unsigned_or_from_strangers_get_no_reply() {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer::direct(&b)], ..View::default() });
    b.apply(&View { peers: &[Peer::direct(&a)], ..View::default() });
    b.spawn_echo();
    echo_once(&a, &b, b"before").await;
    let stranger = Stranger::new().await;
    let outsider = Node::start(77);
    let nonce = [7u8; 8];

    // 不认识的帧类型，包括 QUIC 的版本协商包可能落在的那一段。
    for kind in [0x00u8, 0x0d, 0x3f, 0x80, 0xbf] {
        stranger.send(&[kind; 64], &a).await;
    }
    // 看起来像连接包，但来源地址不是任何对端已验证的直连地址。
    stranger.send(&[0xc0; 1200], &a).await;
    stranger.send(&[0x40; 100], &a).await;
    // 自称是 B 发的探测，签名是瞎编的。
    stranger.send(&probe::Ping { signature: [0; 64], ..ping(&b, a.id, nonce) }.to_bytes(), &a).await;
    // 签名是真的，但签的人不在 A 的 netmap 里。
    stranger.send(&ping(&outsider, a.id, nonce).to_bytes(), &a).await;
    // B 发给别人的探测，被人拿来重放给 A。
    stranger.send(&ping(&b, outsider.id, nonce).to_bytes(), &a).await;
    // 比规定长度短的探测。
    stranger.send(&ping(&b, a.id, nonce).to_bytes()[..probe::Ping::LEN - 1], &a).await;
    // 没有人问过的 PONG，签名是真的。
    let message = probe::Pong::signed_message(b.id, a.id, &nonce, stranger.addr());
    let pong = probe::Pong {
        src: b.id,
        dst: a.id,
        nonce,
        observed: stranger.addr(),
        signature: *b.key.sign(probe::Pong::PURPOSE, &message).as_bytes(),
    };
    stranger.send(&pong.to_bytes(), &a).await;
    // 自称经 relay 转来的包，但来源地址不是任何 relay。
    stranger.send(&relayed(a.id, b.id, &[0xc0; 1200]), &a).await;
    // A 不是 relay：中转会话的握手、保活、要它转发的包、地址查询，都没有人理。
    stranger.send(&Init { node: b.id, nonce }.to_bytes(), &a).await;
    stranger.send(&Auth { node: b.id, nonce, cookie: [0; 16], signature: [0; 64] }.to_bytes(), &a).await;
    stranger.send(&session::Ping { node: b.id, nonce }.to_bytes(), &a).await;
    stranger.send(&relayed(b.id, outsider.id, b"forward me"), &a).await;
    stranger.send(&addr::Query { nonce }.to_bytes(), &a).await;
    // relay 才会发的帧，从一个不是 relay 的地址来。
    stranger.send(&Refused { reason: RefusalReason::NoSession, node: a.id }.to_bytes(), &a).await;
    stranger.send(&[], &a).await;

    stranger.expect_silence().await;
    assert!(a.endpoint.relay_counters().dropped(DropReason::Disabled) >= 5, "归 relay 的帧在没启用 relay 的节点上被丢掉并计数");
    // 这些都没有影响到正常的连接。
    echo_once(&a, &b, b"after").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_properly_signed_ping_from_a_peer_is_answered_with_a_signed_pong_no_larger_than_the_ping() {
    let (a, b) = (Node::start(1), Node::start(2));
    a.apply(&View { peers: &[Peer { endpoints: false, ..Peer::direct(&b) }], ..View::default() });
    a.wait_knows(&b).await;
    // B 的探测从一个 A 没见过的地址来（比如 B 在 NAT 后面）。
    let from_b = Stranger::new().await;
    let nonce = [9u8; 8];
    let request = ping(&b, a.id, nonce).to_bytes();
    from_b.send(&request, &a).await;

    // A 回 PONG，同时反过来向这个地址发 PING 做反方向的验证。两个包谁先到不一定。
    let replies = [from_b.recv().await, from_b.recv().await];
    assert!(replies.iter().all(|reply| reply.len() <= request.len()), "回应不比请求大");
    let pong = replies
        .iter()
        .find_map(|reply| if let Some(probe::Frame::Pong(pong)) = probe::Frame::parse(reply) { Some(pong) } else { None })
        .expect("应该有一个 PONG");
    assert_eq!((pong.src, pong.dst, pong.nonce, pong.observed), (a.id, b.id, nonce, from_b.addr()));
    let message = probe::Pong::signed_message(pong.src, pong.dst, &pong.nonce, pong.observed);
    i3t_crypto::verify(&a.key.public(), probe::Pong::PURPOSE, &message, &Signature::from_bytes(pong.signature))
        .expect("PONG 的签名应该是 A 的");

    let reverse = replies
        .iter()
        .find_map(|reply| if let Some(probe::Frame::Ping(ping)) = probe::Frame::parse(reply) { Some(ping) } else { None })
        .expect("应该有一个反方向的 PING");
    assert_eq!((reverse.src, reverse.dst), (a.id, b.id));
    let message = probe::Ping::signed_message(reverse.src, reverse.dst, &reverse.nonce);
    i3t_crypto::verify(&a.key.public(), probe::Ping::PURPOSE, &message, &Signature::from_bytes(reverse.signature))
        .expect("PING 的签名应该是 A 的");

    // 重放同一个 PING：再回一个 PONG，但反方向的探测一秒之内不重复发，拿它放大不了流量。
    from_b.send(&request, &a).await;
    assert!(matches!(probe::Frame::parse(&from_b.recv().await), Some(probe::Frame::Pong(_))));
    from_b.expect_silence().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_node_refuses_to_forward_for_senders_without_a_session() {
    let (a, r, b) = (Node::start(1), Node::start(9), Node::start(2));
    r.apply(&View { relay_clients: Some(&[&a, &b]), ..View::default() });
    r.wait_tcp_listening().await;
    b.apply(&View { peers: &[Peer::via(&a, &[&r])], relays: &[&r], ..View::default() });
    b.wait_route(&a, "经 relay", common::is_relay).await;
    let stranger = Stranger::new().await;

    // 没有会话的来源自称是 A，要 relay 把包转给 B。
    let forged = relayed(b.id, a.id, &[0xc0; 1200]);
    stranger.send(&forged, &r).await;
    let reply = stranger.recv().await;
    assert!(reply.len() <= forged.len());
    assert_eq!(Frame::parse(&reply), Some(Frame::Error(Refused { reason: RefusalReason::NoSession, node: a.id })));
    // 再发：不转发，这一秒里也不再回错误帧。
    stranger.send(&forged, &r).await;
    stranger.expect_silence().await;

    let counters = r.endpoint.relay_counters();
    assert_eq!(counters.dropped(DropReason::NoSession), 2);
    assert_eq!(counters.forwarded_packets, 0);

    // 握手的头一步对谁都回（应答不比请求大，relay 不记状态）；瞎编的 cookie 和签名在第二步被丢掉，不回应。
    let init = Init { node: a.id, nonce: [1; 8] }.to_bytes();
    stranger.send(&init, &r).await;
    let challenge = stranger.recv().await;
    assert!(challenge.len() <= init.len());
    let Some(Frame::Challenge(challenge)) = Frame::parse(&challenge) else { panic!("应该是 CHALLENGE") };
    stranger.send(&Auth { node: a.id, nonce: [1; 8], cookie: challenge.cookie, signature: [0; 64] }.to_bytes(), &r).await;
    stranger.send(&Auth { node: a.id, nonce: [1; 8], cookie: [0; 16], signature: [0; 64] }.to_bytes(), &r).await;
    stranger.expect_silence().await;
    let counters = r.endpoint.relay_counters();
    assert_eq!((counters.dropped(DropReason::BadSignature), counters.dropped(DropReason::BadCookie)), (1, 1));
    assert_eq!(counters.sessions, 1, "只有 B 的会话");
}
