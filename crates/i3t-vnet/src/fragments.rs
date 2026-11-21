//! 对端发来的被切开的 IPv4 包（IP 分片）：在这里攒齐，核对过才一起写进网卡。
//!
//! 内核拼片用的缓存是整台机器（一个网络命名空间）共用的：Linux 上默认 4MB，拼不齐的包要等 30 秒才扔，满了之后哪块网卡上来的片
//! 都拼不了。对端发来一些永远拼不齐的片就能把它占满。所以拼不齐的片不交给内核：同一个包的片（来源、标识、协议相同）在这里攒着，
//! 从头到尾一个字节不缺、互不重叠、最后一片也到了，才按先后一起写进网卡，内核一收到就拼好、放掉。拼不齐的过一会儿在这里扔掉，
//! 占的是这个进程里有上限的内存，挤掉的也只是那个对端自己的。片乱了序也拼得起来。
//!
//! 包让不让进来看第一片（端口、ICMP 的类型都在那里）：攒齐之后拿第一片去判断（[`crate::engine`]），不让进来的整个扔掉，
//! 后面的片不会单独溜进去。重叠的片整个扔掉：有的系统拼片时让后到的盖掉先到的，后面的片因此能改写第一片里的端口和标志
//! （RFC 1858、RFC 3128）。TCP 不该被切，切开的 TCP 在这之前就扔了。
//!
//! 内存的上限：每个对端最多同时攒 [`MAX_PER_PEER`] 个包、[`PEER_BYTES`] 字节，超出时扔它自己最早的；所有对端加起来最多
//! [`TOTAL_BYTES`] 字节，超出时扔攒得最多的那个对端最早的。一个包从先到的那片算起最多等 [`FRAGMENT_WAIT`]。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::ipv4::Ipv4;

/// 一个对端最多同时攒多少个没齐的包。正常时一个包的片是一口气发出来的，同时没齐的只有一两个。
pub const MAX_PER_PEER: usize = 64;

/// 一个对端攒着的片最多占多少字节（连同每片的开销 [`PIECE_OVERHEAD`]）。装得下几个最大的包（65535 字节）。
pub const PEER_BYTES: usize = 256 * 1024;

/// 所有对端加起来最多占多少字节。
pub const TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// 一个包最多等多久。片是一口气发出来的，正常时差不了一个往返；等不到多半是丢了，上面的协议自己重发。
pub const FRAGMENT_WAIT: Duration = Duration::from_secs(5);

/// 一个包最多多少片。最小的 MTU（576 字节）把最大的包切开是 120 片左右，留些余量。
pub const MAX_PIECES: usize = 192;

/// 每一片除了它自己的字节，记账时另外算这么多：很小的片攒不了太多。
const PIECE_OVERHEAD: usize = 64;

/// 拼起来的内容（头后面的部分）最长多少：IPv4 的包最长 65535 字节，头至少 20 字节。
const MAX_CONTENT: usize = 65535 - 20;

/// 多久扫一次过期的包。
const SWEEP_EVERY: Duration = Duration::from_secs(1);

/// 一片收下之后怎样了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gathered {
    /// 还没齐，攒着。
    Waiting,
    /// 齐了：这个包的所有片，按在包里的先后，第一片在最前面。
    Complete(Vec<Bytes>),
    /// 这一片自己就不对，或者和攒着的对不上：这个包攒着的片连同这一片都扔了。
    Broken(&'static str),
}

/// 攒着的片。
#[derive(Debug)]
pub struct Fragments {
    peers: HashMap<Ipv4Addr, Peer>,
    /// 所有对端加起来占的字节。
    total: usize,
    swept: Instant,
}

impl Default for Fragments {
    fn default() -> Self {
        Self { peers: HashMap::new(), total: 0, swept: Instant::now() }
    }
}

/// 一个包在它的来源里的身份：标识和协议（目的都是本节点的虚拟 IP）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    id: u16,
    protocol: u8,
}

