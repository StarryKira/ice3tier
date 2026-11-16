//! 候选路径和选路（白皮书 §7.1、§7.4、§7.5）。
//!
//! 这里没有 I/O，也不碰会话：外面把此刻的测量值整理成一张图交进来（本节点到各个 relay、骨干矩阵、
//! 目的节点挂靠的 relay、直连），这里列出全部候选路径、算出代价，再按选路模式挑一条。
//!
//! 延迟优先模式的规则：
//!
//! 1. 局域网直连存在就用它，不参与比较。
//! 2. 现在没有路，或者现在的路不在候选里了（断了、relay 要下线）：立刻换到代价最低的候选，不等迟滞。
//! 3. 否则每个评估窗口比一次：有候选比现在的路便宜至少 `max(5ms, 10%)`，并且连续 3 个窗口都是这样，才换过去。
//!    从一条路换到另一条路之后（不管是因为代价还是因为原来的路断了）至少停留 30 秒，才会再因为代价换路；
//!    头一次有路不算换路，所以连接刚建立时不用白等。
//! 4. 挑"最低的"时，代价相差在 5% 以内的候选之间随机取两个，用路径上 relay 负载低的那个。
//!    这样大家不会同时挤到同一个"最优"的 relay 上，又比纯随机更偏向空闲的。

use std::net::SocketAddr;
use std::time::Instant;

use i3t_proto::NodeId;
use i3t_proto::backbone::LinkQuality;
use i3t_proto::datagram::RelayPath;

use crate::cost::{self, Segment};
use crate::matrix::Matrix;
use crate::timing::{MIN_DWELL, SWITCH_WINDOWS};

/// 新的路至少要便宜这么多毫秒……
const SWITCH_MARGIN_MS: f64 = 5.0;
/// ……并且至少是现在这条路的代价的这个比例，取两者里大的。
const SWITCH_MARGIN_RATIO: f64 = 0.10;
/// 代价和最低的相差在这个比例以内的候选算"差不多"，在它们之间按负载二选一。
const NEAR_EQUAL_RATIO: f64 = 0.05;

/// 一条路径是哪一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PathId {
    /// 直连对端的这个地址。
    Direct(SocketAddr),
    /// 按顺序经过这些 relay。
    Relays(RelayPath),
}

/// 路径上的一段：从谁到谁，测出来是多少。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Leg {
    pub(crate) from: NodeId,
    pub(crate) to: NodeId,
    pub(crate) segment: Segment,
}

/// 一条候选路径和它的代价。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) path: PathId,
    pub(crate) legs: Vec<Leg>,
    pub(crate) cost_ms: f64,
    /// 路径上负载最高的那个 relay 的带宽占用百分比；直连是 0。
    pub(crate) load_percent: u8,
    /// 是局域网直连。
    pub(crate) lan: bool,
}

/// 到一个对端的直连，已经验证过、现在能用的。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DirectLink {
    pub(crate) addr: SocketAddr,
    pub(crate) segment: Segment,
    pub(crate) lan: bool,
}

/// 此刻的网络在本节点眼里的样子。只包含确实连得上的边。
pub(crate) struct Graph<'a> {
    pub(crate) me: NodeId,
    /// 一条路径最多经过几个 relay。
    pub(crate) max_hops: usize,
    /// 本节点现在能把包交给它的 relay（会话是通的），和到它的链路质量。
    pub(crate) entries: &'a [(NodeId, Segment)],
    pub(crate) matrix: &'a Matrix,
    pub(crate) now: Instant,
    /// 不能用的 relay：即将下线的、过载的。
    pub(crate) avoid: &'a dyn Fn(NodeId) -> bool,
    /// 刚被告知走不通的骨干连接（从前一个到后一个）。
    pub(crate) broken: &'a dyn Fn(NodeId, NodeId) -> bool,
}

/// 要去的对端。
pub(crate) struct Destination<'a> {
    pub(crate) id: NodeId,
    /// 能把包交到对端手上的 relay，和对端到它的链路质量。对端自己就是 relay 时它自己也在这里，质量是 `None`（没有最后那一段）。
    pub(crate) exits: &'a [(NodeId, Option<Segment>)],
    pub(crate) direct: Option<DirectLink>,
}

