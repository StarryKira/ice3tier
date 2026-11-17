//! 给界面画图的时间序列：每 5 分钟一个点，整点汇总，读的时候缺的段是空的。

use std::time::Instant;

use i3t_model::admin::ErrorCode;
use i3t_model::admin::stats::{Series, SeriesQuery, Unit};
use i3t_model::control::{Report, ToController};
use i3t_model::netmap::Attachment;

use super::super::testing::{CAN_DO_ALL, Lab};

fn query(series: Series, node: Option<i3t_proto::NodeId>) -> SeriesQuery {
    SeriesQuery { series, node, from: None, to: None, step: None }
}

#[test]
fn reports_become_points_every_five_minutes_and_hours_are_averaged() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let relay = lab.join_with(network.id, &[], CAN_DO_ALL);
    lab.controller.enable_relay(network.id, relay.id, i3t_model::admin::RelaySettings::default()).unwrap();
    let a = lab.join(network.id, &[]);
    let wall = lab.controller.lock().clock.now();
    // 一个整点之后的第一段：这样过完两段之后正好跨过下一个整点之前，不汇总；再往后拨一个小时才汇总。
    let hour_start = wall - wall % 3600;
    let start = Instant::now();

    let report = |flows, tx| {
        ToController::Report(Report {
            flows,
            tx_bytes: tx,
            run: 7,
            attached: vec![Attachment { relay: relay.id, rtt_ms: 5 }],
            ..Report::default()
        })
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
