//! 流层跑在 lane 上：客户端 TCP ↔ 入口 ↔ lane ↔ 出口 ↔ 目标 TCP。
//!
//! sealed 用的是现在的流层，一行不改（`i3t_flow::open`、`serve`、`Flow::relay`）：流层不知道头在哪里结束，
//! lane 按"发起方开始读回复时头一定写完了"把它当作 sealed 送出。
//!
//! passthrough 要流层多做两件事（头写完时 `end_header`，搬运前取原始 socket 交给 splice），那是下一阶段的改动。
//! 这里用一个替身：照流层的样子写 OPEN、读 REPLY，加上那两件事，见 `stand_in`。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::{Hop, Pair, quick};
use i3t_conn::{RecvStream, SendStream};
use i3t_flow::{End, Handler, Request};
use i3t_lane::splice::Splicer;
use i3t_lane::{Lanes, Payload};
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Open, Origin, Reply, Target};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const PATIENCE: Duration = Duration::from_secs(10);

fn open_request() -> Open {
    Open { target: Target::Node { port: 443 }, class: Class::Auto, origin: Origin::default() }
}

/// 出口：每条流都连到 `target`，然后用流层自己的搬运。
struct ToTarget {
    target: SocketAddr,
}

impl Handler for ToTarget {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        let target = TcpStream::connect(self.target).await.unwrap();
        let flow = request.accept().await.unwrap();
        let _ = flow.relay(target, future::pending()).await;
    }
}

/// 一对连好的 TCP：`near` 是测试扮演的那一头，`far` 交给被测的搬运。
async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let near = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (far, _) = listener.accept().await.unwrap();
    (near, far)
}

/// 一个目标：把收到的每个字节原样送回去，对方结束了它也结束。
async fn echo_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
                let _ = write.shutdown().await;
            });
        }
    });
    addr
}

/// 一个目标：读到请求的第一块之后回一半的答案，然后用 RST 断开。
async fn resetting_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0; 9];
                let _ = stream.read_exact(&mut request).await;
                stream.write_all(b"half of the ans").await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                stream.set_zero_linger().unwrap();
            });
        }
    });
    addr
}

/// 读到 EOF 或者出错。返回读到的字节，和是不是正常结束的。
async fn drain(stream: &mut (impl AsyncRead + Unpin)) -> (Vec<u8>, io::Result<()>) {
    let mut all = Vec::new();
    let mut buf = [0; 8192];
    loop {
        match timeout(PATIENCE, stream.read(&mut buf)).await.expect("读等得太久") {
            Ok(0) => return (all, Ok(())),
            Ok(len) => all.extend_from_slice(&buf[..len]),
            Err(error) => return (all, Err(error)),
        }
    }
}

/// 现在的流层：入口用 `open` + `established` + `relay`，出口用 `serve`。
async fn through_unmodified_flow(lanes: &Lanes, peer: NodeId, client_side: TcpStream, first_data: &[u8]) -> End {
    let pending = i3t_flow::open(lanes, peer, &open_request(), first_data).await.unwrap();
    let flow = pending.established().await.unwrap();
    flow.relay(client_side, future::pending()).await.end
}

#[tokio::test]
async fn the_unmodified_flow_layer_runs_sealed_on_lanes_in_both_directions_of_initiative() {
    let pair = Pair::new(quick()).await;
    pair.ready(2).await;
    let target = echo_target().await;
    for lanes in [&pair.entry.lanes, &pair.exit.lanes] {
        let lanes = Arc::clone(lanes);
        tokio::spawn(async move { i3t_flow::serve(&*lanes, Arc::new(ToTarget { target })).await });
    }
    for (from, to) in [(&pair.entry, &pair.exit), (&pair.exit, &pair.entry)] {
        let (mut client, client_side) = tcp_pair().await;
        let lanes = Arc::clone(&from.lanes);
        let peer = to.id;
        let relay = tokio::spawn(async move { through_unmodified_flow(&lanes, peer, client_side, b"first ").await });
        let body: Vec<u8> = (0..200_000u32).map(|i| u8::try_from(i % 249).unwrap()).collect();
        client.write_all(&body).await.unwrap();
        client.shutdown().await.unwrap();
        let (echoed, end) = drain(&mut client).await;
        end.unwrap();
        let mut expected = b"first ".to_vec();
        expected.extend_from_slice(&body);
        assert_eq!(echoed.len(), expected.len());
        assert!(echoed == expected, "回显的字节不一样");
        assert!(relay.await.unwrap().is_clean());
    }
    let status = pair.entry.lanes.status();
    assert!(status.sealed_bytes >= 400_000, "{status:?}");
    assert_eq!(status.passthrough_bytes, 0, "流层不改时一律是 sealed");
}

