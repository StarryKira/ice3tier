//! 出站：检查策略、解析域名、连接目标。

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use i3t_access::{EgressPolicy, LocalEgress};
use i3t_model::Netmap;
use i3t_model::netmap::{self, ExitRole, Network};
use i3t_proto::flow::{Host, Reply, Status};
use ipnet::IpNet;
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time;

use crate::dns_cache::{self, DnsCache};
use crate::system::{Os, ResolveError, System};
use crate::{policy, upstream};

/// 连一个地址最多等多久。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 连本机自己的地址最多等多久。本机不会丢包，等不到就是没有。
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// 解析一个域名最多等多久。
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// 一个域名最多取几个地址。它们会被挨个尝试，每个都可能等满 [`CONNECT_TIMEOUT`]，所以要有上限。
const MAX_ADDRESSES: usize = 8;

/// 网卡的地址列表缓存多久。每条流都要用它，而它很少变。
const INTERFACES_TTL: Duration = Duration::from_secs(5);

/// 到目标的连接静默多久之后内核开始发保活探测，以及探测的间隔。
///
/// 这些连接常常要穿过出口所在的家用路由器，静默久了映射会被悄悄回收，而 LLM 这类请求正好会长时间静默。
/// 流本身没有空闲超时，一条连接的死活就靠它判断。
const KEEPALIVE: Duration = Duration::from_secs(25);

/// 出站的拨号器。出口处理别人的流时用它，入口就地出站时也用它（它实现了 [`LocalEgress`]），
/// 所以两条路是同一份出站策略、同一个解析缓存。克隆出来的是同一个。
pub struct Outbound<S = Os> {
    inner: Arc<Inner<S>>,
}

struct Inner<S> {
    system: S,
    netmap: watch::Receiver<Arc<Netmap>>,
    dns: Mutex<DnsCache>,
    interfaces: Mutex<Option<Interfaces>>,
}

struct Interfaces {
    read_at: Instant,
    nets: Arc<[IpNet]>,
}

impl<S> Clone for Outbound<S> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<S> std::fmt::Debug for Outbound<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outbound").finish_non_exhaustive()
    }
}

impl<S: System> Outbound<S> {
    /// `netmap` 只在就地出站时用：那时没有别人给出该按哪一版配置判断。
    pub fn new(netmap: watch::Receiver<Arc<Netmap>>, system: S) -> Self {
        Self { inner: Arc::new(Inner { system, netmap, dns: Mutex::default(), interfaces: Mutex::new(None) }) }
    }

    /// 互联网目标：本节点必须是出口，端口要在允许的范围里，解析出来的每一个地址都要过出站策略。
    pub(crate) async fn internet(&self, netmap: &Netmap, host: &Host, port: u16) -> Result<TcpStream, Reply> {
        let Some(role) = &netmap.me.exit else {
            return Err(Reply::new(Status::NotEnabled, "这个节点没有启用出口"));
        };
        if role.ports.as_ref().is_some_and(|ports| !ports.contains(&port)) {
            return Err(Reply::new(Status::PolicyDenied, format!("出口不允许目标端口 {port}")));
        }
        let addresses = self.resolve(host).await?;
        // 域名可以解析到任何地方，所以要看解析之后的地址；只要有一个不该连，整个请求都拒绝，
        // 不然对方可以把一个内网地址夹在几个公网地址中间碰运气。
        let interfaces = self.interfaces()?;
        if let Some(what) = addresses.iter().find_map(|&ip| forbidden_as_internet(&netmap.network, &interfaces, ip)) {
            return Err(Reply::new(Status::PolicyDenied, format!("目标地址是{what}，出口不替别人连这类地址")));
        }
        let targets = addresses.into_iter().map(|ip| SocketAddr::new(ip, port));
        self.connect_first(targets, CONNECT_TIMEOUT, role.upstream_socks5).await
    }

    /// 局域网目标：本节点必须是出口并且打开了局域网开关，地址必须在本机某块网卡直连的网段里。端口不受限制。
    pub(crate) async fn lan(&self, netmap: &Netmap, addr: SocketAddr) -> Result<TcpStream, Reply> {
        match &netmap.me.exit {
            None => return Err(Reply::new(Status::NotEnabled, "这个节点没有启用出口")),
            Some(ExitRole { lan: false, .. }) => {
                return Err(Reply::new(Status::NotEnabled, "这个出口没有打开「允许访问本机局域网」"));
            }
            Some(ExitRole { lan: true, .. }) => {}
        }
        let ip = addr.ip().to_canonical();
        let attached = self.interfaces()?.iter().any(|net| !net.addr().is_loopback() && subnet_contains(net, ip));
        let refused = if ip.is_loopback() || policy::is_link_local(ip) || policy::is_cloud_metadata(ip) {
            Some("回环、链路本地和云平台的元数据地址不算局域网")
        } else if netmap::is_overlay_address(&netmap.network, ip) {
            Some("覆盖网的地址不算局域网")
        } else if !attached {
            Some("目标不在本机网卡直连的网段里")
        } else {
            None
        };
        match refused {
            Some(why) => Err(Reply::new(Status::PolicyDenied, why)),
            None => self.connect_first([SocketAddr::new(ip, addr.port())], CONNECT_TIMEOUT, None).await,
        }
    }

