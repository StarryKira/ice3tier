//! 骨干延迟矩阵在节点一侧的样子：每个 relay 最新的那一行。帧的格式和它是怎么传过来的见 [`i3t_proto::backbone`]。
//!
//! 这里只管存和查。行是不是从有会话的 relay 来的、签名对不对，由控制任务在放进来之前核对。
//! 每一行记着收到的时刻，过期作废（白皮书 §7.3），所以一个不再发行的 relay 过一阵就从矩阵里消失，
//! 经它的多级路径也就不再是候选。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bytes::Bytes;
use i3t_proto::NodeId;
use i3t_proto::backbone::{LinkQuality, RelayState, Row};

use crate::timing::ESTIMATE_TTL;

#[derive(Debug)]
struct Stored {
    row: Row,
    /// 原样的帧（带着主人的签名）。本节点是 relay 时要把它传给自己的会话。
    frame: Bytes,
    received: Instant,
}

/// 手上的矩阵。
#[derive(Debug, Default)]
pub(crate) struct Matrix {
    rows: HashMap<NodeId, Stored>,
}

impl Matrix {
    fn fresh(&self, origin: NodeId, now: Instant) -> Option<&Stored> {
        self.rows.get(&origin).filter(|stored| now.saturating_duration_since(stored.received) < ESTIMATE_TTL)
    }

    /// `origin` 的这一行是不是比手上的新。不是的话不值得去验它的签名：重放一行旧的换不掉新的。
    pub(crate) fn is_news(&self, origin: NodeId, seq: u64, now: Instant) -> bool {
        self.fresh(origin, now).is_none_or(|stored| seq > stored.row.seq)
    }

    /// 放进一行已经验过签名的。返回这个 relay 的状态是不是变了（包括头一次知道它不是正常服务的状态）。
    pub(crate) fn insert(&mut self, row: Row, frame: Bytes, now: Instant) -> bool {
        let before = self.state(row.origin, now);
        let changed = before != row.state;
        self.rows.insert(row.origin, Stored { row, frame, received: now });
        changed
    }

    /// 从 `from` 出发连得上的 relay，按它自己测的。`from` 的行没有（或者过期了）时什么都没有；
    /// 行里没有列的、写着连不上的 relay 不在里面。
    pub(crate) fn links_from(&self, from: NodeId, now: Instant) -> impl Iterator<Item = (NodeId, LinkQuality)> + '_ {
        let links = self.fresh(from, now).map_or(&[][..], |stored| &stored.row.links[..]);
        links.iter().filter_map(|link| Some((link.relay, link.quality?)))
    }

    /// `relay` 现在的状态。没有它的行时当作正常：没有听说它有事。
    pub(crate) fn state(&self, relay: NodeId, now: Instant) -> RelayState {
        self.fresh(relay, now).map_or(RelayState::Serving, |stored| stored.row.state)
    }

    /// `relay` 的行是多久之前收到的。没有它的行时是 `None`。
    pub(crate) fn age(&self, relay: NodeId, now: Instant) -> Option<Duration> {
        self.fresh(relay, now).map(|stored| now.saturating_duration_since(stored.received))
    }

    /// `relay` 上报的带宽占用百分比。没有它的行时是 0。
    pub(crate) fn load_percent(&self, relay: NodeId, now: Instant) -> u8 {
        self.fresh(relay, now).map_or(0, |stored| stored.row.load_percent)
    }

    /// 还没有过期的行，以及它们是多久之前收到的。
    pub(crate) fn rows(&self, now: Instant) -> impl Iterator<Item = (&Row, Duration)> + '_ {
        self.rows.values().filter_map(move |stored| {
            let age = now.saturating_duration_since(stored.received);
            (age < ESTIMATE_TTL).then_some((&stored.row, age))
        })
    }

    /// 还没有过期的行原样的帧。
    pub(crate) fn frames(&self, now: Instant) -> impl Iterator<Item = &Bytes> + '_ {
        self.rows.values().filter(move |stored| now.saturating_duration_since(stored.received) < ESTIMATE_TTL).map(|stored| &stored.frame)
    }

    /// 只留 `keep` 认的 relay 的行（netmap 里已经没有的 relay 不再留着），顺便扔掉过期的。
    pub(crate) fn retain(&mut self, now: Instant, keep: impl Fn(NodeId) -> bool) {
        self.rows.retain(|origin, stored| keep(*origin) && now.saturating_duration_since(stored.received) < ESTIMATE_TTL);
    }
}

#[cfg(test)]
mod tests {
    use i3t_proto::backbone::Link;

    use super::*;

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn quality(rtt_ms: u16) -> LinkQuality {
        LinkQuality { rtt_ms, jitter_ms: 1, loss_half_percent: 0 }
    }

