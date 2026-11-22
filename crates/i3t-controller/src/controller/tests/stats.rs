//! 给界面画图的时间序列：每 5 分钟一个点，整点汇总，读的时候缺的段是空的。

use std::time::{Duration, Instant};

use i3t_model::admin::stats::{Series, SeriesQuery, Unit};
use i3t_model::admin::{ErrorCode, RelaySettings};
use i3t_model::control::{Report, ToController};
use i3t_model::netmap::Attachment;
use i3t_proto::NodeId;

use super::super::testing::{CAN_DO_ALL, Lab};

fn query(series: Series, node: Option<i3t_proto::NodeId>) -> SeriesQuery {
    SeriesQuery { series, node, from: None, to: None, step: None }
}

#[test]
fn reports_become_points_every_five_minutes_and_hours_are_averaged() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let relay = lab.join_with(network.id, &[], CAN_DO_ALL);
    lab.controller.enable_relay(network.id, relay.id, RelaySettings::default()).unwrap();
    let a = lab.join(network.id, &[]);
    let wall = lab.controller.lock().clock.now();
    // 一个整点之后的第一段：这样过完两段之后正好跨过下一个整点之前，不汇总；再往后拨一个小时才汇总。
    let hour_start = wall - wall % 3600;
    let start = Instant::now();

    let report = |flows, tx| {
        ToController::Report(Box::new(Report {
            flows,
            tx_bytes: tx,
            run: 7,
            attached: vec![Attachment { relay: relay.id, rtt_ms: 5, shared: false }],
            ..Report::default()
        }))
    };
    a.send(&lab.controller, report(2, 0));
    a.send(&lab.controller, report(4, 1_000));
    // 第一次：开始攒这一段，并且数一次在线的节点和挂靠。
    lab.controller.lock().stats_tick(start, hour_start + 10).unwrap();
    lab.controller.lock().stats_tick(start, hour_start + 300).unwrap();

    lab.controller.advance_clock(hour_start + 3_600 - wall);
    let flows = lab.controller.series(network.id, &query(Series::NodeFlows, Some(a.id))).unwrap();
    assert_eq!((flows.step, flows.unit), (300, Unit::Count));
    assert_eq!(flows.points.iter().find(|(at, _)| *at == hour_start).map(|point| point.1), Some(Some(3.0)), "平均数");
    assert_eq!(flows.points.iter().find(|(at, _)| *at == hour_start + 300).map(|point| point.1), Some(None), "没有数据的段是空的");
    assert_eq!(flows.points.len(), 288, "24 小时，每 5 分钟一个点");
    assert_eq!(flows.to, hour_start + 3_300, "只到过完了的那一段");

    let at_start = |series, node| {
        let view = lab.controller.series(network.id, &query(series, node)).unwrap();
        view.points.into_iter().find(|(at, _)| *at == hour_start).and_then(|point| point.1)
    };
    assert_eq!(at_start(Series::Flows, None), Some(3.0), "网络的合计");
    assert_eq!(at_start(Series::Online, None), Some(2.0), "两个节点在线");
    assert_eq!(at_start(Series::RelayAttached, Some(relay.id)), Some(1.0), "a 挂靠在 relay 上");
    assert!(at_start(Series::NodeTx, Some(a.id)).is_some_and(|rate| rate > 0.0));

    lab.controller.lock().stats_tick(start, hour_start + 3_600).unwrap();
    let hourly = SeriesQuery { step: Some(3600), from: Some(hour_start), to: Some(hour_start), ..query(Series::NodeFlows, Some(a.id)) };
    assert_eq!(lab.controller.series(network.id, &hourly).unwrap().points, [(hour_start, Some(3.0))], "整点时汇总出上一个小时的点");
}

