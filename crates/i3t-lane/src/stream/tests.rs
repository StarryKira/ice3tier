//! 版本怎么谈，以及和没升级的节点（`i3t-lane/1`）之间两边各自开的 passthrough 流怎么走（issue #34）。
//!
//! 没升级的节点用这里的帧层照它的样子写出来：它只提供 `i3t-lane/1`；它开 passthrough 的流时 OPENING 是 [`Mode::Early`]，
//! 那条记录之后紧跟着不加密的字节。

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use i3t_conn::{Payload, RecvStream, SendStream};
use i3t_crypto::NodeKey;
use i3t_crypto::tls::Identity;
use i3t_proto::flow::{Class, Open, Origin, Reason, Reply, Status, Target};
use i3t_proto::{NodeId, PublicKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::dialler::await_opening;
use crate::endpoint::{LaneConfig, LanePeer, Lanes};
use crate::error::LaneError;
use crate::frame::{Frame, Mode, Purpose};
use crate::lane::{self, ALPN, ALPN_V1, Dial, Lane, Version};
use crate::status::Book;
use crate::stream::{LaneRecv, LaneSend};

/// 升级了的节点提供的版本，和 [`lane::client_config`]、[`lane::server_config`] 里的一样。
const NEW: &[&[u8]] = &[ALPN, ALPN_V1];
/// 没升级的节点只有这一个。
const OLD: &[&[u8]] = &[ALPN_V1];

const PATIENCE: Duration = Duration::from_secs(5);

fn id(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// `key` 的节点只提供 `alpn` 里的版本，拨 `addr` 上的 `to`：一条给它自己开流用的 lane，HELLO 已经发出。
async fn dial(key: &NodeKey, to: PublicKey, addr: SocketAddr, alpn: &[&[u8]]) -> Lane {
    let mut config = Arc::unwrap_or_clone(lane::client_config(&Identity::new(key).unwrap(), to).unwrap());
    config.alpn_protocols = alpn.iter().map(|alpn| alpn.to_vec()).collect();
    let addrs = [addr];
    let membership = watch::channel(()).1;
    let deadline = Instant::now() + PATIENCE;
    let dial = Dial { addrs: &addrs, config: Arc::new(config), peer: id(1), membership, purpose: Purpose::Own, epoch: 1, deadline };
    let (mut lane, _) = lane::dial(dial).await.unwrap();
    // `dial` 只把 HELLO 排进待发的密文。
    lane.flush().await.unwrap();
    lane
}

/// 回环上握完手的一条 lane 的两头：拨号方提供 `dialler` 里的版本，接受方认 `acceptor` 里的。返回（拨号方的，接受方的）。
async fn ends(dialler: &[&[u8]], acceptor: &[&[u8]]) -> (Lane, Lane) {
    let (dialler_key, acceptor_key) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
    let dialler_public = dialler_key.public();
    let mut server =
        Arc::unwrap_or_clone(lane::server_config(&Identity::new(&acceptor_key).unwrap(), move |key| *key == dialler_public).unwrap());
    server.alpn_protocols = acceptor.iter().map(|alpn| alpn.to_vec()).collect();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // 接受方读 HELLO 时也看着它：发送的一头丢了就当作对端被拿掉了。
    let (_members, membership) = watch::channel(());
    let deadline = Instant::now() + PATIENCE;
    tokio::join!(dial(&dialler_key, acceptor_key.public(), addr, dialler), async {
        let (socket, _) = listener.accept().await.unwrap();
        lane::accept(socket, BytesMut::new(), Arc::new(server), |_| Some((id(2), membership.clone())), deadline, deadline).await.unwrap()
    })
}

fn book() -> Book {
    Book { node: Arc::default(), peer: Arc::default() }
}

/// 本端在 `lane` 上发起一条流。
fn initiate(lane: Lane) -> (LaneSend, LaneRecv) {
    super::initiate(lane, book(), watch::channel(false).1)
}

/// 本端在 `lane` 上等对方开流，接下它。返回两个方向和 OPENING 里的负载方式。
async fn respond(mut lane: Lane) -> (LaneSend, LaneRecv, Mode) {
    let opening = await_opening(&mut lane).await.unwrap().expect("对方没有开流就关了 lane");
    let (send, recv) = super::respond(lane, opening.mode, &opening.header, book(), watch::channel(false).1);
    (send, recv, opening.mode)
}

/// 没升级的发起方开 passthrough 的流：OPENING 是 [`Mode::Early`]，装着 `header`；它所在的记录之后紧跟着 `raw`，不加密。
async fn open_as_old_node(lane: &mut Lane, header: &[u8], raw: &[u8]) {
    lane.queue(&Frame::Opening { mode: Mode::Early, header: Bytes::copy_from_slice(header) });
    lane.flush().await.unwrap();
    lane::write_all(&lane.socket, &mut BytesMut::from(raw)).await.unwrap();
}

async fn read_to_end(recv: &mut LaneRecv) -> io::Result<Vec<u8>> {
    let mut all = Vec::new();
    tokio::time::timeout(PATIENCE, recv.read_to_end(&mut all)).await.expect("读等得太久")?;
    Ok(all)
}

#[tokio::test]
async fn upgraded_nodes_agree_on_v2_and_an_old_node_at_either_end_gets_v1() {
    for (what, dialler, acceptor, expected) in
        [("都升级了", NEW, NEW, Version::V2), ("拨号方没升级", OLD, NEW, Version::V1), ("接受方没升级", NEW, OLD, Version::V1)]
    {
        let (dialled, accepted) = ends(dialler, acceptor).await;
        assert_eq!((dialled.version, accepted.version), (expected, expected), "{what}");
    }
}

/// 本端开的 passthrough 流：两头都升级了时 OPENING 是 [`Mode::Mutual`]；对端没升级时是 sealed（那边不认"等 SWITCH"，
/// 本端也不再在 OPENING 之后马上离开 TLS），那边照旧读得懂，接受方说可以不加密了也一直加密。拨号方和接受方开流都一样。
#[tokio::test]
async fn a_passthrough_stream_towards_an_old_node_is_opened_sealed_and_still_works() {
    for (what, dialler, acceptor, expected) in
        [("都升级了", NEW, NEW, Mode::Mutual), ("拨号方没升级", OLD, NEW, Mode::Sealed), ("接受方没升级", NEW, OLD, Mode::Sealed)]
    {
        for dialler_initiates in [true, false] {
            let (dialled, accepted) = ends(dialler, acceptor).await;
            let (near, far) = if dialler_initiates { (dialled, accepted) } else { (accepted, dialled) };
            let (mut send, mut recv) = initiate(near);
            send.write_all(b"OPEN").await.unwrap();
            send.end_header(Payload::Passthrough);
            let (mut far_send, mut far_recv, mode) = respond(far).await;
            assert_eq!(mode, expected, "{what}，拨号方开流：{dialler_initiates}");
            assert_eq!(far_recv.payload(), Some(expected.payload()));

            let mut open = [0; 4];
            far_recv.read_exact(&mut open).await.unwrap();
            assert_eq!(&open, b"OPEN");
            far_recv.end_header();
            far_send.write_all(b"REPLY").await.unwrap();
            far_send.end_header(Payload::Passthrough);
            if expected == Mode::Sealed {
                assert!(far_send.take_raw().is_none(), "{what}：sealed 的流上接受方不切换");
            }
            far_send.write_all(b", answer").await.unwrap();
            far_send.shutdown().await.unwrap();
            let mut reply = [0; 5];
            recv.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"REPLY");
            recv.end_header();
            assert_eq!(read_to_end(&mut recv).await.unwrap(), b", answer", "{what}");
            send.write_all(b", request").await.unwrap();
            send.shutdown().await.unwrap();
            assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b", request", "{what}");
        }
    }
}