/// 一个对端攒着的包。
#[derive(Debug, Default)]
struct Peer {
    bytes: usize,
    datagrams: HashMap<Key, Datagram>,
}

/// 一个没齐的包。
#[derive(Debug)]
struct Datagram {
    started: Instant,
    /// 最后一片说的内容长度；最后一片还没到时是 `None`。
    end: Option<usize>,
    /// 到了的内容字节数。片之间不重叠、都在最后一片说的长度以内，所以它等于 `end` 时就齐了。
    content: usize,
    /// 占的字节（记账用）。
    cost: usize,
    pieces: Vec<Piece>,
}

#[derive(Debug)]
struct Piece {
    /// 这一片的内容在原包里的范围（字节，头后面的部分算起）。
    start: usize,
    end: usize,
    packet: Bytes,
}

impl Fragments {
    /// 收下对端发来的一片：`ip` 是读过的 `packet`，来源、目的已经核对过。
    pub fn accept(&mut self, ip: &Ipv4<'_>, packet: Bytes, now: Instant) -> Gathered {
        self.sweep(now);
        let source = ip.source();
        let key = Key { id: ip.identification(), protocol: ip.protocol() };
        let start = ip.fragment_offset();
        let end = start + ip.payload().len();
        let last = !ip.more_fragments();
        if let Err(why) = check_alone(start, end, last) {
            self.remove(source, key);
            return Gathered::Broken(why);
        }
        let cost = packet.len() + PIECE_OVERHEAD;
        self.make_room(source, key, cost);
        let peer = self.peers.entry(source).or_default();
        let datagram = peer.datagrams.entry(key).or_insert_with(|| Datagram::new(now));
        if let Err(why) = datagram.add(Piece { start, end, packet }, last) {
            self.remove(source, key);
            return Gathered::Broken(why);
        }
        datagram.cost += cost;
        peer.bytes += cost;
        self.total += cost;
        if !datagram.is_complete() {
            return Gathered::Waiting;
        }
        let mut pieces = self.remove(source, key).map(|datagram| datagram.pieces).unwrap_or_default();
        pieces.sort_unstable_by_key(|piece| piece.start);
        Gathered::Complete(pieces.into_iter().map(|piece| piece.packet).collect())
    }

    /// 还没齐的包有几个。
    #[must_use]
    pub fn pending(&self) -> usize {
        self.peers.values().map(|peer| peer.datagrams.len()).sum()
    }

    /// 攒着的片占的字节（连同每片的开销）。
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.total
    }

    /// 给 `source` 的一片（`cost` 字节，属于包 `key`）腾地方：先按这个对端自己的上限扔它最早的包，再按总的上限扔攒得最多的
    /// 那个对端最早的包。正在攒的这个包不扔。
    fn make_room(&mut self, source: Ipv4Addr, key: Key, cost: usize) {
        while let Some(peer) = self.peers.get(&source) {
            let new = !peer.datagrams.contains_key(&key);
            if !(new && peer.datagrams.len() >= MAX_PER_PEER) && peer.bytes + cost <= PEER_BYTES {
                break;
            }
            let Some(oldest) = peer.oldest(Some(key)) else { break };
            self.remove(source, oldest);
        }
        while self.total + cost > TOTAL_BYTES {
            let victim = self
                .peers
                .iter()
                .filter_map(|(ip, peer)| peer.oldest((*ip == source).then_some(key)).map(|oldest| (peer.bytes, *ip, oldest)))
                .max_by_key(|(bytes, ..)| *bytes);
            let Some((_, ip, oldest)) = victim else { break };
            self.remove(ip, oldest);
        }
    }

    /// 拿掉一个包，返回它。
    fn remove(&mut self, source: Ipv4Addr, key: Key) -> Option<Datagram> {
        let peer = self.peers.get_mut(&source)?;
        let datagram = peer.datagrams.remove(&key)?;
        peer.bytes -= datagram.cost;
        self.total -= datagram.cost;
        if peer.datagrams.is_empty() {
            self.peers.remove(&source);
        }
        Some(datagram)
    }

    /// 扔掉等太久的包。每 [`SWEEP_EVERY`] 最多扫一遍。
    fn sweep(&mut self, now: Instant) {
        if now.saturating_duration_since(self.swept) < SWEEP_EVERY {
            return;
        }
        self.swept = now;
        let mut freed = 0;
        self.peers.retain(|_, peer| {
            peer.datagrams.retain(|_, datagram| {
                let keep = now.saturating_duration_since(datagram.started) < FRAGMENT_WAIT;
                if !keep {
                    peer.bytes -= datagram.cost;
                    freed += datagram.cost;
                }
                keep
            });
            !peer.datagrams.is_empty()
        });
        self.total -= freed;
    }
}

