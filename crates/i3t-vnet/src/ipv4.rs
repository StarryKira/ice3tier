//! 读 IPv4 包的头，和写"不可达"的 ICMP 包：核对来源和目的、按包判断访问规则、本机的程序发往到不了的地方时回一句话，都只要这么多。
//!
//! 不校验头的校验和：从网卡读出来的是本机内核刚写的；写进网卡的，收的内核自己会校验，坏的它丢掉。

use std::net::Ipv4Addr;

/// IP 头里的协议号。
pub const ICMP: u8 = 1;
pub const TCP: u8 = 6;
pub const UDP: u8 = 17;

/// ICMP 的类型和"不可达"的几个代码。
const ICMP_ECHO_REPLY: u8 = 0;
const ICMP_UNREACHABLE: u8 = 3;
const ICMP_REDIRECT: u8 = 5;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_TIME_EXCEEDED: u8 = 11;
const ICMP_PARAMETER_PROBLEM: u8 = 12;

/// TCP 的标志位。
const TCP_SYN: u8 = 0x02;
const TCP_ACK: u8 = 0x10;

/// TCP 头最短多少字节。
const TCP_HEADER_MIN: usize = 20;

/// IP 头里的保留位、"不许分片"（DF）、"后面还有片"（MF），和片偏移占的那 13 位。
const RESERVED_FLAG: u16 = 0x8000;
const DONT_FRAGMENT: u16 = 0x4000;
const MORE_FRAGMENTS: u16 = 0x2000;
const FRAGMENT_OFFSET: u16 = 0x1fff;

/// "不可达"的 ICMP 包里带回原来那个包的多少字节：IP 头加上它后面的一段，整个 ICMP 包不超过 576 字节（RFC 1812）。
/// 收的内核靠带回来的 IP 头和端口（TCP 的还有序号）找到是哪个套接字的包。
const QUOTED_MAX: usize = 576 - 20 - 8;

/// 一个 IPv4 包，只看头。
#[derive(Debug, Clone, Copy)]
pub struct Ipv4<'a> {
    bytes: &'a [u8],
    header_len: usize,
}

/// 读不懂的包。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Malformed {
    #[error("不是 IPv4（版本 {0}）")]
    NotIpv4(u8),
    #[error("包只有 {0} 字节，装不下 IPv4 的头")]
    Truncated(usize),
    #[error("头长 {header_len} 字节、总长 {total_len} 字节，和包的 {len} 字节对不上")]
    Lengths { header_len: usize, total_len: usize, len: usize },
}

impl<'a> Ipv4<'a> {
    /// 读一个完整的包：总长度要正好是 `bytes` 的长度。
    ///
    /// # Errors
    ///
    /// 不是 IPv4，或者头里的长度和包对不上。
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Malformed> {
        let &first = bytes.first().ok_or(Malformed::Truncated(0))?;
        if first >> 4 != 4 {
            return Err(Malformed::NotIpv4(first >> 4));
        }
        if bytes.len() < 20 {
            return Err(Malformed::Truncated(bytes.len()));
        }
        let header_len = usize::from(first & 0x0f) * 4;
        let total_len = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
        if header_len < 20 || header_len > total_len || total_len != bytes.len() {
            return Err(Malformed::Lengths { header_len, total_len, len: bytes.len() });
        }
        Ok(Self { bytes, header_len })
    }

    /// IP 版本：首字节的高 4 位。空的包是 0。
    #[must_use]
    pub fn version_of(bytes: &[u8]) -> u8 {
        bytes.first().map_or(0, |first| first >> 4)
    }