/// 列出到 `dest` 的全部候选路径：直连（如果有），以及经 1 到 `max_hops` 个 relay 的每一条走得通的路径。
///
/// relay 的数量是个位数、级数不超过 4，所以直接穷举；代价里丢包那一项不是各段相加，没法用普通的最短路算法。
pub(crate) fn candidates(graph: &Graph<'_>, dest: &Destination<'_>) -> Vec<Candidate> {
    let mut found = Vec::new();
    if let Some(direct) = dest.direct {
        found.push(Candidate {
            path: PathId::Direct(direct.addr),
            legs: vec![Leg { from: graph.me, to: dest.id, segment: direct.segment }],
            cost_ms: cost::path_cost(&[direct.segment], 0, 0.0),
            load_percent: 0,
            lan: direct.lan,
        });
    }
    for &(entry, segment) in graph.entries {
        if (graph.avoid)(entry) {
            continue;
        }
        let mut walk = Walk { relays: vec![entry], legs: vec![Leg { from: graph.me, to: entry, segment }] };
        walk.extend(graph, dest, &mut found);
    }
    found
}

/// 穷举时走到一半的路径。
struct Walk {
    relays: Vec<NodeId>,
    legs: Vec<Leg>,
}

impl Walk {
    fn extend(&mut self, graph: &Graph<'_>, dest: &Destination<'_>, found: &mut Vec<Candidate>) {
        let Some(&last) = self.relays.last() else { return };
        if let Some((_, exit)) = dest.exits.iter().find(|(relay, _)| *relay == last)
            && let Ok(path) = RelayPath::new(&self.relays)
        {
            let mut legs = self.legs.clone();
            legs.extend(exit.map(|segment| Leg { from: last, to: dest.id, segment }));
            let segments: Vec<Segment> = legs.iter().map(|leg| leg.segment).collect();
            let loads = self.relays.iter().map(|relay| graph.matrix.load_percent(*relay, graph.now));
            let load_ms: f64 = loads.clone().map(cost::load_penalty_ms).sum();
            found.push(Candidate {
                path: PathId::Relays(path),
                cost_ms: cost::path_cost(&segments, self.relays.len(), load_ms),
                legs,
                load_percent: loads.max().unwrap_or(0),
                lan: false,
            });
        }
        // 到了对端自己（它是 relay）就不再往前；本节点自己的 relay 只用来把包交给挂在它上面的对端，不从它再绕骨干。
        if self.relays.len() >= graph.max_hops || last == dest.id || last == graph.me {
            return;
        }
        let onward: Vec<(NodeId, LinkQuality)> = graph.matrix.links_from(last, graph.now).collect();
        for (next, quality) in onward {
            if next == graph.me || self.relays.contains(&next) || (graph.avoid)(next) || (graph.broken)(last, next) {
                continue;
            }
            self.relays.push(next);
            self.legs.push(Leg { from: last, to: next, segment: segment_of(quality) });
            self.extend(graph, dest, found);
            self.relays.pop();
            self.legs.pop();
        }
    }
}

fn segment_of(quality: LinkQuality) -> Segment {
    Segment { rtt_ms: f64::from(quality.rtt_ms), jitter_ms: f64::from(quality.jitter_ms), loss: quality.loss() }
}

/// 默认模式的选择（白皮书 §7.1）：直连可用就走直连，不和 relay 比较；否则只经一个 relay，
/// 按 `relays` 给的顺序取第一个现在能用的。
pub(crate) fn direct_first(
    direct: Option<SocketAddr>,
    relays: impl IntoIterator<Item = NodeId>,
    usable: impl Fn(NodeId) -> bool,
) -> Option<PathId> {
    direct.map(PathId::Direct).or_else(|| {
        let relay = relays.into_iter().find(|relay| usable(*relay))?;
        RelayPath::new(&[relay]).ok().map(PathId::Relays)
    })
}

/// 换路的原因。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Why {
    /// 局域网直连无条件优先。
    Lan,
    /// 之前没有路，现在有了。
    First,
    /// 原来的路不能用了，立刻换到次优的。
    Failover,
    /// 新的路连续几个评估窗口都明显更便宜。两个数是原来的路和新的路此刻的代价。
    Cheaper { from_ms: f64, to_ms: f64 },
    /// 一条能用的路都没有了。
    NoPath,
}

/// 延迟优先模式下，到一个对端的选路状态。
#[derive(Debug, Default)]
pub(crate) struct Selector {
    current: Option<PathId>,
    /// 上一次从一条路换到另一条路的时刻。
    switched: Option<Instant>,
    /// 正在考察的更便宜的路，和它已经连续赢了几个窗口。
    challenger: Option<(PathId, u8)>,
}

