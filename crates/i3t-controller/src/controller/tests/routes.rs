//! 路由视图和按主机名起名字。

use std::net::Ipv4Addr;
use std::time::Duration;

use i3t_model::admin::{MatrixLanes, NetworkId, PathSource, RelaySettings, UpdateNode};
use i3t_model::control::{
    BackboneLink, BackboneRow, Declared, DirectKind, Hello, LaneCounts, LaneReport, LegTransport, PathCandidate, PathDetail, PathSegment,
    PeerLanes, RelayState, Report, Route, RouteAnswer, RouteCause, RouteEntry, RouteKind, RouteSince, ToController,
};
use i3t_model::netmap::RoutingMode;
use i3t_model::update::UpdateStatus;
use i3t_proto::NodeId;

use super::super::testing::{CAN_DO_ALL, Lab, Member, hello, modern_hello};
use super::super::{COOLDOWN_SECS, Controller};

fn direct(peer: NodeId, rtt_ms: u32) -> RouteEntry {
    RouteEntry { peer, route: Route::Direct { addr: "203.0.113.7:20330".parse().unwrap() }, rtt_ms: Some(rtt_ms) }
}

fn via(peer: NodeId, relay: NodeId) -> RouteEntry {
    RouteEntry { peer, route: Route::Relay { relays: vec![relay] }, rtt_ms: Some(40) }
}

fn report(member: &Member, controller: &Controller, routes: Vec<RouteEntry>, full: bool) {
    let report = Report { routes, routes_full: full, update: Some(UpdateStatus::Idle), ..Report::default() };
    member.send(controller, ToController::Report(Box::new(report)));
}

/// 三个互相看得见的节点：两个懂新协议的，一个最初那一版的。
fn three_nodes() -> (Lab, NetworkId, Member, Member, Member) {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let mut a = lab.join_as(network, &modern_hello("0.2.0"));
    let mut b = lab.join_as(network, &modern_hello("0.2.0"));
    let mut old = lab.join_as(network, &hello(Declared::default()));
    lab.settle(&mut [&mut a, &mut b, &mut old]);
    (lab, network, a, b, old)
}

/// 矩阵里的格子：`(from, to, 是不是直连, 过期)`。
fn cells(lab: &Lab, network: NetworkId) -> Vec<(NodeId, NodeId, bool, bool)> {
    let matrix = lab.controller.route_matrix(network).unwrap();
    matrix.entries.iter().map(|entry| (entry.from, entry.to, matches!(entry.route, Route::Direct { .. }), entry.stale)).collect()
}

#[test]
fn the_matrix_follows_full_and_partial_reports() {
    let (lab, network, a, b, old) = three_nodes();
    assert!(lab.controller.route_matrix(network).unwrap().entries.is_empty(), "还没有人上报");
    assert_eq!(lab.controller.route_matrix(network).unwrap().nodes.len(), 3);

    report(&a, &lab.controller, vec![direct(b.id, 12), via(old.id, b.id)], true);
    report(&b, &lab.controller, vec![direct(a.id, 13)], true);
    assert_eq!(cells(&lab, network), [(a.id, b.id, true, false), (a.id, old.id, false, false), (b.id, a.id, true, false)]);

    // 之后的上报只带变化的条目：没提到的保持原样。
    report(&a, &lab.controller, vec![direct(old.id, 20)], false);
    assert_eq!(cells(&lab, network), [(a.id, b.id, true, false), (a.id, old.id, true, false), (b.id, a.id, true, false)]);
    // 完整的上报整个替换：没提到的就是没有了。
    report(&a, &lab.controller, vec![via(b.id, old.id)], true);
    assert_eq!(cells(&lab, network), [(a.id, b.id, false, false), (b.id, a.id, true, false)]);

    // 节点报了它的 netmap 里没有的对端：不记。
    report(&a, &lab.controller, vec![direct(NodeId::new(99).unwrap(), 1)], false);
    assert_eq!(cells(&lab, network).len(), 2);
}

