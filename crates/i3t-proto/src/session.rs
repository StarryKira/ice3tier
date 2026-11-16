//! 中转会话的帧（节点和 relay 之间）。见 docs/architecture.md §5.2。
//!
//! 节点要经某个 relay 收发包，先和它建一个中转会话。走 UDP 时每一帧是一个数据报；走 TCP 时同样的帧装在
//! [`crate::tcp_frame`] 里，握手完全一样。
//!
//! ```text
//! 握手
//! 节点 → relay  INIT       0x02 | 节点编号 4 | 随机数 8 | 填充到 32 字节
//! relay → 节点  CHALLENGE  0x03 | 随机数 8 | cookie 16                                          25 字节
//! 节点 → relay  AUTH       0x04 | 节点编号 4 | 随机数 8 | cookie 16 | 签名 64 | 填充到 96 字节
//! relay → 节点  OK         0x05 | 随机数 8 | 有效期 2 | 地址 7 或 19 | 签名 64                 最长 94 字节
//!
//! 保活
//! 节点 → relay  PING       0x06 | 节点编号 4 | 随机数 8 | 填充到 32 字节
//! relay → 节点  PONG       0x07 | 随机数 8 | 地址 7 或 19                                      最长 28 字节
//!
//! 拒绝
//! relay → 节点  ERROR      0x0c | 原因 1 | 节点编号 4                                            6 字节
//! ```
//!
//! 整数都是大端。地址的编码是：地址族 1 字节（4 或 6）、地址 4 或 16 字节、端口 2 字节。
//! 有效期的单位是秒：这么久收不到 PING，relay 就把会话删掉。
//!
//! # relay 对没有认证过的来源做什么
//!
//! 这几条是格式本身保证的，不靠实现自觉：
//!
//! - **应答不比请求大。** INIT、AUTH、PING 都带填充，比它们各自的应答长；ERROR 比任何能引出它的帧都短。
//!   所以伪造来源地址的人拿 relay 放大不了流量。比规定长度短的请求一律不认。
//! - **来源地址得到证明之前，relay 不记状态，也不验签名。** CHALLENGE 里的 cookie 是 relay 用只有自己知道的密钥
//!   对"来源地址 + 节点编号 + 时间片"算出来的，算完就忘。AUTH 把 cookie 带回来，relay 重算一遍对得上，
//!   才说明对方确实收得到发往这个地址的包，这之后才去验签名、建会话。
//! - CHALLENGE 和 PONG 把请求里的随机数抄回去，不在路径上的人伪造不了。OK 带 relay 的签名。
//!
//! # 签的是什么
//!
//! 签名由调用方用 `i3t_crypto::NodeKey::sign(用途, 消息)` 来算，这里只规定消息的字节：
//!
//! | 帧 | 谁签 | 用途标签 | 消息 |
//! |---|---|---|---|
//! | AUTH | 节点 | [`Auth::PURPOSE`] | [`Auth::signed_message`]：relay 编号、节点编号、随机数、cookie |
//! | OK | relay | [`Accepted::PURPOSE`] | [`Accepted::signed_message`]：relay 编号、节点编号、随机数、有效期、地址 |
//!
//! ERROR 没有认证：它只让节点去重新握手或者换一个 relay，两件事节点自己都会限速。
//!
//! 帧的类型里 `0x02`～`0x07` 是握手和保活。ERROR 排在地址查询之后的 `0x0c`：这一段只有六个值，放不下第七种帧。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::node::NodeId;

pub const KIND_INIT: u8 = 0x02;
pub const KIND_CHALLENGE: u8 = 0x03;
pub const KIND_AUTH: u8 = 0x04;
pub const KIND_OK: u8 = 0x05;
pub const KIND_PING: u8 = 0x06;
pub const KIND_PONG: u8 = 0x07;
pub const KIND_ERROR: u8 = 0x0c;

/// 请求方生成的随机数，应答原样抄回。
pub type Nonce = [u8; NONCE_LEN];
/// relay 发给节点、节点在 AUTH 里带回来的地址证明。内容只有 relay 自己看得懂。
pub type Cookie = [u8; COOKIE_LEN];

pub const NONCE_LEN: usize = 8;
pub const COOKIE_LEN: usize = 16;
/// Ed25519 签名的长度。这个 crate 不依赖密码学库，所以签名在这里只是 64 个字节。
pub const SIGNATURE_LEN: usize = 64;

