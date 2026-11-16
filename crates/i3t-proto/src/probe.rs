//! 探测帧（节点和节点之间的直连验证、打洞、测延迟）。见 docs/architecture.md §5.3。
//!
//! ```text
//! A → B  PING  0x08 | 来源 A 4 | 目的 B 4 | 随机数 8 | 签名 64 | 填充到 100 字节
//! B → A  PONG  0x09 | 来源 B 4 | 目的 A 4 | 随机数 8 | 地址 7 或 19 | 签名 64            最长 100 字节
//! ```
//!
//! 整数都是大端。地址是 B 看到的这个 PING 的来源地址，编码和中转会话帧里的一样（[`crate::session`]）。
//!
//! A 向 B 的某个候选地址发 PING，收到带着同一个随机数、签名正确的 PONG，这个地址才算 B 的已验证直连地址。
//!
//! # 对没有认证过的来源
//!
//! - **应答不比请求大。** PING 带填充，和最长的 PONG 一样长；比规定长度短的 PING 不认。
//! - 每个探测帧都带发送方的签名，所以收的一方要先验签名才知道该不该理。验签名不便宜，
//!   实现要先用不花钱的条件筛一遍（目的是不是自己、来源是不是 netmap 里的对端），并且对陌生地址来的 PING 限速。
//! - 收的一方不因为一个 PING 记下任何长期状态：PONG 里要的东西都在 PING 里。
//!
//! # 签的是什么
//!
//! 签名由调用方用 `i3t_crypto::NodeKey::sign(用途, 消息)` 来算，这里只规定消息的字节：
//!
//! | 帧 | 用途标签 | 消息 |
//! |---|---|---|
//! | PING | [`Ping::PURPOSE`] | [`Ping::signed_message`]：来源、目的、随机数 |
//! | PONG | [`Pong::PURPOSE`] | [`Pong::signed_message`]：来源、目的、随机数、地址 |
//!
//! 消息里有目的节点，所以发给 B 的探测帧拿去给 C 是没用的。重放一个旧的 PING 只能让 B 再回一个不比它大的 PONG；
//! 重放 PONG 没有用，因为随机数对不上。

use std::net::SocketAddr;

use crate::node::NodeId;
use crate::session::{self, MAX_ADDR_LEN, NONCE_LEN, Nonce, SIGNATURE_LEN};

pub const KIND_PING: u8 = 0x08;
pub const KIND_PONG: u8 = 0x09;

/// 类型、来源、目的、随机数。
const HEAD_LEN: usize = 1 + 2 * NodeId::WIRE_LEN + NONCE_LEN;

struct Head {
    src: NodeId,
    dst: NodeId,
    nonce: Nonce,
}

impl Head {
    fn put(&self, kind: u8, buf: &mut Vec<u8>) {
        buf.push(kind);
        buf.extend_from_slice(&self.src.to_be_bytes());
        buf.extend_from_slice(&self.dst.to_be_bytes());
        buf.extend_from_slice(&self.nonce);
    }

    fn take(datagram: &[u8], kind: u8) -> Option<(Self, &[u8])> {
        let (&first, rest) = datagram.split_first()?;
        if first != kind {
            return None;
        }
        let (&src, rest) = rest.split_first_chunk::<{ NodeId::WIRE_LEN }>()?;
        let (&dst, rest) = rest.split_first_chunk::<{ NodeId::WIRE_LEN }>()?;
        let (&nonce, rest) = rest.split_first_chunk::<NONCE_LEN>()?;
        Some((Self { src: NodeId::from_be_bytes(src)?, dst: NodeId::from_be_bytes(dst)?, nonce }, rest))
    }

    fn signed_message(&self, extra_capacity: usize) -> Vec<u8> {
        let mut message = Vec::with_capacity(HEAD_LEN - 1 + extra_capacity);
        message.extend_from_slice(&self.src.to_be_bytes());
        message.extend_from_slice(&self.dst.to_be_bytes());
        message.extend_from_slice(&self.nonce);
        message
    }
}