/// 没升级的发起方开的 passthrough 流照旧：它的方向在 OPENING 之后就是原始字节，头读完之后按顺序接着交出来；
/// 接受方的 SWITCH 之后也是原始字节。
#[tokio::test]
async fn an_old_initiators_passthrough_stream_still_works() {
    let (mut old, new) = ends(OLD, NEW).await;
    open_as_old_node(&mut old, b"OPEN first data", b", raw data").await;
    crate::socket::shutdown_write(&old.socket).unwrap();
    let (mut send, mut recv, mode) = respond(new).await;
    assert_eq!(mode, Mode::Early);
    let mut open = [0; 4];
    recv.read_exact(&mut open).await.unwrap();
    assert_eq!(&open, b"OPEN");
    recv.end_header();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b" first data, raw data");

    send.write_all(b"REPLY").await.unwrap();
    send.end_header(Payload::Passthrough);
    send.write_all(b" raw answer").await.unwrap();
    send.shutdown().await.unwrap();
    assert!(matches!(old.read_frame().await.unwrap(), Some(Frame::Data(reply)) if reply[..] == b"REPLY"[..]));
    assert_eq!(old.read_frame().await.unwrap(), Some(Frame::Switch));
    assert!(old.sealed.at_record_boundary(), "SWITCH 是它那条记录里的最后一个帧");
    let mut answer = old.sealed.take_inbound();
    loop {
        match tokio::time::timeout(PATIENCE, lane::read_more(&old.socket, &mut answer)).await.expect("读等得太久") {
            Ok(()) => {}
            Err(LaneError::Closed) => break,
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(&answer[..], b" raw answer");
}

/// 流层的头只能来自 TLS 里面：没升级的发起方在 OPENING 里只放了半个头、另一半跟在原始字节里时（路上的人改得了的就是这一半），
/// 读出错，不会把后面的原始字节当作头交出来（issue #17）。
#[tokio::test]
async fn an_old_initiator_cannot_complete_a_header_with_bytes_from_outside_tls() {
    let (mut old, new) = ends(OLD, NEW).await;
    open_as_old_node(&mut old, b"OP", b"EN, then data").await;
    let (_send, mut recv, _) = respond(new).await;
    let mut got = [0; 2];
    recv.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"OP");
    let error = recv.read_exact(&mut got).await.expect_err("头的另一半不在 TLS 里，不能交出来");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
}