#[test]
fn a_node_counts_once_on_a_relay_and_only_for_the_attachments_that_are_kept() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    // 不用 `join_offline`：它要 `settle`，会顺带数一次；这里要在都报完之后才数。
    let relay = |lab: &mut Lab, network| {
        let member = lab.join_with(network, &[], CAN_DO_ALL);
        lab.controller.enable_relay(network, member.id, RelaySettings::default()).unwrap();
        member
    };
    let relays: Vec<NodeId> = (0..18).map(|_| relay(&mut lab, network.id).id).collect();
    let not_enabled = lab.join_with(network.id, &[], CAN_DO_ALL);
    let elsewhere = lab.network("work");
    let foreign = relay(&mut lab, elsewhere.id);
    // a 自己也是 relay：报自己的那一条不能靠"不是 relay"挡掉。
    let a = relay(&mut lab, network.id);
    let b = lab.join(network.id, &[]);
    let wall = lab.controller.lock().clock.now();
    let hour_start = wall - wall % 3600;
    let start = Instant::now();

    let attach = |relay, rtt_ms| Attachment { relay, rtt_ms, shared: false };
    // 重复的、自己、没启用 relay 的、别的网络的 relay 排在最前面：都不算数，也不占名额。后面还有 17 个能用的。
    let mut reported = vec![attach(relays[0], 5), attach(relays[0], 99), attach(a.id, 1), attach(not_enabled.id, 1), attach(foreign.id, 1)];
    reported.extend(relays[1..].iter().map(|&relay| attach(relay, 10)));
    a.send(&lab.controller, ToController::Report(Box::new(Report { attached: reported, ..Report::default() })));
    b.send(&lab.controller, ToController::Report(Box::new(Report { attached: vec![attach(relays[0], 7)], ..Report::default() })));

    let nodes = lab.controller.nodes(network.id).unwrap();
    let stored = &nodes.iter().find(|node| node.id == a.id).unwrap().attached;
    assert_eq!(stored.iter().map(|attachment| attachment.relay).collect::<Vec<_>>(), relays[..16], "按报的顺序留前 16 个能用的");
    assert_eq!(stored[0].rtt_ms, 5, "重复的留第一次报的");

    lab.controller.lock().stats_tick(start, hour_start + 10).unwrap();
    lab.controller.lock().stats_tick(start, hour_start + 300).unwrap();

    // 下一段里 a 报它不挂靠了：不用读 relay 的那条路，统计和存下来的照样清空。
    a.send(&lab.controller, ToController::Report(Box::default()));
    let nodes = lab.controller.nodes(network.id).unwrap();
    assert!(nodes.iter().find(|node| node.id == a.id).unwrap().attached.is_empty());
    let later = start + Duration::from_secs(60);
    lab.controller.lock().stats_tick(later, hour_start + 310).unwrap();
    lab.controller.lock().stats_tick(later, hour_start + 600).unwrap();

    lab.controller.advance_clock(hour_start + 3_600 - wall);
    let attached_to = |relay, at| {
        let view = lab.controller.series(network.id, &query(Series::RelayAttached, Some(relay))).unwrap();
        view.points.into_iter().find(|point| point.0 == at).and_then(|point| point.1)
    };
    assert_eq!(attached_to(relays[0], hour_start), Some(2.0), "a 报了两次也只算一个，加上 b");
    for relay in &relays[1..16] {
        assert_eq!(attached_to(*relay, hour_start), Some(1.0), "{relay}");
    }
    assert_eq!(attached_to(relays[16], hour_start), Some(0.0), "没留下的不算：和存下来的是同一份");
    assert_eq!(attached_to(relays[17], hour_start), Some(0.0));
    assert_eq!(attached_to(a.id, hour_start), Some(0.0), "自己挂靠自己不算");
    assert_eq!(attached_to(relays[0], hour_start + 300), Some(1.0), "只剩 b");
    assert_eq!(attached_to(relays[1], hour_start + 300), Some(0.0));
}

#[test]
fn a_series_is_asked_for_with_the_right_node_step_and_range() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let a = lab.join(network.id, &[]);
    let invalid = |query: SeriesQuery| {
        let error = lab.controller.series(network.id, &query).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidRequest, "{query:?}");
    };
    invalid(query(Series::NodeTx, None));
    invalid(query(Series::Online, Some(a.id)));
    invalid(SeriesQuery { step: Some(60), ..query(Series::Online, None) });
    invalid(SeriesQuery { from: Some(2_000), to: Some(1_000), ..query(Series::Online, None) });
    invalid(SeriesQuery { from: Some(0), to: Some(30 * 24 * 3600), step: Some(300), ..query(Series::Online, None) });
    let old = SeriesQuery { from: Some(1_000_000), to: Some(1_000_000 + 7 * 24 * 3600), ..query(Series::Online, None) };
    let view = lab.controller.series(network.id, &old).unwrap();
    assert_eq!((view.step, view.points.len()), (3600, 169), "早于 24 小时的范围用 1 小时的点");
    assert!(view.points.iter().all(|(_, value)| value.is_none()));
}