/// 只看这一片自己就知道不对的。
fn check_alone(start: usize, end: usize, last: bool) -> Result<(), &'static str> {
    if end == start {
        return Err("空的片");
    }
    if !last && !(end - start).is_multiple_of(8) {
        return Err("不是最后一片，长度却不是 8 的倍数");
    }
    if end > MAX_CONTENT {
        return Err("拼起来比 IPv4 的包能有的还长");
    }
    Ok(())
}

impl Peer {
    /// 最早开始攒的包，`skip` 除外。
    fn oldest(&self, skip: Option<Key>) -> Option<Key> {
        self.datagrams.iter().filter(|(key, _)| Some(**key) != skip).min_by_key(|(_, datagram)| datagram.started).map(|(key, _)| *key)
    }
}

impl Datagram {
    fn new(now: Instant) -> Self {
        Self { started: now, end: None, content: 0, cost: 0, pieces: Vec::new() }
    }

    /// 加上一片。对不上的时候不动，说为什么。
    fn add(&mut self, piece: Piece, last: bool) -> Result<(), &'static str> {
        if self.pieces.len() >= MAX_PIECES {
            return Err("一个包的片太多");
        }
        if self.pieces.iter().any(|other| other.start < piece.end && piece.start < other.end) {
            return Err("片和先到的片重叠");
        }
        if last {
            if self.end.is_some() {
                return Err("最后一片来了两次");
            }
            if self.pieces.iter().any(|other| other.end > piece.end) {
                return Err("有片超出了最后一片说的长度");
            }
            self.end = Some(piece.end);
        } else if self.end.is_some_and(|end| piece.end > end) {
            return Err("片超出了最后一片说的长度");
        }
        self.content += piece.end - piece.start;
        self.pieces.push(piece);
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.end == Some(self.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipv4::testing::{icmp, refragment, split, udp};

    const ME: [u8; 4] = [10, 66, 0, 1];
    const B: [u8; 4] = [10, 66, 0, 2];
    const C: [u8; 4] = [10, 66, 0, 3];

    fn give(fragments: &mut Fragments, piece: &[u8], now: Instant) -> Gathered {
        fragments.accept(&Ipv4::parse(piece).unwrap(), Bytes::copy_from_slice(piece), now)
    }

    fn complete(pieces: &[Vec<u8>]) -> Gathered {
        Gathered::Complete(pieces.iter().map(|piece| Bytes::copy_from_slice(piece)).collect())
    }

    #[test]
    fn the_pieces_come_out_together_and_in_order_once_all_have_arrived() {
        let mut fragments = Fragments::default();
        let now = Instant::now();
        let pieces = split(&udp(B, ME, (53, 5000), &[7; 3000]), 1, 1280);
        assert_eq!(pieces.len(), 3);
        assert_eq!(give(&mut fragments, &pieces[2], now), Gathered::Waiting, "最后一片先到");
        assert_eq!(give(&mut fragments, &pieces[0], now), Gathered::Waiting);
        assert_eq!(fragments.pending(), 1);
        assert_eq!(give(&mut fragments, &pieces[1], now), complete(&pieces), "齐了：第一片在前，按先后");
        assert_eq!((fragments.pending(), fragments.bytes()), (0, 0));
    }

    #[test]
    fn pieces_of_different_packets_are_kept_apart() {
        let mut fragments = Fragments::default();
        let now = Instant::now();
        let from_b = split(&udp(B, ME, (53, 5000), &[1; 100]), 9, 48);
        let from_c = split(&udp(C, ME, (53, 5000), &[2; 100]), 9, 48);
        let other_protocol = split(&icmp(B, ME, 0).into_iter().chain([3; 92]).collect::<Vec<u8>>(), 9, 48);
        for pieces in [&from_b, &from_c, &other_protocol] {
            assert_eq!(give(&mut fragments, &pieces[0], now), Gathered::Waiting);
        }
        assert_eq!(fragments.pending(), 3, "标识一样，来源或者协议不一样，是不同的包");
        for pieces in [&from_b, &from_c] {
            assert_eq!(give(&mut fragments, &pieces[1], now), Gathered::Waiting);
            assert_eq!(give(&mut fragments, &pieces[2], now), complete(pieces));
        }
    }

    #[test]
    fn overlapping_or_inconsistent_pieces_break_the_packet() {
        let now = Instant::now();
        let whole = udp(B, ME, (53, 5000), &[5; 100]);
        let pieces = split(&whole, 3, 48);
        let mut fragments = Fragments::default();
        give(&mut fragments, &pieces[0], now);
        // 盖住第一片的后半截（偏移 8）：有的系统拼的时候让它盖掉先到的。
        let mut overlap = whole[..20 + 48].to_vec();
        overlap[2..4].copy_from_slice(&68u16.to_be_bytes());
        assert_eq!(give(&mut fragments, &refragment(overlap, 3, 8, true), now), Gathered::Broken("片和先到的片重叠"));
        assert_eq!(fragments.pending(), 0, "攒着的也扔了");
        assert_eq!(give(&mut fragments, &pieces[1], now), Gathered::Waiting, "之后再来的从头攒");

        let mut fragments = Fragments::default();
        give(&mut fragments, &pieces[2], now);
        assert_eq!(give(&mut fragments, &pieces[2], now), Gathered::Broken("片和先到的片重叠"), "同一片来两次");
        give(&mut fragments, &pieces[2], now);
        let longer_last = refragment(pieces[1].clone(), 3, 48, false);
        assert_eq!(give(&mut fragments, &longer_last, now), Gathered::Broken("最后一片来了两次"));
        give(&mut fragments, &pieces[1], now);
        assert_eq!(give(&mut fragments, &refragment(pieces[2].clone(), 3, 40, false), now), Gathered::Broken("片和先到的片重叠"));

        let mut fragments = Fragments::default();
        give(&mut fragments, &pieces[2], now);
        let beyond = refragment(pieces[0].clone(), 3, 112, true);
        assert_eq!(give(&mut fragments, &beyond, now), Gathered::Broken("片超出了最后一片说的长度"));
        give(&mut fragments, &refragment(pieces[1].clone(), 3, 200, true), now);
        assert_eq!(give(&mut fragments, &pieces[2], now), Gathered::Broken("有片超出了最后一片说的长度"));
    }

    #[test]
    fn a_piece_that_is_wrong_by_itself_is_refused() {
        let now = Instant::now();
        let mut fragments = Fragments::default();
        let odd = refragment(udp(B, ME, (53, 5000), &[1; 13]), 4, 0, true);
        assert_eq!(give(&mut fragments, &odd, now), Gathered::Broken("不是最后一片，长度却不是 8 的倍数"));
        let empty = refragment(crate::ipv4::testing::packet(17, B, ME, &[]), 4, 64, false);
        assert_eq!(give(&mut fragments, &empty, now), Gathered::Broken("空的片"));
        let huge = refragment(udp(B, ME, (53, 5000), &[1; 40]), 4, 65512, false);
        assert_eq!(give(&mut fragments, &huge, now), Gathered::Broken("拼起来比 IPv4 的包能有的还长"));
        assert_eq!(fragments.pending(), 0);
    }

    #[test]
    fn packets_that_never_complete_are_dropped_after_a_while() {
        let start = Instant::now();
        let mut fragments = Fragments::default();
        let pieces = split(&udp(B, ME, (53, 5000), &[1; 100]), 5, 48);
        give(&mut fragments, &pieces[0], start);
        give(&mut fragments, &pieces[1], start);
        let other = split(&udp(B, ME, (53, 5000), &[1; 100]), 6, 48);
        give(&mut fragments, &other[0], start + FRAGMENT_WAIT);
        assert_eq!(fragments.pending(), 1, "等太久的扔了");
        assert_eq!(give(&mut fragments, &pieces[2], start + FRAGMENT_WAIT), Gathered::Waiting, "最后一片来晚了：前面的已经没了");
    }

    #[test]
    fn a_peer_cannot_hold_more_than_its_share_and_only_loses_its_own_oldest() {
        let start = Instant::now();
        let mut fragments = Fragments::default();
        let first = |source: [u8; 4], id: u16| split(&udp(source, ME, (53, 5000), &[1; 100]), id, 48).remove(0);
        let at = |n: u16| start + Duration::from_millis(u64::from(n));
        give(&mut fragments, &first(C, 1), start);
        for id in 0..u16::try_from(MAX_PER_PEER + 5).unwrap() {
            assert_eq!(give(&mut fragments, &first(B, id), at(id)), Gathered::Waiting);
        }
        assert_eq!(fragments.pending(), MAX_PER_PEER + 1, "B 最多攒这么多个，C 的那个还在");
        let rest_of_c = split(&udp(C, ME, (53, 5000), &[1; 100]), 1, 48);
        give(&mut fragments, &rest_of_c[1], at(100));
        assert_eq!(give(&mut fragments, &rest_of_c[2], at(100)), complete(&rest_of_c));
        let rest_of_oldest = split(&udp(B, ME, (53, 5000), &[1; 100]), 0, 48);
        assert_eq!(give(&mut fragments, &rest_of_oldest[1], at(100)), Gathered::Waiting);
        assert_eq!(give(&mut fragments, &rest_of_oldest[2], at(100)), Gathered::Waiting, "B 最早的那个已经被挤掉了，拼不齐");

        // 字节数的上限：几个很大的包，都差最后一片。
        let mut fragments = Fragments::default();
        let big = |id: u16| split(&udp(B, ME, (53, 5000), &[1; 60000]), id, 1480);
        for id in 0..6 {
            let pieces = big(id);
            for piece in &pieces[..pieces.len() - 1] {
                give(&mut fragments, piece, at(id));
                assert!(fragments.bytes() <= PEER_BYTES, "{}", fragments.bytes());
            }
        }
        assert!(fragments.pending() < 6, "早的被挤掉了");
        let newest = big(5);
        assert_eq!(give(&mut fragments, newest.last().unwrap(), at(10)), complete(&newest), "正在攒的那个不挤");
    }

    #[test]
    fn all_peers_together_stay_under_the_total_and_the_biggest_gives_way() {
        let start = Instant::now();
        let mut fragments = Fragments::default();
        let mut n = 0u64;
        for host in 2..=70u8 {
            for id in 0..u16::try_from(MAX_PER_PEER).unwrap() {
                n += 1;
                let piece = split(&udp([10, 66, 1, host], ME, (53, 5000), &[1; 3000]), id, 1480).remove(0);
                give(&mut fragments, &piece, start + Duration::from_micros(n));
                assert!(fragments.bytes() <= TOTAL_BYTES);
            }
        }
        assert!(fragments.bytes() > TOTAL_BYTES - PEER_BYTES, "满了：{}", fragments.bytes());
        let peers = fragments.peers.len();
        let small = split(&udp(B, ME, (53, 5000), &[1; 100]), 1, 48);
        give(&mut fragments, &small[0], start + Duration::from_secs(1));
        assert_eq!(fragments.peers.len(), peers + 1, "新来的对端也攒得下");
        give(&mut fragments, &small[1], start + Duration::from_secs(1));
        assert_eq!(give(&mut fragments, &small[2], start + Duration::from_secs(1)), complete(&small));
    }
}