const FAMILY_V4: u8 = 4;
const FAMILY_V6: u8 = 6;
/// 地址编码之后最长多少字节（IPv6 时）。
pub(crate) const MAX_ADDR_LEN: usize = 1 + 16 + 2;

/// 把地址追加到 `buf` 末尾。IPv4 映射的 IPv6 地址按 IPv4 编码：双栈 socket 上收到的 IPv4 包就是这种形式。
pub(crate) fn put_addr(buf: &mut Vec<u8>, addr: SocketAddr) {
    match addr.ip().to_canonical() {
        IpAddr::V4(ip) => {
            buf.push(FAMILY_V4);
            buf.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            buf.push(FAMILY_V6);
            buf.extend_from_slice(&ip.octets());
        }
    }
    buf.extend_from_slice(&addr.port().to_be_bytes());
}

/// 从 `bytes` 开头取出一个地址，返回地址和剩下的字节。
pub(crate) fn take_addr(bytes: &[u8]) -> Option<(SocketAddr, &[u8])> {
    let (&family, rest) = bytes.split_first()?;
    let (ip, rest) = match family {
        FAMILY_V4 => {
            let (&octets, rest) = rest.split_first_chunk::<4>()?;
            (IpAddr::V4(Ipv4Addr::from(octets)), rest)
        }
        FAMILY_V6 => {
            let (&octets, rest) = rest.split_first_chunk::<16>()?;
            (IpAddr::V6(Ipv6Addr::from(octets)), rest)
        }
        _ => return None,
    };
    let (&port, rest) = rest.split_first_chunk::<2>()?;
    Some((SocketAddr::new(ip, u16::from_be_bytes(port)), rest))
}

/// 请求帧开头共同的部分：类型、节点编号、随机数。`kind` 对不上或者长度不够 `min_len` 时返回 `None`。
fn parse_request_head(datagram: &[u8], kind: u8, min_len: usize) -> Option<(NodeId, Nonce, &[u8])> {
    if datagram.len() < min_len {
        return None;
    }
    let (&first, rest) = datagram.split_first()?;
    if first != kind {
        return None;
    }
    let (&node, rest) = rest.split_first_chunk::<{ NodeId::WIRE_LEN }>()?;
    let (&nonce, rest) = rest.split_first_chunk::<NONCE_LEN>()?;
    Some((NodeId::from_be_bytes(node)?, nonce, rest))
}

fn request_head(bytes: &mut [u8], kind: u8, node: NodeId, nonce: &Nonce) {
    bytes[0] = kind;
    bytes[1..1 + NodeId::WIRE_LEN].copy_from_slice(&node.to_be_bytes());
    bytes[1 + NodeId::WIRE_LEN..REQUEST_HEAD_LEN].copy_from_slice(nonce);
}

const REQUEST_HEAD_LEN: usize = 1 + NodeId::WIRE_LEN + NONCE_LEN;

/// INIT：节点请 relay 发一个 cookie。relay 收到它不记任何状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Init {
    pub node: NodeId,
    pub nonce: Nonce,
}

impl Init {
    /// 帧的长度，含填充。比 CHALLENGE 长。
    pub const LEN: usize = 32;

    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        request_head(&mut bytes, KIND_INIT, self.node, &self.nonce);
        bytes
    }

    /// 不是一个完整的 INIT 时返回 `None`。比 [`Init::LEN`] 短的不认，否则应答就可能比请求大。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (node, nonce, _padding) = parse_request_head(datagram, KIND_INIT, Self::LEN)?;
        Some(Self { node, nonce })
    }
}

/// CHALLENGE：relay 对 INIT 的回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Challenge {
    /// 原样抄回 INIT 里的随机数。
    pub nonce: Nonce,
    pub cookie: Cookie,
}

impl Challenge {
    pub const LEN: usize = 1 + NONCE_LEN + COOKIE_LEN;

    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        bytes[0] = KIND_CHALLENGE;
        bytes[1..=NONCE_LEN].copy_from_slice(&self.nonce);
        bytes[1 + NONCE_LEN..].copy_from_slice(&self.cookie);
        bytes
    }

    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let bytes: &[u8; Self::LEN] = datagram.try_into().ok()?;
        let (&[KIND_CHALLENGE], rest) = bytes.split_first_chunk::<1>()? else { return None };
        let (&nonce, cookie) = rest.split_first_chunk::<NONCE_LEN>()?;
        Some(Self { nonce, cookie: *cookie.first_chunk::<COOKIE_LEN>()? })
    }
}