#[tokio::test]
async fn an_upstream_reset_in_the_middle_of_an_answer_reaches_the_client_as_a_reset_through_the_flow_layer() {
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let target = resetting_target().await;
    let lanes = Arc::clone(&pair.exit.lanes);
    tokio::spawn(async move { i3t_flow::serve(&*lanes, Arc::new(ToTarget { target })).await });
    let (mut client, client_side) = tcp_pair().await;
    let lanes = Arc::clone(&pair.entry.lanes);
    let peer = pair.exit.id;
    let relay = tokio::spawn(async move { through_unmodified_flow(&lanes, peer, client_side, b"").await });
    client.write_all(b"request!!").await.unwrap();
    let (got, end) = drain(&mut client).await;
    assert_eq!(got, b"half of the ans");
    let error = end.expect_err("半截的回答之后必须是重置，不能是正常结束");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert!(!relay.await.unwrap().is_clean());
}

/// 下一阶段流层要做的事的替身。
mod stand_in {
    use std::sync::Arc;

    use i3t_lane::splice::Splicer;
    use i3t_lane::{LaneRecv, LaneSend, Lanes, Payload};
    use i3t_proto::NodeId;
    use i3t_proto::flow::{DecodeError, Decoded, Opening, Reply};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// 照流层的 `header::read`：一次只读解析器要的字节，不多读。
    pub async fn read_header<T>(recv: &mut (impl AsyncRead + Unpin), decode: fn(&[u8]) -> Result<Decoded<T>, DecodeError>) -> T {
        let mut buf = Vec::new();
        loop {
            match decode(&buf).unwrap() {
                Decoded::Complete { value, .. } => return value,
                Decoded::Incomplete { more } => {
                    let have = buf.len();
                    buf.resize(have + more, 0);
                    recv.read_exact(&mut buf[have..]).await.unwrap();
                }
            }
        }
    }

    /// 首批数据：读完整的第一条 TLS 记录（入口模块的 `first_data::read` 的简化版）。
    pub async fn first_data(client: &mut TcpStream) -> Vec<u8> {
        let mut header = [0; 5];
        client.read_exact(&mut header).await.unwrap();
        let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
        let mut record = header.to_vec();
        record.resize(5 + len, 0);
        client.read_exact(&mut record[5..]).await.unwrap();
        record
    }

    /// 两个原始方向交给 splice。接收方向用 `wait_raw`：对端的 SWITCH 可能还在路上。
    pub async fn relay(splicer: &Splicer, mut send: LaneSend, mut recv: LaneRecv, user: TcpStream) -> Option<std::io::Error> {
        match (send.take_raw(), recv.wait_raw().await) {
            (Some(raw_send), Some(raw_recv)) => {
                let (lane, unsent) = raw_send.into_parts();
                let (_, received) = raw_recv.into_parts();
                let relayed = splicer.relay(&user, received, &lane, unsent).await;
                relayed.error.map(|error| std::io::Error::other(error.to_string()))
            }
            _ => panic!("这个替身只用在两个方向都已经是原始字节的流上"),
        }
    }

