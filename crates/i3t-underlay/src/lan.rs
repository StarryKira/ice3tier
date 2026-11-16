//! 局域网发现：向本地网段的组播地址发信标，听同一个网络里的对端发的信标。信标的格式见 [`i3t_proto::beacon`]。
//!
//! 认出来的对端只是多了一个直连的候选地址（信标的来源 IP 加上信标里的端口），用之前照样要经过带签名的探测，
//! 所以这里不做任何认证之外的判断：认不出来的信标丢掉，认出来的报上去。
//!
//! 信标不走节点收包的那个端口：组播要求大家听同一个端口号，而节点的端口是可以配置的。这里另开的 socket
//! 只收发信标，只在本地网段上（TTL 是 1），不需要在路由器上放行。
//!
//! 从哪些地址发：每块网卡上的私有 IPv4 地址，以及不是链路本地的 IPv6 地址。公网 IPv4 地址所在的网段（机房里的
//! 二层网络）不发，那里没有要找的对端。IPv6 的信标从全球地址或者唯一本地地址发出，收的一方直接拿来源地址当候选；
//! 链路本地地址要带网卡编号才能用，不拿它当候选。

use std::collections::HashSet;
use std::future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use i3t_proto::PublicKey;
use i3t_proto::beacon::{Beacon, TAG_LEN};
use ring::hmac;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::ifaddr::{self, InterfaceAddr};

/// 信标发往的组播地址。IPv4 的取自"管理范围内"的那一段（和 SSDP 一样），IPv6 的是链路本地范围，都不会被路由器转出本地网段。
pub const GROUP_V4: Ipv4Addr = Ipv4Addr::new(239, 255, 105, 51);
pub const GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0x69, 0x3374);
/// 信标的端口。
pub const PORT: u16 = 20329;

/// 每秒最多处理多少个信标。每个信标要拿每个对端的公钥各算一次，有人在局域网里灌信标时不能让它占住 CPU。
const BEACONS_PER_SEC: u32 = 50;
/// 收信标出错之后歇多久再收，免得空转。
const RECEIVE_ERROR_PAUSE: Duration = Duration::from_secs(1);

/// 从哪些网卡收发信标。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Interfaces {
    /// 本机所有启用着的网卡，按模块文档里的规则挑地址。
    #[default]
    Auto,
    /// 只用这些本机地址所在的网卡，不做筛选。测试用（回环）。
    Only(Vec<IpAddr>),
}

/// 局域网发现的参数。默认值就是产品用的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanConfig {
    pub v4: SocketAddrV4,
    /// `None` 表示不用 IPv6。
    pub v6: Option<SocketAddrV6>,
    pub interfaces: Interfaces,
    /// 多久发一次信标。
    pub interval: Duration,
}

impl Default for LanConfig {
    fn default() -> Self {
        Self {
            v4: SocketAddrV4::new(GROUP_V4, PORT),
            v6: Some(SocketAddrV6::new(GROUP_V6, PORT, 0, 0)),
            interfaces: Interfaces::Auto,
            interval: Duration::from_secs(10),
        }
    }
}

/// 在局域网里看到了一个对端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sighting {
    /// 是哪个对端（按公钥认出来的）。
    pub key: PublicKey,
    /// 它的候选地址：信标的来源 IP 加上信标里写的端口。还没有验证过。
    pub addr: SocketAddr,
}

type OnSighting = Box<dyn Fn(Sighting) + Send + Sync>;

struct Inner {
    config: LanConfig,
    /// 本节点的公钥算出来的 HMAC 密钥，给自己的信标算标签用。
    own: hmac::Key,
    port: u16,
    /// 要认的对端。HMAC 密钥在名单变化时算好，收到信标时不用再算。
    peers: RwLock<Vec<(PublicKey, hmac::Key)>>,
    announcing: AtomicBool,
    on_sighting: OnSighting,
}

