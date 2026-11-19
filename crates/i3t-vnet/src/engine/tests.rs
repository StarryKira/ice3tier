//! 引擎的测试：网卡换成内存里的两个队列（本机的内核写进来的、写给本机内核的），连接层换成一个假的，看每个包的去向。

use std::collections::HashSet;
use std::sync::atomic::AtomicUsize;

use i3t_proto::flow::Fragment;
use tokio::sync::Mutex as AsyncMutex;

use super::*;
use crate::ipv4::testing::{icmp, tcp, udp};

const ME: [u8; 4] = [10, 66, 0, 1];
const B: [u8; 4] = [10, 66, 0, 2];
const C: [u8; 4] = [10, 66, 0, 3];
const SOON: Duration = Duration::from_secs(5);

fn node(raw: u32) -> NodeId {
    NodeId::new(raw).unwrap()
}

/// 内存里的网卡。
struct FakeDevice {
    from_kernel: AsyncMutex<mpsc::UnboundedReceiver<io::Result<Vec<u8>>>>,
    to_kernel: mpsc::UnboundedSender<Vec<u8>>,
}

impl Device for FakeDevice {
    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(packet) = self.from_kernel.lock().await.recv().await else { return std::future::pending().await };
        let packet = packet?;
        buf[..packet.len()].copy_from_slice(&packet);
        Ok(packet.len())
    }

    async fn send(&self, packet: &[u8]) -> io::Result<()> {
        let _ = self.to_kernel.send(packet.to_vec());
        Ok(())
    }
}

/// 假的连接层：哪些对端连着、一个数据报最长多少、建连接的结果；发出去的帧记下来，进来的帧从一个队列里取。
#[derive(Clone)]
struct FakeNet(Arc<NetState>);

struct NetState {
    connected: std::sync::Mutex<HashSet<NodeId>>,
    max: AtomicUsize,
    refuse: std::sync::Mutex<HashSet<NodeId>>,
    dial_delay: Duration,
    sent: mpsc::UnboundedSender<(NodeId, Bytes)>,
    inbox: AsyncMutex<mpsc::UnboundedReceiver<Datagram>>,
}

impl Packets for FakeNet {
    type Sender = Self;

    fn packet_sender(&self) -> Self {
        self.clone()
    }

    async fn recv_packets(&self, into: &mut Vec<Datagram>, limit: usize) -> usize {
        self.0.inbox.lock().await.recv_many(into, limit).await
    }
}

impl PacketSender for FakeNet {
    fn send_packet(&self, peer: NodeId, frame: Bytes) -> Result<(), DatagramSendError> {
        if !self.0.connected.lock().unwrap().contains(&peer) {
            return Err(DatagramSendError::NotConnected(peer));
        }
        let max = self.0.max.load(Ordering::Relaxed);
        if frame.len() > max {
            return Err(DatagramSendError::TooLarge { len: frame.len(), max });
        }
        self.0.sent.send((peer, frame)).unwrap();
        Ok(())
    }

    async fn connect(&self, peer: NodeId) -> Result<(), OpenError> {
        tokio::time::sleep(self.0.dial_delay).await;
        if self.0.refuse.lock().unwrap().contains(&peer) {
            return Err(OpenError::NoRoute(peer));
        }
        self.0.connected.lock().unwrap().insert(peer);
        Ok(())
    }
}

/// 一个跑着的引擎和它的四个口子。
struct World {
    /// 本机的程序发出的包从这里进网卡。
    app: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
    /// 写进网卡（交给本机内核）的包。
    kernel: mpsc::UnboundedReceiver<Vec<u8>>,
    /// 发给对端的帧。
    wire: mpsc::UnboundedReceiver<(NodeId, Bytes)>,
    /// 对端发来的帧从这里进来。
    peers: mpsc::UnboundedSender<Datagram>,
    net: FakeNet,
    netmap: watch::Sender<Arc<Netmap>>,
    counters: Arc<Counters>,
    engine: tokio::task::JoinHandle<Stopped>,
}

fn netmap(peers: &serde_json::Value, no_vnet: &[u32]) -> Netmap {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "network": { "name": "lab", "domain": "lab.i3t", "cidr": "10.66.0.0/16" },
        "me": { "id": 1, "name": "me", "virtual_ip": "10.66.0.1" },
        "peers": peers,
        "no_vnet": no_vnet,
    }))
    .unwrap()
}