    /// 本节点自己的服务：只用本机的非回环地址去连。
    ///
    /// 所以只监听 127.0.0.1 的服务连不上，不会因为装了这个程序就对组网可见；要暴露这样的服务得显式配端口转发。
    /// 本机有几个地址就挨个试：服务可能只监听在其中一个上。
    pub(crate) async fn node_port(&self, port: u16) -> Result<TcpStream, Reply> {
        let interfaces = self.interfaces()?;
        let own = interfaces.iter().map(IpNet::addr).filter(|ip| !(ip.is_loopback() || ip.is_unspecified() || policy::is_link_local(*ip)));
        // IPv4 在前：只监听 0.0.0.0 的服务最常见。
        let (v4, v6): (Vec<IpAddr>, Vec<IpAddr>) = own.partition(IpAddr::is_ipv4);
        let targets: Vec<SocketAddr> = v4.into_iter().chain(v6).map(|ip| SocketAddr::new(ip, port)).collect();
        if targets.is_empty() {
            return Err(Reply::new(Status::Unreachable, "这个节点没有非回环的地址，它上面的服务不可达"));
        }
        self.connect_first(targets, LOCAL_CONNECT_TIMEOUT, None).await
    }

    /// 端口转发指向的地址。回环地址和局域网地址都可以：下发给本节点的端口转发就是访问它们的显式途径。
    pub(crate) async fn forward(&self, addr: SocketAddr) -> Result<TcpStream, Reply> {
        self.connect_first([addr], CONNECT_TIMEOUT, None).await
    }

    /// 回环监听器上的就地出站：调用者是本机的程序，不加任何限制。
    async fn unrestricted(&self, host: &Host, port: u16) -> Result<TcpStream, Reply> {
        let targets = self.resolve(host).await?.into_iter().map(|ip| SocketAddr::new(ip, port));
        self.connect_first(targets, CONNECT_TIMEOUT, None).await
    }

    /// 域名对应的地址，最多 [`MAX_ADDRESSES`] 个。本来就是地址的不用解析。
    async fn resolve(&self, host: &Host) -> Result<Vec<IpAddr>, Reply> {
        let name = match host {
            Host::Ip(ip) => return Ok(vec![ip.to_canonical()]),
            Host::Name(name) => dns_cache::normalize(name.as_str()),
        };
        if let Some(cached) = self.inner.dns.lock().expect("dns cache mutex poisoned").get(&name, Instant::now()) {
            return Ok(cached);
        }
        let resolved = match time::timeout(RESOLVE_TIMEOUT, self.inner.system.resolve(&name)).await {
            Ok(Ok(resolved)) => resolved,
            Ok(Err(ResolveError::NotFound)) => return Err(Reply::new(Status::NameNotFound, "域名不存在")),
            Ok(Err(ResolveError::Failed(reason))) => return Err(Reply::new(Status::ResolverFailure, reason)),
            Err(_) => return Err(Reply::new(Status::ResolverFailure, format!("{RESOLVE_TIMEOUT:?} 之内没有解析出来"))),
        };
        let mut addresses: Vec<IpAddr> = Vec::with_capacity(resolved.len().min(MAX_ADDRESSES));
        for ip in resolved.into_iter().map(|ip| ip.to_canonical()) {
            if addresses.len() < MAX_ADDRESSES && !addresses.contains(&ip) {
                addresses.push(ip);
            }
        }
        if addresses.is_empty() {
            return Err(Reply::new(Status::NameNotFound, "域名没有地址记录"));
        }
        self.inner.dns.lock().expect("dns cache mutex poisoned").put(name, addresses.clone(), Instant::now());
        Ok(addresses)
    }

