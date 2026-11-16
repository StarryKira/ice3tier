//! 一个端口上收到的 UDP 数据报怎么分类，以及经 relay 转发时加在包前面的中转头。
//!
//! 节点只用一个 UDP 端口。端到端连接的包和路径层自己的帧都从这里进来，靠首字节区分：
//!
//! - QUIC 包的首字节一定带 `0x40` 这一位（RFC 9000 §17.2、§17.3 的 Fixed Bit）。带这一位的数据报是对端直连发来的连接包，
//!   整个交给连接层。这要求两端都不启用 RFC 9287 的"把这一位随机化"。
//! - 不带这一位的是路径层的帧，首字节是帧的类型，取值在 `0x00`～`0x3f` 之间。
//!   `0x80`～`0xbf` 不分配给任何帧：QUIC 的版本协商包可能落在这一段（它的这一位是随机填的）。
//!   两端的 QUIC 版本相同时不会有这种包，收到就按不认识的帧丢掉。
//!
//! 帧的类型：
//!
//! | 首字节 | 帧 | 格式在哪里 |
//! |---|---|---|
//! | `0x01` | 经 relay 转发的包 | 本模块 |
//! | `0x02`～`0x07`、`0x0c` | 中转会话帧 | [`crate::session`] |
//! | `0x08`、`0x09` | 探测帧 | [`crate::probe`] |
//! | `0x0a`、`0x0b` | 地址查询与应答 | [`crate::addr`] |
//!
//! [`Datagram::parse`] 一次调用就把上面这些全部分出来。其余的首字节不认识，数据报直接丢掉，不回应。
//!
//! 经 relay 转发的包：
//!
//! ```text
//! 0      1              5              9
//! +------+--------------+--------------+------------------
//! | 0x01 | 目的节点编号 | 来源节点编号 | 连接层的包 ...
//! +------+--------------+--------------+------------------
//! ```
//!
//! relay 只看前 9 个字节：按目的节点查出它挂上来的地址，把整个数据报原样转出去，不改一个字节。
//! 来源节点编号是给目的节点看的，它据此知道这个包属于和谁的连接。relay 要核对来源节点就是这个数据报所属的中转会话的主人，
//! 不然任何节点都能冒别人的名字往外发包。

use crate::node::NodeId;
use crate::{addr, probe, session};

/// 连接层的一个包最长多少字节。
///
/// 直连和经 relay 这两条路必须装得下同样大的包。连接是按当前那条路探测最大包长的，
/// 如果直连能发的包比经 relay 的大，换到 relay 那条路之后大包就发不出去了。所以两条路都按"加上中转头"来算：
/// 1400 加中转头、UDP 头和 IPv6 头是 1457 字节，以太网（1500）和 PPPoE（1492）都装得下。
/// 更小的链路靠连接层自己的路径 MTU 探测，它从 1200 起步往上试，这里是它的上限。
pub const MAX_CONN_PACKET_LEN: u16 = 1400;

const QUIC_FIXED_BIT: u8 = 0x40;
const KIND_RELAYED: u8 = 0x01;

const _: () = {
    const UDP_HEADER: usize = 8;
    const IPV6_HEADER: usize = 40;
    const PPPOE_MTU: usize = 1492;
    assert!(MAX_CONN_PACKET_LEN as usize + RelayHeader::LEN + UDP_HEADER + IPV6_HEADER <= PPPOE_MTU);
    // 帧的类型不能和 QUIC 包的首字节混淆。
    assert!(KIND_RELAYED < QUIC_FIXED_BIT);
    assert!(session::KIND_ERROR < QUIC_FIXED_BIT && probe::KIND_PONG < QUIC_FIXED_BIT && addr::KIND_REPLY < QUIC_FIXED_BIT);
};

/// 经 relay 转发的包前面的头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayHeader {
    /// 包要送到哪个节点。
    pub dst: NodeId,
    /// 包是哪个节点发出的。
    pub src: NodeId,
}

impl RelayHeader {
    /// 编码之后占的字节数：类型 1 字节，两个节点编号各 4 字节。
    pub const LEN: usize = 1 + 2 * NodeId::WIRE_LEN;

    #[must_use]
    pub const fn to_bytes(self) -> [u8; Self::LEN] {
        let [d0, d1, d2, d3] = self.dst.to_be_bytes();
        let [s0, s1, s2, s3] = self.src.to_be_bytes();
        [KIND_RELAYED, d0, d1, d2, d3, s0, s1, s2, s3]
    }

    /// `bytes` 的首字节已经确认是 [`KIND_RELAYED`]。
    const fn from_bytes(bytes: &[u8; Self::LEN]) -> Result<Self, DatagramError> {
        let [_, d0, d1, d2, d3, s0, s1, s2, s3] = *bytes;
        match (NodeId::from_be_bytes([d0, d1, d2, d3]), NodeId::from_be_bytes([s0, s1, s2, s3])) {
            (Some(dst), Some(src)) => Ok(Self { dst, src }),
            _ => Err(DatagramError::ZeroNodeId),
        }
    }
}