/// AUTH：节点带着 cookie 和签名来建会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Auth {
    pub node: NodeId,
    /// 和 INIT 里的是同一个随机数。
    pub nonce: Nonce,
    pub cookie: Cookie,
    /// 节点对 [`Auth::signed_message`] 的签名。
    pub signature: [u8; SIGNATURE_LEN],
}

impl Auth {
    /// 帧的长度，含填充。比最长的 OK 长。
    pub const LEN: usize = 96;
    /// 签名的用途标签。
    pub const PURPOSE: &str = "relay-session-auth";
    /// [`Auth::signed_message`] 的长度。
    pub const SIGNED_LEN: usize = 2 * NodeId::WIRE_LEN + NONCE_LEN + COOKIE_LEN;

    /// 节点签的消息。带上 relay 的编号，这个签名就不能拿到别的 relay 上去用；
    /// cookie 里有来源地址和时间片，所以也不能换一个地址或者隔很久再用。
    #[must_use]
    pub fn signed_message(relay: NodeId, node: NodeId, nonce: &Nonce, cookie: &Cookie) -> [u8; Self::SIGNED_LEN] {
        let mut message = [0; Self::SIGNED_LEN];
        let (ids, rest) = message.split_at_mut(2 * NodeId::WIRE_LEN);
        ids[..NodeId::WIRE_LEN].copy_from_slice(&relay.to_be_bytes());
        ids[NodeId::WIRE_LEN..].copy_from_slice(&node.to_be_bytes());
        rest[..NONCE_LEN].copy_from_slice(nonce);
        rest[NONCE_LEN..].copy_from_slice(cookie);
        message
    }

    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        request_head(&mut bytes, KIND_AUTH, self.node, &self.nonce);
        let cookie_end = REQUEST_HEAD_LEN + COOKIE_LEN;
        bytes[REQUEST_HEAD_LEN..cookie_end].copy_from_slice(&self.cookie);
        bytes[cookie_end..cookie_end + SIGNATURE_LEN].copy_from_slice(&self.signature);
        bytes
    }

    /// 不是一个完整的 AUTH 时返回 `None`。只看格式，cookie 和签名由 relay 去验。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (node, nonce, rest) = parse_request_head(datagram, KIND_AUTH, Self::LEN)?;
        let (&cookie, rest) = rest.split_first_chunk::<COOKIE_LEN>()?;
        Some(Self { node, nonce, cookie, signature: *rest.first_chunk::<SIGNATURE_LEN>()? })
    }
}

/// OK：会话建好了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    /// 原样抄回 AUTH 里的随机数。
    pub nonce: Nonce,
    /// 多少秒收不到 PING 之后 relay 删掉会话。
    pub valid_for: u16,
    /// relay 看到的节点地址。走 TCP 时是那条 TCP 的来源地址，不能当作 UDP 的候选端点。
    pub observed: SocketAddr,
    /// relay 对 [`Accepted::signed_message`] 的签名。
    pub signature: [u8; SIGNATURE_LEN],
}

impl Accepted {
    /// 帧最长多少字节（IPv6 地址时）。
    pub const MAX_LEN: usize = 1 + NONCE_LEN + 2 + MAX_ADDR_LEN + SIGNATURE_LEN;
    /// 签名的用途标签。
    pub const PURPOSE: &str = "relay-session-ok";

    /// relay 签的消息：帧里除了签名之外的全部内容，前面加上双方的编号。
    #[must_use]
    pub fn signed_message(relay: NodeId, node: NodeId, nonce: &Nonce, valid_for: u16, observed: SocketAddr) -> Vec<u8> {
        let mut message = Vec::with_capacity(2 * NodeId::WIRE_LEN + NONCE_LEN + 2 + MAX_ADDR_LEN);
        message.extend_from_slice(&relay.to_be_bytes());
        message.extend_from_slice(&node.to_be_bytes());
        message.extend_from_slice(nonce);
        message.extend_from_slice(&valid_for.to_be_bytes());
        put_addr(&mut message, observed);
        message
    }