/// 正在运行的局域网发现。丢弃这个值就停止收发。
pub struct LanDiscovery {
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

impl LanDiscovery {
    /// 开始收发信标。要在 tokio 运行时里调用。
    ///
    /// `key` 是本节点的公钥，`port` 是本节点收包的 UDP 端口。认出一个对端的信标时在后台任务里调用 `on_sighting`，
    /// 它不能阻塞。一开始不认任何对端，要认谁用 [`set_peers`](Self::set_peers) 告诉它。
    ///
    /// # Errors
    ///
    /// IPv4 和 IPv6 的组播 socket 一个都建不起来（端口被不肯共用的程序占着，或者系统不让用组播）。
    pub fn start(config: LanConfig, key: PublicKey, port: u16, on_sighting: impl Fn(Sighting) + Send + Sync + 'static) -> io::Result<Self> {
        let v4 = receiver(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.v4.port()));
        let v6 = config.v6.map(|group| receiver(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), group.port())));
        let (v4, v6) = match (v4, v6) {
            (Err(error), None | Some(Err(_))) => return Err(error),
            (v4, v6) => (v4.ok(), v6.and_then(Result::ok)),
        };
        let inner = Arc::new(Inner {
            config,
            own: hmac_key(&key),
            port,
            peers: RwLock::new(Vec::new()),
            announcing: AtomicBool::new(true),
            on_sighting: Box::new(on_sighting),
        });
        let task = tokio::spawn(run(Arc::clone(&inner), v4, v6));
        Ok(Self { inner, task })
    }

    /// 要认哪些对端的信标。netmap 变了就调用。
    ///
    /// # Panics
    ///
    /// 后台任务 panic 过（锁中毒）。
    pub fn set_peers(&self, keys: &[PublicKey]) {
        let peers = keys.iter().map(|key| (*key, hmac_key(key))).collect();
        *self.inner.peers.write().expect("lan peers lock poisoned") = peers;
    }

    /// 本节点发不发信标。不建立直连的节点、只拨出的节点不发，但照样听。
    pub fn set_announcing(&self, announcing: bool) {
        self.inner.announcing.store(announcing, Ordering::Relaxed);
    }
}

impl Drop for LanDiscovery {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for LanDiscovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanDiscovery").field("config", &self.inner.config).finish_non_exhaustive()
    }
}

fn hmac_key(key: &PublicKey) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes())
}

fn tag(key: &hmac::Key, salt: &[u8; 8], port: u16) -> [u8; TAG_LEN] {
    let full = hmac::sign(key, &Beacon::tag_message(salt, port));
    *full.as_ref().first_chunk::<TAG_LEN>().expect("HMAC-SHA256 的输出是 32 字节，比标签长")
}

/// 收信标的 socket：绑在组播的端口上，允许同一台机器上的别的进程也绑（它们各收到一份）。
fn receiver(bind: SocketAddr) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(bind), Type::DGRAM, Some(Protocol::UDP))?;
    if bind.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&bind.into())?;
    UdpSocket::from_std(socket.into())
}

/// 本机在一个网段上的地址：从它发信标，在它所在的网卡上听。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Segment {
    V4(Ipv4Addr),
    V6 { ip: Ipv6Addr, index: u32 },
}

impl Inner {
    fn segments(&self) -> Vec<Segment> {
        let all = ifaddr::interface_addresses();
        let chosen: Vec<InterfaceAddr> = match &self.config.interfaces {
            Interfaces::Auto => all.into_iter().filter(|interface| announces_on(interface.ip)).collect(),
            Interfaces::Only(ips) => all.into_iter().filter(|interface| ips.contains(&interface.ip)).collect(),
        };
        let mut segments = Vec::new();
        let mut v6_interfaces = HashSet::new();
        for interface in chosen {
            match (interface.ip, interface.index) {
                (IpAddr::V4(ip), _) => segments.push(Segment::V4(ip)),
                // 一块网卡上常有好几个 IPv6 地址（临时地址），发一个就够了。
                (IpAddr::V6(ip), Some(index)) if self.config.v6.is_some() && v6_interfaces.insert(index) => {
                    segments.push(Segment::V6 { ip, index });
                }
                (IpAddr::V6(_), _) => {}
            }
        }
        segments
    }