    /// 本机网卡上的地址和网段。读不到时拒绝请求：没有它就判断不了目标是不是在本机的内网里。
    fn interfaces(&self) -> Result<Arc<[IpNet]>, Reply> {
        let mut cached = self.inner.interfaces.lock().expect("interfaces mutex poisoned");
        if let Some(interfaces) = cached.as_ref().filter(|interfaces| interfaces.read_at.elapsed() < INTERFACES_TTL) {
            return Ok(Arc::clone(&interfaces.nets));
        }
        let nets: Arc<[IpNet]> = self
            .inner
            .system
            .interfaces()
            .map_err(|error| Reply::new(Status::InternalError, format!("读不到本机网卡的地址：{error}")))?
            .into();
        *cached = Some(Interfaces { read_at: Instant::now(), nets: Arc::clone(&nets) });
        Ok(nets)
    }

    /// 挨个尝试 `targets`，返回第一条连上的连接。都连不上时返回其中最说明问题的那个失败。
    async fn connect_first(
        &self,
        targets: impl IntoIterator<Item = SocketAddr>,
        timeout: Duration,
        upstream: Option<SocketAddr>,
    ) -> Result<TcpStream, Reply> {
        let mut failure: Option<Reply> = None;
        for target in targets {
            let attempt = match time::timeout(timeout, self.connect_one(target, upstream)).await {
                Ok(attempt) => attempt,
                Err(_) => Err(Reply::new(Status::TimedOut, format!("{timeout:?} 之内没有连上"))),
            };
            match attempt {
                Ok(stream) => {
                    tune(&stream);
                    return Ok(stream);
                }
                Err(reply) => {
                    if failure.as_ref().is_none_or(|kept| rank(reply.status) > rank(kept.status)) {
                        failure = Some(reply);
                    }
                }
            }
        }
        Err(failure.unwrap_or_else(|| Reply::new(Status::Unreachable, "没有可以连的地址")))
    }

    async fn connect_one(&self, target: SocketAddr, upstream: Option<SocketAddr>) -> Result<TcpStream, Reply> {
        let Some(proxy) = upstream else {
            return self.inner.system.connect(target).await.map_err(|error| Reply::new(connect_status(&error), error.to_string()));
        };
        let mut stream = self
            .inner
            .system
            .connect(proxy)
            .await
            .map_err(|error| Reply::new(Status::InternalError, format!("连不上上游代理：{error}")))?;
        upstream::connect(&mut stream, target).await?;
        Ok(stream)
    }
}

impl<S: System> LocalEgress for Outbound<S> {
    async fn connect(&self, host: &Host, port: u16, policy: EgressPolicy) -> Result<TcpStream, Reply> {
        match policy {
            EgressPolicy::Unrestricted => self.unrestricted(host, port).await,
            EgressPolicy::Exit => {
                let netmap = Arc::clone(&self.inner.netmap.borrow());
                self.internet(&netmap, host, port).await
            }
        }
    }
}

/// `ip` 不能当互联网目标去连时，返回它是什么地址。
fn forbidden_as_internet(network: &Network, interfaces: &[IpNet], ip: IpAddr) -> Option<&'static str> {
    if let Some(what) = policy::not_public(ip) {
        Some(what)
    } else if netmap::is_overlay_address(network, ip) {
        Some("覆盖网的地址")
    } else if interfaces.iter().any(|net| subnet_contains(net, ip)) {
        Some("本机网卡所在网段的地址")
    } else {
        None
    }
}

/// `ip` 是不是在这块网卡的网段里。前缀长度是 0 的网卡地址（没有网段信息）只算它自己。
fn subnet_contains(interface: &IpNet, ip: IpAddr) -> bool {
    if interface.prefix_len() == 0 { interface.addr() == ip } else { interface.contains(&ip) }
}

/// 给到目标的连接设置选项：关掉 Nagle，打开内核保活。设不上不影响这条连接能用，只记一行。
fn tune(stream: &TcpStream) {
    let keepalive = TcpKeepalive::new().with_time(KEEPALIVE).with_interval(KEEPALIVE);
    let tuned = stream.set_nodelay(true).and_then(|()| SockRef::from(stream).set_tcp_keepalive(&keepalive));
    if let Err(error) = tuned {
        tracing::warn!(%error, "到目标的连接设置不了 NODELAY 或者保活，静默久了可能被中间的路由器回收");
    }
}

/// 连接失败算哪一种。前几种是目标的问题；剩下的（文件描述符用完之类）是出口自己的问题。
fn connect_status(error: &io::Error) -> Status {
    match error.kind() {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset => Status::ConnectionRefused,
        io::ErrorKind::TimedOut => Status::TimedOut,
        io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkDown
        | io::ErrorKind::AddrNotAvailable
        | io::ErrorKind::PermissionDenied => Status::Unreachable,
        _ => Status::InternalError,
    }
}