#[test]
fn an_offline_node_keeps_its_last_routes_marked_stale_and_ages() {
    let (mut lab, network, a, b, _old) = three_nodes();
    report(&a, &lab.controller, vec![direct(b.id, 12)], true);
    let entry = lab.controller.route_matrix(network).unwrap().entries.remove(0);
    assert!(entry.age_secs <= 1 && !entry.stale);
    assert_eq!(lab.controller.routes(network, a.id).unwrap().routes.len(), 1);

    lab.controller.disconnect(a.session);
    assert_eq!(cells(&lab, network), [(a.id, b.id, true, true)], "离线之后留着最后报的，标成过期");
    let matrix = lab.controller.route_matrix(network).unwrap();
    assert!(matrix.nodes.iter().any(|node| node.node.id == a.id && !node.online));
    let table = lab.controller.routes(network, a.id).unwrap();
    assert!(!table.online && table.routes.is_empty(), "单个节点的路由表仍然只在它在线时给出");

    // 重新连上来之后的第一次上报是完整的，替换掉离线之前的那一份。
    let again = lab.connect_as(a.key, &modern_hello("0.2.0")).unwrap();
    assert_eq!(cells(&lab, network), [(a.id, b.id, true, true)], "刚连上还没上报：手上的还是上一条连接报的，仍然算过期");
    report(&again, &lab.controller, vec![via(b.id, b.id)], true);
    assert_eq!(cells(&lab, network), [(a.id, b.id, false, false)]);

    lab.controller.delete_node(network, a.id).unwrap();
    assert!(cells(&lab, network).is_empty(), "删掉的节点不留在矩阵里");
}

#[tokio::test]
async fn route_views_show_flows_traffic_who_uses_a_relay_and_the_recent_switches() {
    use i3t_model::control::{PeerUse, RelayPairUse, RelayUse, RouteSwitch};

    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let capable = Hello { declared: CAN_DO_ALL, ..modern_hello("0.2.0") };
    let (mut a, mut b, mut old) = (lab.join_as(network, &capable), lab.join_as(network, &capable), lab.join_as(network, &capable));
    lab.settle(&mut [&mut a, &mut b, &mut old]);
    lab.controller.enable_relay(network, b.id, RelaySettings::default()).unwrap();
    let used = PeerUse { peer: b.id, flows: 3, tx_bytes: 4096, rx_bytes: 1024 };
    let relayed = Route::Relay { relays: vec![b.id] };
    let direct_route = direct(b.id, 12).route;
    let switches = vec![
        RouteSwitch { peer: b.id, from: Route::None, to: relayed.clone(), cause: RouteCause::RelayReady, age_secs: 30 },
        RouteSwitch { peer: b.id, from: relayed.clone(), to: direct_route.clone(), cause: RouteCause::DirectVerified, age_secs: 5 },
        RouteSwitch { peer: NodeId::new(99).unwrap(), from: Route::None, to: relayed.clone(), cause: RouteCause::RelayReady, age_secs: 1 },
    ];
    let report = Report {
        routes: vec![direct(b.id, 12), via(old.id, b.id)],
        routes_full: true,
        peer_use: vec![used],
        switches,
        ..Report::default()
    };
    a.send(&lab.controller, ToController::Report(Box::new(report)));
    let users = RelayUse { window_secs: 10, pairs: vec![RelayPairUse { from: a.id, to: old.id, bytes: 50_000 }], other_bytes: 7 };
    b.send(&lab.controller, ToController::Report(Box::new(Report { relay_use: Some(users.clone()), ..Report::default() })));

    // 节点的路由表：有连接的对端写着流数和流量，没有连接的是空的。
    let table = lab.controller.routes(network, a.id).unwrap();
    let row = |peer: NodeId| table.routes.iter().find(|entry| entry.peer.id == peer).unwrap();
    assert_eq!((row(b.id).flows, row(b.id).tx_bytes, row(b.id).rx_bytes), (Some(3), Some(4096), Some(1024)));
    assert_eq!((row(old.id).flows, row(old.id).tx_bytes), (None, None));

    // relay 说谁在用它。
    let relay = lab.controller.nodes(network).unwrap().into_iter().find(|node| node.id == b.id).unwrap();
    assert_eq!(relay.relay.unwrap().users, Some(users));

    // 换路的历史：新的在前，网络之外的对端不记。
    let controller = std::sync::Arc::new(lab.controller);
    let view = controller.path_view(network, a.id, b.id, None, None).unwrap();
    let steps: Vec<(Route, Route, RouteCause)> =
        view.forward.switches.iter().map(|switch| (switch.from.clone(), switch.to.clone(), switch.cause.clone())).collect();
    assert_eq!(steps, [(relayed.clone(), direct_route, RouteCause::DirectVerified), (Route::None, relayed, RouteCause::RelayReady)]);
    assert!(view.forward.switches[0].at > view.forward.switches[1].at);
    assert!(view.reverse.switches.is_empty(), "另一个方向没有报过");
}