    #[must_use]
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::MAX_LEN);
        bytes.push(KIND_OK);
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.valid_for.to_be_bytes());
        put_addr(&mut bytes, self.observed);
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    /// 不是一个完整的 OK 时返回 `None`。只看格式，签名由节点去验。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (&[KIND_OK], rest) = datagram.split_first_chunk::<1>()? else { return None };
        let (&nonce, rest) = rest.split_first_chunk::<NONCE_LEN>()?;
        let (&valid_for, rest) = rest.split_first_chunk::<2>()?;
        let (observed, rest) = take_addr(rest)?;
        let signature = *<&[u8; SIGNATURE_LEN]>::try_from(rest).ok()?;
        Some(Self { nonce, valid_for: u16::from_be_bytes(valid_for), observed, signature })
    }
}

/// PING：会话的保活。relay 只认从会话绑定的地址（或那条 TCP）来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    pub node: NodeId,
    pub nonce: Nonce,
}

impl Ping {
    /// 帧的长度，含填充。比最长的 PONG 长。
    pub const LEN: usize = 32;

    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        request_head(&mut bytes, KIND_PING, self.node, &self.nonce);
        bytes
    }

    /// 不是一个完整的 PING 时返回 `None`。比 [`Ping::LEN`] 短的不认。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (node, nonce, _padding) = parse_request_head(datagram, KIND_PING, Self::LEN)?;
        Some(Self { node, nonce })
    }
}

/// PONG：relay 对 PING 的回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pong {
    /// 原样抄回 PING 里的随机数。
    pub nonce: Nonce,
    /// relay 看到的节点地址。
    pub observed: SocketAddr,
}

impl Pong {
    /// 帧最长多少字节（IPv6 地址时）。
    pub const MAX_LEN: usize = 1 + NONCE_LEN + MAX_ADDR_LEN;

    #[must_use]
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::MAX_LEN);
        bytes.push(KIND_PONG);
        bytes.extend_from_slice(&self.nonce);
        put_addr(&mut bytes, self.observed);
        bytes
    }

    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (&[KIND_PONG], rest) = datagram.split_first_chunk::<1>()? else { return None };
        let (&nonce, rest) = rest.split_first_chunk::<NONCE_LEN>()?;
        let (observed, []) = take_addr(rest)? else { return None };
        Some(Self { nonce, observed })
    }
}

/// relay 拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefusalReason {
    /// 发来的帧自称属于某个节点，但这个节点在 relay 上没有会话，或者会话不是绑定在这个来源上的。
    /// 帧里的节点编号就是那个节点。收到的节点应该重新握手。
    NoSession,
    /// 要转发的包的目的节点在这个 relay 上没有会话。帧里的节点编号是目的节点。收到的节点应该换一个 relay。
    PeerUnattached,
}

impl RefusalReason {
    const fn code(self) -> u8 {
        match self {
            Self::NoSession => 1,
            Self::PeerUnattached => 2,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::NoSession),
            2 => Some(Self::PeerUnattached),
            _ => None,
        }
    }
}

/// ERROR：relay 没有照办。没有认证，relay 按来源地址限速地发。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    pub reason: RefusalReason,
    /// 和哪个节点有关，含义见 [`RefusalReason`]。
    pub node: NodeId,
}

impl Refused {
    pub const LEN: usize = 1 + 1 + NodeId::WIRE_LEN;

    #[must_use]
    pub const fn to_bytes(self) -> [u8; Self::LEN] {
        let [n0, n1, n2, n3] = self.node.to_be_bytes();
        [KIND_ERROR, self.reason.code(), n0, n1, n2, n3]
    }

    /// 不是一个完整的 ERROR，或者原因码不认识时返回 `None`。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let &[KIND_ERROR, code, n0, n1, n2, n3] = datagram else { return None };
        Some(Self { reason: RefusalReason::from_code(code)?, node: NodeId::from_be_bytes([n0, n1, n2, n3])? })
    }
}

/// 任意一种中转会话帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Init(Init),
    Challenge(Challenge),
    Auth(Auth),
    Ok(Accepted),
    Ping(Ping),
    Pong(Pong),
    Error(Refused),
}