/// 几个地址都连不上时报哪一个失败：越具体的越优先。"拒绝连接"说明主机在、端口没开，比"不可达"有用。
const fn rank(status: Status) -> u8 {
    match status {
        Status::ConnectionRefused => 4,
        Status::TimedOut => 3,
        Status::Unreachable => 2,
        Status::PolicyDenied => 1,
        Status::Ok
        | Status::NameNotFound
        | Status::AccessDenied
        | Status::NotEnabled
        | Status::UnknownForward
        | Status::ResolverFailure
        | Status::InternalError => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_errors_are_attributed_to_the_target_unless_they_are_local() {
        let status = |kind: io::ErrorKind| connect_status(&io::Error::from(kind));
        assert_eq!(status(io::ErrorKind::ConnectionRefused), Status::ConnectionRefused);
        assert_eq!(status(io::ErrorKind::TimedOut), Status::TimedOut);
        assert_eq!(status(io::ErrorKind::NetworkUnreachable), Status::Unreachable);
        assert_eq!(status(io::ErrorKind::HostUnreachable), Status::Unreachable);
        assert_eq!(status(io::ErrorKind::AddrNotAvailable), Status::Unreachable);
        assert_eq!(status(io::ErrorKind::OutOfMemory), Status::InternalError);
        assert_eq!(status(io::ErrorKind::Other), Status::InternalError);
    }

    #[test]
    fn an_interface_without_prefix_information_only_covers_its_own_address() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        let lan: IpNet = "192.168.1.5/24".parse().unwrap();
        assert!(subnet_contains(&lan, ip("192.168.1.200")));
        assert!(subnet_contains(&lan, ip("192.168.1.5")));
        assert!(!subnet_contains(&lan, ip("192.168.2.1")));

        let unknown: IpNet = "203.0.113.5/0".parse().unwrap();
        assert!(subnet_contains(&unknown, ip("203.0.113.5")));
        assert!(!subnet_contains(&unknown, ip("8.8.8.8")));
    }

    #[test]
    fn internet_targets_exclude_the_overlay_range_and_every_attached_subnet_whatever_range_they_use() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        // 覆盖网和本机网卡用的都是公网模样的地址段：固定的那张表认不出它们，要靠这两项检查。
        let network = Network {
            name: "odd".into(),
            domain: "odd.i3t".into(),
            cidr: "44.128.0.0/16".parse().unwrap(),
            routing: Default::default(),
            max_relay_hops: 2,
            reach: Default::default(),
        };
        let interfaces: Vec<IpNet> =
            vec!["127.0.0.1/8".parse().unwrap(), "198.20.0.5/24".parse().unwrap(), "2400:db00::5/64".parse().unwrap()];

        assert_eq!(forbidden_as_internet(&network, &interfaces, ip("44.128.3.4")), Some("覆盖网的地址"));
        assert_eq!(forbidden_as_internet(&network, &interfaces, ip("198.20.0.77")), Some("本机网卡所在网段的地址"));
        assert_eq!(
            forbidden_as_internet(&network, &interfaces, ip("198.20.0.5")),
            Some("本机网卡所在网段的地址"),
            "本机自己的地址也在里面"
        );
        assert_eq!(forbidden_as_internet(&network, &interfaces, ip("2400:db00::99")), Some("本机网卡所在网段的地址"));
        assert_eq!(forbidden_as_internet(&network, &interfaces, ip("10.1.2.3")), Some("私有地址"));

        for public in ["44.129.0.1", "198.20.1.1", "2400:db00:0:1::1", "1.1.1.1"] {
            assert_eq!(forbidden_as_internet(&network, &interfaces, ip(public)), None, "{public}");
        }
    }

    #[tokio::test]
    async fn connections_to_targets_have_nagle_off_and_keepalive_probes_after_25_idle_seconds() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        assert!(!SockRef::from(&stream).keepalive().unwrap(), "系统默认不开保活");

        tune(&stream);

        assert!(stream.nodelay().unwrap());
        let socket = SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        // 读回这两个值的接口 Windows 上没有。
        #[cfg(not(windows))]
        {
            assert_eq!(socket.tcp_keepalive_time().unwrap(), Duration::from_secs(25));
            assert_eq!(socket.tcp_keepalive_interval().unwrap(), Duration::from_secs(25));
        }
    }

    #[test]
    fn the_most_specific_failure_wins() {
        assert!(rank(Status::ConnectionRefused) > rank(Status::TimedOut));
        assert!(rank(Status::TimedOut) > rank(Status::Unreachable));
        assert!(rank(Status::Unreachable) > rank(Status::InternalError));
    }
}
