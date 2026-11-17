//! 透明访问要用的几种套接字（只在 Linux 上）。
//!
//! - 入口这一头：接被改到本机的 TCP 连接的监听（[`tcp_listener`]），取回它原来的目标（[`original_destination`]）；
//!   接 TPROXY 交过来的 UDP 报文（[`udp_divert_socket`]、[`recv_diverted`]），每个报文带着它原来的目标；
//!   以原目标的地址回报文给应用（[`udp_reply_socket`]）。
//! - 目标这一头：以发起节点的虚拟 IP 为源地址连本机的服务（[`connect_presenting`]）。
//!
//! `IP_TRANSPARENT` 和 `SO_MARK` 要 `CAP_NET_ADMIN`。
//!
//! # Safety
//!
//! 带原目标收 UDP 报文要用 `recvmsg` 和它的控制消息，`IP_RECVORIGDSTADDR` 要用 `setsockopt`：标准库、tokio 和 socket2
//! 都没有包这两样，所以这里直接调 libc，`unsafe` 只在这个模块里，对外都是安全的函数。

use std::io;
use std::mem::{self, MaybeUninit};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, RawFd};

use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::io::Interest;
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};

use crate::{MARK, PORT};

/// 入口接拦截下来的 TCP 连接的地址。
#[must_use]
pub const fn entry() -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT)
}

/// 在 [`entry`] 上开的 TCP 监听。要在 tokio 运行时里调用。
///
/// # Errors
///
/// 端口被占着，或者没有绑 1024 以下端口的权限。
pub fn tcp_listener() -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::V4(entry()).into())?;
    socket.listen(1024)?;
    TcpListener::from_std(socket.into())
}

/// 一条被改到本机的连接原来要去哪里（`SO_ORIGINAL_DST`）。
///
/// 不是被改过来的连接（有人直接连了入口的端口）这里得到的就是入口自己的地址：调用方按"不是组网的目标"拒绝它。
///
/// # Errors
///
/// 内核里没有这条连接的连接跟踪记录。
pub fn original_destination(stream: &TcpStream) -> io::Result<SocketAddrV4> {
    let address = SockRef::from(stream).original_dst_v4()?;
    address.as_socket_ipv4().ok_or_else(|| io::Error::other("原目标不是 IPv4 地址"))
}

/// 接 TPROXY 交过来的 UDP 报文的套接字，在 [`entry`] 上。要在 tokio 运行时里调用。
///
/// # Errors
///
/// 端口被占着，或者没有权限（`IP_TRANSPARENT`、低端口）。
pub fn udp_divert_socket() -> io::Result<UdpSocket> {
    udp_with_original(entry(), true)
}

/// 一个收报文时带上原目标的 UDP 套接字。TPROXY 只把报文交给 `IP_TRANSPARENT` 的套接字。
fn udp_with_original(bind: SocketAddrV4, transparent: bool) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    if transparent {
        socket.set_ip_transparent_v4(true)?;
    }
    set_int_option(socket.as_raw_fd(), libc::SOL_IP, libc::IP_RECVORIGDSTADDR, 1)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::V4(bind).into())?;
    UdpSocket::from_std(socket.into())
}

/// 收到的一个被拦截的 UDP 报文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diverted {
    /// 报文的长度（内容在调用方给的缓冲区里）。
    pub len: usize,
    /// 发报文的应用的地址。
    pub from: SocketAddrV4,
    /// 应用要发往的地址。
    pub to: SocketAddrV4,
}

/// 在 [`udp_divert_socket`] 上收一个报文，带着它原来的目标。比 `buf` 长的报文被截断（UDP 的报文最长 65535 字节，给这么大就不会）。
///
/// # Errors
///
/// 收的时候出错，或者报文没有带原目标（不是 TPROXY 交过来的：有人直接发给了入口的端口）。
pub async fn recv_diverted(socket: &UdpSocket, buf: &mut [u8]) -> io::Result<Diverted> {
    socket.async_io(Interest::READABLE, || recv_with_original(socket.as_raw_fd(), buf)).await
}

/// 以 `local`（应用原来要发往的地址）为源地址、发给 `client` 的套接字。同一个 `client` 之后发往 `local` 的报文也由它收：
/// TPROXY 先找已经连上的套接字，找到了就不经过 [`udp_divert_socket`]。要在 tokio 运行时里调用。
///
/// # Errors
///
/// 没有权限，或者系统不给套接字。
pub fn udp_reply_socket(local: SocketAddrV4, client: SocketAddrV4) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_ip_transparent_v4(true)?;
    // 好几个应用同时和同一个对端地址说话时，各有一个绑在同一个地址上的套接字，按连上的对方分。
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::V4(local).into())?;
    socket.connect(&SocketAddr::V4(client).into())?;
    UdpSocket::from_std(socket.into())
}

/// 以 `source`（发起节点的虚拟 IP）为源地址连本机的服务 `target`。
///
/// 套接字带着 [`MARK`]：服务回的包在 nft 里按连接认出来、打上同样的标记，经策略路由在本机投递，回到这个套接字。
/// `target` 必须是本机的、非回环的地址：内核不肯把源地址是回环地址的包发往别的地址，所以回环上的服务回不了包。
///
/// # Errors
///
/// 没有权限，或者连不上。
pub async fn connect_presenting(source: Ipv4Addr, target: SocketAddr) -> io::Result<TcpStream> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_ip_transparent_v4(true)?;
    socket.set_mark(MARK)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(source, 0)).into())?;
    let socket = TcpSocket::from_std_stream(socket.into());
    socket.connect(target).await
}