/// 一个 UDP 数据报（或 TCP 中转里的一帧）解析出来的内容。
///
/// 带着包的两种借用原来的缓冲区，不拷贝；其余几种是路径层自己的帧，都很短，解析成值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datagram<'a> {
    /// 对端直连发来的连接包，就是整个数据报。
    Conn(&'a [u8]),
    /// 经 relay 转发的包。`packet` 是去掉中转头之后的连接包。
    Relayed { header: RelayHeader, packet: &'a [u8] },
    /// 中转会话帧。[`session::Frame::is_request`] 为真的归 relay 处理，其余是 relay 的应答，归路径层。
    Session(session::Frame),
    /// 探测帧，归路径层。
    Probe(probe::Frame),
    /// 地址查询。
    AddrQuery(addr::Query),
    /// 地址查询的应答。
    AddrReply(addr::Reply),
}

impl<'a> Datagram<'a> {
    /// # Errors
    ///
    /// 数据报是空的、帧的类型不认识、帧不完整或者里面的节点编号是 0。
    /// 这些数据报都应该直接丢掉，不回应。
    pub fn parse(datagram: &'a [u8]) -> Result<Self, DatagramError> {
        let &first = datagram.first().ok_or(DatagramError::Empty)?;
        if first & QUIC_FIXED_BIT != 0 {
            return Ok(Self::Conn(datagram));
        }
        let malformed = DatagramError::Malformed { kind: first };
        match first {
            KIND_RELAYED => {
                let (head, packet) = datagram
                    .split_first_chunk::<{ RelayHeader::LEN }>()
                    .ok_or(DatagramError::TruncatedRelayHeader { len: datagram.len() })?;
                Ok(Self::Relayed { header: RelayHeader::from_bytes(head)?, packet })
            }
            kind if session::Frame::is_session_kind(kind) => session::Frame::parse(datagram).map(Self::Session).ok_or(malformed),
            probe::KIND_PING | probe::KIND_PONG => probe::Frame::parse(datagram).map(Self::Probe).ok_or(malformed),
            addr::KIND_QUERY => addr::Query::parse(datagram).map(Self::AddrQuery).ok_or(malformed),
            addr::KIND_REPLY => addr::Reply::parse(datagram).map(Self::AddrReply).ok_or(malformed),
            kind => Err(DatagramError::UnknownKind(kind)),
        }
    }
}

/// 只取出一个经 relay 转发的数据报要去的节点，别的都不看。不是这种数据报时返回 `None`。
///
/// relay 用它决定往哪里转。内核一次交上来的一批数据报可能来自不同的节点、发往不同的节点，
/// 所以要对每一个数据报调用，不能看了第一个就把整批转给同一个目的地。
#[must_use]
pub fn relayed_dst(datagram: &[u8]) -> Option<NodeId> {
    match datagram.split_first_chunk::<{ 1 + NodeId::WIRE_LEN }>()? {
        (&[KIND_RELAYED, d0, d1, d2, d3], _) => NodeId::from_be_bytes([d0, d1, d2, d3]),
        _ => None,
    }
}

/// 数据报解析不了的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DatagramError {
    #[error("数据报是空的")]
    Empty,
    #[error("不认识的帧类型 {0:#04x}")]
    UnknownKind(u8),
    #[error("中转头要 {} 字节，整个数据报只有 {len} 字节", RelayHeader::LEN)]
    TruncatedRelayHeader { len: usize },
    #[error("中转头里的节点编号是 0")]
    ZeroNodeId,
    #[error("类型是 {kind:#04x} 的帧不完整，或者内容不合格式")]
    Malformed { kind: u8 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn relayed(dst: u32, src: u32, packet: &[u8]) -> Vec<u8> {
        let mut datagram = RelayHeader { dst: node(dst), src: node(src) }.to_bytes().to_vec();
        datagram.extend_from_slice(packet);
        datagram
    }

    #[test]
    fn packets_with_the_quic_fixed_bit_go_to_the_connection_untouched() {
        for first in [0x40, 0x7f, 0xc0, 0xff] {
            let datagram = [first, 1, 2, 3];
            assert_eq!(Datagram::parse(&datagram), Ok(Datagram::Conn(&datagram)));
            assert_eq!(relayed_dst(&datagram), None);
        }
    }

    #[test]
    fn relayed_frame_has_the_documented_layout() {
        let datagram = relayed(2, 0x0a0b_0c0d, b"quic");
        assert_eq!(datagram, [0x01, 0, 0, 0, 2, 0x0a, 0x0b, 0x0c, 0x0d, b'q', b'u', b'i', b'c']);
    }

    #[test]
    fn relayed_frame_round_trips() {
        let datagram = relayed(7, 9, b"payload");
        let expected = Datagram::Relayed { header: RelayHeader { dst: node(7), src: node(9) }, packet: b"payload" };
        assert_eq!(Datagram::parse(&datagram), Ok(expected));
        assert_eq!(relayed_dst(&datagram), Some(node(7)));
    }

    #[test]
    fn relayed_frame_may_carry_an_empty_packet() {
        let datagram = relayed(7, 9, b"");
        assert!(matches!(Datagram::parse(&datagram), Ok(Datagram::Relayed { packet: [], .. })));
    }

    #[test]
    fn truncated_relay_header_is_rejected() {
        let datagram = relayed(7, 9, b"");
        for len in 1..RelayHeader::LEN {
            assert_eq!(Datagram::parse(&datagram[..len]), Err(DatagramError::TruncatedRelayHeader { len }));
        }
        // 目的节点那 4 个字节不全时，relay 的快速路径也不能给出结果。
        for len in 0..=NodeId::WIRE_LEN {
            assert_eq!(relayed_dst(&datagram[..len]), None);
        }
    }

    #[test]
    fn zero_node_ids_are_rejected() {
        assert_eq!(Datagram::parse(&[0x01, 0, 0, 0, 0, 0, 0, 0, 9]), Err(DatagramError::ZeroNodeId));
        assert_eq!(Datagram::parse(&[0x01, 0, 0, 0, 7, 0, 0, 0, 0]), Err(DatagramError::ZeroNodeId));
        assert_eq!(relayed_dst(&[0x01, 0, 0, 0, 0, 0, 0, 0, 9]), None);
    }

    #[test]
    fn empty_and_unknown_datagrams_are_rejected() {
        assert_eq!(Datagram::parse(&[]), Err(DatagramError::Empty));
        for kind in [0x00, 0x0d, 0x3f, 0x80, 0xbf] {
            assert_eq!(Datagram::parse(&[kind, 0, 0, 0, 1, 0, 0, 0, 2]), Err(DatagramError::UnknownKind(kind)));
            assert_eq!(relayed_dst(&[kind, 0, 0, 0, 1, 0, 0, 0, 2]), None);
        }
    }

    #[test]
    fn session_frames_are_classified_by_their_first_byte() {
        let nonce = [7; session::NONCE_LEN];
        let observed = "203.0.113.7:20330".parse().unwrap();
        let frames = [
            session::Frame::Init(session::Init { node: node(1), nonce }),
            session::Frame::Challenge(session::Challenge { nonce, cookie: [3; session::COOKIE_LEN] }),
            session::Frame::Auth(session::Auth { node: node(1), nonce, cookie: [3; 16], signature: [4; 64] }),
            session::Frame::Ok(session::Accepted { nonce, valid_for: 30, observed, signature: [4; 64] }),
            session::Frame::Ping(session::Ping { node: node(1), nonce }),
            session::Frame::Pong(session::Pong { nonce, observed }),
            session::Frame::Error(session::Refused { reason: session::RefusalReason::NoSession, node: node(1) }),
        ];
        for frame in frames {
            let bytes = match frame {
                session::Frame::Init(init) => init.to_bytes().to_vec(),
                session::Frame::Challenge(challenge) => challenge.to_bytes().to_vec(),
                session::Frame::Auth(auth) => auth.to_bytes().to_vec(),
                session::Frame::Ok(ok) => ok.to_bytes(),
                session::Frame::Ping(ping) => ping.to_bytes().to_vec(),
                session::Frame::Pong(pong) => pong.to_bytes(),
                session::Frame::Error(refused) => refused.to_bytes().to_vec(),
            };
            assert_eq!(Datagram::parse(&bytes), Ok(Datagram::Session(frame)));
            assert_eq!(relayed_dst(&bytes), None);
        }
    }

    #[test]
    fn probe_and_address_frames_are_classified_by_their_first_byte() {
        let nonce = [7; 8];
        let observed = "[2001:db8::1]:443".parse().unwrap();
        let ping = probe::Ping { src: node(1), dst: node(2), nonce, signature: [4; 64] };
        assert_eq!(Datagram::parse(&ping.to_bytes()), Ok(Datagram::Probe(probe::Frame::Ping(ping))));
        let pong = probe::Pong { src: node(2), dst: node(1), nonce, observed, signature: [4; 64] };
        assert_eq!(Datagram::parse(&pong.to_bytes()), Ok(Datagram::Probe(probe::Frame::Pong(pong))));

        let query = addr::Query { nonce };
        assert_eq!(Datagram::parse(&query.to_bytes()), Ok(Datagram::AddrQuery(query)));
        let reply = addr::Reply { nonce, observed };
        assert_eq!(Datagram::parse(&reply.to_bytes()), Ok(Datagram::AddrReply(reply)));
    }

    #[test]
    fn truncated_frames_of_a_known_kind_are_rejected_as_malformed() {
        for kind in [0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c] {
            assert_eq!(Datagram::parse(&[kind, 0, 0, 0, 1]), Err(DatagramError::Malformed { kind }));
        }
    }
}
