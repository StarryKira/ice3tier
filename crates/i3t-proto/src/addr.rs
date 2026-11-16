//! 地址查询与应答："你看到我的地址是什么"。
//!
//! 节点靠它知道自己在 NAT 外面的地址，不依赖第三方的 STUN 服务。能回答的有 controller（专门开一个 UDP 端口）、
//! relay 和已经连上的对端。
//!
//! ```text
//! 查询  0x0a | 随机数 8 字节 | 填充到 32 字节
//! 应答  0x0b | 随机数 8 字节 | 地址族 1 字节（4 或 6） | 地址 4 或 16 字节 | 端口 2 字节（大端）
//! ```
//!
//! 查询固定 32 字节，应答最长 28 字节：应答不比查询大，所以伪造来源地址的人拿它放大不了流量。
//! 回答的一方不记任何状态。随机数由查询方生成，用来把应答和查询对上，也让不在路径上的人伪造不了应答。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// 查询帧的首字节。
pub const KIND_QUERY: u8 = 0x0a;
/// 应答帧的首字节。
pub const KIND_REPLY: u8 = 0x0b;

const NONCE_LEN: usize = 8;
const FAMILY_V4: u8 = 4;
const FAMILY_V6: u8 = 6;

/// 一次地址查询。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    pub nonce: [u8; NONCE_LEN],
}

impl Query {
    /// 查询帧的长度。比最长的应答还长，多出来的是填充。
    pub const LEN: usize = 32;

    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        bytes[0] = KIND_QUERY;
        bytes[1..=NONCE_LEN].copy_from_slice(&self.nonce);
        bytes
    }

    /// 不是一个完整的查询帧时返回 `None`。比 [`Query::LEN`] 短的一律不认，否则应答就可能比查询大。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        if datagram.len() < Self::LEN {
            return None;
        }
        let (&[KIND_QUERY], rest) = datagram.split_first_chunk::<1>()? else { return None };
        Some(Self { nonce: *rest.first_chunk::<NONCE_LEN>()? })
    }
}

/// 对一次查询的回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply {
    /// 原样抄回查询里的随机数。
    pub nonce: [u8; NONCE_LEN],
    /// 回答的一方看到的查询来源地址。
    pub observed: SocketAddr,
}

impl Reply {
    /// 应答帧最长多少字节（IPv6 地址时）。
    pub const MAX_LEN: usize = 1 + NONCE_LEN + 1 + 16 + 2;

    #[must_use]
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::MAX_LEN);
        bytes.push(KIND_REPLY);
        bytes.extend_from_slice(&self.nonce);
        // IPv4 映射的 IPv6 地址按 IPv4 报告：双栈 socket 上收到的 IPv4 包就是这种形式。
        match self.observed.ip().to_canonical() {
            IpAddr::V4(ip) => {
                bytes.push(FAMILY_V4);
                bytes.extend_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                bytes.push(FAMILY_V6);
                bytes.extend_from_slice(&ip.octets());
            }
        }
        bytes.extend_from_slice(&self.observed.port().to_be_bytes());
        bytes
    }

    /// 不是一个完整的应答帧时返回 `None`。
    #[must_use]
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let (&[KIND_REPLY], rest) = datagram.split_first_chunk::<1>()? else { return None };
        let (&nonce, rest) = rest.split_first_chunk::<NONCE_LEN>()?;
        let (&family, rest) = rest.split_first()?;
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
        let &[hi, lo] = rest else { return None };
        Some(Self { nonce, observed: SocketAddr::new(ip, u16::from_be_bytes([hi, lo])) })
    }
}

const _: () = assert!(Reply::MAX_LEN <= Query::LEN);

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    #[test]
    fn query_round_trips_and_is_padded() {
        let bytes = Query { nonce: NONCE }.to_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..9], [0x0a, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(bytes[9..].iter().all(|&byte| byte == 0));
        assert_eq!(Query::parse(&bytes), Some(Query { nonce: NONCE }));
    }

    #[test]
    fn short_or_foreign_queries_are_ignored() {
        let bytes = Query { nonce: NONCE }.to_bytes();
        assert_eq!(Query::parse(&bytes[..31]), None);
        assert_eq!(Query::parse(&[0x0b; 32]), None);
        assert_eq!(Query::parse(&[]), None);
    }

    #[test]
    fn reply_round_trips_for_both_address_families() {
        for observed in ["203.0.113.7:20330", "[2001:db8::1]:443"] {
            let reply = Reply { nonce: NONCE, observed: observed.parse().unwrap() };
            let bytes = reply.to_bytes();
            assert!(bytes.len() <= Query::LEN);
            assert_eq!(Reply::parse(&bytes), Some(reply));
        }
    }

    #[test]
    fn mapped_ipv4_is_reported_as_ipv4() {
        let reply = Reply { nonce: NONCE, observed: "[::ffff:203.0.113.7]:5000".parse().unwrap() };
        let parsed = Reply::parse(&reply.to_bytes()).unwrap();
        assert_eq!(parsed.observed, "203.0.113.7:5000".parse().unwrap());
    }

    #[test]
    fn malformed_replies_are_ignored() {
        let bytes = Reply { nonce: NONCE, observed: "203.0.113.7:20330".parse().unwrap() }.to_bytes();
        assert_eq!(Reply::parse(&bytes[..bytes.len() - 1]), None);
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(Reply::parse(&trailing), None);
        let mut bad_family = bytes;
        bad_family[9] = 5;
        assert_eq!(Reply::parse(&bad_family), None);
    }
}