impl Frame {
    /// 首字节是不是中转会话帧的类型。
    #[must_use]
    pub const fn is_session_kind(kind: u8) -> bool {
        matches!(kind, KIND_INIT..=KIND_PONG | KIND_ERROR)
    }

    /// 首字节不是中转会话帧的类型，或者帧不完整时返回 `None`。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        match *datagram.first()? {
            KIND_INIT => Init::parse(datagram).map(Self::Init),
            KIND_CHALLENGE => Challenge::parse(datagram).map(Self::Challenge),
            KIND_AUTH => Auth::parse(datagram).map(Self::Auth),
            KIND_OK => Accepted::parse(datagram).map(Self::Ok),
            KIND_PING => Ping::parse(datagram).map(Self::Ping),
            KIND_PONG => Pong::parse(datagram).map(Self::Pong),
            KIND_ERROR => Refused::parse(datagram).map(Self::Error),
            _ => None,
        }
    }

    /// 这一帧是节点发给 relay 的（由 relay 处理），还是 relay 发给节点的（由路径层处理）。
    #[must_use]
    pub const fn is_request(&self) -> bool {
        match self {
            Self::Init(_) | Self::Auth(_) | Self::Ping(_) => true,
            Self::Challenge(_) | Self::Ok(_) | Self::Pong(_) | Self::Error(_) => false,
        }
    }
}