/// PING：`src` 问 `dst`"你在这个地址上吗"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    pub src: NodeId,
    pub dst: NodeId,
    pub nonce: Nonce,
    /// `src` 对 [`Ping::signed_message`] 的签名。
    pub signature: [u8; SIGNATURE_LEN],
}

impl Ping {
    /// 帧的长度，含填充。和最长的 PONG 一样长。
    pub const LEN: usize = Pong::MAX_LEN;
    /// 签名的用途标签。
    pub const PURPOSE: &str = "probe-ping";

    #[must_use]
    pub fn signed_message(src: NodeId, dst: NodeId, nonce: &Nonce) -> Vec<u8> {
        Head { src, dst, nonce: *nonce }.signed_message(0)
    }

    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        let (head, rest) = bytes.split_at_mut(HEAD_LEN);
        head[0] = KIND_PING;
        head[1..1 + NodeId::WIRE_LEN].copy_from_slice(&self.src.to_be_bytes());
        head[1 + NodeId::WIRE_LEN..1 + 2 * NodeId::WIRE_LEN].copy_from_slice(&self.dst.to_be_bytes());
        head[1 + 2 * NodeId::WIRE_LEN..].copy_from_slice(&self.nonce);
        rest[..SIGNATURE_LEN].copy_from_slice(&self.signature);
        bytes
    }

    /// 不是一个完整的 PING 时返回 `None`。比 [`Ping::LEN`] 短的不认，否则应答就可能比请求大。
    /// 只看格式，签名由收的一方去验。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        if datagram.len() < Self::LEN {
            return None;
        }
        let (head, rest) = Head::take(datagram, KIND_PING)?;
        Some(Self { src: head.src, dst: head.dst, nonce: head.nonce, signature: *rest.first_chunk::<SIGNATURE_LEN>()? })
    }
}

/// PONG：`src` 回答 `dst` 的 PING。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pong {
    /// 回答的一方，也就是 PING 的目的节点。
    pub src: NodeId,
    /// PING 的来源节点。
    pub dst: NodeId,
    /// 原样抄回 PING 里的随机数。
    pub nonce: Nonce,
    /// `src` 看到的 PING 的来源地址，`dst` 靠它知道自己在 NAT 外面的地址。
    pub observed: SocketAddr,
    /// `src` 对 [`Pong::signed_message`] 的签名。
    pub signature: [u8; SIGNATURE_LEN],
}

impl Pong {
    /// 帧最长多少字节（IPv6 地址时）。
    pub const MAX_LEN: usize = HEAD_LEN + MAX_ADDR_LEN + SIGNATURE_LEN;
    /// 签名的用途标签。
    pub const PURPOSE: &str = "probe-pong";

    #[must_use]
    pub fn signed_message(src: NodeId, dst: NodeId, nonce: &Nonce, observed: SocketAddr) -> Vec<u8> {
        let mut message = Head { src, dst, nonce: *nonce }.signed_message(MAX_ADDR_LEN);
        session::put_addr(&mut message, observed);
        message
    }

    #[must_use]
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::MAX_LEN);
        Head { src: self.src, dst: self.dst, nonce: self.nonce }.put(KIND_PONG, &mut bytes);
        session::put_addr(&mut bytes, self.observed);
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    /// 不是一个完整的 PONG 时返回 `None`。只看格式，签名和随机数由收的一方去核对。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (head, rest) = Head::take(datagram, KIND_PONG)?;
        let (observed, rest) = session::take_addr(rest)?;
        let signature = *<&[u8; SIGNATURE_LEN]>::try_from(rest).ok()?;
        Some(Self { src: head.src, dst: head.dst, nonce: head.nonce, observed, signature })
    }
}

/// 任意一种探测帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Ping(Ping),
    Pong(Pong),
}

impl Frame {
    /// 首字节不是探测帧的类型，或者帧不完整时返回 `None`。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        match *datagram.first()? {
            KIND_PING => Ping::parse(datagram).map(Self::Ping),
            KIND_PONG => Pong::parse(datagram).map(Self::Pong),
            _ => None,
        }
    }
}