    /// 入口：开流、写 OPEN 和首批数据、选负载方式、读 REPLY，然后搬运。
    pub async fn initiate(lanes: &Lanes, peer: NodeId, mut client: TcpStream, splicer: &Splicer) -> Option<std::io::Error> {
        let first = first_data(&mut client).await;
        let payload = Payload::for_first_data(&first);
        let (mut send, mut recv) = lanes.open_stream(peer).await.unwrap();
        let mut header = Vec::new();
        super::open_request().encode(&mut header);
        header.extend_from_slice(&first);
        send.write_all(&header).await.unwrap();
        send.end_header(payload);
        let reply = read_header(&mut recv, Reply::decode).await;
        assert!(reply.status.is_ok());
        relay(splicer, send, recv, client).await
    }

    /// 出口：读 OPEN、连目标、回 REPLY、结束头，然后搬运。
    pub async fn respond(lanes: Arc<Lanes>, target: std::net::SocketAddr, splicer: Arc<Splicer>) {
        use i3t_conn::Acceptor;
        while let Some(incoming) = lanes.accept().await {
            let splicer = Arc::clone(&splicer);
            tokio::spawn(async move {
                let (mut send, mut recv) = (incoming.send, incoming.recv);
                let Opening::Stream(_open) = read_header(&mut recv, Opening::decode).await else { panic!("不是流") };
                let target = TcpStream::connect(target).await.unwrap();
                let mut reply = Vec::new();
                Reply::ok().encode(&mut reply);
                send.write_all(&reply).await.unwrap();
                send.end_header(Payload::Passthrough);
                let _ = relay(&splicer, send, recv, target).await;
            });
        }
    }
}

/// 一对 TLS 的两端：服务器的证书由测试自己签，客户端只信它。
mod user_tls {
    use std::sync::Arc;

    use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
    use rustls::{ClientConfig, RootCertStore, ServerConfig};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    pub const NAME: &str = "secret-hostname.example";

    pub fn configs() -> (TlsConnector, TlsAcceptor) {
        let issued = rcgen::generate_simple_self_signed(vec![NAME.to_owned()]).unwrap();
        let cert = CertificateDer::from(issued.cert.der().to_vec());
        let key = PrivateKeyDer::try_from(issued.signing_key.serialize_der()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(cert).unwrap();
        let client = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        (TlsConnector::from(Arc::new(client)), TlsAcceptor::from(Arc::new(server)))
    }

    pub fn name() -> ServerName<'static> {
        ServerName::try_from(NAME).unwrap()
    }
}

/// 一个 TLS 服务器：读到客户端说完，把内容倒过来送回去，然后 close_notify。
async fn tls_server(acceptor: tokio_rustls::TlsAcceptor) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let mut tls = acceptor.accept(stream).await.unwrap();
                let mut request = Vec::new();
                tls.read_to_end(&mut request).await.unwrap();
                request.reverse();
                tls.write_all(&request).await.unwrap();
                tls.shutdown().await.unwrap();
            });
        }
    });
    addr
}