/// [`Mode::Mutual`] 的流上发起方的 SWITCH 只能是回应：接受方自己还没切换时收到它是协议错误，后面的原始字节不交出去。
/// 所以接受方看过目标的回答之前，发起方的方向上不会有一个字节不加密。
#[tokio::test]
async fn an_initiator_switch_that_answers_nothing_is_a_protocol_error() {
    let (mut initiator, new) = ends(NEW, NEW).await;
    initiator.queue(&Frame::Opening { mode: Mode::Mutual, header: Bytes::from_static(b"OPEN") });
    initiator.queue(&Frame::Switch);
    initiator.flush().await.unwrap();
    lane::write_all(&initiator.socket, &mut BytesMut::from(&b"plaintext before the responder switched"[..])).await.unwrap();
    let (_send, mut recv, mode) = respond(new).await;
    assert_eq!(mode, Mode::Mutual);
    let mut open = [0; 4];
    recv.read_exact(&mut open).await.unwrap();
    recv.end_header();
    let error = read_to_end(&mut recv).await.expect_err("没有人要的 SWITCH");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
}

/// 出口：只数见过几个请求，然后拒绝。
struct Watching(Arc<AtomicUsize>);

impl i3t_flow::Handler for Watching {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: i3t_flow::Request<S, R>) {
        self.0.fetch_add(1, Ordering::Relaxed);
        request.refuse(&Reply::new(Status::AccessDenied, "测试")).await;
    }
}

/// 流层也一样：没升级的节点开过来的流，OPEN 的后一半跟在原始字节里时，出口的流层以协议错误重置流，处理者见不到这个请求
/// （issue #17）。升级了的发起方在接受方切换之前不会离开 TLS，用不着这一条；没升级的还会。
#[tokio::test]
async fn the_flow_layer_does_not_take_an_open_that_an_old_node_completed_outside_tls() {
    let (old_key, new_key) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
    let lanes = Arc::new(Lanes::new(&new_key, LaneConfig::default()).unwrap());
    lanes.set_peers([LanePeer { id: id(1), key: old_key.public(), dial: Vec::new() }]).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(Watching(Arc::clone(&seen)));
    let serving = Arc::clone(&lanes);
    tokio::spawn(async move { tokio::join!(serving.serve(listener), i3t_flow::serve(&*serving, handler)) });

    let mut old = dial(&old_key, new_key.public(), addr, OLD).await;
    let mut open = Vec::new();
    Open { target: Target::Node { port: 443 }, class: Class::Auto, origin: Origin::default() }.encode(&mut open);
    let (sealed, raw) = open.split_at(open.len() / 2);
    open_as_old_node(&mut old, sealed, &[raw, &i3t_flow::testing::client_hello(b"")].concat()).await;
    let answer = tokio::time::timeout(PATIENCE, old.read_frame()).await.expect("等出口的回答等得太久").unwrap();
    assert_eq!(answer, Some(Frame::Abort(Reason::ProtocolError.to_wire())), "出口应该以协议错误重置这条流");
    assert_eq!(seen.load(Ordering::Relaxed), 0, "处理者不该见到这个请求");
}
