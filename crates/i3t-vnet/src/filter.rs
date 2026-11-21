//! 按包执行访问规则。
//!
//! controller 的规则说的是"a 可以访问 b"，回程自然放行；同一个事实就是 netmap 里每个对端的 `inbound`（它可以访问本节点），
//! 和建流时查的是同一个。对端不被允许访问本节点时，它发来的包里只有本节点发起的会话的回包能进来：
//!
//! - TCP：第一个握手包（有 SYN 没有 ACK）丢掉，别的放行。别的 TCP 包开不了连接，内核对不存在的连接只回 RST。
//!   被切开的、头不完整的 TCP 丢掉（看不准标志）；
//! - ping：请求丢掉，回答和差错报文（不可达、超时、参数问题）放行。差错报文带回来的原包是 UDP 的话，要是下面那张表里的会话：
//!   不然它可以随便编一个本机的端口，让那个套接字收到一个错误；
//! - UDP：本节点最近往它的这个端口、从本机的这个端口发过包，才放行（[`Replies`]，一张有上限的小表）；
//! - 被切开的包按第一片判断：对端发来的片先攒齐（[`crate::fragments`]），拿第一片来问这里。单独的后面几片丢掉；
//! - 别的协议丢掉：看不出是不是回包。
//!
//! 差错报文说的是不是本节点发给这个对端的包，不管对端能不能访问本节点都要核对，在 [`crate::engine`] 里。
//!
//! 规则双向都允许时（新建的网络默认 `all <-> all`）什么都不记、什么都不查。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use crate::ipv4::{Conversation, Ipv4, UDP};

/// 表里最多记多少个 UDP 会话。满了先扔过期的，还满就扔最旧的：最多是更早的那个会话的回包进不来了。
/// 每个几十字节，表最大一两百 KB。
pub const MAX_REPLIES: usize = 4096;

/// 本节点往一个地址和端口发过包之后，多久之内那里来的包算回包。和 UDP 端口转发的会话空闲回收一样长。
pub const REPLY_WINDOW: Duration = Duration::from_secs(60);

/// 一个包让不让进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// 对端不被允许访问本节点，而这个包开的是一次新的会话（或者看不出它不是）。
    NewConversation,
}

/// 本节点最近往不能访问本节点的对端发过的 UDP：（对端的地址，对端的端口，本机的端口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Flow {
    peer: Ipv4Addr,
    peer_port: u16,
    local_port: u16,
}

/// 那张小表。
#[derive(Debug, Default)]
pub struct Replies {
    flows: HashMap<Flow, Instant>,
}

impl Replies {
    /// 本机发往不能访问本节点的对端的一个包：是 UDP 的话记下来。
    pub fn sent(&mut self, packet: &Ipv4<'_>, now: Instant) {
        let Conversation::Udp { source, destination } = packet.conversation() else { return };
        let flow = Flow { peer: packet.destination(), peer_port: destination, local_port: source };
        if !self.flows.contains_key(&flow) && self.flows.len() >= MAX_REPLIES {
            self.make_room(now);
        }
        self.flows.insert(flow, now);
    }

    /// 不能访问本节点的对端发来的一个包让不让进来。被切开的包问的是它的第一片。
    #[must_use]
    pub fn admit(&self, packet: &Ipv4<'_>, now: Instant) -> Verdict {
        let pass = match packet.conversation() {
            Conversation::TcpOngoing => true,
            Conversation::IcmpAnswer if packet.is_icmp_error() => packet.quoted().is_some_and(|quoted| self.quotes_ours(&quoted, now)),
            Conversation::IcmpAnswer => true,
            Conversation::TcpOpen | Conversation::IcmpRequest | Conversation::LaterFragment | Conversation::Other => false,
            Conversation::Udp { source, destination } => {
                self.expects(Flow { peer: packet.source(), peer_port: source, local_port: destination }, now)
            }
        };
        if pass { Verdict::Pass } else { Verdict::NewConversation }
    }