fn peer(raw: u32, inbound: bool, outbound: bool) -> serde_json::Value {
    serde_json::json!({
        "id": raw, "key": i3t_proto::PublicKey::from_bytes([u8::try_from(raw).unwrap(); 32]).to_string(),
        "name": format!("n{raw}"), "virtual_ip": format!("10.66.0.{raw}"), "inbound": inbound, "outbound": outbound
    })
}

fn world(map: Netmap, connected: &[u32], dial_delay: Duration) -> World {
    let (app, from_kernel) = mpsc::unbounded_channel();
    let (to_kernel, kernel) = mpsc::unbounded_channel();
    let (sent, wire) = mpsc::unbounded_channel();
    let (peers, inbox) = mpsc::unbounded_channel();
    let net = FakeNet(Arc::new(NetState {
        connected: std::sync::Mutex::new(connected.iter().map(|raw| node(*raw)).collect()),
        max: AtomicUsize::new(1400),
        refuse: std::sync::Mutex::default(),
        dial_delay,
        sent,
        inbox: AsyncMutex::new(inbox),
    }));
    let device = Arc::new(FakeDevice { from_kernel: AsyncMutex::new(from_kernel), to_kernel });
    let (netmap, netmap_rx) = watch::channel(Arc::new(map));
    let counters = Arc::new(Counters::default());
    let engine = tokio::spawn(Engine::new(device, Arc::new(net.clone()), netmap_rx, Arc::clone(&counters)).run());
    World { app, kernel, wire, peers, net, netmap, counters, engine }
}

impl World {
    fn app_sends(&self, packet: Vec<u8>) {
        self.app.send(Ok(packet)).unwrap();
    }

    async fn on_wire(&mut self) -> (NodeId, Bytes) {
        tokio::time::timeout(SOON, self.wire.recv()).await.expect("等发给对端的帧超时").unwrap()
    }

    async fn kernel_gets(&mut self) -> Vec<u8> {
        tokio::time::timeout(SOON, self.kernel.recv()).await.expect("等写进网卡的包超时").unwrap()
    }

    async fn nothing_on_wire(&mut self) {
        assert!(tokio::time::timeout(Duration::from_millis(100), self.wire.recv()).await.is_err(), "不该有帧发出去");
    }

    async fn nothing_to_kernel(&mut self) {
        assert!(tokio::time::timeout(Duration::from_millis(100), self.kernel.recv()).await.is_err(), "不该有包写进网卡");
    }

    fn peer_sends(&self, raw: u32, frame: Vec<u8>) {
        self.peers.send(Datagram { peer: node(raw), payload: frame.into() }).unwrap();
    }

    fn snapshot(&self) -> Traffic {
        self.counters.snapshot()
    }
}

fn whole(packet: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    PacketHeader::Whole.encode(&mut frame);
    frame.extend_from_slice(packet);
    frame
}

/// 一个"不可达"的 ICMP 包的类型、代码和它带回来的原包的开头。
fn icmp_answer(packet: &[u8]) -> (u8, u8, Vec<u8>) {
    let ip = Ipv4::parse(packet).unwrap();
    let payload = ip.payload();
    (payload[0], payload[1], payload[8..].to_vec())
}

#[tokio::test]
async fn a_packet_to_a_connected_peer_goes_out_as_one_frame_and_a_reply_comes_in() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true)]), &[]), &[2], Duration::ZERO);
    let ping = icmp(ME, B, 8);
    world.app_sends(ping.clone());
    let (to, frame) = world.on_wire().await;
    assert_eq!((to, &frame[..]), (node(2), &whole(&ping)[..]), "一个包一个帧：头一个字节，后面原样");

    let reply = icmp(B, ME, 0);
    world.peer_sends(2, whole(&reply));
    assert_eq!(world.kernel_gets().await, reply, "原样写进网卡");
    let traffic = world.snapshot();
    assert_eq!((traffic.sent_packets, traffic.received_packets), (1, 1));
    assert_eq!((traffic.sent_bytes, traffic.received_bytes), (u64::try_from(ping.len()).unwrap(), u64::try_from(reply.len()).unwrap()));
    assert_eq!(traffic.dropped, VnetDrops::default());
}