fn socklen<T>() -> libc::socklen_t {
    libc::socklen_t::try_from(mem::size_of::<T>()).expect("套接字选项和地址结构都只有几十个字节")
}

fn set_int_option(fd: RawFd, level: libc::c_int, name: libc::c_int, value: libc::c_int) -> io::Result<()> {
    // SAFETY: `value` 活到调用结束，长度给的是它自己的长度；内核只读这几个字节。
    // - Precondition: `fd` 是调用方手里一个有效的套接字。
    // - Justification: 指针来自一个局部变量的引用，不会悬空，也不会被内核写。
    let rc = unsafe { libc::setsockopt(fd, level, name, (&raw const value).cast(), socklen::<libc::c_int>()) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// 控制消息的缓冲区：8 字节对齐（`cmsghdr` 要求的对齐），放得下一个 `IP_ORIGDSTADDR` 还有余量。
#[repr(C, align(8))]
struct Control([MaybeUninit<u8>; 128]);

fn recv_with_original(fd: RawFd, buf: &mut [u8]) -> io::Result<Diverted> {
    // SAFETY: 全零的 `sockaddr_in` 和 `msghdr` 都是合法的值（C 的结构体，里面只有整数和指针）。
    let mut from: libc::sockaddr_in = unsafe { mem::zeroed() };
    let mut control = Control([MaybeUninit::uninit(); 128]);
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    // SAFETY: 同上。
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_name = (&raw mut from).cast();
    msg.msg_namelen = socklen::<libc::sockaddr_in>();
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = (&raw mut control).cast();
    // glibc 上这个字段是 usize，musl 上是 socklen_t：转换在前者上是多余的，在后者上不是。
    #[allow(clippy::useless_conversion)]
    let control_len = mem::size_of::<Control>().try_into().expect("控制消息的缓冲区只有 128 字节");
    msg.msg_controllen = control_len;

    // SAFETY: `msg` 里的每个指针都指向这个函数里的局部变量或者调用方的 `buf`，长度都是它们自己的长度，调用期间都活着；
    // 内核只往这些地方写，不超过给的长度。
    let received = unsafe { libc::recvmsg(fd, &raw mut msg, 0) };
    let len = usize::try_from(received).map_err(|_| io::Error::last_os_error())?;

    let mut original = None;
    // SAFETY: `msg` 是刚被 `recvmsg` 填好的，`msg_control` 和 `msg_controllen` 描述的是内核写进 `control` 的控制消息；
    // `CMSG_FIRSTHDR`、`CMSG_NXTHDR` 只在这段范围里走，到头时返回空指针。
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&raw const msg) };
    while !header.is_null() {
        // SAFETY: `header` 不是空指针，是上面两个宏给的、落在控制消息范围里的一个 `cmsghdr`。
        let (level, kind) = unsafe { ((*header).cmsg_level, (*header).cmsg_type) };
        if level == libc::SOL_IP && kind == libc::IP_ORIGDSTADDR {
            // SAFETY: `IP_ORIGDSTADDR` 消息的数据就是一个 `sockaddr_in`（`ip(7)`），内核按它的长度写好了；
            // 数据不一定按 `sockaddr_in` 对齐，所以按字节读。
            let address: libc::sockaddr_in = unsafe { std::ptr::read_unaligned(libc::CMSG_DATA(header).cast()) };
            original = Some(v4(&address));
        }
        // SAFETY: 同 `CMSG_FIRSTHDR`。
        header = unsafe { libc::CMSG_NXTHDR(&raw const msg, header) };
    }
    let to = original.ok_or_else(|| io::Error::other("报文没有带原目标，不是被拦截下来的"))?;
    Ok(Diverted { len, from: v4(&from), to })
}

fn v4(address: &libc::sockaddr_in) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)), u16::from_be(address.sin_port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不经过 TPROXY 时，原目标就是报文真的发往的地址；这样不需要权限就能把收报文和读控制消息的那一段走一遍。
    #[tokio::test]
    async fn a_datagram_is_received_with_the_address_it_was_sent_to() {
        let receiver = udp_with_original(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), false).unwrap();
        let SocketAddr::V4(at) = receiver.local_addr().unwrap() else { panic!("绑的是 IPv4") };
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.send_to(b"ping", at).await.unwrap();
        let mut buf = [0; 64];
        let diverted = recv_diverted(&receiver, &mut buf).await.unwrap();
        assert_eq!(&buf[..diverted.len], b"ping");
        assert_eq!(SocketAddr::V4(diverted.from), sender.local_addr().unwrap());
        assert_eq!(diverted.to, at);
    }

    #[tokio::test]
    async fn a_datagram_without_its_original_destination_is_an_error() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.send_to(b"x", receiver.local_addr().unwrap()).await.unwrap();
        let mut buf = [0; 16];
        assert!(recv_diverted(&receiver, &mut buf).await.is_err());
    }
}