/// 一条 passthrough 的流，两头是真的 TLS 客户端和服务器，中间三处录音：客户端到入口、lane、出口到服务器。
/// 返回 (客户端发出的, 服务器发出的, lane 上从出口到入口的, lane 上从入口到出口的)。
async fn passthrough_with_recordings(splicer: Splicer, lane_chunk: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let pair = Pair::new(quick()).await;
    *pair.hop.chunk.lock().unwrap() = lane_chunk;
    pair.ready(1).await;
    let (connector, acceptor) = user_tls::configs();
    let server = tls_server(acceptor).await;
    let to_server = Hop::start(server).await;
    let splicer = Arc::new(splicer);
    tokio::spawn(stand_in::respond(Arc::clone(&pair.exit.lanes), to_server.addr, Arc::clone(&splicer)));

    let entry_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let from_client = Hop::start(entry_listener.local_addr().unwrap()).await;
    let lanes = Arc::clone(&pair.entry.lanes);
    let peer = pair.exit.id;
    let entry_splicer = Arc::clone(&splicer);
    let entry = tokio::spawn(async move {
        let (client_side, _) = entry_listener.accept().await.unwrap();
        stand_in::initiate(&lanes, peer, client_side, &entry_splicer).await
    });

    let tcp = TcpStream::connect(from_client.addr).await.unwrap();
    let mut tls = connector.connect(user_tls::name(), tcp).await.unwrap();
    let request: Vec<u8> = (0..150_000u32).map(|i| u8::try_from(i % 241).unwrap()).collect();
    tls.write_all(&request).await.unwrap();
    tls.shutdown().await.unwrap();
    let mut answer = Vec::new();
    timeout(PATIENCE, tls.read_to_end(&mut answer)).await.unwrap().unwrap();
    let mut expected = request.clone();
    expected.reverse();
    assert!(answer == expected, "经过 lane 之后 TLS 里的内容不一样");
    assert!(entry.await.unwrap().is_none(), "入口一侧的搬运应该正常结束");

    let (client_sent, server_sent) = {
        let client = from_client.recordings.lock().unwrap();
        let server = to_server.recordings.lock().unwrap();
        let client = client[0].lock().unwrap();
        let server = server[0].lock().unwrap();
        (client.forward.clone(), server.backward.clone())
    };
    let recordings = pair.hop.recordings.lock().unwrap();
    // 承载这条流的那条 lane：出口到入口的方向以服务器发出的字节结尾。
    let lane =
        recordings.iter().find(|recording| recording.lock().unwrap().forward.ends_with(&server_sent)).expect("找不到承载这条流的 lane");
    let lane = lane.lock().unwrap();
    (client_sent, server_sent, lane.forward.clone(), lane.backward.clone())
}