#[tokio::test]
async fn packets_to_no_node_or_to_a_node_without_a_nic_are_answered_unreachable_at_once() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true), peer(3, true, true)]), &[3]), &[2, 3], Duration::ZERO);
    let syn = tcp(ME, [10, 66, 0, 9], (40000, 22), 0x02);
    world.app_sends(syn.clone());
    let answer = world.kernel_gets().await;
    let ip = Ipv4::parse(&answer).unwrap();
    assert_eq!((ip.source(), ip.destination()), (Ipv4Addr::new(10, 66, 0, 9), Ipv4Addr::from(ME)), "像是那台主机说的");
    assert_eq!(icmp_answer(&answer), (3, 1, syn.clone()), "主机不可达，带回原包");

    world.app_sends(icmp(ME, C, 8));
    assert_eq!(icmp_answer(&world.kernel_gets().await).1, 1, "对端没有网卡");
    world.nothing_on_wire().await;
    assert_eq!(world.snapshot().dropped.unreachable, 2);

    // 组播、广播、不在地址段里的包都不发，组播和广播也不回。
    world.app_sends(udp(ME, [224, 0, 0, 251], (5353, 5353), b"mdns"));
    world.app_sends(udp(ME, [10, 66, 255, 255], (1, 2), b"broadcast"));
    world.nothing_to_kernel().await;
    world.nothing_on_wire().await;
}

#[tokio::test]
async fn new_conversations_to_a_peer_we_may_not_reach_fail_early_but_everything_else_goes() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, false)]), &[]), &[2], Duration::ZERO);
    world.app_sends(tcp(ME, B, (40000, 22), 0x02));
    assert_eq!(icmp_answer(&world.kernel_gets().await).1, 13, "被管理员禁止");
    world.app_sends(icmp(ME, B, 8));
    assert_eq!(icmp_answer(&world.kernel_gets().await).1, 13);
    // 它可以访问本节点：本节点回它的包照发。
    let syn_ack = tcp(ME, B, (22, 40000), 0x12);
    world.app_sends(syn_ack.clone());
    assert_eq!(&world.on_wire().await.1[1..], &syn_ack[..]);
    let udp_reply = udp(ME, B, (53, 5000), b"answer");
    world.app_sends(udp_reply.clone());
    assert_eq!(&world.on_wire().await.1[1..], &udp_reply[..]);
    assert_eq!(world.snapshot().dropped.not_allowed, 2);
}

#[tokio::test]
async fn packets_sent_while_the_connection_comes_up_are_kept_and_go_out_in_order() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true)]), &[]), &[], Duration::from_millis(200));
    let first: Vec<Vec<u8>> = (0..3u16).map(|seq| udp(ME, B, (5000, 7), &seq.to_be_bytes())).collect();
    for packet in &first {
        world.app_sends(packet.clone());
    }
    for packet in &first {
        let (to, frame) = world.on_wire().await;
        assert_eq!((to, &frame[1..]), (node(2), &packet[..]), "第一个包没有丢，先后不变");
    }
    assert_eq!(world.snapshot().sent_packets, 3);
}

#[tokio::test]
async fn a_peer_that_cannot_be_reached_gets_its_waiting_packets_answered_and_later_ones_at_once() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true)]), &[]), &[], Duration::from_millis(100));
    world.net.0.refuse.lock().unwrap().insert(node(2));
    let syn = tcp(ME, B, (40000, 22), 0x02);
    world.app_sends(syn.clone());
    world.app_sends(icmp(ME, B, 8));
    assert_eq!(icmp_answer(&world.kernel_gets().await), (3, 1, syn), "排着的包在建不起来之后回不可达");
    assert_eq!(icmp_answer(&world.kernel_gets().await).1, 1);
    let started = Instant::now();
    world.app_sends(icmp(ME, B, 8));
    assert_eq!(icmp_answer(&world.kernel_gets().await).1, 1);
    assert!(started.elapsed() < Duration::from_millis(80), "之后的包当场回，不再去建：{:?}", started.elapsed());
    world.nothing_on_wire().await;
}