    fn announce(&self, segment: Segment) -> io::Result<()> {
        let salt = crate::random_bytes();
        let beacon = Beacon { port: self.port, salt, tag: tag(&self.own, &salt, self.port) }.to_bytes();
        let (socket, group): (Socket, SocketAddr) = match (segment, self.config.v6) {
            (Segment::V4(ip), _) => {
                let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
                socket.bind(&SocketAddr::new(IpAddr::V4(ip), 0).into())?;
                socket.set_multicast_if_v4(&ip)?;
                socket.set_multicast_ttl_v4(1)?;
                // 同一台机器上的别的节点也要收得到。
                socket.set_multicast_loop_v4(true)?;
                (socket, SocketAddr::V4(self.config.v4))
            }
            (Segment::V6 { ip, index }, Some(group)) => {
                let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
                socket.bind(&SocketAddr::new(IpAddr::V6(ip), 0).into())?;
                socket.set_multicast_if_v6(index)?;
                socket.set_multicast_hops_v6(1)?;
                socket.set_multicast_loop_v6(true)?;
                (socket, SocketAddr::V6(SocketAddrV6::new(*group.ip(), group.port(), 0, index)))
            }
            (Segment::V6 { .. }, None) => return Ok(()),
        };
        socket.send_to(&beacon, &SockAddr::from(group)).map(|_sent| ())
    }

    /// 一个数据报是不是某个对端的信标。
    fn recognise(&self, datagram: &[u8], from: SocketAddr) -> Option<Sighting> {
        let beacon = Beacon::parse(datagram)?;
        let ip = from.ip().to_canonical();
        // 链路本地的 IPv6 地址要带网卡编号才发得过去，这里不拿它当候选。
        if !ifaddr::is_candidate(ip) && !ip.is_loopback() {
            return None;
        }
        let peers = self.peers.read().expect("lan peers lock poisoned");
        let (key, _) = peers.iter().find(|(_, key)| tag(key, &beacon.salt, beacon.port) == beacon.tag)?;
        Some(Sighting { key: *key, addr: SocketAddr::new(ip, beacon.port) })
    }
}

/// 自动挑地址时，这个地址所在的网段发不发信标。
fn announces_on(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(_) => ifaddr::is_candidate(ip) && ifaddr::is_private(ip),
        IpAddr::V6(_) => ifaddr::is_candidate(ip),
    }
}

async fn run(inner: Arc<Inner>, v4: Option<UdpSocket>, v6: Option<UdpSocket>) {
    let mut tick = tokio::time::interval(inner.config.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut joined: HashSet<Segment> = HashSet::new();
    let mut budget = Budget { window: Instant::now(), used: 0 };
    let (mut buf4, mut buf6) = ([0u8; 2 * Beacon::LEN], [0u8; 2 * Beacon::LEN]);
    loop {
        let received = tokio::select! {
            _ = tick.tick() => {
                round(&inner, v4.as_ref(), v6.as_ref(), &mut joined);
                continue;
            }
            received = receive(v4.as_ref(), &mut buf4) => received.map(|(len, from)| (&buf4[..len], from)),
            received = receive(v6.as_ref(), &mut buf6) => received.map(|(len, from)| (&buf6[..len], from)),
        };
        match received {
            Ok((datagram, from)) => {
                if budget.take(Instant::now())
                    && let Some(sighting) = inner.recognise(datagram, from)
                {
                    tracing::trace!(addr = %sighting.addr, "在局域网里看到了一个对端的信标");
                    (inner.on_sighting)(sighting);
                }
            }
            Err(error) => {
                tracing::debug!(%error, "收局域网信标出错");
                tokio::time::sleep(RECEIVE_ERROR_PAUSE).await;
            }
        }
    }
}

/// 一轮：在新出现的网卡上加入组播组，然后从每个网段各发一个信标。
fn round(inner: &Inner, v4: Option<&UdpSocket>, v6: Option<&UdpSocket>, joined: &mut HashSet<Segment>) {
    let segments = inner.segments();
    // 网卡没了之后内核自己会退组；从记录里去掉，它再出现时重新加入。
    joined.retain(|segment| segments.contains(segment));
    for &segment in &segments {
        if !joined.contains(&segment) {
            let result = match (segment, v4, v6, inner.config.v6) {
                (Segment::V4(ip), Some(socket), _, _) => socket.join_multicast_v4(*inner.config.v4.ip(), ip),
                (Segment::V6 { index, .. }, _, Some(socket), Some(group)) => socket.join_multicast_v6(group.ip(), index),
                (Segment::V4(_), None, _, _) | (Segment::V6 { .. }, _, _, _) => continue,
            };
            match result {
                Ok(()) => tracing::debug!(?segment, "开始在这个网段上听局域网信标"),
                // 同一块网卡上的另一个地址已经加入过了。
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
                Err(error) => {
                    tracing::debug!(?segment, %error, "在这个网段上加入不了组播组");
                    continue;
                }
            }
            joined.insert(segment);
        }
        if inner.announcing.load(Ordering::Relaxed)
            && let Err(error) = inner.announce(segment)
        {
            tracing::debug!(?segment, %error, "局域网信标没有发出去");
        }
    }
}

async fn receive(socket: Option<&UdpSocket>, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await,
        None => future::pending().await,
    }
}