fn first_record_len(bytes: &[u8]) -> usize {
    5 + usize::from(u16::from_be_bytes([bytes[3], bytes[4]]))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[tokio::test]
async fn passthrough_carries_the_inner_tls_records_unchanged_after_the_opening_exchange() {
    let (client_sent, server_sent, exit_to_entry, entry_to_exit) = passthrough_with_recordings(Splicer::new(), 0).await;
    let hello = first_record_len(&client_sent);
    // 入口到出口：lane 自己的 TLS（握手、OPENING，里面装着 ClientHello）之后，正好是客户端在 ClientHello 之后发出的全部字节。
    assert!(entry_to_exit.ends_with(&client_sent[hello..]), "入口到出口的方向在开流之后必须是客户端的原始字节");
    // 出口到入口：lane 自己的 TLS（握手、HELLO、REPLY、SWITCH）之后，正好是服务器发出的全部字节。
    assert!(exit_to_entry.ends_with(&server_sent));
    // ClientHello 只在 lane 的 TLS 里面走过：线上看不到客户端要访问的名字。
    assert!(contains(&client_sent, user_tls::NAME.as_bytes()), "客户端的 ClientHello 里有名字");
    assert!(!contains(&entry_to_exit, user_tls::NAME.as_bytes()), "lane 上不能看到明文的名字");
    // lane 自己的 TLS 部分很小：开流之前和之后的字节差不多全是用户的。
    let lane_overhead = entry_to_exit.len() - (client_sent.len() - hello);
    assert!(lane_overhead < hello + 2048, "入口到出口方向 lane 自己的字节 {lane_overhead}");
}

#[tokio::test]
async fn passthrough_survives_a_lane_that_delivers_tiny_segments() {
    for chunk in [1, 3, 100] {
        let (client_sent, server_sent, exit_to_entry, entry_to_exit) = passthrough_with_recordings(Splicer::new(), chunk).await;
        let hello = first_record_len(&client_sent);
        assert!(entry_to_exit.ends_with(&client_sent[hello..]), "每段 {chunk} 字节");
        assert!(exit_to_entry.ends_with(&server_sent), "每段 {chunk} 字节");
    }
}

#[tokio::test]
async fn splice_and_user_space_copy_move_identical_bytes() {
    // 经过 lane 的真 TLS 流，两种引擎各跑一遍：TLS 能解开、内容对（在里面检查）。
    let _ = passthrough_with_recordings(Splicer::new(), 0).await;
    let _ = passthrough_with_recordings(Splicer::copying(), 0).await;

    // 再直接比一次：同一份字节分别用两种引擎搬过两条 TCP。
    let data: Vec<u8> = (0..20_000_000u32).map(|i| u8::try_from((i * 7 + i / 255) % 256).unwrap()).collect();
    for splicer in [Splicer::new(), Splicer::copying()] {
        let (mut source, source_side) = tcp_pair().await;
        let (mut sink, sink_side) = tcp_pair().await;
        let relay = async { splicer.relay(&source_side, bytes::Bytes::new(), &sink_side, bytes::Bytes::from_static(b"prefix")).await };
        let send = async {
            source.write_all(&data).await.unwrap();
            source.shutdown().await.unwrap();
        };
        let receive = async {
            let mut got = Vec::new();
            sink.read_to_end(&mut got).await.unwrap();
            sink.shutdown().await.unwrap();
            got
        };
        let (relayed, (), got) = tokio::join!(relay, send, receive);
        assert!(relayed.error.is_none());
        assert_eq!(&got[..6], b"prefix");
        assert!(got[6..] == data[..], "引擎 {splicer:?} 搬出来的字节不一样");
        let moved = splicer.spliced_bytes() + splicer.copied_bytes();
        assert_eq!(moved, u64::try_from(data.len() + 6).unwrap());
    }
}

#[tokio::test]
async fn an_upstream_reset_in_the_middle_of_an_answer_reaches_the_client_as_a_reset_in_passthrough() {
    for splicer in [Splicer::new(), Splicer::copying()] {
        let pair = Pair::new(quick()).await;
        pair.ready(1).await;
        let target = resetting_target().await;
        let splicer = Arc::new(splicer);
        tokio::spawn(stand_in::respond(Arc::clone(&pair.exit.lanes), target, Arc::clone(&splicer)));
        let (mut client, client_side) = tcp_pair().await;
        let lanes = Arc::clone(&pair.entry.lanes);
        let peer = pair.exit.id;
        let entry = tokio::spawn(async move { stand_in::initiate(&lanes, peer, client_side, &splicer).await });
        // 看上去像 ClientHello 的首批数据，让入口选 passthrough。
        client.write_all(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0, 0, 0]).await.unwrap();
        let (got, end) = drain(&mut client).await;
        assert_eq!(got, b"half of the ans");
        let error = end.expect_err("半截的回答之后必须是重置，不能是正常结束");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert!(entry.await.unwrap().is_some(), "入口一侧的搬运要报异常结束");
    }
}

#[tokio::test]
async fn sealed_mode_still_runs_when_the_responder_never_switches() {
    // 发起方选了 passthrough，接受方（现在的流层）不调 end_header：接受方的方向一直在 TLS 里，照样读得懂。
    let pair = Pair::new(quick()).await;
    pair.ready(1).await;
    let target = echo_target().await;
    let lanes = Arc::clone(&pair.exit.lanes);
    tokio::spawn(async move { i3t_flow::serve(&*lanes, Arc::new(ToTarget { target })).await });
    let (mut send, mut recv) = pair.entry.lanes.open_stream(pair.exit.id).await.unwrap();
    let mut header = Vec::new();
    open_request().encode(&mut header);
    header.extend_from_slice(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0, 0, 0]);
    send.write_all(&header).await.unwrap();
    send.end_header(Payload::Passthrough);
    let reply = stand_in::read_header(&mut recv, Reply::decode).await;
    assert!(reply.status.is_ok());
    assert!(recv.take_raw().is_none(), "接受方没有 SWITCH，它的方向还在 TLS 里");
    send.write_all(b"raw up").await.unwrap();
    send.shutdown().await.unwrap();
    let (echoed, end) = drain(&mut recv).await;
    end.unwrap();
    assert_eq!(echoed, [&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0, 0, 0][..], b"raw up"].concat());
}
