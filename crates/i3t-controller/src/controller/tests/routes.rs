//! 路由视图（白皮书 §7.7）和按主机名起名字。

use std::net::Ipv4Addr;
use std::time::Duration;

use i3t_model::admin::{NetworkId, PathSource, UpdateNode};
use i3t_model::control::{Declared, Hello, Report, Route, RouteAnswer, RouteEntry, RouteSince, ToController};
use i3t_model::update::UpdateStatus;
use i3t_proto::NodeId;

use super::super::testing::{Lab, Member, hello, modern_hello};
use super::super::{COOLDOWN_SECS, Controller};

fn direct(peer: NodeId, rtt_ms: u32) -> RouteEntry {
    RouteEntry { peer, route: Route::Direct { addr: "203.0.113.7:20330".parse().unwrap() }, rtt_ms: Some(rtt_ms) }
}

fn via(peer: NodeId, relay: NodeId) -> RouteEntry {
    RouteEntry { peer, route: Route::Relay { relays: vec![relay] }, rtt_ms: Some(40) }
}

fn report(member: &Member, controller: &Controller, routes: Vec<RouteEntry>, full: bool) {
    let report = Report { routes, routes_full: full, update: Some(UpdateStatus::Idle), ..Report::default() };
    member.send(controller, ToController::Report(report));
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
    let since = RouteSince { secs_ago: 42, reason: "直连打通了".to_owned() };
    let answer = RouteAnswer {
        peer: b.id,
        route: Some(Route::Direct { addr: "198.51.100.9:20330".parse().unwrap() }),
        rtt_ms: Some(8),
        since: Some(since.clone()),
    };
    a.send(&controller, ToController::RouteAnswer { request: to_a, answer });
    b.send(
        &controller,
        ToController::RouteAnswer { request: to_b, answer: RouteAnswer { peer: a.id, route: None, rtt_ms: None, since: None } },
    );
    let view = asking.await.unwrap();
    assert_eq!((view.forward.from.id, view.forward.to.id, view.reverse.from.id), (a.id, b.id, b.id));
    assert_eq!((view.forward.source, view.forward.rtt_ms, view.forward.age_secs), (PathSource::Query, Some(8), Some(0)));
    assert_eq!(view.forward.since, Some(since));
    assert!(matches!(view.forward.route, Some(Route::Direct { .. })));
    assert_eq!((view.reverse.source, view.reverse.route.clone()), (PathSource::NotPeers, None), "b 说 a 不在它的 netmap 里");

    // 最初那一版的节点不懂这条查询，不问它，用它上报的；a 不回答，等几秒等不到就也用上报的。
    let view = controller.path(network, a.id, old.id).await.unwrap();
    assert_eq!((view.forward.source, view.forward.rtt_ms), (PathSource::Report, Some(15)));
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
        let forged = RouteAnswer { peer: old.id, route: Some(Route::None), rtt_ms: None, since: None };
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
