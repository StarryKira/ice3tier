//! 把路由层给人看的选路情况（白皮书 §7.7）写成节点协议里的样子，交给 controller：
//! 上报里每条路的细节（[`kinds`]），现问时的候选路径和代价（[`detail`]），以及骨干矩阵（[`backbone`]）。
//!
//! 路由层用的是浮点数的毫秒和 0 到 1 的丢包率；协议里换成整数（微秒、千分之几），消息因此可以直接比较相等。

use std::time::Duration;

use i3t_conn::{BackboneRow, CandidateReport, DirectKind, LegReport, PathReport};
use i3t_model::control::{self, PathCandidate, PathDetail, PathSegment, Route, RouteKind};
use i3t_proto::backbone::RelayState;

/// 现在这条路的细节。说不出任何细节的对端返回 `None`，上报里就不带它。
///
/// 经 relay 时第一段走 UDP 还是 TCP，路由层现在不给，所以总是 `None`。
pub(crate) fn kind(report: &PathReport) -> Option<RouteKind> {
    let direct = direct_kind(report);
    direct.map(|direct| RouteKind { peer: report.peer, direct: Some(direct), relay_leg: None })
}

/// 到这个对端的选路情况：模式、为什么是现在这条、考虑过的候选路径。
pub(crate) fn detail(report: &PathReport) -> PathDetail {
    PathDetail {
        mode: Some(report.mode),
        direct: direct_kind(report),
        relay_leg: None,
        reason: report.reason.clone(),
        candidates: report.candidates.iter().map(candidate).collect(),
    }
}

/// 本节点手上的骨干矩阵。
pub(crate) fn backbone(rows: &[BackboneRow]) -> Vec<control::BackboneRow> {
    rows.iter()
        .map(|row| control::BackboneRow {
            relay: row.relay,
            state: match row.state {
                RelayState::Serving => control::RelayState::Serving,
                RelayState::Overloaded => control::RelayState::Overloaded,
                RelayState::Draining => control::RelayState::Draining,
            },
            load_percent: row.load_percent,
            age_ms: row.age_ms,
            links: row
                .links
                .iter()
                .map(|link| control::BackboneLink {
                    to: link.to,
                    rtt_ms: link.rtt_ms.map(u32::from),
                    jitter_ms: u32::from(link.jitter_ms),
                    loss_permille: permille(link.loss),
                })
                .collect(),
        })
        .collect()
}

/// 直连的类型，只在现在走的就是直连时才有。
fn direct_kind(report: &PathReport) -> Option<control::DirectKind> {
    if !matches!(report.route, Route::Direct { .. }) {
        return None;
    }
    report.direct.map(|kind| match kind {
        DirectKind::Lan => control::DirectKind::Lan,
        DirectKind::Ipv6 => control::DirectKind::Ipv6,
        DirectKind::Reachable => control::DirectKind::Reachable,
        DirectKind::Punched => control::DirectKind::Punched,
    })
}

fn candidate(candidate: &CandidateReport) -> PathCandidate {
    PathCandidate {
        route: candidate.route.clone(),
        cost_us: u64::from(micros(candidate.cost_ms)),
        legs: candidate.legs.iter().map(segment).collect(),
        load_percent: candidate.load_percent,
        current: candidate.current,
    }
}

fn segment(leg: &LegReport) -> PathSegment {
    PathSegment {
        from: leg.from,
        to: leg.to,
        rtt_us: micros(leg.rtt_ms),
        jitter_us: micros(leg.jitter_ms),
        loss_permille: permille(leg.loss),
    }
}

/// 毫秒换成微秒，四舍五入。借 `Duration` 来换，不用 `as`：负数和不是数的值得 0，大得放不下的封顶。
fn micros(ms: f64) -> u32 {
    match Duration::try_from_secs_f64(ms / 1000.0) {
        Ok(duration) => u32::try_from(rounded_thousandths(duration)).unwrap_or(u32::MAX),
        Err(_) if ms > 0.0 => u32::MAX,
        Err(_) => 0,
    }
}