    #[must_use]
    pub fn source(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.bytes[12], self.bytes[13], self.bytes[14], self.bytes[15])
    }

    #[must_use]
    pub fn destination(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.bytes[16], self.bytes[17], self.bytes[18], self.bytes[19])
    }

    #[must_use]
    pub fn protocol(&self) -> u8 {
        self.bytes[9]
    }

    /// 头有多长（字节）：20，带 IP 选项时更长。
    #[must_use]
    pub fn header_len(&self) -> usize {
        self.header_len
    }

    #[must_use]
    pub fn ttl(&self) -> u8 {
        self.bytes[8]
    }

    /// 头的校验和对不对。读包时不核对它（见模块的说明），只有要先替内核把关的地方（[`crate::fragments`]）问。
    #[must_use]
    pub fn header_checksum_ok(&self) -> bool {
        checksum(&self.bytes[..self.header_len]) == 0
    }

    /// 标志里有没有置"不许分片"（DF）或者保留位。
    #[must_use]
    pub fn dont_fragment_or_reserved(&self) -> bool {
        self.flags_and_offset() & (DONT_FRAGMENT | RESERVED_FLAG) != 0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// 标识：同一个包切出来的片标识相同（加上来源、目的和协议，内核按这几样把片拼回去）。
    #[must_use]
    pub fn identification(&self) -> u16 {
        u16::from_be_bytes([self.bytes[4], self.bytes[5]])
    }

    fn flags_and_offset(&self) -> u16 {
        u16::from_be_bytes([self.bytes[6], self.bytes[7]])
    }

    /// 后面还有片（MF）：这是一个被切开的包的一片，不是最后一片。
    #[must_use]
    pub fn more_fragments(&self) -> bool {
        self.flags_and_offset() & MORE_FRAGMENTS != 0
    }

    /// 这一片的内容在原来那个包里从第几个字节开始（头后面的部分算起；片偏移乘 8）。没切开的包是 0。
    #[must_use]
    pub fn fragment_offset(&self) -> usize {
        usize::from(self.flags_and_offset() & FRAGMENT_OFFSET) * 8
    }

    /// 片偏移不是 0：这是一个被切开的包的后面几片，里面没有端口。
    #[must_use]
    pub fn is_later_fragment(&self) -> bool {
        self.flags_and_offset() & FRAGMENT_OFFSET != 0
    }

    /// 这是一个被切开的包的一片（第一片、中间的或者最后一片）。
    #[must_use]
    pub fn is_fragment(&self) -> bool {
        self.more_fragments() || self.is_later_fragment()
    }

    /// 头后面的部分。
    #[must_use]
    pub fn payload(&self) -> &'a [u8] {
        &self.bytes[self.header_len..]
    }

    /// 这个包在会话里是什么，按包判断访问规则用。
    #[must_use]
    pub fn conversation(&self) -> Conversation {
        if self.is_later_fragment() {
            return Conversation::LaterFragment;
        }
        let payload = self.payload();
        match self.protocol() {
            // 被切开的 TCP 不认：两端按网卡的 MTU 定段长，节点之间的 TCP 不会被切；切开的第一片可以短得连完整的头都没有，
            // 标志还能被后面重叠的片改写（RFC 1858）。头不完整的也不认。
            TCP if self.more_fragments() || payload.len() < TCP_HEADER_MIN => Conversation::Other,
            TCP => match payload[13] {
                flags if flags & TCP_SYN != 0 && flags & TCP_ACK == 0 => Conversation::TcpOpen,
                _ => Conversation::TcpOngoing,
            },
            UDP => match payload.first_chunk::<4>() {
                Some(&[s0, s1, d0, d1]) => {
                    Conversation::Udp { source: u16::from_be_bytes([s0, s1]), destination: u16::from_be_bytes([d0, d1]) }
                }
                None => Conversation::Other,
            },
            ICMP => match payload.first() {
                Some(&ICMP_ECHO_REQUEST) => Conversation::IcmpRequest,
                Some(&(ICMP_ECHO_REPLY | ICMP_UNREACHABLE | ICMP_TIME_EXCEEDED | ICMP_PARAMETER_PROBLEM)) => Conversation::IcmpAnswer,
                // 别的 ICMP（时间戳请求、路由器通告……）当作新的会话：不确定是不是回答，就不放。
                _ => Conversation::Other,
            },
            _ => Conversation::Other,
        }
    }

    /// 是不是一个 ICMP 的差错报文（不可达、超时、参数问题）。对差错报文不再回差错报文（RFC 1122 3.2.2）。
    /// 被切开的包的后面几片看不出来（头后面是原包中间的任意字节），不算。
    #[must_use]
    pub fn is_icmp_error(&self) -> bool {
        matches!(self.icmp_type(), Some(ICMP_UNREACHABLE | ICMP_TIME_EXCEEDED | ICMP_PARAMETER_PROBLEM))
    }

    /// 是不是一个 ICMP 重定向：路由器告诉主机去某个地址换一个下一跳。后面几片同上，不算。
    #[must_use]
    pub fn is_icmp_redirect(&self) -> bool {
        self.icmp_type() == Some(ICMP_REDIRECT)
    }

    /// ICMP 的类型：协议是 ICMP、不是被切开的包的后面几片、读得到类型时才有。
    fn icmp_type(&self) -> Option<u8> {
        (self.protocol() == ICMP && !self.is_later_fragment()).then(|| self.payload().first().copied()).flatten()
    }

    /// ICMP 差错报文里带回来的原包：只有它开头的一段，至少要有完整的 IP 头。读它的来源、目的、协议，和头后面的端口
    /// （带回来的够长的话：RFC 792 要求至少带回头后面的 8 个字节）。不是差错报文，或者带回来的连头都不完整，是 `None`。
    ///
    /// 返回的包只有带回来的那一段：头里写的总长度说的是原来那个包，比它长。
    #[must_use]
    pub fn quoted(&self) -> Option<Ipv4<'a>> {
        if !self.is_icmp_error() {
            return None;
        }
        let inner = self.payload().get(8..)?;
        let &first = inner.first()?;
        let header_len = usize::from(first & 0x0f) * 4;
        (first >> 4 == 4 && header_len >= 20 && inner.len() >= header_len).then_some(Ipv4 { bytes: inner, header_len })
    }
}