#[tokio::test]
async fn a_packet_too_large_for_the_connection_goes_as_two_pieces_and_pieces_are_put_back_together() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true)]), &[]), &[2], Duration::ZERO);
    world.net.0.max.store(1162, Ordering::Relaxed);
    let big = udp(ME, B, (5000, 7), &[0x5a; 1252]);
    assert_eq!(big.len(), 1280);
    world.app_sends(big.clone());
    let (_, first) = world.on_wire().await;
    let (_, second) = world.on_wire().await;
    assert_eq!(first.len(), 1162);
    let mut rebuilt = Vec::new();
    for frame in [&first, &second] {
        let (header, piece) = PacketHeader::decode(frame).unwrap();
        assert!(matches!(header, PacketHeader::Piece(Fragment { count: 2, .. })));
        rebuilt.extend_from_slice(piece);
    }
    assert_eq!(rebuilt, big);
    assert_eq!(world.snapshot().sent_packets, 1, "一个包算一个");

    // 对端切过来的两片，后一片先到。
    let reply = udp(B, ME, (7, 5000), &[0xa5; 1252]);
    let pieces: Vec<Vec<u8>> = PacketPieces::split(9, &reply, 1162).unwrap().collect();
    world.peer_sends(2, pieces[1].clone());
    world.peer_sends(2, pieces[0].clone());
    assert_eq!(world.kernel_gets().await, reply);
}

#[tokio::test]
async fn packets_from_a_peer_must_carry_its_own_address_and_ours() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true), peer(3, true, true)]), &[]), &[], Duration::ZERO);
    world.peer_sends(2, whole(&icmp(C, ME, 8)));
    world.peer_sends(2, whole(&icmp(B, [10, 66, 0, 7], 8)));
    world.peer_sends(2, whole(&icmp(B, [192, 168, 1, 5], 8)));
    world.peer_sends(9, whole(&icmp([10, 66, 0, 9], ME, 8)));
    world.peer_sends(2, vec![0x04, 0x60, 0, 0]);
    world.peer_sends(2, vec![0x05, 0x45]);
    world.nothing_to_kernel().await;
    let dropped = world.snapshot().dropped;
    assert_eq!((dropped.bad_source, dropped.bad_destination, dropped.malformed), (2, 2, 2), "{dropped:?}");
    let good = icmp(B, ME, 8);
    world.peer_sends(2, whole(&good));
    assert_eq!(world.kernel_gets().await, good, "对的照常进来");
    // 本机发出、来源不是本节点的虚拟 IP 的（经这台机器转发的别人的包）不发。
    world.app_sends(icmp([192, 168, 1, 9], B, 8));
    world.nothing_on_wire().await;
    assert_eq!(world.snapshot().dropped.bad_source, 3);
}

#[tokio::test]
async fn a_peer_that_may_not_reach_us_gets_only_answers_in() {
    let mut world = world(netmap(&serde_json::json!([peer(2, false, true)]), &[]), &[2], Duration::ZERO);
    world.peer_sends(2, whole(&tcp(B, ME, (40000, 22), 0x02)));
    world.peer_sends(2, whole(&icmp(B, ME, 8)));
    world.peer_sends(2, whole(&udp(B, ME, (53, 5000), b"unasked")));
    world.nothing_to_kernel().await;
    assert_eq!(world.snapshot().dropped.not_allowed, 3);

    // 本节点先发了：回包进来。
    let query = udp(ME, B, (5000, 53), b"query");
    world.app_sends(query);
    world.on_wire().await;
    let answer = udp(B, ME, (53, 5000), b"answer");
    world.peer_sends(2, whole(&answer));
    assert_eq!(world.kernel_gets().await, answer);
    let syn_ack = tcp(B, ME, (22, 40000), 0x12);
    world.peer_sends(2, whole(&syn_ack));
    assert_eq!(world.kernel_gets().await, syn_ack);
    let pong = icmp(B, ME, 0);
    world.peer_sends(2, whole(&pong));
    assert_eq!(world.kernel_gets().await, pong);
}

#[tokio::test]
async fn the_engine_follows_the_netmap_and_stops_when_the_device_fails() {
    let mut world = world(netmap(&serde_json::json!([peer(2, true, true)]), &[]), &[2, 3], Duration::ZERO);
    world.app_sends(icmp(ME, C, 8));
    assert_eq!(icmp_answer(&world.kernel_gets().await).1, 1, "还不是节点");
    let mut next = netmap(&serde_json::json!([peer(2, true, true), peer(3, true, true)]), &[]);
    next.version = 2;
    world.netmap.send_replace(Arc::new(next));
    world.app_sends(icmp(ME, C, 8));
    assert_eq!(world.on_wire().await.0, node(3), "netmap 换了：它成了节点");

    world.app.send(Err(io::Error::other("网卡没了"))).unwrap();
    let stopped = tokio::time::timeout(SOON, world.engine).await.unwrap().unwrap();
    assert!(matches!(stopped, Stopped::Device(_)), "{stopped:?}");
}