/// 0 到 1 的比例换成千分之几，四舍五入，超出范围的取边界。和 [`micros`] 一样借 `Duration` 来换：比例当成秒数，
/// 毫秒数就是千分之几。
fn permille(fraction: f64) -> u16 {
    let clamped = if fraction.is_nan() { 0.0 } else { fraction.clamp(0.0, 1.0) };
    Duration::try_from_secs_f64(clamped).map_or(0, |duration| u16::try_from((rounded_thousandths(duration) + 500) / 1000).unwrap_or(1000))
}

/// 一段时间有多少微秒，四舍五入：浮点数换算时 20.4 毫秒会变成 20.399999…，直接截断就少了 1。
fn rounded_thousandths(duration: Duration) -> u128 {
    (duration.as_nanos() + 500) / 1000
}

#[cfg(test)]
mod tests {
    use i3t_conn::BackboneLink;
    use i3t_model::netmap::RoutingMode;
    use i3t_proto::NodeId;

    use super::*;

    fn id(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn report(route: Route, direct: Option<DirectKind>) -> PathReport {
        PathReport {
            peer: id(4),
            mode: RoutingMode::LowestCost,
            route,
            direct,
            reason: "直连代价最低".to_owned(),
            since_switch_ms: Some(1500),
            candidates: vec![CandidateReport {
                route: Route::Relay { relays: vec![id(9)] },
                cost_ms: 41.25,
                legs: vec![LegReport { from: id(1), to: id(9), rtt_ms: 20.4, jitter_ms: 0.5, loss: 0.012 }],
                load_percent: 30,
                current: false,
            }],
        }
    }

    #[test]
    fn a_path_report_becomes_integer_microseconds_and_permille() {
        let direct = Route::Direct { addr: "192.168.1.5:20330".parse().unwrap() };
        let detail = detail(&report(direct.clone(), Some(DirectKind::Lan)));
        assert_eq!(
            (detail.mode, detail.direct, detail.reason.as_str()),
            (Some(RoutingMode::LowestCost), Some(control::DirectKind::Lan), "直连代价最低")
        );
        let candidate = &detail.candidates[0];
        assert_eq!((candidate.cost_us, candidate.load_percent, candidate.current), (41_250, 30, false));
        assert_eq!(candidate.legs[0], PathSegment { from: id(1), to: id(9), rtt_us: 20_400, jitter_us: 500, loss_permille: 12 });
        assert_eq!(kind(&report(direct, Some(DirectKind::Punched))).map(|kind| kind.direct), Some(Some(control::DirectKind::Punched)));
    }

    #[test]
    fn a_relayed_path_has_no_direct_kind_and_says_nothing_in_the_report() {
        let relayed = report(Route::Relay { relays: vec![id(9)] }, Some(DirectKind::Reachable));
        assert_eq!(detail(&relayed).direct, None, "直连的类型只说现在走的直连");
        assert_eq!(kind(&relayed), None);
    }

    #[test]
    fn odd_measurements_are_clamped_instead_of_wrapping() {
        assert_eq!((micros(-1.0), micros(f64::NAN), micros(1e30), micros(0.0004)), (0, 0, u32::MAX, 0));
        assert_eq!((permille(-0.5), permille(2.0), permille(f64::NAN), permille(0.5), permille(0.0126)), (0, 1000, 0, 500, 13));
    }

    #[test]
    fn backbone_rows_keep_unreachable_links_as_null() {
        let rows = [BackboneRow {
            relay: id(9),
            state: RelayState::Draining,
            load_percent: 80,
            age_ms: 2500,
            links: vec![
                BackboneLink { to: id(3), rtt_ms: None, jitter_ms: 0, loss: 1.0 },
                BackboneLink { to: id(5), rtt_ms: Some(42), jitter_ms: 3, loss: 0.005 },
            ],
        }];
        let converted = backbone(&rows);
        assert_eq!((converted[0].state, converted[0].load_percent, converted[0].age_ms), (control::RelayState::Draining, 80, 2500));
        let links: Vec<_> = converted[0].links.iter().map(|link| (link.to, link.rtt_ms, link.jitter_ms, link.loss_permille)).collect();
        assert_eq!(links, [(id(3), None, 0, 1000), (id(5), Some(42), 3, 5)]);
    }
}