impl Selector {
    /// 现在选定的路。
    pub(crate) const fn current(&self) -> Option<PathId> {
        self.current
    }

    /// 按此刻的候选重新考虑。`window` 为真表示这一次调用是一个评估窗口的边界：只有这时才数"连续几个窗口更优"；
    /// 别的时候只处理"现在的路没了"和局域网直连。`pick(n)` 返回 0 到 n-1 之间的一个随机数。
    ///
    /// 路换了的时候返回原因。
    pub(crate) fn choose(
        &mut self,
        now: Instant,
        candidates: &[Candidate],
        window: bool,
        pick: &mut dyn FnMut(usize) -> usize,
    ) -> Option<Why> {
        if let Some(lan) = candidates.iter().find(|candidate| candidate.lan) {
            self.challenger = None;
            return self.switch_to(Some(lan.path), Why::Lan, now);
        }
        let Some(current) = self.current.and_then(|path| candidates.iter().find(|candidate| candidate.path == path)) else {
            self.challenger = None;
            let all: Vec<&Candidate> = candidates.iter().collect();
            let chosen = two_choice(&near_best(&all), pick).map(|candidate| candidate.path);
            let why = match (self.current, chosen) {
                (_, None) => Why::NoPath,
                (Some(_), Some(_)) => Why::Failover,
                (None, Some(_)) => Why::First,
            };
            return self.switch_to(chosen, why, now);
        };
        if !window {
            return None;
        }
        let margin = (current.cost_ms * SWITCH_MARGIN_RATIO).max(SWITCH_MARGIN_MS);
        let cheaper: Vec<&Candidate> =
            candidates.iter().filter(|candidate| candidate.path != current.path && current.cost_ms - candidate.cost_ms >= margin).collect();
        let near = near_best(&cheaper);
        // 上一个窗口考察的那条路还在"明显更便宜"的一组里就接着数；不在了就从头数另一条。
        let (path, wins) = match self.challenger {
            Some((path, wins)) if near.iter().any(|candidate| candidate.path == path) => (path, wins.saturating_add(1)),
            Some(_) | None => match two_choice(&near, pick) {
                Some(chosen) => (chosen.path, 1),
                None => {
                    self.challenger = None;
                    return None;
                }
            },
        };
        self.challenger = Some((path, wins));
        let dwelled = self.switched.is_none_or(|at| now.saturating_duration_since(at) >= MIN_DWELL);
        let to_ms = near.iter().find(|candidate| candidate.path == path)?.cost_ms;
        if wins < SWITCH_WINDOWS || !dwelled {
            return None;
        }
        self.challenger = None;
        let from_ms = current.cost_ms;
        self.switch_to(Some(path), Why::Cheaper { from_ms, to_ms }, now)
    }

    fn switch_to(&mut self, path: Option<PathId>, why: Why, now: Instant) -> Option<Why> {
        if self.current == path {
            return None;
        }
        if self.current.is_some() && path.is_some() {
            self.switched = Some(now);
        }
        self.current = path;
        Some(why)
    }
}

/// `candidates` 里代价和最低的相差在 5% 以内的那些。
fn near_best<'a>(candidates: &[&'a Candidate]) -> Vec<&'a Candidate> {
    let Some(best) = candidates.iter().map(|candidate| candidate.cost_ms).min_by(f64::total_cmp) else { return Vec::new() };
    candidates.iter().copied().filter(|candidate| candidate.cost_ms <= best * (1.0 + NEAR_EQUAL_RATIO)).collect()
}

