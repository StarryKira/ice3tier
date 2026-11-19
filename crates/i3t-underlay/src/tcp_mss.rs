//! 本进程开出去、接进来的 TCP 都通告一个压低了的最大分段长度（MSS），躲开路径 MTU 黑洞。
//!
//! 有的路送不到 1500 字节的包，也不回 ICMP（路径 MTU 黑洞），发的一方就不知道要把分段改小。实际网络上量到：
//! 一台只能走 TCP 的服务器收不到 1488 字节的 IP 包，1440 字节的都收得到，网卡的 MTU 却是 1500。握手和小的消息过得去，
//! 连接建得起来；一旦对端有一个满 1500 字节的分段要发，它就一直重发同样大小的分段（`net.ipv4.tcp_mtu_probing=0`
//! 时内核不会自己试小的），连接卡死：控制连接 90 秒收不完一份 netmap，经 TCP 中转的连接上第一条流就卡住，
//! lane、出口连目标也一样。所以看上去时好时坏：小消息照常，大的永远到不了。
//!
//! MSS 是握手时由收的一方通告给发的一方的：在本端的 socket 上设 `TCP_MAXSEG`，对端发过来的分段就不会超过它，
//! 对端是不是我们的程序都一样（出口连的网站、上游代理）。本端自己发的分段也不超过它。拨号的 socket 在 `connect`
//! 之前设，监听的 socket 在 `listen` 之前设：Linux 上接进来的连接继承监听 socket 的这一项，握手的应答里通告的就是它。
//!
//! 用 [`connect`] 和 [`listen`] 代替 tokio 的 `TcpStream::connect` 和 `TcpListener::bind`；自己用 socket2 建 socket 的地方
//! 在 `connect`、`listen` 之前调用 [`clamp`]。本机回环上的连接（本机状态接口）不走这里，它们不过任何路。
//!
//! 平台：Linux、macOS 和别的 Unix 都有 `TCP_MAXSEG`，可以在连接之前设（macOS 上接进来的连接继不继承监听 socket 的这一项
//! 由内核决定）。Windows 只能读不能设，那里 [`clamp`] 什么都不做，用系统的 MSS。

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};

use socket2::SockRef;
use tokio::net::{TcpListener, TcpSocket, TcpStream};

/// 默认的 MSS：1360。
///
/// 带着 Linux 默认的时间戳选项（12 字节），一个分段的 IP 包最大是 1360 + 20 + 20 = 1400 字节：比实际网络上量到的那个黑洞
/// 送得到的最大的包（1440 字节）小 40 字节，也放得进常见的隧道（WireGuard 1420、GRE 1476、PPPoE 1492、IPsec 多在 1400 以上），
/// IPv6 下是 1420 字节。代价是包数：1 MB 的数据以前 725 个分段（每个 1448 字节），现在 778 个（每个 1348 字节），多 7%，
/// 线上多出的头部不到千分之三。取 1400 时（包 1440 字节）正好卡在量到的边上，经 WireGuard 的路又放不下；
/// 取 IPv6 的最小 MTU 对应的 1220 要多 20% 的包，换来的只是 1280 到 1400 之间那些很少见的路。
pub const DEFAULT: u16 = 1360;

/// 设置里允许的范围（0 另算，是不设）。下限是 IPv4 要求每台主机都收得下的 576 字节的包；上限是巨帧。
pub const RANGE: std::ops::RangeInclusive<u16> = 536..=8960;

/// 本进程之后开的 TCP 用的 MSS；0 是不设、用系统的。
static MSS: AtomicU16 = AtomicU16::new(DEFAULT);

/// 换掉本进程之后开的 TCP 用的 MSS。0 是不设，用系统的（按网卡的 MTU 算，路径上的黑洞它不知道）。已经建好的连接不变。
pub fn set(mss: u16) {
    MSS.store(mss, Ordering::Relaxed);
}

/// 现在用的 MSS；不设时是 `None`。
#[must_use]
pub fn get() -> Option<u16> {
    std::num::NonZeroU16::new(MSS.load(Ordering::Relaxed)).map(std::num::NonZeroU16::get)
}