const _: () = {
    // 应答不比请求大。
    assert!(Challenge::LEN <= Init::LEN);
    assert!(Accepted::MAX_LEN <= Auth::LEN);
    assert!(Pong::MAX_LEN <= Ping::LEN);
    // ERROR 由 PING 或者一个中转帧（最短就是一个中转头）引出。
    assert!(Refused::LEN <= crate::datagram::RelayHeader::LEN);
    assert!(Refused::LEN <= Ping::LEN);
    // 填充之前的内容装得进规定的长度。
    assert!(REQUEST_HEAD_LEN <= Init::LEN && REQUEST_HEAD_LEN <= Ping::LEN);
    assert!(REQUEST_HEAD_LEN + COOKIE_LEN + SIGNATURE_LEN <= Auth::LEN);
};

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: Nonce = [1, 2, 3, 4, 5, 6, 7, 8];
    const COOKIE: Cookie = [0xc0; COOKIE_LEN];
    const SIGNATURE: [u8; SIGNATURE_LEN] = [0x5a; SIGNATURE_LEN];

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn v4() -> SocketAddr {
        "203.0.113.7:20330".parse().unwrap()
    }

    fn v6() -> SocketAddr {
        "[2001:db8::1]:443".parse().unwrap()
    }

    #[test]
    fn init_has_the_documented_layout_and_round_trips() {
        let init = Init { node: node(0x0a0b_0c0d), nonce: NONCE };
        let bytes = init.to_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..13], [0x02, 0x0a, 0x0b, 0x0c, 0x0d, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(bytes[13..].iter().all(|&byte| byte == 0));
        assert_eq!(Init::parse(&bytes), Some(init));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Init(init)));
    }

    #[test]
    fn challenge_has_the_documented_layout_and_round_trips() {
        let challenge = Challenge { nonce: NONCE, cookie: COOKIE };
        let bytes = challenge.to_bytes();
        assert_eq!(bytes.len(), 25);
        assert_eq!(&bytes[..9], [0x03, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&bytes[9..], COOKIE);
        assert_eq!(Challenge::parse(&bytes), Some(challenge));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Challenge(challenge)));
    }

    #[test]
    fn auth_has_the_documented_layout_and_round_trips() {
        let auth = Auth { node: node(7), nonce: NONCE, cookie: COOKIE, signature: SIGNATURE };
        let bytes = auth.to_bytes();
        assert_eq!(bytes.len(), 96);
        assert_eq!(&bytes[..13], [0x04, 0, 0, 0, 7, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&bytes[13..29], COOKIE);
        assert_eq!(&bytes[29..93], SIGNATURE);
        assert_eq!(&bytes[93..], [0, 0, 0]);
        assert_eq!(Auth::parse(&bytes), Some(auth));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Auth(auth)));
    }

    #[test]
    fn ok_has_the_documented_layout_and_round_trips_for_both_address_families() {
        let ok = Accepted { nonce: NONCE, valid_for: 0x0102, observed: v4(), signature: SIGNATURE };
        let bytes = ok.to_bytes();
        assert_eq!(&bytes[..11], [0x05, 1, 2, 3, 4, 5, 6, 7, 8, 0x01, 0x02]);
        assert_eq!(&bytes[11..18], [4, 203, 0, 113, 7, 0x4f, 0x6a]);
        assert_eq!(&bytes[18..], SIGNATURE);
        assert_eq!(Accepted::parse(&bytes), Some(ok));

        let ok = Accepted { observed: v6(), ..ok };
        let bytes = ok.to_bytes();
        assert_eq!(bytes.len(), Accepted::MAX_LEN);
        assert_eq!(bytes[11], 6);
        assert_eq!(Accepted::parse(&bytes), Some(ok));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Ok(ok)));
    }

    #[test]
    fn ping_and_pong_have_the_documented_layout_and_round_trip() {
        let ping = Ping { node: node(9), nonce: NONCE };
        let bytes = ping.to_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..13], [0x06, 0, 0, 0, 9, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(bytes[13..].iter().all(|&byte| byte == 0));
        assert_eq!(Ping::parse(&bytes), Some(ping));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Ping(ping)));

        for observed in [v4(), v6()] {
            let pong = Pong { nonce: NONCE, observed };
            let bytes = pong.to_bytes();
            assert_eq!(&bytes[..9], [0x07, 1, 2, 3, 4, 5, 6, 7, 8]);
            assert_eq!(Pong::parse(&bytes), Some(pong));
            assert_eq!(Frame::parse(&bytes), Some(Frame::Pong(pong)));
        }
        assert_eq!(Pong { nonce: NONCE, observed: v6() }.to_bytes().len(), Pong::MAX_LEN);
    }

    #[test]
    fn error_has_the_documented_layout_and_round_trips() {
        let refused = Refused { reason: RefusalReason::PeerUnattached, node: node(0x0102_0304) };
        assert_eq!(refused.to_bytes(), [0x0c, 2, 1, 2, 3, 4]);
        assert_eq!(Refused::parse(&refused.to_bytes()), Some(refused));
        let no_session = Refused { reason: RefusalReason::NoSession, node: node(5) };
        assert_eq!(no_session.to_bytes(), [0x0c, 1, 0, 0, 0, 5]);
        assert_eq!(Frame::parse(&no_session.to_bytes()), Some(Frame::Error(no_session)));
        assert_eq!(Refused::parse(&[0x0c, 9, 0, 0, 0, 5]), None, "不认识的原因码");
        assert_eq!(Refused::parse(&[0x0c, 1, 0, 0, 0, 0]), None, "节点编号是 0");
    }

    #[test]
    fn every_reply_fits_inside_the_request_that_can_trigger_it() {
        let challenge = Challenge { nonce: NONCE, cookie: COOKIE }.to_bytes();
        assert!(challenge.len() <= Init { node: node(1), nonce: NONCE }.to_bytes().len());

        let auth = Auth { node: node(1), nonce: NONCE, cookie: COOKIE, signature: SIGNATURE }.to_bytes();
        let ping = Ping { node: node(1), nonce: NONCE }.to_bytes();
        let error = Refused { reason: RefusalReason::NoSession, node: node(1) }.to_bytes();
        for observed in [v4(), v6()] {
            let ok = Accepted { nonce: NONCE, valid_for: 30, observed, signature: SIGNATURE }.to_bytes();
            assert!(ok.len() <= auth.len());
            assert!(Pong { nonce: NONCE, observed }.to_bytes().len() <= ping.len());
        }
        // ERROR 由 PING 或者一个中转帧引出，最短的中转帧只有一个中转头。
        assert!(error.len() <= ping.len());
        assert!(error.len() <= crate::datagram::RelayHeader::LEN);
    }

    #[test]
    fn requests_shorter_than_their_padded_length_are_not_recognised() {
        let init = Init { node: node(1), nonce: NONCE }.to_bytes();
        let auth = Auth { node: node(1), nonce: NONCE, cookie: COOKIE, signature: SIGNATURE }.to_bytes();
        let ping = Ping { node: node(1), nonce: NONCE }.to_bytes();
        for len in 0..Init::LEN {
            assert_eq!(Init::parse(&init[..len]), None);
            assert_eq!(Ping::parse(&ping[..len]), None);
        }
        for len in 0..Auth::LEN {
            assert_eq!(Auth::parse(&auth[..len]), None);
        }
        assert_eq!(Frame::parse(&init[..Init::LEN - 1]), None);
    }

    #[test]
    fn malformed_replies_are_not_recognised() {
        let challenge = Challenge { nonce: NONCE, cookie: COOKIE }.to_bytes();
        assert_eq!(Challenge::parse(&challenge[..24]), None);
        assert_eq!(Challenge::parse(&[&challenge[..], &[0]].concat()), None);

        let ok = Accepted { nonce: NONCE, valid_for: 30, observed: v4(), signature: SIGNATURE }.to_bytes();
        assert_eq!(Accepted::parse(&ok[..ok.len() - 1]), None);
        assert_eq!(Accepted::parse(&[&ok[..], &[0]].concat()), None);
        let mut bad_family = ok;
        bad_family[11] = 5;
        assert_eq!(Accepted::parse(&bad_family), None);

        let pong = Pong { nonce: NONCE, observed: v6() }.to_bytes();
        assert_eq!(Pong::parse(&pong[..pong.len() - 1]), None);
        assert_eq!(Pong::parse(&[&pong[..], &[0]].concat()), None);
    }

    #[test]
    fn frames_with_a_zero_node_id_or_a_foreign_kind_are_not_recognised() {
        let mut init = Init { node: node(1), nonce: NONCE }.to_bytes();
        init[1..5].fill(0);
        assert_eq!(Init::parse(&init), None);
        let ping = Ping { node: node(1), nonce: NONCE }.to_bytes();
        assert_eq!(Init::parse(&ping), None, "PING 不是 INIT，虽然长得一样");
        assert_eq!(Frame::parse(&[]), None);
        assert_eq!(Frame::parse(&[0x08; 32]), None);
    }

    #[test]
    fn mapped_ipv4_is_reported_as_ipv4() {
        let pong = Pong { nonce: NONCE, observed: "[::ffff:203.0.113.7]:5000".parse().unwrap() };
        assert_eq!(Pong::parse(&pong.to_bytes()).unwrap().observed, "203.0.113.7:5000".parse().unwrap());
    }

    #[test]
    fn auth_signature_covers_relay_node_nonce_and_cookie() {
        let message = Auth::signed_message(node(0x0102_0304), node(7), &NONCE, &COOKIE);
        assert_eq!(&message[..4], [1, 2, 3, 4]);
        assert_eq!(&message[4..8], [0, 0, 0, 7]);
        assert_eq!(&message[8..16], NONCE);
        assert_eq!(&message[16..], COOKIE);
        assert_ne!(message, Auth::signed_message(node(9), node(7), &NONCE, &COOKIE), "换一个 relay 签名就不同");
    }

    #[test]
    fn ok_signature_covers_both_ids_and_the_whole_frame_body() {
        let message = Accepted::signed_message(node(3), node(7), &NONCE, 30, v4());
        assert_eq!(&message[..8], [0, 0, 0, 3, 0, 0, 0, 7]);
        assert_eq!(&message[8..16], NONCE);
        assert_eq!(&message[16..18], [0, 30]);
        assert_eq!(&message[18..], [4, 203, 0, 113, 7, 0x4f, 0x6a]);
        for other in [
            Accepted::signed_message(node(3), node(8), &NONCE, 30, v4()),
            Accepted::signed_message(node(3), node(7), &[9; 8], 30, v4()),
            Accepted::signed_message(node(3), node(7), &NONCE, 31, v4()),
            Accepted::signed_message(node(3), node(7), &NONCE, 30, v6()),
        ] {
            assert_ne!(message, other);
        }
    }

    #[test]
    fn requests_and_replies_are_told_apart() {
        let request = Frame::Ping(Ping { node: node(1), nonce: NONCE });
        let reply = Frame::Pong(Pong { nonce: NONCE, observed: v4() });
        assert!(request.is_request() && !reply.is_request());
        assert!(Frame::is_session_kind(0x02) && Frame::is_session_kind(0x07) && Frame::is_session_kind(0x0c));
        assert!(!Frame::is_session_kind(0x01) && !Frame::is_session_kind(0x08) && !Frame::is_session_kind(0x0b));
    }
}