/// 一个包在它的会话里是什么：开头（发起一次新的会话），还是之后的、回答的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversation {
    /// TCP 的第一个握手包：有 SYN 没有 ACK。
    TcpOpen,
    /// 别的 TCP 包。它们开不了新的连接：对一个不存在的连接，内核只回 RST。
    TcpOngoing,
    /// ping 的请求。
    IcmpRequest,
    /// ping 的回答和差错报文（不可达、超时、参数问题），都是对本机发出的包的回答。
    IcmpAnswer,
    Udp {
        source: u16,
        destination: u16,
    },
    /// 被切开的包的后面几片：没有端口可看，单看它判断不了。对端发来的片在 [`crate::fragments`] 里攒齐了、按第一片判断。
    LaterFragment,
    /// 别的协议，或者看不出来的包（包括被切开的 TCP、头不完整的 TCP）。
    Other,
}

/// "不可达"的代码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Unreachable {
    /// 主机不可达：地址段里没有这个节点、它没有网卡、连不上它。内核告诉程序 `EHOSTUNREACH`（"No route to host"），
    /// 正在连的 TCP 当场失败。
    Host = 1,
    /// 被管理员禁止：访问规则不允许本节点访问它。内核告诉程序的也是 `EHOSTUNREACH`。
    Prohibited = 13,
}

/// 给本机发出的包 `original` 写一个"不可达"的 ICMP 包，写回网卡交给本机的内核。不该回答的包返回 `None`：
/// 差错报文本身、被切开的包的后面几片、发往组播或者广播地址的（RFC 1122 3.2.2）。
///
/// 来源写成原来那个包要去的地址，像是那台主机说的：写成本节点自己的地址的话，内核会把它当作伪造的包丢掉
/// （从外面的网卡进来、来源却是本机的地址）；原来的目的地址在地址段里，经这块网卡走，这样写内核认。
#[must_use]
pub fn unreachable(original: &Ipv4<'_>, code: Unreachable) -> Option<Vec<u8>> {
    let destination = original.destination();
    if original.is_icmp_error() || original.is_later_fragment() || destination.is_multicast() || destination.is_broadcast() {
        return None;
    }
    let quoted = &original.bytes[..original.bytes.len().min(QUOTED_MAX)];
    let total = 20 + 8 + quoted.len();
    let mut packet = Vec::with_capacity(total);
    let total_len = u16::try_from(total).expect("ICMP 包不超过 576 字节");
    packet.extend_from_slice(&[0x45, 0xc0]);
    packet.extend_from_slice(&total_len.to_be_bytes());
    // 标识 0，不分片，TTL 64，协议 ICMP，校验和先写 0。
    packet.extend_from_slice(&[0, 0, 0, 0, 64, ICMP, 0, 0]);
    packet.extend_from_slice(&destination.octets());
    packet.extend_from_slice(&original.source().octets());
    let header_sum = checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&header_sum.to_be_bytes());
    packet.extend_from_slice(&[ICMP_UNREACHABLE, code as u8, 0, 0, 0, 0, 0, 0]);
    packet.extend_from_slice(quoted);
    let icmp_sum = checksum(&packet[20..]);
    packet[22..24].copy_from_slice(&icmp_sum.to_be_bytes());
    Some(packet)
}