/// 给一个还没有连接、还没有监听的 TCP socket 设上 MSS。设不上不是错误（这个系统没有这个选项）：用系统的 MSS，记一行 debug。
pub fn clamp<'s, S>(socket: &'s S)
where
    SockRef<'s>: From<&'s S>,
{
    let Some(mss) = get() else { return };
    let socket = SockRef::from(socket);
    #[cfg(unix)]
    if let Err(error) = socket.set_tcp_mss(u32::from(mss)) {
        tracing::debug!(%error, mss, "设置不了 TCP_MAXSEG，这条连接用系统的 MSS");
    }
    #[cfg(not(unix))]
    let _ = (socket, mss);
}

/// 连 `addr`，握手里通告设好的 MSS。代替 `TcpStream::connect`。
///
/// # Errors
///
/// 建不了 socket，或者连不上，原样返回操作系统的错误。
pub async fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
    let socket = new_socket(addr)?;
    clamp(&socket);
    socket.connect(addr).await
}

/// 连 `host`（`主机:端口`，主机可以是域名），握手里通告设好的 MSS。代替用字符串调用的 `TcpStream::connect`，做法和它一样：
/// 解析出的地址按顺序一个个试，第一个连上的为准，都连不上时返回最后一个错误。
///
/// # Errors
///
/// 解析不了，或者每个地址都连不上。
pub async fn connect_host(host: &str) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::InvalidInput, "这个名字解析不出任何地址");
    for addr in tokio::net::lookup_host(host).await? {
        match connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// 在 `addr` 上监听，接进来的连接通告设好的 MSS。代替 `TcpListener::bind`，做法和它一样：Unix 上设 `SO_REUSEADDR`
/// （进程重启之后不用等旧连接的 TIME_WAIT 过去），排队的连接最多 1024 条。要在 tokio 运行时里调用。
///
/// # Errors
///
/// 建不了 socket、绑不上（端口被占着、地址不是本机的），原样返回操作系统的错误。
pub fn listen(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = new_socket(addr)?;
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    clamp(&socket);
    socket.bind(addr)?;
    socket.listen(1024)
}

fn new_socket(addr: SocketAddr) -> io::Result<TcpSocket> {
    match addr {
        SocketAddr::V4(_) => TcpSocket::new_v4(),
        SocketAddr::V6(_) => TcpSocket::new_v6(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn mss_of(stream: &TcpStream) -> u32 {
        SockRef::from(stream).tcp_mss().expect("TCP 的 MSS 读得出来")
    }

    #[tokio::test]
    async fn dialled_and_accepted_connections_both_use_the_clamped_segment_size() {
        // 回环的 MTU 是 65536：不压的话两头的 MSS 都是六万多。
        let listener = listen("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        let (dialled, accepted) = tokio::join!(connect(addr), listener.accept());
        let (mut dialled, (mut accepted, _)) = (dialled.unwrap(), accepted.unwrap());
        // 发一大块过去，确认连接照常能用，MSS 也是建立之后实际在用的那个。
        let block = vec![7u8; 256 * 1024];
        let (written, read) = tokio::join!(dialled.write_all(&block), async {
            let mut received = vec![0u8; block.len()];
            accepted.read_exact(&mut received).await.map(|_| received)
        });
        written.unwrap();
        assert_eq!(read.unwrap(), block);
        for (side, stream) in [("拨出的", &dialled), ("接进来的", &accepted)] {
            let mss = mss_of(stream);
            assert!(mss <= u32::from(DEFAULT) && mss >= u32::from(DEFAULT) - 40, "{side}连接的 MSS 是 {mss}，应该压在 {DEFAULT} 以内");
        }
    }

    #[tokio::test]
    async fn a_listener_bound_by_the_system_default_does_not_clamp() {
        // 对照：不经这里建的 socket 在回环上的 MSS 远大于默认值，上面那个测试量到的确实是设上去的。
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (dialled, accepted) = tokio::join!(TcpStream::connect(listener.local_addr().unwrap()), listener.accept());
        let (dialled, (accepted, _)) = (dialled.unwrap(), accepted.unwrap());
        assert!(mss_of(&dialled) > 10_000 && mss_of(&accepted) > 10_000);
    }
}