/// 每秒处理信标的额度。
struct Budget {
    window: Instant,
    used: u32,
}

impl Budget {
    fn take(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.used = 0;
        }
        self.used += 1;
        self.used <= BEACONS_PER_SEC
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    fn key(seed: u8) -> PublicKey {
        PublicKey::from_bytes([seed; 32])
    }

    /// 回环上的一份配置，端口是系统现挑的空闲端口，免得和并行的测试撞上。
    fn loopback_config() -> LanConfig {
        let free = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        LanConfig {
            v4: SocketAddrV4::new(GROUP_V4, free),
            v6: None,
            interfaces: Interfaces::Only(vec![LOOPBACK]),
            interval: Duration::from_millis(50),
        }
    }

    fn start(config: &LanConfig, key: PublicKey, port: u16) -> (LanDiscovery, mpsc::UnboundedReceiver<Sighting>) {
        let (seen, sightings) = mpsc::unbounded_channel();
        let report = move |sighting| {
            // 测试结束、收的一头没了之后还会有信标进来，那时发不出去是正常的。
            let _closed = seen.send(sighting);
        };
        let discovery = LanDiscovery::start(config.clone(), key, port, report).unwrap();
        (discovery, sightings)
    }

    /// 这台机器的回环上组播通不通。不通的话（有的系统不让）这些测试没法做，直接跳过。
    async fn loopback_multicast_works(config: &LanConfig) -> bool {
        let (probe, mut sightings) = start(config, key(250), 1);
        probe.set_peers(&[key(250)]);
        let works = tokio::time::timeout(Duration::from_millis(1500), sightings.recv()).await.is_ok();
        if !works {
            eprintln!("这台机器的回环上组播不通，跳过局域网发现的测试");
        }
        works
    }

    #[tokio::test]
    async fn a_peer_on_the_same_segment_is_recognised_by_its_key_and_reported_with_its_port() {
        let config = loopback_config();
        if !loopback_multicast_works(&config).await {
            return;
        }
        let (a, mut seen_by_a) = start(&config, key(1), 4001);
        let (b, mut seen_by_b) = start(&config, key(2), 4002);
        a.set_peers(&[key(2), key(9)]);
        b.set_peers(&[key(1)]);

        let sighting = tokio::time::timeout(Duration::from_secs(5), seen_by_a.recv()).await.expect("A 应该看到 B 的信标").unwrap();
        assert_eq!(sighting, Sighting { key: key(2), addr: SocketAddr::new(LOOPBACK, 4002) });
        let sighting = tokio::time::timeout(Duration::from_secs(5), seen_by_b.recv()).await.expect("B 应该看到 A 的信标").unwrap();
        assert_eq!(sighting, Sighting { key: key(1), addr: SocketAddr::new(LOOPBACK, 4001) });
    }