/// 在差不多的候选之间随机取两个，用负载低的；负载一样就用先取到的那个（也就是随机的）。
fn two_choice<'a>(near: &[&'a Candidate], pick: &mut dyn FnMut(usize) -> usize) -> Option<&'a Candidate> {
    match *near {
        [] => None,
        [only] => Some(only),
        _ => {
            let first = pick(near.len()) % near.len();
            // 第二个从剩下的里面取，保证和第一个不是同一个。
            let second = (first + 1 + pick(near.len() - 1) % (near.len() - 1)) % near.len();
            let (first, second) = (near[first], near[second]);
            Some(if second.load_percent < first.load_percent { second } else { first })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use i3t_proto::backbone::{Link, RelayState, Row};

    use super::*;
    use crate::timing::EVAL_WINDOW;

    const A: u32 = 1;
    const B: u32 = 8;
    const C: u32 = 9;
    const D: u32 = 2;

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn addr() -> SocketAddr {
        "203.0.113.7:20330".parse().unwrap()
    }

    fn segment(rtt_ms: f64, loss_percent: f64) -> Segment {
        Segment { rtt_ms, jitter_ms: 0.0, loss: loss_percent / 100.0 }
    }

    fn relays(ids: &[u32]) -> PathId {
        PathId::Relays(RelayPath::new(&ids.iter().map(|&id| node(id)).collect::<Vec<_>>()).unwrap())
    }

    /// 一行矩阵：`origin` 到 `links` 里各个 relay 的延迟（毫秒）和丢包（单位 0.5%）。
    fn row(matrix: &mut Matrix, origin: u32, load_percent: u8, links: &[(u32, u16, u8)], now: Instant) {
        let links = links
            .iter()
            .map(|&(relay, rtt_ms, loss_half_percent)| Link {
                relay: node(relay),
                quality: Some(LinkQuality { rtt_ms, jitter_ms: 0, loss_half_percent }),
            })
            .collect();
        let row = Row { origin: node(origin), seq: 1, state: RelayState::Serving, load_percent, links };
        let frame = Bytes::from(row.to_bytes(&[0; 64]));
        matrix.insert(row, frame, now);
    }

    /// 白皮书 §7.1 的算例。骨干 B—C 的丢包是 0.1%，矩阵的刻度是 0.5%，所以这里写成 0：代价差不到 0.3ms，四舍五入之后一样。
    struct Example {
        matrix: Matrix,
        now: Instant,
        entries: Vec<(NodeId, Segment)>,
        exits: Vec<(NodeId, Option<Segment>)>,
    }

    impl Example {
        fn new() -> Self {
            let now = Instant::now();
            let mut matrix = Matrix::default();
            row(&mut matrix, B, 0, &[(C, 140, 0)], now);
            row(&mut matrix, C, 0, &[(B, 140, 0)], now);
            Self {
                matrix,
                now,
                entries: vec![(node(B), segment(35.0, 0.2)), (node(C), segment(200.0, 3.0))],
                exits: vec![(node(C), Some(segment(8.0, 0.0))), (node(B), Some(segment(170.0, 0.5)))],
            }
        }

        fn candidates(&self, max_hops: usize, direct: bool) -> Vec<Candidate> {
            let graph = Graph {
                me: node(A),
                max_hops,
                entries: &self.entries,
                matrix: &self.matrix,
                now: self.now,
                avoid: &|_| false,
                broken: &|_, _| false,
            };
            let direct = direct.then(|| DirectLink { addr: addr(), segment: segment(190.0, 4.0), lan: false });
            candidates(&graph, &Destination { id: node(D), exits: &self.exits, direct })
        }
    }

    fn cost_of(candidates: &[Candidate], path: PathId) -> f64 {
        candidates.iter().find(|candidate| candidate.path == path).unwrap_or_else(|| panic!("候选里应该有 {path:?}")).cost_ms.round()
    }

    /// 不随机：总是取第一个。
    fn first(_: usize) -> usize {
        0
    }

    #[test]
    fn the_worked_example_lists_the_four_candidates_with_the_costs_in_the_design() {
        let example = Example::new();
        let found = example.candidates(2, true);
        assert_eq!(cost_of(&found, PathId::Direct(addr())), 201.0);
        assert_eq!(cost_of(&found, relays(&[C])), 218.0);
        assert_eq!(cost_of(&found, relays(&[B])), 208.0);
        assert_eq!(cost_of(&found, relays(&[B, C])), 186.0);
        // 还有一条绕远的 A→C→B→D，它也是走得通的候选，只是代价高得多。
        assert_eq!(found.len(), 5);
        assert!(cost_of(&found, relays(&[C, B])) > 500.0);

        let two_hop = found.iter().find(|candidate| candidate.path == relays(&[B, C])).unwrap();
        let hops: Vec<(NodeId, NodeId)> = two_hop.legs.iter().map(|leg| (leg.from, leg.to)).collect();
        assert_eq!(hops, [(node(A), node(B)), (node(B), node(C)), (node(C), node(D))], "每一段从谁到谁都记着，给界面用");
    }

    #[test]
    fn each_mode_picks_the_path_the_design_says() {
        let example = Example::new();
        let now = example.now;

        // 延迟优先，最大级数 2：走 A→B→C→D。
        let mut selector = Selector::default();
        assert_eq!(selector.choose(now, &example.candidates(2, true), true, &mut first), Some(Why::First));
        assert_eq!(selector.current(), Some(relays(&[B, C])));

        // 延迟优先，最大级数 1：不考虑两级路径，剩下的候选里直连最低。
        let mut selector = Selector::default();
        let one_hop = example.candidates(1, true);
        assert_eq!(one_hop.len(), 3);
        selector.choose(now, &one_hop, true, &mut first);
        assert_eq!(selector.current(), Some(PathId::Direct(addr())));

        // 默认模式：直连可用就走直连，不比较。
        let attached = [node(C), node(B)];
        assert_eq!(direct_first(Some(addr()), attached, |_| true), Some(PathId::Direct(addr())));
        // 直连建不起来时只用一个 relay：离 D 最近的 C，约 208ms（200 + 8，默认模式不算代价）。
        assert_eq!(direct_first(None, attached, |_| true), Some(relays(&[C])));
        // 首选不可用时用 D 的下一个。
        assert_eq!(direct_first(None, attached, |relay| relay != node(C)), Some(relays(&[B])));
        assert_eq!(direct_first(None, attached, |_| false), None);
    }

    #[test]
    fn only_edges_that_really_connect_are_walked() {
        let mut example = Example::new();
        // B 的行里没有 C 了（连不上）：两级路径 A→B→C→D 不存在，反方向 A→C→B→D 还在。
        example.matrix = Matrix::default();
        row(&mut example.matrix, C, 0, &[(B, 140, 0)], example.now);
        let found = example.candidates(2, false);
        let paths: Vec<PathId> = found.iter().map(|candidate| candidate.path).collect();
        assert_eq!(paths, [relays(&[B]), relays(&[C]), relays(&[C, B])]);

        // 没有直连、没有能进去的 relay：没有候选。
        example.entries.clear();
        assert!(example.candidates(4, false).is_empty());
    }

    #[test]
    fn relays_to_avoid_and_broken_links_are_left_out() {
        let example = Example::new();
        let graph = |avoid: &dyn Fn(NodeId) -> bool, broken: &dyn Fn(NodeId, NodeId) -> bool| {
            let graph =
                Graph { me: node(A), max_hops: 2, entries: &example.entries, matrix: &example.matrix, now: example.now, avoid, broken };
            let found = candidates(&graph, &Destination { id: node(D), exits: &example.exits, direct: None });
            found.iter().map(|candidate| candidate.path).collect::<Vec<_>>()
        };
        // C 即将下线：经过它的每一条路径都不是候选，不管它排在第几个。
        assert_eq!(graph(&|relay| relay == node(C), &|_, _| false), [relays(&[B])]);
        // B 说它到 C 没有骨干连接：只少了 A→B→C→D。
        assert_eq!(graph(&|_| false, &|from, to| (from, to) == (node(B), node(C))), [relays(&[B]), relays(&[C]), relays(&[C, B])]);
    }

    #[test]
    fn a_path_never_visits_a_relay_twice_and_respects_the_hop_limit() {
        let now = Instant::now();
        let mut matrix = Matrix::default();
        // 五个 relay 两两相通。
        let ids = [11, 12, 13, 14, 15];
        for origin in ids {
            let links: Vec<(u32, u16, u8)> = ids.iter().filter(|&&other| other != origin).map(|&other| (other, 10, 0)).collect();
            row(&mut matrix, origin, 0, &links, now);
        }
        let entries = [(node(11), segment(5.0, 0.0))];
        let exits = [(node(15), Some(segment(5.0, 0.0)))];
        let count = |max_hops| {
            let graph = Graph { me: node(A), max_hops, entries: &entries, matrix: &matrix, now, avoid: &|_| false, broken: &|_, _| false };
            let found = candidates(&graph, &Destination { id: node(D), exits: &exits, direct: None });
            for candidate in &found {
                let PathId::Relays(path) = candidate.path else { panic!("没有直连") };
                assert!(path.as_slice().len() <= max_hops);
                assert_eq!((path.first(), path.last()), (node(11), node(15)));
            }
            found.len()
        };
        // 11 → 15；中间再插 1 个（3 种）；再插 2 个（3×2 种）。
        assert_eq!((count(1), count(2), count(3), count(4)), (0, 1, 4, 10));
    }

    #[test]
    fn a_destination_that_is_itself_a_relay_ends_the_path_and_this_nodes_own_relay_is_not_a_transit() {
        let now = Instant::now();
        let mut matrix = Matrix::default();
        row(&mut matrix, B, 0, &[(D, 30, 0), (C, 10, 0)], now);
        row(&mut matrix, D, 0, &[(C, 10, 0)], now);
        row(&mut matrix, A, 0, &[(B, 5, 0)], now);
        // 对端 D 自己是 relay；本节点 A 也是 relay，D 在 A 上有会话。
        let entries = [(node(B), segment(20.0, 0.0)), (node(D), segment(60.0, 0.0)), (node(A), segment(0.0, 0.0))];
        let exits = [(node(D), None), (node(A), Some(segment(60.0, 0.0)))];
        let graph = Graph { me: node(A), max_hops: 3, entries: &entries, matrix: &matrix, now, avoid: &|_| false, broken: &|_, _| false };
        let found = candidates(&graph, &Destination { id: node(D), exits: &exits, direct: None });
        let paths: Vec<PathId> = found.iter().map(|candidate| candidate.path).collect();
        assert_eq!(paths, [relays(&[B, D]), relays(&[D]), relays(&[A])], "到了 D 就停；A 自己的 relay 不再往骨干上绕");
        assert_eq!(found[0].legs.len(), 2, "对端自己是最后一个 relay，没有再往后的一段");
        assert!((found[0].cost_ms - 52.0).abs() < 1e-9);
    }

    fn candidate(path: PathId, cost_ms: f64) -> Candidate {
        Candidate { path, legs: Vec::new(), cost_ms, load_percent: 0, lan: false }
    }

    /// 从 `start` 起每个评估窗口调用一次，返回第几个窗口（从 1 数）换了路。
    fn windows_until_switch(selector: &mut Selector, start: Instant, candidates: &[Candidate], limit: u32) -> Option<u32> {
        (1..=limit).find(|&window| selector.choose(start + EVAL_WINDOW * window, candidates, true, &mut first).is_some())
    }

    #[test]
    fn a_cheaper_path_must_win_three_consecutive_windows_before_the_switch() {
        let start = Instant::now();
        let (old, new) = (relays(&[B]), relays(&[C]));
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(old, 100.0)], true, &mut first);

        let better = [candidate(old, 100.0), candidate(new, 80.0)];
        assert_eq!(selector.choose(start + EVAL_WINDOW, &better, true, &mut first), None);
        assert_eq!(selector.choose(start + EVAL_WINDOW * 2, &better, true, &mut first), None);
        // 窗口之间的调用不算数：多调用几次也不会提前。
        assert_eq!(selector.choose(start + EVAL_WINDOW * 2 + Duration::from_millis(50), &better, false, &mut first), None);
        assert_eq!(selector.current(), Some(old));
        assert_eq!(selector.choose(start + EVAL_WINDOW * 3, &better, true, &mut first), Some(Why::Cheaper { from_ms: 100.0, to_ms: 80.0 }));
        assert_eq!(selector.current(), Some(new));

        // 中间有一个窗口不够好，就要重新数三个。
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(old, 100.0)], true, &mut first);
        let barely = [candidate(old, 100.0), candidate(new, 95.0)];
        selector.choose(start + EVAL_WINDOW, &better, true, &mut first);
        selector.choose(start + EVAL_WINDOW * 2, &better, true, &mut first);
        assert_eq!(selector.choose(start + EVAL_WINDOW * 3, &barely, true, &mut first), None);
        assert_eq!(windows_until_switch(&mut selector, start + EVAL_WINDOW * 3, &better, 10), Some(3));
    }

    #[test]
    fn the_margin_is_five_milliseconds_or_ten_percent_whichever_is_larger() {
        let start = Instant::now();
        let (old, new) = (relays(&[B]), relays(&[C]));
        let switches = |old_ms: f64, new_ms: f64| {
            let mut selector = Selector::default();
            selector.choose(start, &[candidate(old, old_ms)], true, &mut first);
            windows_until_switch(&mut selector, start, &[candidate(old, old_ms), candidate(new, new_ms)], 10).is_some()
        };
        // 低延迟：10% 不到 5ms，按 5ms 算。
        assert!(!switches(20.0, 15.5));
        assert!(switches(20.0, 15.0));
        // 高延迟：按 10% 算。
        assert!(!switches(200.0, 181.0));
        assert!(switches(200.0, 180.0));
    }

    #[test]
    fn a_failover_also_starts_the_dwell_but_the_first_route_does_not() {
        let start = Instant::now();
        let (one, two) = (relays(&[B]), relays(&[C]));
        // 头一次有路：不算换路，更便宜的候选连续赢三个窗口就换，不用等 30 秒。
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(one, 100.0)], true, &mut first);
        assert_eq!(windows_until_switch(&mut selector, start, &[candidate(one, 100.0), candidate(two, 50.0)], 10), Some(3));

        // 原来的路断了，立刻换到另一条；断的那条马上又好了，而且便宜得多：也要在新的路上停满 30 秒。
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(one, 50.0), candidate(two, 100.0)], true, &mut first);
        assert_eq!(selector.choose(start + EVAL_WINDOW, &[candidate(two, 100.0)], false, &mut first), Some(Why::Failover));
        let recovered = [candidate(one, 50.0), candidate(two, 100.0)];
        let back = windows_until_switch(&mut selector, start + EVAL_WINDOW, &recovered, 20).unwrap();
        assert_eq!(EVAL_WINDOW * back, MIN_DWELL, "断了又好的路不会让连接每隔十几秒来回换一次");
    }

    #[test]
    fn after_a_cost_based_switch_the_route_stays_for_thirty_seconds() {
        let start = Instant::now();
        let (first_path, second_path) = (relays(&[B]), relays(&[C]));
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(first_path, 100.0)], true, &mut first);
        let towards_second = [candidate(first_path, 100.0), candidate(second_path, 50.0)];
        assert_eq!(windows_until_switch(&mut selector, start, &towards_second, 10), Some(3));
        let switched = start + EVAL_WINDOW * 3;

        // 条件马上反过来：三个窗口之后还不到 30 秒，继续等；满 30 秒的那个窗口才换回去。
        let towards_first = [candidate(first_path, 50.0), candidate(second_path, 100.0)];
        let back = windows_until_switch(&mut selector, switched, &towards_first, 20).unwrap();
        assert_eq!(EVAL_WINDOW * back, MIN_DWELL, "换回去发生在停留满 30 秒的时候，而不是 15 秒");
        assert_eq!(selector.current(), Some(first_path));
    }

    #[test]
    fn a_dead_path_is_left_at_once_without_waiting_for_windows_or_dwell() {
        let start = Instant::now();
        let (old, new, worse) = (relays(&[B]), relays(&[C]), relays(&[B, C]));
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(old, 100.0), candidate(new, 120.0), candidate(worse, 300.0)], true, &mut first);
        assert_eq!(selector.current(), Some(old));

        // 原来的路不在候选里了（它的 relay 断了，或者宣布要下线）。不是评估窗口的边界也立刻换到次优的。
        let without_old = [candidate(new, 120.0), candidate(worse, 300.0)];
        assert_eq!(selector.choose(start + Duration::from_millis(50), &without_old, false, &mut first), Some(Why::Failover));
        assert_eq!(selector.current(), Some(new));

        // 一条都没有了：没有路。恢复之后是"有路了"，同样不等。
        assert_eq!(selector.choose(start + Duration::from_millis(100), &[], false, &mut first), Some(Why::NoPath));
        assert_eq!(selector.current(), None);
        assert_eq!(selector.choose(start + Duration::from_millis(150), &[], false, &mut first), None);
        assert_eq!(selector.choose(start + Duration::from_millis(200), &without_old, false, &mut first), Some(Why::First));
    }

    #[test]
    fn a_lan_path_wins_unconditionally_and_immediately() {
        let start = Instant::now();
        let relay = relays(&[B]);
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(relay, 10.0)], true, &mut first);
        // 局域网直连的代价更高（比如无线信号差）也照样用它，不等窗口。
        let lan = Candidate { lan: true, ..candidate(PathId::Direct(addr()), 40.0) };
        assert_eq!(
            selector.choose(start + Duration::from_millis(50), &[candidate(relay, 10.0), lan.clone()], false, &mut first),
            Some(Why::Lan)
        );
        assert_eq!(selector.current(), Some(PathId::Direct(addr())));
        assert_eq!(selector.choose(start + EVAL_WINDOW, &[candidate(relay, 10.0), lan], true, &mut first), None, "之后一直留在它上面");
        // 不是局域网的直连没有这个待遇：要按代价比。
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(relay, 10.0)], true, &mut first);
        let wan = candidate(PathId::Direct(addr()), 40.0);
        assert_eq!(selector.choose(start + EVAL_WINDOW, &[candidate(relay, 10.0), wan], true, &mut first), None);
        assert_eq!(selector.current(), Some(relay));
    }

    /// 一个可以重现的随机数序列（SplitMix64）：相邻的种子给出的序列互不相关，可以拿种子当"第几个节点"用。
    fn sequence(seed: u64) -> impl FnMut(usize) -> usize {
        let mut state = seed;
        move |bound| {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut mixed = state;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            mixed ^= mixed >> 31;
            usize::try_from(mixed % u64::try_from(bound).unwrap()).unwrap()
        }
    }

    #[test]
    fn near_equal_relays_share_the_nodes_instead_of_all_taking_the_single_best() {
        let start = Instant::now();
        // 三个 relay 的代价相差不到 5%，第四个差得远。
        let paths = [relays(&[11]), relays(&[12]), relays(&[13]), relays(&[14])];
        let options = [candidate(paths[0], 100.0), candidate(paths[1], 102.0), candidate(paths[2], 104.0), candidate(paths[3], 130.0)];
        let mut chosen = [0u32; 4];
        for node_seed in 0..300 {
            let mut selector = Selector::default();
            selector.choose(start, &options, true, &mut sequence(node_seed));
            let index = paths.iter().position(|path| Some(*path) == selector.current()).unwrap();
            chosen[index] += 1;
        }
        assert_eq!(chosen[3], 0, "差得远的那个没有人选");
        assert!(chosen[..3].iter().all(|&count| count > 60), "差不多的三个各分到一批节点，而不是全挤在最便宜的那个上：{chosen:?}");
    }

    #[test]
    fn between_two_near_equal_relays_the_less_loaded_one_is_preferred() {
        let start = Instant::now();
        let (busy, idle, other) = (relays(&[11]), relays(&[12]), relays(&[13]));
        let options = [
            Candidate { load_percent: 70, ..candidate(busy, 100.0) },
            Candidate { load_percent: 10, ..candidate(idle, 103.0) },
            Candidate { load_percent: 40, ..candidate(other, 104.0) },
        ];
        let mut chosen = [0u32; 3];
        for node_seed in 0..300 {
            let mut selector = Selector::default();
            selector.choose(start, &options, true, &mut sequence(node_seed));
            let index = [busy, idle, other].iter().position(|path| Some(*path) == selector.current()).unwrap();
            chosen[index] += 1;
        }
        // 随机取两个、用负载低的：最闲的那个只要被取到就赢（三分之二），最忙的那个只要被取到就输。
        assert_eq!(chosen[0], 0, "{chosen:?}");
        assert!(chosen[1] > chosen[2] && chosen[2] > 60, "{chosen:?}");
    }

    #[test]
    fn the_challenger_is_not_re_drawn_every_window() {
        let start = Instant::now();
        let (old, one, two) = (relays(&[B]), relays(&[11]), relays(&[12]));
        let mut selector = Selector::default();
        selector.choose(start, &[candidate(old, 200.0)], true, &mut first);
        // 两条差不多便宜的新路。每个窗口都重新随机的话，很难连续三次抽到同一条，路就永远换不过去。
        let options = [candidate(old, 200.0), candidate(one, 100.0), candidate(two, 101.0)];
        let mut random = sequence(7);
        let switched =
            (1..=3).find_map(|window| selector.choose(start + EVAL_WINDOW * window, &options, true, &mut random).map(|_| window));
        assert_eq!(switched, Some(3), "第一个窗口抽中的那条连续赢三个窗口就换");
    }

    #[test]
    fn relay_load_is_part_of_the_cost_and_the_busiest_relay_on_the_path_is_reported() {
        let mut example = Example::new();
        example.matrix = Matrix::default();
        row(&mut example.matrix, B, 90, &[(C, 140, 0)], example.now);
        row(&mut example.matrix, C, 30, &[(B, 140, 0)], example.now);
        let found = example.candidates(2, false);
        // B 的占用到了 90%：经过它的路径各加 25ms。
        assert_eq!(cost_of(&found, relays(&[B])), 233.0);
        assert_eq!(cost_of(&found, relays(&[B, C])), 211.0);
        assert_eq!(cost_of(&found, relays(&[C])), 218.0);
        let load = |path: PathId| found.iter().find(|candidate| candidate.path == path).unwrap().load_percent;
        assert_eq!((load(relays(&[B, C])), load(relays(&[C]))), (90, 30));
    }
}
