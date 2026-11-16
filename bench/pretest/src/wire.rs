//! 路径层的线上格式。
//!
//! QUIC 包的首字节一定带 0x40 这一位（本工具关掉了 grease_quic_bit），
//! 所以首字节不带这一位的数据报都是路径层自己的帧，可以和直连的 QUIC 包共用一个端口。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

use anyhow::Context;

/// 经 relay 转发的数据帧在 QUIC 包前面加的头：类型 1 字节，目的与来源节点各 4 字节。
pub const RELAY_HEADER_LEN: usize = 9;

const VIRTUAL_PORT: u16 = 20330;
const QUIC_FIXED_BIT: u8 = 0x40;
const KIND_DATA: u8 = 0x01;
const KIND_REGISTER: u8 = 0x02;

/// 节点编号。同时决定 QUIC 眼里这个节点的固定地址，换路时这个地址不变。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(u16);

impl NodeId {
    pub fn virtual_addr(self) -> SocketAddr {
        let [hi, lo] = self.0.to_be_bytes();
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 66, hi, lo)), VIRTUAL_PORT)
    }

    fn to_wire(self) -> [u8; 4] {
        u32::from(self.0).to_be_bytes()
    }

    fn from_wire(bytes: &[u8]) -> Option<Self> {
        let raw = u32::from_be_bytes(bytes.try_into().ok()?);
        u16::try_from(raw).ok().map(Self)
    }
}

impl FromStr for NodeId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw: u16 = s.parse().with_context(|| format!("节点编号要在 0～65535 之间：{s}"))?;
        Ok(Self(raw))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// 一个 UDP 数据报（或 TCP 中转里的一帧）解析出来的内容。
#[derive(Debug)]
pub enum Frame {
    /// 直连的 QUIC 包，整个数据报原样交给 QUIC。
    Quic,
    Data {
        dst: NodeId,
        src: NodeId,
    },
    Register {
        id: NodeId,
    },
}

impl Frame {
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (&kind, rest) = datagram.split_first()?;
        if kind & QUIC_FIXED_BIT != 0 {
            return Some(Self::Quic);
        }
        match kind {
            KIND_DATA => {
                let dst = NodeId::from_wire(rest.get(..4)?)?;
                let src = NodeId::from_wire(rest.get(4..8)?)?;
                Some(Self::Data { dst, src })
            }
            KIND_REGISTER => Some(Self::Register { id: NodeId::from_wire(rest.get(..4)?)? }),
            _ => None,
        }
    }
}

/// 数据帧里目的节点所在的 4 个字节。relay 用它判断一批包是不是发往同一个节点，不必逐个解析。
pub fn data_dst_bytes(frame: &[u8]) -> Option<&[u8]> {
    match frame.first() {
        Some(&KIND_DATA) => frame.get(1..5),
        _ => None,
    }
}

pub fn push_data_header(buf: &mut Vec<u8>, dst: NodeId, src: NodeId) {
    buf.push(KIND_DATA);
    buf.extend_from_slice(&dst.to_wire());
    buf.extend_from_slice(&src.to_wire());
}

pub fn register_frame(id: NodeId) -> [u8; 5] {
    let [a, b, c, d] = id.to_wire();
    [KIND_REGISTER, a, b, c, d]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_frame_round_trips() {
        let (dst, src): (NodeId, NodeId) = ("2".parse().unwrap(), "1".parse().unwrap());
        let mut buf = Vec::new();
        push_data_header(&mut buf, dst, src);
        buf.extend_from_slice(b"payload");
        assert_eq!(buf.len(), RELAY_HEADER_LEN + 7);
        match Frame::parse(&buf) {
            Some(Frame::Data { dst: d, src: s }) => assert_eq!((d, s), (dst, src)),
            other => panic!("解析结果不是数据帧：{other:?}"),
        }
        assert_eq!(&buf[RELAY_HEADER_LEN..], b"payload");
        assert_eq!(data_dst_bytes(&buf), Some(&dst.to_wire()[..]));
    }

    #[test]
    fn quic_packets_are_recognised_by_fixed_bit() {
        assert!(matches!(Frame::parse(&[0x40, 0, 0]), Some(Frame::Quic)));
        assert!(matches!(Frame::parse(&[0xc3, 0, 0]), Some(Frame::Quic)));
        assert!(Frame::parse(&[0x3f]).is_none());
    }

    #[test]
    fn virtual_addr_is_stable_per_node() {
        let id: NodeId = "258".parse().unwrap();
        assert_eq!(id.virtual_addr().to_string(), "10.66.1.2:20330");
    }
}