#[tokio::test]
async fn a_path_is_asked_from_both_ends_and_falls_back_to_the_last_report() {
    let (lab, network, mut a, mut b, mut old) = three_nodes();
    report(&a, &lab.controller, vec![direct(b.id, 12), direct(old.id, 15)], true);
    report(&old, &lab.controller, vec![via(a.id, b.id)], true);
    let controller = std::sync::Arc::new(lab.controller);

    // a 和 b 都回答。
    let asking = tokio::spawn({
        let controller = std::sync::Arc::clone(&controller);
        let (from, to) = (a.id, b.id);
        async move { controller.path(network, from, to).await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.receive();
    b.receive();
    let (&(to_a, a_peer), &(to_b, b_peer)) = (a.queries.last().unwrap(), b.queries.last().unwrap());
    assert_eq!((a_peer, b_peer), (b.id, a.id), "每一端被问的是到对端怎么走");
    let since = RouteSince { secs_ago: 42, reason: "直连打通了".to_owned(), cause: Some(RouteCause::DirectVerified) };
    let answer = RouteAnswer {
        peer: b.id,
        route: Some(Route::Direct { addr: "198.51.100.9:20330".parse().unwrap() }),
        rtt_ms: Some(8),
        since: Some(since.clone()),
        detail: None,
        connection_rtt_ms: Some(11),
    };
    a.send(&controller, ToController::RouteAnswer { request: to_a, answer });
    b.send(
        &controller,
        ToController::RouteAnswer {
            request: to_b,
            answer: RouteAnswer { peer: a.id, route: None, rtt_ms: None, since: None, detail: None, connection_rtt_ms: None },
        },
    );
    let view = asking.await.unwrap();
    assert_eq!((view.forward.from.id, view.forward.to.id, view.reverse.from.id), (a.id, b.id, b.id));
    assert_eq!((view.forward.source, view.forward.rtt_ms, view.forward.age_secs), (PathSource::Query, Some(8), Some(0)));
    assert_eq!(view.forward.connection_rtt_ms, Some(11), "推算的和连接实测的并排给出");
    assert_eq!(view.forward.since, Some(since));
    assert!(matches!(view.forward.route, Some(Route::Direct { .. })));
    assert_eq!((view.reverse.source, view.reverse.route.clone()), (PathSource::NotPeers, None), "b 说 a 不在它的 netmap 里");

    // 最初那一版的节点不懂这条查询，不问它，用它上报的；a 不回答，等几秒等不到就也用上报的。
    let view = controller.path(network, a.id, old.id).await.unwrap();
    assert_eq!((view.forward.source, view.forward.rtt_ms, view.forward.connection_rtt_ms), (PathSource::Report, Some(15), None));
    assert_eq!(view.reverse.source, PathSource::Report);
    assert_eq!(view.reverse.route, Some(Route::Relay { relays: vec![b.id] }));
    old.receive();
    assert!(old.queries.is_empty(), "不懂新协议的节点收不到路由查询");

    // 别的节点冒名回答不算数；没有上报过、也问不到的方向是没有数据。
    let view = {
        let asking = tokio::spawn({
            let controller = std::sync::Arc::clone(&controller);
            let (from, to) = (b.id, old.id);
            async move { controller.path(network, from, to).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        b.receive();
        let request = b.queries.last().unwrap().0;
        let forged =
            RouteAnswer { peer: old.id, route: Some(Route::None), rtt_ms: None, since: None, detail: None, connection_rtt_ms: None };
        a.send(&controller, ToController::RouteAnswer { request, answer: forged.clone() });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!asking.is_finished(), "a 替 b 答的不算");
        b.send(&controller, ToController::RouteAnswer { request, answer: forged });
        asking.await.unwrap()
    };
    assert_eq!((view.forward.source, view.forward.route.clone()), (PathSource::Query, Some(Route::None)));
    assert_eq!((view.reverse.source, view.reverse.age_secs), (PathSource::None, None));

    assert!(controller.path(network, a.id, a.id).await.is_err());
    assert!(controller.path(network, a.id, NodeId::new(99).unwrap()).await.is_err());
}

fn kind(peer: NodeId, direct: Option<DirectKind>, relay_leg: Option<LegTransport>) -> RouteKind {
    RouteKind { peer, direct, relay_leg, direct_status: None, alternative: None }
}

fn report_with_kinds(member: &Member, controller: &Controller, routes: Vec<RouteEntry>, kinds: Vec<RouteKind>, full: bool) {
    let report = Report { routes, route_kinds: kinds, routes_full: full, update: Some(UpdateStatus::Idle), ..Report::default() };
    member.send(controller, ToController::Report(Box::new(report)));
}

#[test]
fn matrix_cells_carry_the_senders_lanes_to_that_peer_while_it_is_online() {
    let (lab, network, a, b, old) = three_nodes();
    let to_b = PeerLanes {
        peer: b.id,
        dials: true,
        control: true,
        idle: 3,
        ready: 2,
        parked_at_peer: 1,
        udp_path: Some(false),
        counts: LaneCounts { in_use: 4, ..LaneCounts::default() },
    };
    let lanes = Some(Box::new(LaneReport { peers: vec![to_b], ..LaneReport::default() }));
    let routes = vec![direct(b.id, 12), direct(old.id, 9)];
    let from_a = Report { routes, routes_full: true, update: Some(UpdateStatus::Idle), lanes, ..Report::default() };
    a.send(&lab.controller, ToController::Report(Box::new(from_a)));
    report(&b, &lab.controller, vec![direct(a.id, 13)], true);

    let lanes_of = |controller: &Controller| -> Vec<_> {
        controller.route_matrix(network).unwrap().entries.iter().map(|entry| (entry.from, entry.to, entry.lanes)).collect()
    };
    let seen_by_a = MatrixLanes { control: true, in_use: 4, idle: 6 };
    assert_eq!(
        lanes_of(&lab.controller),
        [(a.id, b.id, Some(seen_by_a)), (a.id, old.id, None), (b.id, a.id, None)],
        "只有发送方为这个对端报了的格子带着，空闲的三种加在一起"
    );

    lab.controller.disconnect(a.session);
    assert_eq!(
        lanes_of(&lab.controller),
        [(a.id, b.id, None), (a.id, old.id, None), (b.id, a.id, None)],
        "离线之后格子留着（标成过期），连接数不留"
    );
}

#[test]
fn route_lists_say_how_a_direct_path_was_made_and_how_the_first_relay_leg_travels() {
    let (lab, network, a, b, old) = three_nodes();
    let kinds = vec![kind(b.id, Some(DirectKind::Punched), None), kind(old.id, None, Some(LegTransport::Tcp))];
    report_with_kinds(&a, &lab.controller, vec![direct(b.id, 12), via(old.id, b.id)], kinds, true);
    // 旧版本的节点不报细节。
    report(&b, &lab.controller, vec![direct(a.id, 13)], true);

    let table = lab.controller.routes(network, a.id).unwrap();
    let details: Vec<_> = table.routes.iter().map(|route| (route.peer.id, route.direct, route.relay_leg)).collect();
    assert_eq!(details, [(b.id, Some(DirectKind::Punched), None), (old.id, None, Some(LegTransport::Tcp))]);
    let matrix = lab.controller.route_matrix(network).unwrap();
    let cells: Vec<_> = matrix.entries.iter().map(|entry| (entry.from, entry.to, entry.direct, entry.relay_leg)).collect();
    assert_eq!(
        cells,
        [(a.id, b.id, Some(DirectKind::Punched), None), (a.id, old.id, None, Some(LegTransport::Tcp)), (b.id, a.id, None, None)]
    );

    // 只带变化的上报：这一条的细节说不出来了，原来记的那一份也不再对；没提到的保持原样。
    report_with_kinds(&a, &lab.controller, vec![direct(old.id, 9)], Vec::new(), false);
    let table = lab.controller.routes(network, a.id).unwrap();
    let details: Vec<_> = table.routes.iter().map(|route| (route.peer.id, route.direct, route.relay_leg)).collect();
    assert_eq!(details, [(b.id, Some(DirectKind::Punched), None), (old.id, None, None)]);
    // 细节说的对端不在这次报的条目里：不记。
    report_with_kinds(&a, &lab.controller, Vec::new(), vec![kind(old.id, Some(DirectKind::Lan), None)], false);
    assert_eq!(lab.controller.routes(network, a.id).unwrap().routes[1].direct, None);
}

#[test]
fn route_lists_say_where_the_direct_path_stands_and_what_the_other_mode_would_pick() {
    use i3t_model::control::{DirectCause, DirectState, DirectStatus, RouteAlternative};

    let (lab, network, a, b, old) = three_nodes();
    let failed = DirectStatus { state: DirectState::Failed, kind: None, cause: Some(DirectCause::NoAnswer), attempted_secs_ago: Some(30) };
    let never = DirectStatus { state: DirectState::NotTried, kind: None, cause: None, attempted_secs_ago: None };
    let kinds = vec![
        RouteKind { direct_status: Some(failed), ..kind(b.id, None, Some(LegTransport::Udp)) },
        RouteKind {
            direct_status: Some(never),
            alternative: Some(RouteAlternative { route: Route::Relay { relays: vec![b.id] }, rtt_ms: Some(48) }),
            ..kind(old.id, None, Some(LegTransport::Udp))
        },
    ];
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    report_with_kinds(&a, &lab.controller, vec![via(b.id, old.id), via(old.id, old.id)], kinds, true);
    // 旧版本的节点不报。
    report(&b, &lab.controller, vec![direct(a.id, 13)], true);

    let table = lab.controller.routes(network, a.id).unwrap();
    let to_b = table.routes[0].direct_status.unwrap();
    assert_eq!((to_b.state, to_b.kind, to_b.cause), (DirectState::Failed, None, Some(DirectCause::NoAnswer)));
    let at = to_b.attempted_at.unwrap();
    assert!(at + 30 >= before && at + 30 <= before + 2, "节点说 30 秒之前，换成 controller 的时刻：{at} {before}");
    let to_old = table.routes[1].direct_status.unwrap();
    assert_eq!((to_old.state, to_old.attempted_at), (DirectState::NotTried, None));
    assert_eq!(lab.controller.routes(network, b.id).unwrap().routes[0].direct_status, None);
    assert_eq!(table.routes[0].alternative, None, "节点没有报另一种模式会选的路");
    let other = table.routes[1].alternative.clone().unwrap();
    assert_eq!((other.route, other.rtt_ms), (Route::Relay { relays: vec![b.id] }, Some(48)), "另一种模式会选的路原样给出");
    let raw = serde_json::to_value(&table.routes[0]).unwrap();
    assert_eq!(
        raw["direct_status"],
        serde_json::json!({ "state": "failed", "kind": null, "cause": { "code": "no_answer" }, "attempted_at": at })
    );
}

#[tokio::test]
async fn a_path_shows_the_candidates_the_sender_weighed_and_why_it_chose_one() {
    let (lab, network, mut a, mut b, old) = three_nodes();
    report_with_kinds(&b, &lab.controller, vec![direct(a.id, 13)], vec![kind(a.id, Some(DirectKind::Lan), None)], true);
    let controller = std::sync::Arc::new(lab.controller);
    let asking = tokio::spawn({
        let controller = std::sync::Arc::clone(&controller);
        let (from, to) = (a.id, b.id);
        async move { controller.path(network, from, to).await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.receive();
    let request = a.queries.last().unwrap().0;
    let relay = old.id;
    let candidate = |route: Route, cost_us: u64, current: bool| PathCandidate {
        route,
        cost_us,
        legs: vec![PathSegment { from: a.id, to: b.id, rtt_us: 12_400, jitter_us: 300, loss_permille: 2 }],
        load_percent: 0,
        current,
    };
    let chosen = Route::Relay { relays: vec![relay] };
    let detail = PathDetail {
        mode: Some(RoutingMode::LowestCost),
        direct: None,
        relay_leg: Some(LegTransport::Udp),
        reason: "经 relay 代价最低".to_owned(),
        cause: Some(RouteCause::LowestCost { candidates: 2, cost_us: Some(30_000) }),
        candidates: vec![
            candidate(chosen.clone(), 30_000, true),
            candidate(Route::Direct { addr: "198.51.100.9:20330".parse().unwrap() }, 45_000, false),
        ],
    };
    let answer = RouteAnswer {
        peer: b.id,
        route: Some(chosen.clone()),
        rtt_ms: Some(30),
        since: None,
        detail: Some(detail),
        connection_rtt_ms: None,
    };
    a.send(&controller, ToController::RouteAnswer { request, answer });
    // b 不回答：退回到它上报的那一份，直连的类型取自上报。
    b.receive();
    let view = asking.await.unwrap();

    let forward = &view.forward;
    assert_eq!((forward.mode, forward.relay_leg, forward.direct), (Some(RoutingMode::LowestCost), Some(LegTransport::Udp), None));
    assert_eq!(forward.reason.as_deref(), Some("经 relay 代价最低"));
    assert_eq!(forward.cause, Some(RouteCause::LowestCost { candidates: 2, cost_us: Some(30_000) }), "原因的代码原样转给管理 API");
    let candidates = forward.candidates.as_ref().unwrap();
    assert_eq!(
        candidates.iter().map(|candidate| (candidate.cost_us, candidate.current)).collect::<Vec<_>>(),
        [(30_000, true), (45_000, false)]
    );
    assert_eq!(candidates[0].route, chosen);
    let reverse = &view.reverse;
    assert_eq!((reverse.source, reverse.direct), (PathSource::Report, Some(DirectKind::Lan)));
    assert_eq!((reverse.mode, reverse.reason.clone(), reverse.candidates.clone()), (None, None, None), "上报里没有候选，只有现问才有");
}

#[tokio::test]
async fn the_backbone_is_asked_from_online_nodes_and_each_relay_keeps_its_freshest_row() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let relay_hello = Hello { declared: CAN_DO_ALL, ..modern_hello("0.2.0") };
    let (mut r1, mut r2) = (lab.join_as(network, &relay_hello), lab.join_as(network, &relay_hello));
    let mut silent = lab.join_as(network, &relay_hello);
    let mut node = lab.join_as(network, &modern_hello("0.2.0"));
    let mut ancient = lab.join_as(network, &hello(Declared::default()));
    for relay in [&r1, &r2, &silent] {
        let settings = RelaySettings::default();
        lab.controller.enable_relay(network, relay.id, settings).unwrap();
    }
    lab.settle(&mut [&mut r1, &mut r2, &mut silent, &mut node, &mut ancient]);
    let controller = std::sync::Arc::new(lab.controller);

    let asking = tokio::spawn({
        let controller = std::sync::Arc::clone(&controller);
        async move { controller.backbone(network).await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    for member in [&mut r1, &mut r2, &mut silent, &mut node, &mut ancient] {
        member.receive();
    }
    assert!(ancient.backbone_queries.is_empty(), "不懂这条查询的旧节点不问");
    assert_eq!([&r1, &r2, &silent, &node].map(|member| member.backbone_queries.len()), [1, 1, 1, 1], "先问 relay，再问别的节点");

    let row = |relay: NodeId, age_ms: u64, to: NodeId, rtt_ms: u32| BackboneRow {
        relay,
        state: RelayState::Serving,
        load_percent: 5,
        age_ms,
        links: vec![BackboneLink { to, rtt_ms: Some(rtt_ms), jitter_ms: 1, loss_permille: 0 }],
    };
    let stranger = NodeId::new(99).unwrap();
    // r1 手上有 r2 的行（没有它自己的）；node 手上 r1、r2 的都有，r1 的比 r2 给的新，r2 的比 r1 给的旧。
    let answer = |member: &Member, rows: Vec<BackboneRow>| {
        let request = member.backbone_queries[0];
        member.send(&controller, ToController::BackboneAnswer { request, rows });
    };
    answer(&r1, vec![row(r2.id, 300, r1.id, 20), row(stranger, 1, r1.id, 1)]);
    answer(&r2, vec![row(r1.id, 900, r2.id, 21)]);
    answer(&node, vec![row(r1.id, 100, r2.id, 22), row(r2.id, 800, r1.id, 23), row(r1.id, 50, stranger, 1)]);
    // r1 冒名替 silent 回答：不算数。silent 自己一直不回答。
    r1.send(&controller, ToController::BackboneAnswer { request: silent.backbone_queries[0], rows: vec![row(silent.id, 1, r1.id, 1)] });
    let view = asking.await.unwrap();

    let rows: Vec<_> = view
        .relays
        .iter()
        .map(|relay| {
            (relay.relay.id, relay.reported_by, relay.age_ms, relay.links.iter().map(|link| (link.to, link.rtt_ms)).collect::<Vec<_>>())
        })
        .collect();
    assert_eq!(
        rows,
        [
            (r1.id, Some(node.id), Some(50), vec![]),
            (r2.id, Some(r1.id), Some(300), vec![(r1.id, Some(20))]),
            (silent.id, None, None, vec![]),
        ],
        "每个 relay 取最新的一行；不在这个网络里的 relay 和链路不收"
    );
    assert_eq!((view.relays[0].state, view.relays[0].load_percent, view.relays[2].state), (Some(RelayState::Serving), Some(5), None));
    assert!(view.relays.iter().all(|relay| relay.online));
    assert_eq!(view.answered.iter().map(|node| node.id).collect::<Vec<_>>(), [r1.id, r2.id, node.id]);
}

#[test]
fn new_nodes_are_named_after_their_hostname() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let named = |hostname: &str| Hello { hostname: hostname.to_owned(), ..hello(Declared::default()) };
    let name_of = |lab: &Lab, id: NodeId| lab.controller.node(network, id).unwrap().node.name;

    let nas = lab.join_as(network, &named("NAS-01.lan"));
    assert_eq!(name_of(&lab, nas.id), "nas-01");
    assert_eq!(lab.controller.node(network, nas.id).unwrap().node.hostname, "NAS-01.lan", "原样留着给人看");
    let twin = lab.join_as(network, &named("nas-01"));
    let triplet = lab.join_as(network, &named("nas_01"));
    assert_eq!((name_of(&lab, twin.id), name_of(&lab, triplet.id)), ("nas-01-2".to_owned(), "nas-01-3".to_owned()));

    // 取不出名字的主机名、没有报主机名的旧节点：还是按编号起名字。
    for hostname in ["", "localhost", "服务器", "42"] {
        let node = lab.join_as(network, &named(hostname));
        assert_eq!(name_of(&lab, node.id), format!("node-{}", node.id), "{hostname:?}");
    }
    // `node-<数字>` 留给默认名字，免得以后编号恰好是那个数的节点撞名。
    let clash = lab.join_as(network, &named("node-9"));
    assert_eq!(name_of(&lab, clash.id), format!("node-{}", clash.id));

    // 管理员改过的名字、还在冷却期的名字，都不能再分出去。
    lab.controller.update_node(network, twin.id, &UpdateNode { name: Some("media".to_owned()), ..UpdateNode::default() }).unwrap();
    let media = lab.join_as(network, &named("media"));
    assert_eq!(name_of(&lab, media.id), "media-2");
    lab.controller.disconnect(nas.session);
    lab.controller.delete_node(network, nas.id).unwrap();
    let reborn = lab.join_as(network, &named("nas-01"));
    assert_eq!(name_of(&lab, reborn.id), "nas-01-2", "nas-01 在冷却期，nas-01-2 已经空出来了");
    lab.controller.advance_clock(COOLDOWN_SECS + 1);
    let later = lab.join_as(network, &named("nas-01"));
    assert_eq!(name_of(&lab, later.id), "nas-01");

    // 另一个网络里的同名节点互不相干；主机名后来变了，名字不跟着变。
    let other = lab.network("lab").id;
    let elsewhere = lab.join_as(other, &named("nas-01"));
    assert_eq!(lab.controller.node(other, elsewhere.id).unwrap().node.name, "nas-01");
    lab.controller.disconnect(later.session);
    let renamed_host = lab.connect_as(later.key, &named("backup")).unwrap();
    let detail = lab.controller.node(network, renamed_host.id).unwrap().node;
    assert_eq!((detail.name.as_str(), detail.hostname.as_str()), ("nas-01", "backup"));
    assert_eq!(detail.virtual_ip, lab.netmap(renamed_host.id).me.virtual_ip);
    let _: Ipv4Addr = detail.virtual_ip;
}