/// 互联网校验和（RFC 1071）。
fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = bytes.chunks(2).map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !u16::try_from(sum).expect("折叠之后不超过 16 位")
}

#[cfg(test)]
pub(crate) mod testing {
    //! 测试里用的包。
    use super::{ICMP, TCP, UDP, checksum};

    /// 一个 IPv4 包：头 20 字节，后面是 `payload`。
    pub(crate) fn packet(protocol: u8, source: [u8; 4], destination: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let total = u16::try_from(20 + payload.len()).unwrap();
        let mut bytes = vec![0x45, 0];
        bytes.extend_from_slice(&total.to_be_bytes());
        bytes.extend_from_slice(&[0x12, 0x34, 0x40, 0, 64, protocol, 0, 0]);
        bytes.extend_from_slice(&source);
        bytes.extend_from_slice(&destination);
        let sum = checksum(&bytes);
        bytes[10..12].copy_from_slice(&sum.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    /// 一个 TCP 包，`flags` 是标志字节。
    pub(crate) fn tcp(source: [u8; 4], destination: [u8; 4], ports: (u16, u16), flags: u8) -> Vec<u8> {
        let mut segment = Vec::new();
        segment.extend_from_slice(&ports.0.to_be_bytes());
        segment.extend_from_slice(&ports.1.to_be_bytes());
        segment.extend_from_slice(&[0, 0, 0x10, 0, 0, 0, 0, 0, 0x50, flags, 0xff, 0xff, 0, 0, 0, 0]);
        packet(TCP, source, destination, &segment)
    }

    pub(crate) fn udp(source: [u8; 4], destination: [u8; 4], ports: (u16, u16), data: &[u8]) -> Vec<u8> {
        let mut datagram = Vec::new();
        datagram.extend_from_slice(&ports.0.to_be_bytes());
        datagram.extend_from_slice(&ports.1.to_be_bytes());
        datagram.extend_from_slice(&u16::try_from(8 + data.len()).unwrap().to_be_bytes());
        datagram.extend_from_slice(&[0, 0]);
        datagram.extend_from_slice(data);
        packet(UDP, source, destination, &datagram)
    }

    pub(crate) fn icmp(source: [u8; 4], destination: [u8; 4], kind: u8) -> Vec<u8> {
        packet(ICMP, source, destination, &[kind, 0, 0, 0, 0, 1, 0, 1])
    }

    /// 一个 ICMP 差错报文（`kind` 是类型，`code` 是代码），带回 `original` 的开头（最多头加 8 个字节之外的 `extra` 个）。
    pub(crate) fn icmp_error(source: [u8; 4], destination: [u8; 4], kind: u8, code: u8, original: &[u8]) -> Vec<u8> {
        let mut payload = vec![kind, code, 0, 0, 0, 0, 0x02, 0x28];
        payload.extend_from_slice(original);
        packet(ICMP, source, destination, &payload)
    }

    /// IP 头的校验和（校验和那两个字节要先清零）。
    pub(crate) fn header_checksum(header: &[u8]) -> u16 {
        checksum(header)
    }

    /// 改写一个包（[`packet`] 造的）的分片字段：标识 `id`，片偏移 `offset`（字节，8 的倍数），`more` 是 MF。
    pub(crate) fn refragment(mut bytes: Vec<u8>, id: u16, offset: usize, more: bool) -> Vec<u8> {
        let field = u16::try_from(offset / 8).unwrap() | if more { 0x2000 } else { 0 };
        bytes[4..6].copy_from_slice(&id.to_be_bytes());
        bytes[6..8].copy_from_slice(&field.to_be_bytes());
        bytes[10..12].fill(0);
        let sum = checksum(&bytes[..20]);
        bytes[10..12].copy_from_slice(&sum.to_be_bytes());
        bytes
    }

    /// 像内核那样把一个包（头 20 字节）切成片：每片头后面最多 `size` 个字节（8 的倍数），标识 `id`。
    pub(crate) fn split(whole: &[u8], id: u16, size: usize) -> Vec<Vec<u8>> {
        let (header, payload) = whole.split_at(20);
        let chunks: Vec<&[u8]> = payload.chunks(size).collect();
        let last = chunks.len() - 1;
        chunks
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| {
                let mut bytes = header.to_vec();
                bytes[2..4].copy_from_slice(&u16::try_from(20 + chunk.len()).unwrap().to_be_bytes());
                bytes.extend_from_slice(chunk);
                refragment(bytes, id, index * size, index != last)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{icmp, icmp_error, packet, refragment, split, tcp, udp};
    use super::*;

    const A: [u8; 4] = [10, 66, 0, 1];
    const B: [u8; 4] = [10, 66, 0, 2];

    #[test]
    fn the_header_gives_addresses_protocol_and_payload() {
        let bytes = udp(A, B, (5000, 53), b"query");
        let ip = Ipv4::parse(&bytes).unwrap();
        assert_eq!((ip.source(), ip.destination(), ip.protocol(), ip.len()), (A.into(), B.into(), UDP, 33));
        assert_eq!(&ip.payload()[8..], b"query");
        assert_eq!(ip.conversation(), Conversation::Udp { source: 5000, destination: 53 });
        assert!(!ip.is_later_fragment());
    }

    #[test]
    fn packets_that_are_not_whole_ipv4_packets_are_refused() {
        assert_eq!(Ipv4::parse(&[]).unwrap_err(), Malformed::Truncated(0));
        let mut v6 = vec![0x60; 40];
        assert_eq!(Ipv4::parse(&v6).unwrap_err(), Malformed::NotIpv4(6));
        v6[0] = 0x45;
        assert!(matches!(Ipv4::parse(&v6[..10]), Err(Malformed::Truncated(10))));
        let mut bytes = udp(A, B, (1, 2), b"x");
        bytes.push(0);
        assert!(matches!(Ipv4::parse(&bytes), Err(Malformed::Lengths { .. })), "总长度和包对不上");
        let mut short_header = udp(A, B, (1, 2), b"x");
        short_header[0] = 0x44;
        assert!(matches!(Ipv4::parse(&short_header), Err(Malformed::Lengths { header_len: 16, .. })));
        assert_eq!(Ipv4::version_of(&[0x45]), 4);
        assert_eq!(Ipv4::version_of(&[]), 0);
    }

    #[test]
    fn conversations_are_told_apart_by_what_can_open_one() {
        let kind = |bytes: Vec<u8>| Ipv4::parse(&bytes).unwrap().conversation();
        assert_eq!(kind(tcp(A, B, (40000, 22), TCP_SYN)), Conversation::TcpOpen);
        assert_eq!(kind(tcp(A, B, (22, 40000), TCP_SYN | TCP_ACK)), Conversation::TcpOngoing, "SYN-ACK 是回答");
        assert_eq!(kind(tcp(A, B, (22, 40000), TCP_ACK)), Conversation::TcpOngoing);
        assert_eq!(kind(icmp(A, B, ICMP_ECHO_REQUEST)), Conversation::IcmpRequest);
        assert_eq!(kind(icmp(A, B, ICMP_ECHO_REPLY)), Conversation::IcmpAnswer);
        assert_eq!(kind(icmp(A, B, ICMP_UNREACHABLE)), Conversation::IcmpAnswer);
        assert_eq!(kind(icmp(A, B, 13)), Conversation::Other, "时间戳请求不是回答");
        assert_eq!(kind(packet(47, A, B, b"gre")), Conversation::Other);
        assert_eq!(kind(packet(TCP, A, B, b"short")), Conversation::Other, "看不到标志的 TCP");
        let mut later = udp(A, B, (1, 2), b"rest");
        later[6..8].copy_from_slice(&0x00b9u16.to_be_bytes());
        assert_eq!(kind(later), Conversation::LaterFragment);
    }

    #[test]
    fn fragments_tell_their_identification_offset_and_whether_more_follow() {
        let whole = udp(A, B, (5000, 53), &[1; 100]);
        let ip = Ipv4::parse(&whole).unwrap();
        assert!(!ip.is_fragment(), "不许分片（DF）的整包");
        assert_eq!((ip.identification(), ip.fragment_offset(), ip.more_fragments()), (0x1234, 0, false));
        let pieces = split(&whole, 77, 48);
        assert_eq!(pieces.len(), 3);
        let parsed: Vec<Ipv4<'_>> = pieces.iter().map(|piece| Ipv4::parse(piece).unwrap()).collect();
        let fields: Vec<_> =
            parsed.iter().map(|ip| (ip.identification(), ip.fragment_offset(), ip.more_fragments(), ip.payload().len())).collect();
        assert_eq!(fields, [(77, 0, true, 48), (77, 48, true, 48), (77, 96, false, 12)]);
        assert!(parsed.iter().all(Ipv4::is_fragment));
        assert_eq!(parsed[0].conversation(), Conversation::Udp { source: 5000, destination: 53 }, "第一片里有端口");
        assert_eq!(parsed[1].conversation(), Conversation::LaterFragment);
        assert_eq!(parsed[2].conversation(), Conversation::LaterFragment, "最后一片");
    }

    #[test]
    fn the_header_tells_its_length_ttl_checksum_and_flags() {
        let mut bytes = udp(A, B, (5000, 53), b"x");
        let ip = Ipv4::parse(&bytes).unwrap();
        assert_eq!((ip.header_len(), ip.ttl()), (20, 64));
        assert!(ip.header_checksum_ok());
        assert!(ip.dont_fragment_or_reserved(), "测试造的整包置了 DF");
        let cut = refragment(bytes.clone(), 1, 0, true);
        assert!(!Ipv4::parse(&cut).unwrap().dont_fragment_or_reserved(), "切开的片没有 DF");
        bytes[11] ^= 1;
        assert!(!Ipv4::parse(&bytes).unwrap().header_checksum_ok());
        bytes[6] = 0x80;
        assert!(Ipv4::parse(&bytes).unwrap().dont_fragment_or_reserved(), "保留位");
        // 带 4 个字节的选项（一个 NOP 和三个"选项结束"）。
        let mut with_options = vec![0x46, 0, 0, 28, 0, 1, 0x20, 0, 64, UDP, 0, 0, 10, 66, 0, 2, 10, 66, 0, 1, 1, 0, 0, 0];
        with_options.extend_from_slice(&[0, 53, 19, 136]);
        let sum = checksum(&with_options[..24]);
        with_options[10..12].copy_from_slice(&sum.to_be_bytes());
        let ip = Ipv4::parse(&with_options).unwrap();
        assert_eq!(ip.header_len(), 24);
        assert!(ip.header_checksum_ok());
    }

    #[test]
    fn a_tcp_segment_that_is_cut_or_too_short_for_its_header_is_not_judged_by_its_flags() {
        let kind = |bytes: &[u8]| Ipv4::parse(bytes).unwrap().conversation();
        // RFC 1858 的手法：第一片短得只有 16 个字节、标志是 ACK，后面偏移 8 的一片把标志改写成 SYN。
        let ack = tcp(A, B, (40000, 22), TCP_ACK);
        let mut head = ack[..36].to_vec();
        head[2..4].copy_from_slice(&36u16.to_be_bytes());
        assert_eq!(kind(&refragment(head, 9, 0, true)), Conversation::Other, "切开的 TCP 的第一片");
        assert_eq!(kind(&refragment(ack.clone(), 9, 8, false)), Conversation::LaterFragment);
        assert_eq!(kind(&packet(TCP, A, B, &ack[20..39])), Conversation::Other, "头不够 20 个字节");
        assert_eq!(kind(&ack), Conversation::TcpOngoing, "对照：完整的");
    }

    #[test]
    fn an_icmp_error_gives_the_packet_it_quotes() {
        let sent = udp(A, B, (5000, 53), b"query");
        let error = icmp_error(B, A, ICMP_UNREACHABLE, 3, &sent[..28]);
        let ip = Ipv4::parse(&error).unwrap();
        assert!(ip.is_icmp_error());
        let quoted = ip.quoted().unwrap();
        assert_eq!((quoted.source(), quoted.destination(), quoted.protocol()), (A.into(), B.into(), UDP));
        assert_eq!(quoted.conversation(), Conversation::Udp { source: 5000, destination: 53 });
        let header_only = icmp_error(B, A, ICMP_TIME_EXCEEDED, 0, &sent[..20]);
        assert_eq!(
            Ipv4::parse(&header_only).unwrap().quoted().map(|quoted| quoted.conversation()),
            Some(Conversation::Other),
            "头后面没带"
        );
        let cut = icmp_error(B, A, ICMP_UNREACHABLE, 3, &sent[..19]);
        assert!(Ipv4::parse(&cut).unwrap().quoted().is_none(), "头都不完整");
        let not_ipv4 = icmp_error(B, A, ICMP_PARAMETER_PROBLEM, 0, &[0x60; 28]);
        assert!(Ipv4::parse(&not_ipv4).unwrap().quoted().is_none());
        assert!(Ipv4::parse(&icmp(B, A, ICMP_ECHO_REPLY)).unwrap().quoted().is_none(), "ping 的回答不是差错报文");
        let redirect = icmp_error(B, A, ICMP_REDIRECT, 1, &sent[..28]);
        assert!(Ipv4::parse(&redirect).unwrap().is_icmp_redirect());
        assert!(!ip.is_icmp_redirect());
        // 后面几片头后面是原包中间的字节，碰巧是 3 或者 5 也不算。
        let later_error = refragment(icmp_error(B, A, ICMP_UNREACHABLE, 3, &sent[..28]), 5, 8, false);
        assert!(!Ipv4::parse(&later_error).unwrap().is_icmp_error());
        assert!(Ipv4::parse(&later_error).unwrap().quoted().is_none());
        let later_redirect = refragment(redirect, 5, 8, false);
        assert!(!Ipv4::parse(&later_redirect).unwrap().is_icmp_redirect());
    }

    #[test]
    fn an_unreachable_answer_quotes_the_packet_and_comes_from_its_destination() {
        let original = tcp(A, [10, 66, 0, 9], (40000, 80), TCP_SYN);
        let parsed = Ipv4::parse(&original).unwrap();
        let answer = unreachable(&parsed, Unreachable::Host).unwrap();
        let ip = Ipv4::parse(&answer).unwrap();
        assert_eq!((ip.source(), ip.destination(), ip.protocol()), (Ipv4Addr::new(10, 66, 0, 9), A.into(), ICMP));
        assert_eq!(checksum(&answer[..20]), 0, "头的校验和对得上");
        assert_eq!(checksum(&answer[20..]), 0, "ICMP 的校验和对得上");
        assert_eq!(&answer[20..22], &[ICMP_UNREACHABLE, 1]);
        assert_eq!(&answer[28..], &original[..], "整个原包带回去（不长）");
        assert_eq!(ip.conversation(), Conversation::IcmpAnswer);

        let prohibited = unreachable(&parsed, Unreachable::Prohibited).unwrap();
        assert_eq!(prohibited[21], 13);

        let big = udp(A, B, (1, 2), &[7; 1200]);
        let quoted = unreachable(&Ipv4::parse(&big).unwrap(), Unreachable::Host).unwrap();
        assert_eq!(quoted.len(), 576, "大包只带回开头");
    }

    #[test]
    fn errors_later_fragments_and_group_addresses_get_no_answer() {
        let error = icmp(A, B, ICMP_UNREACHABLE);
        assert_eq!(unreachable(&Ipv4::parse(&error).unwrap(), Unreachable::Host), None, "不对差错报文回差错报文");
        let request = icmp(A, B, ICMP_ECHO_REQUEST);
        assert!(unreachable(&Ipv4::parse(&request).unwrap(), Unreachable::Host).is_some(), "ping 的请求要回");
        let mut later = udp(A, B, (1, 2), b"x");
        later[6..8].copy_from_slice(&0x0010u16.to_be_bytes());
        assert_eq!(unreachable(&Ipv4::parse(&later).unwrap(), Unreachable::Host), None);
        for group in [[224, 0, 0, 251], [255, 255, 255, 255]] {
            let mdns = udp(A, group, (5353, 5353), b"q");
            assert_eq!(unreachable(&Ipv4::parse(&mdns).unwrap(), Unreachable::Host), None);
        }
    }
}