const _: () = {
    assert!(Pong::MAX_LEN <= Ping::LEN);
    assert!(HEAD_LEN + SIGNATURE_LEN <= Ping::LEN);
};

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: Nonce = [1, 2, 3, 4, 5, 6, 7, 8];
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

    fn ping() -> Ping {
        Ping { src: node(0x0a0b_0c0d), dst: node(2), nonce: NONCE, signature: SIGNATURE }
    }

    #[test]
    fn ping_has_the_documented_layout_and_round_trips() {
        let bytes = ping().to_bytes();
        assert_eq!(bytes.len(), 100);
        assert_eq!(&bytes[..17], [0x08, 0x0a, 0x0b, 0x0c, 0x0d, 0, 0, 0, 2, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&bytes[17..81], SIGNATURE);
        assert!(bytes[81..].iter().all(|&byte| byte == 0));
        assert_eq!(Ping::parse(&bytes), Some(ping()));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Ping(ping())));
    }

    #[test]
    fn pong_has_the_documented_layout_and_round_trips_for_both_address_families() {
        let pong = Pong { src: node(2), dst: node(1), nonce: NONCE, observed: v4(), signature: SIGNATURE };
        let bytes = pong.to_bytes();
        assert_eq!(&bytes[..17], [0x09, 0, 0, 0, 2, 0, 0, 0, 1, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&bytes[17..24], [4, 203, 0, 113, 7, 0x4f, 0x6a]);
        assert_eq!(&bytes[24..], SIGNATURE);
        assert_eq!(Pong::parse(&bytes), Some(pong));

        let pong = Pong { observed: v6(), ..pong };
        let bytes = pong.to_bytes();
        assert_eq!(bytes.len(), Pong::MAX_LEN);
        assert_eq!(Pong::parse(&bytes), Some(pong));
        assert_eq!(Frame::parse(&bytes), Some(Frame::Pong(pong)));
    }

    #[test]
    fn pong_is_never_larger_than_ping() {
        let ping_len = ping().to_bytes().len();
        for observed in [v4(), v6()] {
            let pong = Pong { src: node(2), dst: node(1), nonce: NONCE, observed, signature: SIGNATURE };
            assert!(pong.to_bytes().len() <= ping_len);
        }
    }

    #[test]
    fn a_ping_shorter_than_its_padded_length_is_not_recognised() {
        let bytes = ping().to_bytes();
        for len in 0..Ping::LEN {
            assert_eq!(Ping::parse(&bytes[..len]), None, "{len}");
        }
    }

    #[test]
    fn malformed_frames_are_not_recognised() {
        let pong = Pong { src: node(2), dst: node(1), nonce: NONCE, observed: v4(), signature: SIGNATURE }.to_bytes();
        assert_eq!(Pong::parse(&pong[..pong.len() - 1]), None);
        assert_eq!(Pong::parse(&[&pong[..], &[0]].concat()), None);
        let mut bad_family = pong.clone();
        bad_family[17] = 5;
        assert_eq!(Pong::parse(&bad_family), None);
        let mut zero_src = pong;
        zero_src[1..5].fill(0);
        assert_eq!(Pong::parse(&zero_src), None);

        let mut zero_dst = ping().to_bytes();
        zero_dst[5..9].fill(0);
        assert_eq!(Ping::parse(&zero_dst), None);
        assert_eq!(Ping::parse(&[0x09; 100]), None);
        assert_eq!(Frame::parse(&[]), None);
        assert_eq!(Frame::parse(&[0x07; 100]), None);
    }

    #[test]
    fn signatures_cover_direction_nonce_and_observed_address() {
        let message = Ping::signed_message(node(1), node(2), &NONCE);
        assert_eq!(message, [0, 0, 0, 1, 0, 0, 0, 2, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_ne!(message, Ping::signed_message(node(2), node(1), &NONCE), "方向反过来签名就不同");

        let message = Pong::signed_message(node(2), node(1), &NONCE, v4());
        assert_eq!(&message[..16], [0, 0, 0, 2, 0, 0, 0, 1, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&message[16..], [4, 203, 0, 113, 7, 0x4f, 0x6a]);
        assert_ne!(message, Pong::signed_message(node(2), node(1), &NONCE, v6()));
        assert_ne!(message, Pong::signed_message(node(2), node(1), &[9; 8], v4()));
    }
}