    fn row(origin: u32, seq: u64, state: RelayState, links: &[(u32, Option<u16>)]) -> Row {
        let links = links.iter().map(|&(relay, rtt)| Link { relay: node(relay), quality: rtt.map(quality) }).collect();
        Row { origin: node(origin), seq, state, load_percent: 30, links }
    }

    impl Matrix {
        fn link(&self, from: NodeId, to: NodeId, now: Instant) -> Option<LinkQuality> {
            self.links_from(from, now).find(|(relay, _)| *relay == to).map(|(_, quality)| quality)
        }
    }

    fn insert(matrix: &mut Matrix, row: Row, now: Instant) -> bool {
        let frame = Bytes::from(row.to_bytes(&[0; 64]));
        matrix.insert(row, frame, now)
    }

    #[test]
    fn links_are_read_from_the_row_of_the_relay_that_measured_them() {
        let now = Instant::now();
        let mut matrix = Matrix::default();
        insert(&mut matrix, row(8, 1, RelayState::Serving, &[(9, Some(140)), (7, None)]), now);
        assert_eq!(matrix.link(node(8), node(9), now), Some(quality(140)));
        assert_eq!(matrix.link(node(8), node(7), now), None, "行里写着连不上");
        assert_eq!(matrix.link(node(8), node(6), now), None, "行里没有列");
        assert_eq!(matrix.link(node(9), node(8), now), None, "9 的行还没有收到，不拿 8 的测量值代替");
        assert_eq!(matrix.links_from(node(8), now).collect::<Vec<_>>(), [(node(9), quality(140))]);
        assert_eq!(matrix.links_from(node(9), now).count(), 0);
        assert_eq!(matrix.load_percent(node(8), now), 30);
        assert_eq!(matrix.load_percent(node(9), now), 0);
    }

    #[test]
    fn only_a_newer_row_replaces_the_one_held() {
        let now = Instant::now();
        let mut matrix = Matrix::default();
        assert!(matrix.is_news(node(8), 5, now), "头一行总是新的");
        insert(&mut matrix, row(8, 5, RelayState::Serving, &[(9, Some(140))]), now);
        assert!(!matrix.is_news(node(8), 5, now), "同一行再来一遍");
        assert!(!matrix.is_news(node(8), 4, now), "重放的旧行");
        assert!(matrix.is_news(node(8), 6, now));
        assert!(matrix.is_news(node(9), 1, now), "别的 relay 各算各的");
    }

    #[test]
    fn a_state_change_is_reported_once() {
        let now = Instant::now();
        let mut matrix = Matrix::default();
        assert_eq!(matrix.state(node(8), now), RelayState::Serving, "没有听说过的 relay 当作正常");
        assert!(!insert(&mut matrix, row(8, 1, RelayState::Serving, &[]), now));
        assert!(insert(&mut matrix, row(8, 2, RelayState::Draining, &[]), now), "变成即将下线");
        assert!(!insert(&mut matrix, row(8, 3, RelayState::Draining, &[]), now), "还是即将下线，不算变化");
        assert_eq!(matrix.state(node(8), now), RelayState::Draining);
        assert!(insert(&mut matrix, row(8, 4, RelayState::Serving, &[]), now), "它回来了");
        assert!(insert(&mut matrix, row(9, 1, RelayState::Overloaded, &[]), now), "头一行就是过载");
    }

    #[test]
    fn rows_expire_and_then_any_sequence_number_is_news_again() {
        let start = Instant::now();
        let mut matrix = Matrix::default();
        insert(&mut matrix, row(8, 100, RelayState::Draining, &[(9, Some(140))]), start);
        let later = start + ESTIMATE_TTL;
        assert_eq!(matrix.link(node(8), node(9), later - Duration::from_millis(1)), Some(quality(140)));
        assert_eq!(matrix.link(node(8), node(9), later), None, "过期作废");
        assert_eq!(matrix.state(node(8), later), RelayState::Serving, "过期的通知也不再算数");
        assert_eq!((matrix.rows(later).count(), matrix.frames(later).count()), (0, 0));
        assert!(matrix.is_news(node(8), 1, later), "relay 重启之后时钟往回跳了也不会一直被拒绝");
    }

    #[test]
    fn rows_of_relays_that_left_the_netmap_are_dropped() {
        let now = Instant::now();
        let mut matrix = Matrix::default();
        insert(&mut matrix, row(8, 1, RelayState::Serving, &[]), now);
        insert(&mut matrix, row(9, 1, RelayState::Serving, &[]), now);
        assert_eq!(matrix.frames(now).count(), 2);
        matrix.retain(now, |relay| relay == node(9));
        assert_eq!(matrix.rows(now).map(|(row, _)| row.origin).collect::<Vec<_>>(), [node(9)]);
    }
}