    #[tokio::test]
    async fn beacons_of_strangers_and_of_silent_nodes_are_not_reported() {
        let config = loopback_config();
        if !loopback_multicast_works(&config).await {
            return;
        }
        let (listener, mut seen) = start(&config, key(1), 4001);
        listener.set_peers(&[key(2)]);
        // 不在名单里的节点发的信标：认不出来。
        let (_stranger, _) = start(&config, key(3), 4003);
        // 名单里的节点，但它不发信标。
        let (quiet, _) = start(&config, key(2), 4002);
        quiet.set_announcing(false);
        assert!(tokio::time::timeout(Duration::from_millis(500), seen.recv()).await.is_err(), "不该报出任何对端");

        quiet.set_announcing(true);
        let sighting = tokio::time::timeout(Duration::from_secs(5), seen.recv()).await.expect("开始发信标之后应该被看到").unwrap();
        assert_eq!(sighting.key, key(2));
    }

    #[tokio::test]
    async fn a_beacon_is_matched_against_the_tag_not_against_anything_fixed() {
        let inner = Inner {
            config: LanConfig::default(),
            own: hmac_key(&key(1)),
            port: 20330,
            peers: RwLock::new(vec![(key(2), hmac_key(&key(2)))]),
            announcing: AtomicBool::new(true),
            on_sighting: Box::new(|_| {}),
        };
        let from: SocketAddr = "192.168.1.7:55555".parse().unwrap();
        let beacon = |signer: u8, salt: [u8; 8], port: u16| Beacon { port, salt, tag: tag(&hmac_key(&key(signer)), &salt, port) };

        let genuine = beacon(2, [7; 8], 4002);
        assert_eq!(inner.recognise(&genuine.to_bytes(), from), Some(Sighting { key: key(2), addr: "192.168.1.7:4002".parse().unwrap() }));
        // 两个信标的盐不同，标签也就不同：看不出是同一台机器发的。
        assert_ne!(genuine.tag, beacon(2, [8; 8], 4002).tag);
        // 改了端口，标签就对不上了。
        assert_eq!(inner.recognise(&Beacon { port: 4999, ..genuine }.to_bytes(), from), None);
        assert_eq!(inner.recognise(&beacon(3, [7; 8], 4002).to_bytes(), from), None, "不认识的公钥");
        assert_eq!(inner.recognise(&beacon(1, [7; 8], 20330).to_bytes(), from), None, "自己的信标不在名单里");
        assert_eq!(inner.recognise(b"not a beacon", from), None);
        // 链路本地的 IPv6 来源没法直接当候选地址。
        assert_eq!(inner.recognise(&genuine.to_bytes(), "[fe80::1]:5".parse().unwrap()), None);
        let global: SocketAddr = "[2001:db8::7]:5".parse().unwrap();
        assert_eq!(inner.recognise(&genuine.to_bytes(), global).map(|sighting| sighting.addr), Some("[2001:db8::7]:4002".parse().unwrap()));
    }

    #[test]
    fn beacons_are_announced_on_private_ipv4_and_routable_ipv6_segments_only() {
        for ip in ["192.168.1.5", "10.0.0.1", "2001:db8::1", "fd00::1"] {
            assert!(announces_on(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["203.0.113.7", "127.0.0.1", "169.254.1.2", "fe80::1", "::1"] {
            assert!(!announces_on(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn a_flood_of_beacons_is_processed_at_a_bounded_rate() {
        let start = Instant::now();
        let mut budget = Budget { window: start, used: 0 };
        let processed = (0..10_000).filter(|_| budget.take(start)).count();
        assert_eq!(processed, 50);
        assert!(budget.take(start + Duration::from_secs(1)), "下一秒又有额度");
    }
}
