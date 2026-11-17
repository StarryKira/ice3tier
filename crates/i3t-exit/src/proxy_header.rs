//! PROXY protocol 的头（HAProxy 公开的规范，v1 和 v2）：端口转发的目标节点连上目标之后先发它，
//! 认它的服务（nginx 的 `proxy_protocol`、HAProxy、Postfix 等）据此看到发起节点的虚拟 IP，而不是目标节点自己的地址。
//!
//! 头里的来源只能是连接的握手认出的发起节点的虚拟 IP（调用方保证），端口是目标节点这一侧这条 TCP 的本地端口：
//! 服务看到的就是"那个节点从这个端口连过来"，和透明访问打开时的样子一样（白皮书 §9.5）。目的是这条 TCP 实际连到的地址。
//! 两个地址要同一族：目标是 IPv6 时虚拟 IP 写成 IPv4 映射的 IPv6 地址。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use i3t_model::netmap::ProxyProtocol;

/// v2 的头开头的 12 个字节。
const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// 版本 2，命令 PROXY（转述别人的连接）。
const V2_PROXY: u8 = 0x21;
const V2_TCP4: u8 = 0x11;
const V2_TCP6: u8 = 0x21;

/// `initiator` 从 `local_port` 连到 `destination` 的头。
pub(crate) fn encode(version: ProxyProtocol, initiator: Ipv4Addr, local_port: u16, destination: SocketAddr) -> Vec<u8> {
    let destination_ip = destination.ip().to_canonical();
    let source_ip = match destination_ip {
        IpAddr::V4(_) => IpAddr::V4(initiator),
        IpAddr::V6(_) => IpAddr::V6(initiator.to_ipv6_mapped()),
    };
    let (source_port, destination_port) = (local_port, destination.port());
    match version {
        ProxyProtocol::V1 => {
            let family = if source_ip.is_ipv4() { "TCP4" } else { "TCP6" };
            format!("PROXY {family} {source_ip} {destination_ip} {source_port} {destination_port}\r\n").into_bytes()
        }
        ProxyProtocol::V2 => {
            let mut header = V2_SIGNATURE.to_vec();
            header.push(V2_PROXY);
            match (source_ip, destination_ip) {
                (IpAddr::V4(source), IpAddr::V4(destination)) => {
                    header.push(V2_TCP4);
                    header.extend_from_slice(&12u16.to_be_bytes());
                    header.extend_from_slice(&source.octets());
                    header.extend_from_slice(&destination.octets());
                }
                (source, destination) => {
                    header.push(V2_TCP6);
                    header.extend_from_slice(&36u16.to_be_bytes());
                    header.extend_from_slice(&as_v6(source).octets());
                    header.extend_from_slice(&as_v6(destination).octets());
                }
            }
            header.extend_from_slice(&source_port.to_be_bytes());
            header.extend_from_slice(&destination_port.to_be_bytes());
            header
        }
    }
}

fn as_v6(ip: IpAddr) -> std::net::Ipv6Addr {
    match ip {
        IpAddr::V4(ip) => ip.to_ipv6_mapped(),
        IpAddr::V6(ip) => ip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: Ipv4Addr = Ipv4Addr::new(10, 66, 0, 2);

    #[test]
    fn v1_is_one_line_with_the_peers_virtual_ip() {
        let header = encode(ProxyProtocol::V1, PEER, 41000, "127.0.0.1:5432".parse().unwrap());
        assert_eq!(header, b"PROXY TCP4 10.66.0.2 127.0.0.1 41000 5432\r\n");
        let header = encode(ProxyProtocol::V1, PEER, 41000, "[::1]:8080".parse().unwrap());
        assert_eq!(header, b"PROXY TCP6 ::ffff:10.66.0.2 ::1 41000 8080\r\n");
        let header = encode(ProxyProtocol::V1, PEER, 1, "[::ffff:192.168.1.10]:80".parse().unwrap());
        assert_eq!(header, b"PROXY TCP4 10.66.0.2 192.168.1.10 1 80\r\n", "映射的 IPv4 目标按 IPv4 写");
        assert!(header.len() <= 107, "v1 的头最长 107 字节");
    }

    #[test]
    fn v2_is_the_binary_header_for_tcp_over_ipv4_and_ipv6() {
        let header = encode(ProxyProtocol::V2, PEER, 0x1234, "192.168.1.10:80".parse().unwrap());
        let mut expected = b"\r\n\r\n\0\r\nQUIT\n\x21\x11\x00\x0c".to_vec();
        expected.extend_from_slice(&[10, 66, 0, 2, 192, 168, 1, 10, 0x12, 0x34, 0, 80]);
        assert_eq!(header, expected);

        let header = encode(ProxyProtocol::V2, PEER, 7, "[2001:db8::5]:443".parse().unwrap());
        assert_eq!(&header[12..16], &[0x21, 0x21, 0, 36]);
        assert_eq!(&header[16..32], &PEER.to_ipv6_mapped().octets());
        assert_eq!(&header[32..48], &"2001:db8::5".parse::<std::net::Ipv6Addr>().unwrap().octets());
        assert_eq!(&header[48..], &[0, 7, 1, 187]);
    }
}