    /// 差错报文带回来的本机发出的原包：UDP 的要是表里的会话（看不到端口的也不放），别的协议由内核自己对（TCP 核对序号）。
    fn quotes_ours(&self, quoted: &Ipv4<'_>, now: Instant) -> bool {
        if quoted.protocol() != UDP {
            return true;
        }
        match quoted.conversation() {
            Conversation::Udp { source, destination } => {
                self.expects(Flow { peer: quoted.destination(), peer_port: destination, local_port: source }, now)
            }
            _ => false,
        }
    }

    fn expects(&self, flow: Flow, now: Instant) -> bool {
        self.flows.get(&flow).is_some_and(|sent| now.saturating_duration_since(*sent) < REPLY_WINDOW)
    }

    /// 表里现在有几条（含过期还没扔的）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.flows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    fn make_room(&mut self, now: Instant) {
        self.flows.retain(|_, sent| now.saturating_duration_since(*sent) < REPLY_WINDOW);
        if self.flows.len() >= MAX_REPLIES
            && let Some(oldest) = self.flows.iter().min_by_key(|(_, sent)| **sent).map(|(flow, _)| *flow)
        {
            self.flows.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipv4::testing::{icmp, icmp_error, refragment, tcp, udp};

    const ME: [u8; 4] = [10, 66, 0, 1];
    const PEER: [u8; 4] = [10, 66, 0, 2];

    fn verdict(replies: &Replies, bytes: &[u8], now: Instant) -> Verdict {
        replies.admit(&Ipv4::parse(bytes).unwrap(), now)
    }

    #[test]
    fn a_peer_that_may_not_reach_us_cannot_open_anything_but_its_answers_pass() {
        let replies = Replies::default();
        let now = Instant::now();
        assert_eq!(verdict(&replies, &tcp(PEER, ME, (40000, 22), 0x02), now), Verdict::NewConversation, "SYN");
        assert_eq!(verdict(&replies, &tcp(PEER, ME, (22, 40000), 0x12), now), Verdict::Pass, "回给本机的 SYN-ACK");
        assert_eq!(verdict(&replies, &tcp(PEER, ME, (22, 40000), 0x10), now), Verdict::Pass);
        assert_eq!(verdict(&replies, &icmp(PEER, ME, 8), now), Verdict::NewConversation, "ping 的请求");
        assert_eq!(verdict(&replies, &icmp(PEER, ME, 0), now), Verdict::Pass, "ping 的回答");
        let syn = tcp(ME, PEER, (40000, 22), 0x02);
        assert_eq!(verdict(&replies, &icmp_error(PEER, ME, 3, 3, &syn[..28]), now), Verdict::Pass, "不可达");
        assert_eq!(verdict(&replies, &icmp(PEER, ME, 3), now), Verdict::NewConversation, "没有带回原包的不可达");
        assert_eq!(verdict(&replies, &udp(PEER, ME, (53, 5000), b"x"), now), Verdict::NewConversation, "没有先例的 UDP");
        let gre = crate::ipv4::testing::packet(47, PEER, ME, b"x");
        assert_eq!(verdict(&replies, &gre, now), Verdict::NewConversation, "看不出是不是回包的协议");
    }

    #[test]
    fn a_later_fragment_alone_and_a_cut_tcp_segment_do_not_pass() {
        let replies = Replies::default();
        let now = Instant::now();
        let later = refragment(udp(PEER, ME, (53, 5000), b"rest of it"), 4, 8, false);
        assert_eq!(verdict(&replies, &later, now), Verdict::NewConversation, "只有后面的片，判断不了");
        let gre_later = refragment(crate::ipv4::testing::packet(47, PEER, ME, b"x"), 5, 8, true);
        assert_eq!(verdict(&replies, &gre_later, now), Verdict::NewConversation);
        let cut_ack = refragment(tcp(PEER, ME, (22, 40000), 0x10), 6, 0, true);
        assert_eq!(verdict(&replies, &cut_ack, now), Verdict::NewConversation, "切开的 TCP 看不准标志");
    }

    #[test]
    fn an_icmp_error_about_udp_passes_only_for_a_conversation_we_started() {
        let mut replies = Replies::default();
        let now = Instant::now();
        let query = udp(ME, PEER, (5000, 53), b"query");
        let error = |quoted: &[u8]| icmp_error(PEER, ME, 3, 3, quoted);
        assert_eq!(verdict(&replies, &error(&query[..28]), now), Verdict::NewConversation, "本节点没发过");
        replies.sent(&Ipv4::parse(&query).unwrap(), now);
        assert_eq!(verdict(&replies, &error(&query[..28]), now), Verdict::Pass, "端口不可达：说的是本节点刚发的");
        let other_port = udp(ME, PEER, (5001, 53), b"query");
        assert_eq!(verdict(&replies, &error(&other_port[..28]), now), Verdict::NewConversation, "本机别的端口");
        assert_eq!(verdict(&replies, &error(&query[..20]), now), Verdict::NewConversation, "看不到端口");
        assert_eq!(verdict(&replies, &error(&query[..12]), now), Verdict::NewConversation, "带回来的头都不完整");
        let later = refragment(query.clone(), 1, 8, false);
        assert_eq!(verdict(&replies, &error(&later[..28]), now), Verdict::NewConversation, "说的是后面的一片，没有端口");
        let syn = tcp(ME, PEER, (40000, 22), 0x02);
        assert_eq!(verdict(&replies, &error(&syn[..28]), now), Verdict::Pass, "TCP 的由内核按序号核对");
    }

    #[test]
    fn udp_answers_pass_only_from_the_address_and_port_we_sent_to_and_only_for_a_while() {
        let mut replies = Replies::default();
        let now = Instant::now();
        replies.sent(&Ipv4::parse(&udp(ME, PEER, (5000, 53), b"query")).unwrap(), now);
        assert_eq!(replies.len(), 1);
        assert_eq!(verdict(&replies, &udp(PEER, ME, (53, 5000), b"answer"), now), Verdict::Pass);
        assert_eq!(verdict(&replies, &udp(PEER, ME, (54, 5000), b"x"), now), Verdict::NewConversation, "别的端口");
        assert_eq!(verdict(&replies, &udp(PEER, ME, (53, 5001), b"x"), now), Verdict::NewConversation, "本机别的端口");
        assert_eq!(verdict(&replies, &udp([10, 66, 0, 3], ME, (53, 5000), b"x"), now), Verdict::NewConversation, "别的对端");
        assert_eq!(verdict(&replies, &udp(PEER, ME, (53, 5000), b"late"), now + REPLY_WINDOW), Verdict::NewConversation, "过期了");
        replies.sent(&Ipv4::parse(&tcp(ME, PEER, (40000, 22), 0x02)).unwrap(), now);
        assert_eq!(replies.len(), 1, "TCP 不用记");
    }

    #[test]
    fn the_table_never_grows_past_its_limit_and_drops_the_stale_then_the_oldest() {
        let mut replies = Replies::default();
        let start = Instant::now();
        let port = |n: usize| u16::try_from(n % 60000 + 1).unwrap();
        for n in 0..MAX_REPLIES {
            let at = start + Duration::from_millis(u64::try_from(n).unwrap());
            replies.sent(&Ipv4::parse(&udp(ME, PEER, (port(n), 53), b"")).unwrap(), at);
        }
        assert_eq!(replies.len(), MAX_REPLIES);
        let later = start + Duration::from_secs(30);
        replies.sent(&Ipv4::parse(&udp(ME, PEER, (61000, 53), b"")).unwrap(), later);
        assert_eq!(replies.len(), MAX_REPLIES, "满了扔一条最旧的");
        assert_eq!(verdict(&replies, &udp(PEER, ME, (53, port(0)), b""), later), Verdict::NewConversation, "最旧的那条没了");
        assert_eq!(verdict(&replies, &udp(PEER, ME, (53, port(1)), b""), later), Verdict::Pass);
        let much_later = start + REPLY_WINDOW + Duration::from_secs(6);
        replies.sent(&Ipv4::parse(&udp(ME, PEER, (62000, 53), b"")).unwrap(), much_later);
        assert_eq!(replies.len(), 2, "满了的时候先扔过期的：只剩刚才和这一条");
    }
}
