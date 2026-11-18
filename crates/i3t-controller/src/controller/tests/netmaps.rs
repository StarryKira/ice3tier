//! 每个节点拿到的 netmap：最小权限，内容变了才动版本号，只发给受影响的节点。

use i3t_model::admin::{Direction, ForwardInput, LaneBlock, LaneOpening, PairInput, RelaySettings, UpdateNode, UpdateProxy};
use i3t_model::control::{Declared, Hello, Report, ToController};
use i3t_model::netmap::{self, Attachment, ExitRole, ForwardProtocol, LaneMode, ListenerProtocol, Reach, Verifier};

use super::super::Controller;
use super::super::testing::{CAN_DO_ALL, Lab, REMOTE, proxy};
use super::{add_rule, drop_default_rule, peer_ids, relations, rename};

/// 启用 relay 的设置：只有带宽上限，地址由 controller 自己找出来。
fn relay_settings() -> RelaySettings {
    RelaySettings { bandwidth_mbps: Some(100) }
}

#[test]
fn the_default_network_is_fully_connected() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let nodes: Vec<_> = (0..4).map(|_| lab.join_offline(network.id, &[], Declared::default())).collect();
    for &node in &nodes {
        let netmap = lab.netmap(node);
        let others: Vec<_> = nodes.iter().copied().filter(|other| *other != node).collect();
        assert_eq!(peer_ids(&netmap), others);
        assert!(netmap.peers.iter().all(|peer| peer.inbound && peer.outbound));
        assert_eq!(netmap.me.id, node);
        assert_eq!((netmap.network.name.as_str(), netmap.network.domain.as_str()), ("home", "home.i3t"));
        assert!(netmap.relays.is_empty() && netmap.relay_clients.is_empty() && netmap.me.proxies.is_empty());
        assert!(netmap.me.relay.is_none() && netmap.me.exit.is_none(), "能力默认全关");
    }
    let first = lab.controller.node(network.id, nodes[0]).unwrap().node;
    let seen = lab.netmap(nodes[1]).peer(nodes[0]).cloned().unwrap();
    assert_eq!((seen.key, seen.name, seen.virtual_ip), (first.key, first.name, first.virtual_ip));
    assert_eq!(seen.endpoints, vec![std::net::SocketAddr::new(REMOTE, 20330)], "来源 IP 加自报的端口是一个候选端点");
}

#[test]
fn a_one_way_rule_gives_inbound_to_one_side_and_outbound_to_the_other() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    add_rule(&lab, network.id, "laptops", "servers", Direction::OneWay);
    let laptop = lab.join_offline(network.id, &["laptops"], Declared::default());
    let server = lab.join_offline(network.id, &["servers"], Declared::default());
    assert_eq!(relations(&lab.netmap(laptop)), vec![(server, false, true)]);
    assert_eq!(relations(&lab.netmap(server)), vec![(laptop, true, false)]);
}

#[test]
fn nodes_in_groups_no_rule_connects_do_not_see_each_other_at_all() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    add_rule(&lab, network.id, "a", "c", Direction::Both);
    let a = lab.join_offline(network.id, &["a"], CAN_DO_ALL);
    let b = lab.join_offline(network.id, &["b"], CAN_DO_ALL);
    let c = lab.join_offline(network.id, &["c"], CAN_DO_ALL);
    rename(&lab, network.id, b, "hidden-b");
    lab.controller.enable_exit(network.id, b, ExitRole::default()).unwrap();

    assert_eq!(peer_ids(&lab.netmap(a)), vec![c]);
    assert_eq!(peer_ids(&lab.netmap(c)), vec![a]);
    assert!(lab.netmap(b).peers.is_empty());
    let b_key = lab.controller.node(network.id, b).unwrap().node.key.to_string();
    for node in [a, c] {
        let json = serde_json::to_string(&lab.netmap(node)).unwrap();
        assert!(!json.contains(&b_key) && !json.contains("hidden-b"), "没有规则连通的节点连公钥和名字都不该出现");
    }
    let graph = lab.controller.access_graph(network.id).unwrap();
    assert_eq!(graph.unconnected.iter().map(|node| node.id).collect::<Vec<_>>(), vec![b]);
}

#[test]
fn two_networks_never_see_each_other() {
    let mut lab = Lab::new();
    let (home, office) = (lab.network("home"), lab.network("office"));
    let ours = lab.join_offline(home.id, &["shared"], CAN_DO_ALL);
    let theirs = lab.join_offline(office.id, &["shared"], CAN_DO_ALL);
    let their_relay = lab.join_offline(office.id, &["shared"], CAN_DO_ALL);
    lab.controller.enable_relay(office.id, their_relay, relay_settings()).unwrap();
    lab.controller.enable_exit(office.id, their_relay, ExitRole::default()).unwrap();
    lab.controller.create_proxy(office.id, proxy(theirs, "127.0.0.1:1080", their_relay)).unwrap();
    add_rule(&lab, home.id, "shared", "shared", Direction::Both);

    let netmap = lab.netmap(ours);
    assert!(netmap.peers.is_empty() && netmap.relays.is_empty() && netmap.relay_clients.is_empty() && netmap.me.proxies.is_empty());
    assert_eq!(netmap.network.name, "home");
    assert_eq!(peer_ids(&lab.netmap(theirs)), vec![their_relay]);
    assert_eq!(lab.netmap(their_relay).relay_clients.iter().map(|client| client.id).collect::<Vec<_>>(), vec![theirs]);
    assert!(home.cidr.contains(&netmap.me.virtual_ip) && !office.cidr.contains(&netmap.me.virtual_ip));
}

#[test]
fn relays_are_listed_for_everyone_but_relay_clients_only_for_relays() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    let relay = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let a = lab.join_offline(network.id, &["a"], Declared::default());
    let b = lab.join_offline(network.id, &["b"], Declared::default());
    lab.controller.enable_relay(network.id, relay, relay_settings()).unwrap();
    let relay_view = lab.controller.node(network.id, relay).unwrap().node;

    for node in [a, b] {
        let netmap = lab.netmap(node);
        assert!(netmap.peers.is_empty(), "没有任何规则：relay 不是对端");
        assert_eq!(netmap.relays.len(), 1);
        let listed = &netmap.relays[0];
        assert_eq!((listed.id, listed.key, listed.name.as_str()), (relay, relay_view.key, relay_view.name.as_str()));
        let observed = std::net::SocketAddr::new(REMOTE, 20330);
        assert_eq!(listed.udp, vec![observed], "controller 看到的地址，UDP 和 TCP 是同一组");
        assert_eq!(listed.tcp, vec![observed]);
        assert!(netmap.relay_clients.is_empty(), "全网的公钥只给 relay");
        assert!(netmap.me.relay.is_none());
    }
    let netmap = lab.netmap(relay);
    assert_eq!(netmap.relay_clients.iter().map(|client| client.id).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(netmap.me.relay.as_ref().unwrap().bandwidth_mbps, Some(100));
    assert!(netmap.relays.is_empty(), "relay 的列表里没有它自己");

    // 节点可以固定挂靠到 relay；relay 停用之后固定的那一项不再下发。
    lab.controller.update_node(network.id, a, &UpdateNode { pinned_relays: Some(vec![relay]), ..UpdateNode::default() }).unwrap();
    assert_eq!(lab.netmap(a).me.pinned_relays, vec![relay]);
    let not_a_relay = UpdateNode { pinned_relays: Some(vec![b]), ..UpdateNode::default() };
    assert!(lab.controller.update_node(network.id, a, &not_a_relay).is_err());
    lab.controller.disable_relay(network.id, relay).unwrap();
    assert!(lab.netmap(a).relays.is_empty() && lab.netmap(a).me.pinned_relays.is_empty());
    assert!(lab.netmap(relay).relay_clients.is_empty());
}

#[test]
fn a_proxy_goes_only_to_its_node_with_a_verifier_and_never_the_password() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let gateway = lab.join_offline(network.id, &[], Declared::default());
    let exit = lab.join_offline(network.id, &[], CAN_DO_ALL);
    lab.controller.enable_exit(network.id, exit, ExitRole::default()).unwrap();
    let mut input = proxy(gateway, "192.168.1.5:8080", exit);
    input.protocol = ListenerProtocol::Http;
    input.allow_from = vec!["192.168.1.0/24".parse().unwrap()];
    let created = lab.controller.create_proxy(network.id, input).unwrap();
    let local = lab.controller.create_proxy(network.id, proxy(gateway, "127.0.0.1:8080", exit)).unwrap();

    let netmap = lab.netmap(gateway);
    let [lan, open] = &netmap.me.proxies[..] else { panic!("{:?}", netmap.me.proxies) };
    assert_eq!(
        (lan.id, lan.bind, lan.protocol, lan.exit, lan.allow_from.len()),
        (created.id, created.bind, ListenerProtocol::Http, exit, 1)
    );
    let verifier = lan.auth.expect("不在回环上的代理一定要认证");
    assert_eq!(verifier.hash_of(created.username.as_bytes(), created.password.as_bytes()), verifier.hash.0);
    let text = serde_json::to_string(&netmap).unwrap();
    assert!(!text.contains(&created.password) && !text.contains(&created.username), "用户名和密码都不下发，只有校验值");
    assert_eq!((open.id, open.auth), (local.id, None), "回环上没有写用户名和密码的不认证");
    assert!(lab.netmap(exit).me.proxies.is_empty(), "出口拿不到经它出去的代理");

    // 改了密码，盐跟着换。
    let before: Verifier = verifier;
    let changed = UpdateProxy { password: Some("another".into()), ..UpdateProxy::default() };
    lab.controller.update_proxy(network.id, created.id, changed).unwrap();
    let after = lab.netmap(gateway).me.proxies[0].auth.unwrap();
    assert_ne!(after.salt, before.salt);
    assert_eq!(after.hash_of(created.username.as_bytes(), b"another"), after.hash.0);

    // 停用的不下发，删掉的也没了。
    let off = UpdateProxy { enabled: Some(false), ..UpdateProxy::default() };
    lab.controller.update_proxy(network.id, local.id, off).unwrap();
    assert_eq!(lab.netmap(gateway).me.proxies.iter().map(|proxy| proxy.id).collect::<Vec<_>>(), [created.id]);
    lab.controller.delete_proxy(network.id, created.id).unwrap();
    assert!(lab.netmap(gateway).me.proxies.is_empty());
}

#[test]
fn a_forward_is_delivered_to_both_ends_only_while_the_listener_may_access_the_target() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    let listen = lab.join_offline(network.id, &["laptops"], Declared::default());
    let target = lab.join_offline(network.id, &["servers"], CAN_DO_ALL);
    let bystander = lab.join_offline(network.id, &["laptops"], CAN_DO_ALL);
    lab.switch_on_forward_target(network.id, target);
    let input = ForwardInput {
        name: "db".to_owned(),
        protocol: ForwardProtocol::Udp,
        listen_node: listen,
        bind: "127.0.0.1:5432".parse().unwrap(),
        target_node: target,
        target_addr: "192.168.1.10:5432".parse().unwrap(),
        allow_from: vec!["127.0.0.0/8".parse().unwrap()],
        enabled: true,
        class: None,
        proxy_protocol: None,
    };
    let created = lab.controller.create_forward(network.id, input.clone()).unwrap();
    assert_eq!(created.status, i3t_model::admin::ForwardStatus::AccessDenied);
    assert!(lab.netmap(listen).me.forwards.is_empty() && lab.netmap(target).me.forward_targets.is_empty(), "转发不绕过访问规则");

    let rule = add_rule(&lab, network.id, "laptops", "servers", Direction::OneWay);
    let listening = lab.netmap(listen).me.forwards;
    assert_eq!(listening.len(), 1);
    assert_eq!(
        (listening[0].id, listening[0].protocol, listening[0].bind, listening[0].target),
        (created.id, ForwardProtocol::Udp, input.bind, target)
    );
    assert_eq!(listening[0].allow_from, input.allow_from);
    let serving = lab.netmap(target).me.forward_targets;
    assert_eq!(
        serving,
        vec![netmap::ForwardTarget {
            id: created.id,
            protocol: ForwardProtocol::Udp,
            addr: input.target_addr,
            listener: listen,
            proxy_protocol: None
        }]
    );
    let bystander_map = lab.netmap(bystander);
    assert!(bystander_map.me.forwards.is_empty() && bystander_map.me.forward_targets.is_empty());

    // 同一个节点既监听又是目标（监听在虚拟 IP 上的用法）不需要规则。
    let own = ForwardInput { listen_node: target, bind: "10.66.0.2:8080".parse().unwrap(), ..input.clone() };
    let own = lab.controller.create_forward(network.id, own).unwrap();
    assert!(lab.netmap(target).me.forwards.iter().any(|forward| forward.id == own.id && forward.target == target));
    assert_eq!(lab.netmap(target).me.forward_targets.len(), 2);

    lab.controller.replace_forward(network.id, created.id, ForwardInput { enabled: false, ..input.clone() }).unwrap();
    assert!(lab.netmap(listen).me.forwards.is_empty());
    lab.controller.replace_forward(network.id, created.id, input).unwrap();
    lab.controller.delete_rule(network.id, rule).unwrap();
    assert!(lab.netmap(listen).me.forwards.is_empty() && lab.netmap(target).me.forward_targets.len() == 1);
}

#[test]
fn peer_details_follow_least_privilege() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    add_rule(&lab, network.id, "users", "exits", Direction::OneWay);
    let mut exit = lab.join_with(network.id, &["exits"], CAN_DO_ALL);
    let relay = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let user = lab.join_offline(network.id, &["users"], Declared::default());
    lab.controller.enable_relay(network.id, relay, relay_settings()).unwrap();
    lab.controller.enable_exit(network.id, exit.id, ExitRole { lan: true, ports: None, ..ExitRole::default() }).unwrap();
    lab.controller.set_pair(network.id, user, exit.id, PairInput { keep_alive: Some(true), lane: None, congestion: None }).unwrap();
    let report = Report {
        endpoints: vec!["192.168.1.5:20330".parse().unwrap(), "203.0.113.50:20330".parse().unwrap(), "192.168.1.5:20330".parse().unwrap()],
        // 挂靠关系里只有真的是 relay 的才下发，并且按延迟排。
        attached: vec![Attachment { relay: user, rtt_ms: 1, shared: false }, Attachment { relay, rtt_ms: 30, shared: false }],
        ..Report::default()
    };
    lab.controller.handle(exit.session, REMOTE, ToController::Report(Box::new(report))).unwrap();
    lab.settle(&mut [&mut exit]);

    let seen_by_user = lab.netmap(user).peer(exit.id).cloned().unwrap();
    assert_eq!(seen_by_user.exit, Some(netmap::PeerExit { lan: true, class: None, optimistic: None }));
    assert!(seen_by_user.keep_alive);
    assert_eq!(seen_by_user.attached, vec![Attachment { relay, rtt_ms: 30, shared: false }]);
    let expected: Vec<std::net::SocketAddr> =
        vec!["192.168.1.5:20330".parse().unwrap(), "203.0.113.50:20330".parse().unwrap(), std::net::SocketAddr::new(REMOTE, 20330)];
    assert_eq!(seen_by_user.endpoints, expected);
    let me = &exit.netmap().me;
    assert_eq!(me.exit, Some(ExitRole { lan: true, ports: None, ..ExitRole::default() }));

    // 出口只能被 users 访问；它自己看 user 时没有出口的提示，user 也不是出口。
    let seen_by_exit = exit.netmap().peer(user).cloned().unwrap();
    assert!(seen_by_exit.inbound && !seen_by_exit.outbound && seen_by_exit.exit.is_none() && seen_by_exit.keep_alive);

    // 只走 relay 的节点不向对端暴露地址；只主动连接的节点带着标记。
    let hide = UpdateNode { reach: Some(Some(Reach::RelayOnly)), dial_out_only: Some(true), ..UpdateNode::default() };
    lab.controller.update_node(network.id, exit.id, &hide).unwrap();
    lab.settle(&mut [&mut exit]);
    let hidden = lab.netmap(user).peer(exit.id).cloned().unwrap();
    assert!(hidden.endpoints.is_empty() && hidden.dial_out_only);
    assert_eq!((exit.netmap().me.reach, exit.netmap().me.dial_out_only), (Some(Reach::RelayOnly), true));
    lab.controller.update_node(network.id, exit.id, &UpdateNode { reach: Some(None), ..UpdateNode::default() }).unwrap();
    assert_eq!(lab.netmap(user).peer(exit.id).unwrap().endpoints.len(), 3);
}

#[test]
fn the_version_moves_only_when_the_content_changes() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut a = lab.join(network.id, &[]);
    let mut b = lab.join(network.id, &[]);
    lab.settle(&mut [&mut a, &mut b]);
    let (version, received) = (a.netmap().version, a.snapshots.len());
    let node = lab.controller.node(network.id, a.id).unwrap().node;
    assert_eq!((node.netmap_version, node.sent_version, node.acked_version), (version, version, version));

    // 这些操作都不改变 a 的 netmap 的内容。
    rename(&lab, network.id, b.id, &b.netmap().me.name.clone());
    lab.controller.update_node(network.id, b.id, &UpdateNode::default()).unwrap();
    let disabled = i3t_model::admin::CreateRule {
        name: String::new(),
        src: lab.group(network.id, "x"),
        dst: lab.group(network.id, "y"),
        direction: Direction::Both,
        enabled: false,
    };
    // 分组改名也不出节点协议。
    let x = lab.controller.update_group(network.id, disabled.src, &i3t_model::admin::UpdateGroup { name: Some("X2".to_owned()) });
    assert_eq!(x.unwrap().name, "X2");
    lab.controller.create_rule(network.id, &disabled).unwrap();
    lab.controller.update_network(network.id, &i3t_model::admin::UpdateNetwork::default()).unwrap();
    lab.settle(&mut [&mut a, &mut b]);
    assert_eq!((a.netmap().version, a.snapshots.len()), (version, received));
    assert_eq!(lab.netmap(a.id).version, version);

    rename(&lab, network.id, b.id, "renamed");
    lab.settle(&mut [&mut a, &mut b]);
    assert_eq!((a.netmap().version, a.snapshots.len()), (version + 1, received + 1));
    assert_eq!(a.netmap().peer(b.id).unwrap().name, "renamed");
    assert_eq!(a.netmap(), &lab.netmap(a.id), "查到的 netmap 就是发出去的那一份");
}

#[test]
fn only_affected_nodes_get_a_new_netmap() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    add_rule(&lab, network.id, "a", "a", Direction::Both);
    let mut a1 = lab.join(network.id, &["a"]);
    let mut a2 = lab.join(network.id, &["a"]);
    let mut outsider = lab.join(network.id, &["b"]);
    lab.settle(&mut [&mut a1, &mut a2, &mut outsider]);
    let before = (a1.snapshots.len(), a2.snapshots.len(), outsider.snapshots.len());

    rename(&lab, network.id, a1.id, "renamed");
    lab.settle(&mut [&mut a1, &mut a2, &mut outsider]);
    assert_eq!((a1.snapshots.len(), a2.snapshots.len(), outsider.snapshots.len()), (before.0 + 1, before.1 + 1, before.2));

    // 一条不涉及任何现有节点的规则，谁也不用重发。
    add_rule(&lab, network.id, "c", "d", Direction::Both);
    lab.settle(&mut [&mut a1, &mut a2, &mut outsider]);
    assert_eq!((a1.snapshots.len(), a2.snapshots.len(), outsider.snapshots.len()), (before.0 + 1, before.1 + 1, before.2));
}

#[test]
fn a_restart_keeps_identity_versions_and_the_admin_token() {
    let dir = tempfile::tempdir().unwrap();
    let opened = Controller::open(dir.path(), "ctl.example.com:20331".to_owned()).unwrap();
    let admin_token = opened.admin_token.expect("第一次启动要生成管理令牌");
    assert!(opened.controller.is_admin_token(&admin_token) && !opened.controller.is_admin_token("wrong"));
    let controller_key = opened.controller.public_key();
    let mut lab = Lab::around(opened.controller);
    let network = lab.network("home");
    let mut a = lab.join(network.id, &[]);
    let mut b = lab.join(network.id, &[]);
    lab.settle(&mut [&mut a, &mut b]);
    let (version, a_key) = (a.netmap().version, a.key);
    let expected = a.netmap().clone();
    drop(lab);

    let reopened = Controller::open(dir.path(), "ctl.example.com:20331".to_owned()).unwrap();
    assert!(reopened.admin_token.is_none(), "管理令牌只在第一次启动时给出");
    assert!(reopened.controller.is_admin_token(&admin_token));
    assert_eq!(reopened.controller.public_key(), controller_key);
    let mut lab = Lab::around(reopened.controller);
    let node = lab.controller.node(network.id, a.id).unwrap().node;
    assert!(!node.online && node.last_seen_at.is_none(), "只注册过、从来没有带着 Hello 连上来过的节点没有最近在线的时间");
    let a = lab.connect(a_key, Declared::default()).unwrap();
    assert_eq!(a.netmap().version, version, "内容没变，重启之后版本号也不变");
    assert_eq!(a.netmap(), &expected);

    let fresh = lab.controller.issue_admin_token().unwrap();
    assert!(lab.controller.is_admin_token(&fresh) && !lab.controller.is_admin_token(&admin_token));
}

#[test]
fn the_pair_view_says_how_old_each_lane_report_is_and_whether_new_tcp_streams_use_lanes_now() {
    use i3t_model::control::{LANE_REVISION, LaneReport, PROTOCOL_REVISION, PeerLanes};

    let mut lab = Lab::new();
    let network = lab.network("home");
    let hello = Hello { protocol: PROTOCOL_REVISION.max(LANE_REVISION), ..super::super::testing::hello(CAN_DO_ALL) };
    let mut relay = lab.join_as(network.id, &hello);
    let mut member = lab.join_as(network.id, &hello);
    lab.settle(&mut [&mut relay, &mut member]);
    lab.controller.enable_relay(network.id, relay.id, relay_settings()).unwrap();
    let lane = PairInput { keep_alive: None, lane: Some(LaneMode::UdpFallback), congestion: None };
    lab.controller.set_pair(network.id, relay.id, member.id, lane).unwrap();
    let report = |peer, udp_path| {
        let lanes = PeerLanes {
            peer,
            dials: false,
            control: true,
            idle: 0,
            ready: 0,
            parked_at_peer: 0,
            udp_path,
            counts: i3t_model::control::LaneCounts::default(),
        };
        ToController::Report(Box::new(Report {
            lanes: Some(Box::new(LaneReport { peers: vec![lanes], ..LaneReport::default() })),
            ..Report::default()
        }))
    };
    member.send(&lab.controller, report(relay.id, Some(false)));
    relay.send(&lab.controller, report(member.id, Some(true)));

    let pair = lab.controller.pairs(network.id).unwrap().remove(0);
    assert!(pair.lanes.iter().all(|side| side.reported_secs_ago == Some(0)), "刚报的：{:?}", pair.lanes);
    let mut opening = pair.lane_effect.unwrap().opening;
    opening.sort_by_key(|side| side.node);
    let expected =
        [LaneOpening { node: relay.id, udp_path: true, lanes: false }, LaneOpening { node: member.id, udp_path: false, lanes: true }];
    let mut expected = expected.to_vec();
    expected.sort_by_key(|side| side.node);
    assert_eq!(opening, expected, "共用连接上没有 UDP 的那一边在 lane 上开新的 TCP 流");
    assert_eq!(lab.controller.node(network.id, member.id).unwrap().reported_secs_ago, Some(0));

    // 没有 UDP、控制 lane 却不在（拨号方那一段有 UDP，不拨；或者还没拨上来）：新的流直接走共用的连接。
    let mut down = report(relay.id, Some(false));
    if let ToController::Report(report) = &mut down {
        report.lanes.as_mut().unwrap().peers[0].control = false;
    }
    member.send(&lab.controller, down);
    let opening = lab.controller.pairs(network.id).unwrap().remove(0).lane_effect.unwrap().opening;
    let member_side = opening.iter().find(|side| side.node == member.id).unwrap();
    assert_eq!((member_side.udp_path, member_side.lanes), (false, false), "lane 没建着时不说走 ice3proxy");

    lab.controller
        .set_pair(network.id, relay.id, member.id, PairInput { keep_alive: None, lane: Some(LaneMode::Always), congestion: None })
        .unwrap();
    assert!(lab.controller.pairs(network.id).unwrap()[0].lane_effect.as_ref().unwrap().opening.is_empty(), "只在 udp_fallback 时说");
}

/// 一个说着指定版本节点协议的节点加入，保持在线。
fn join_speaking(lab: &mut Lab, network: i3t_model::admin::NetworkId, protocol: u32) -> super::super::testing::Member {
    let hello = Hello { protocol, ..super::super::testing::modern_hello("0.1.19") };
    lab.join_as(network, &hello)
}

#[test]
fn a_pair_without_a_setting_gets_udp_fallback_only_when_both_nodes_dial_on_demand() {
    use i3t_model::control::LANE_DEFAULT_REVISION;

    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut a = join_speaking(&mut lab, network.id, LANE_DEFAULT_REVISION);
    let mut b = join_speaking(&mut lab, network.id, LANE_DEFAULT_REVISION);
    // 上一版的节点：懂 ice3proxy，但在 udp_fallback 下一直拨着。
    let mut old = join_speaking(&mut lab, network.id, LANE_DEFAULT_REVISION - 1);
    lab.settle(&mut [&mut a, &mut b, &mut old]);
    let lane_of = |member: &super::super::testing::Member, peer| member.netmap().peer(peer).unwrap().lane;

    assert_eq!((lane_of(&a, b.id), lane_of(&b, a.id)), (LaneMode::UdpFallback, LaneMode::UdpFallback), "没有设置：默认的 udp_fallback");
    assert_eq!((lane_of(&a, old.id), lane_of(&old, a.id)), (LaneMode::Off, LaneMode::Off), "有一个是旧节点：照旧是 off，两边一样");
    assert!(lab.controller.pairs(network.id).unwrap().is_empty(), "默认值不是存下的设置，列表里没有");

    // 存下的设置照原样：off 就是 off；旧节点也照样拿到存下的 always。
    let set = |mode| PairInput { keep_alive: None, lane: Some(mode), congestion: None };
    lab.controller.set_pair(network.id, a.id, b.id, set(LaneMode::Off)).unwrap();
    lab.controller.set_pair(network.id, a.id, old.id, set(LaneMode::Always)).unwrap();
    lab.settle(&mut [&mut a, &mut b, &mut old]);
    assert_eq!((lane_of(&a, b.id), lane_of(&b, a.id)), (LaneMode::Off, LaneMode::Off), "存下的 off");
    assert_eq!((lane_of(&a, old.id), lane_of(&old, a.id)), (LaneMode::Always, LaneMode::Always));
    // 去掉设置回到默认。
    lab.controller.delete_pair(network.id, a.id, b.id).unwrap();
    lab.controller.delete_pair(network.id, a.id, old.id).unwrap();
    lab.settle(&mut [&mut a, &mut b, &mut old]);
    assert_eq!((lane_of(&a, b.id), lane_of(&a, old.id)), (LaneMode::UdpFallback, LaneMode::Off));

    // 旧节点更新之后重新连上来：它的对端随之拿到默认值，不用等别的变化。
    lab.controller.disconnect(old.session);
    let mut updated =
        lab.connect_as(old.key, &Hello { protocol: LANE_DEFAULT_REVISION, ..super::super::testing::modern_hello("0.1.19") }).unwrap();
    lab.settle(&mut [&mut a, &mut b, &mut updated]);
    assert_eq!((lane_of(&a, updated.id), lane_of(&updated, a.id)), (LaneMode::UdpFallback, LaneMode::UdpFallback));
    // 又换回旧的程序（回滚）：两边回到 off。
    lab.controller.disconnect(updated.session);
    let mut rolled_back =
        lab.connect_as(old.key, &Hello { protocol: LANE_DEFAULT_REVISION - 1, ..super::super::testing::modern_hello("0.1.18") }).unwrap();
    lab.settle(&mut [&mut a, &mut b, &mut rolled_back]);
    assert_eq!((lane_of(&a, rolled_back.id), lane_of(&rolled_back, a.id)), (LaneMode::Off, LaneMode::Off));
}

#[test]
fn the_view_of_a_pair_without_a_setting_says_it_is_the_default_and_what_stops_it() {
    use i3t_model::control::{LANE_DEFAULT_REVISION, LANE_REVISION};

    let mut lab = Lab::new();
    let network = lab.network("home");
    let declared = Hello { protocol: LANE_DEFAULT_REVISION, ..super::super::testing::hello(CAN_DO_ALL) };
    let mut relay = lab.join_as(network.id, &declared);
    let mut member = join_speaking(&mut lab, network.id, LANE_DEFAULT_REVISION);
    let mut old = join_speaking(&mut lab, network.id, LANE_REVISION);
    lab.settle(&mut [&mut relay, &mut member, &mut old]);
    lab.controller.enable_relay(network.id, relay.id, relay_settings()).unwrap();

    let view = lab.controller.pair(network.id, member.id, relay.id).unwrap();
    assert!(!view.stored, "没有存下的设置");
    let setting = view.setting;
    assert_eq!((setting.a, setting.b), (relay.id.min(member.id), relay.id.max(member.id)), "先后顺序无所谓");
    assert_eq!((setting.keep_alive, setting.lane, setting.congestion), (false, LaneMode::UdpFallback, None), "各项都是默认值");
    let effect = view.lane_effect.unwrap();
    assert!(effect.effective && effect.dialers == vec![member.id], "{effect:?}");
    assert!(effect.summary.starts_with("没有设置，用默认"), "{}", effect.summary);

    // 默认值要两边都懂按需拨号；存下的设置只要懂 ice3proxy。
    let blocked = lab.controller.pair(network.id, old.id, relay.id).unwrap().lane_effect.unwrap().blocked;
    assert_eq!(blocked, vec![LaneBlock::NodeTooOld { node: old.id }]);
    let stored = PairInput { keep_alive: None, lane: Some(LaneMode::UdpFallback), congestion: None };
    let view = lab.controller.set_pair(network.id, old.id, relay.id, stored).unwrap();
    assert!(view.stored && view.lane_effect.unwrap().effective, "存下的 udp_fallback 照旧发给旧节点");

    // 只设常驻的新的一对：ice3proxy 存下的是默认值。
    let keep = PairInput { keep_alive: Some(true), lane: None, congestion: None };
    let view = lab.controller.set_pair(network.id, member.id, relay.id, keep).unwrap();
    assert!(view.stored);
    assert_eq!((view.setting.keep_alive, view.setting.lane), (true, LaneMode::PAIR_DEFAULT));

    assert_eq!(lab.controller.pair(network.id, member.id, member.id).unwrap_err().code(), i3t_model::admin::ErrorCode::InvalidRequest);
    let other = lab.network("work");
    assert_eq!(lab.controller.pair(other.id, member.id, relay.id).unwrap_err().code(), i3t_model::admin::ErrorCode::NotFound);
}

#[test]
fn a_lane_setting_reaches_both_nodes_and_the_pair_view_says_why_it_does_not_work_yet() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let entry = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let exit = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let lane = |mode| PairInput { keep_alive: None, lane: Some(mode), congestion: None };

    let view = lab.controller.set_pair(network.id, exit, entry, lane(LaneMode::Always)).unwrap();
    assert_eq!((view.setting.a, view.setting.b, view.setting.keep_alive, view.setting.lane), (entry, exit, false, LaneMode::Always));
    let effect = view.lane_effect.unwrap();
    assert!(!effect.effective);
    assert_eq!(effect.blocked, vec![LaneBlock::NoRelay], "两边都没有启用 relay，没有 TCP 端口可拨");
    assert_eq!(lab.netmap(entry).peer(exit).unwrap().lane, LaneMode::Always, "两边的 netmap 里是同一个设置");
    assert_eq!(lab.netmap(exit).peer(entry).unwrap().lane, LaneMode::Always);

    // entry 启用了 relay：exit 拨它。
    lab.controller.enable_relay(network.id, entry, relay_settings()).unwrap();
    let pairs = lab.controller.pairs(network.id).unwrap();
    let effect = pairs[0].lane_effect.clone().unwrap();
    assert!(effect.effective && effect.blocked.is_empty(), "{effect:?}");
    assert_eq!(effect.dialers, vec![exit]);
    assert!(effect.summary.contains("共用的连接"), "要说明共用的连接和它的路照常在：{}", effect.summary);

    // 只改常驻，ice3proxy 的设置不变。
    let view =
        lab.controller.set_pair(network.id, entry, exit, PairInput { keep_alive: Some(true), lane: None, congestion: None }).unwrap();
    assert_eq!((view.setting.keep_alive, view.setting.lane), (true, LaneMode::Always));
    let view = lab.controller.set_pair(network.id, entry, exit, lane(LaneMode::UdpFallback)).unwrap();
    assert_eq!((view.setting.keep_alive, view.setting.lane), (true, LaneMode::UdpFallback));
    assert_eq!(lab.netmap(exit).peer(entry).unwrap().lane, LaneMode::UdpFallback);

    // 在线的旧版本节点不懂 ice3proxy。
    let mut old = lab.join(network.id, &[]);
    lab.settle(&mut [&mut old]);
    let effect = lab.controller.set_pair(network.id, old.id, entry, lane(LaneMode::Always)).unwrap().lane_effect.unwrap();
    assert_eq!(effect.blocked, vec![LaneBlock::NodeTooOld { node: old.id }]);

    // 访问规则不再连通它们。
    drop_default_rule(&lab, network.id);
    let pairs = lab.controller.pairs(network.id).unwrap();
    let pair = pairs.iter().find(|pair| (pair.setting.a, pair.setting.b) == (entry, exit)).unwrap();
    assert_eq!(pair.lane_effect.as_ref().unwrap().blocked, vec![LaneBlock::NotConnected]);

    // 去掉设置回到默认：这两个节点说的是最初那一版节点协议，不懂按需拨号，默认的 udp_fallback 不发给它们，是 off；
    // 节点删掉时它的设置一起没了。
    add_rule(&lab, network.id, "all", "all", Direction::Both);
    lab.controller.delete_pair(network.id, entry, exit).unwrap();
    assert_eq!(lab.netmap(exit).peer(entry).unwrap().lane, LaneMode::Off);
    lab.controller.set_pair(network.id, entry, exit, lane(LaneMode::Always)).unwrap();
    lab.controller.delete_node(network.id, exit).unwrap();
    let left: Vec<_> = lab.controller.pairs(network.id).unwrap().into_iter().map(|pair| (pair.setting.a, pair.setting.b)).collect();
    assert_eq!(left, vec![(entry.min(old.id), entry.max(old.id))]);
}

#[test]
fn a_chosen_profile_reaches_only_that_node_and_its_report_shows_in_the_detail() {
    use i3t_model::control::PerformanceReport;
    use i3t_model::netmap::Profile;

    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut a = lab.join(network.id, &[]);
    let mut b = lab.join(network.id, &[]);
    lab.settle(&mut [&mut a, &mut b]);
    assert_eq!(a.netmap().me.profile, None, "没有选档位时节点用自己的");

    let low = UpdateNode { profile: Some(Some(Profile::LowFootprint)), ..UpdateNode::default() };
    let node = lab.controller.update_node(network.id, a.id, &low).unwrap();
    assert_eq!(node.profile, Some(Profile::LowFootprint));
    let version_of_b = b.netmap().version;
    lab.settle(&mut [&mut a, &mut b]);
    assert_eq!(a.netmap().me.profile, Some(Profile::LowFootprint));
    assert_eq!(b.netmap().version, version_of_b, "别的节点的 netmap 不因为它的档位变");

    let running =
        PerformanceReport { profile: Profile::LowFootprint, threads: 2, restart_pending: true, overrides: vec!["threads".into()] };
    a.send(&lab.controller, ToController::Report(Box::new(Report { performance: Some(Box::new(running.clone())), ..Report::default() })));
    assert_eq!(lab.controller.node(network.id, a.id).unwrap().performance, Some(running));
    assert_eq!(lab.controller.node(network.id, b.id).unwrap().performance, None, "旧版本或者还没报过的节点没有");

    let cleared = lab.controller.update_node(network.id, a.id, &UpdateNode { profile: Some(None), ..UpdateNode::default() }).unwrap();
    assert_eq!(cleared.profile, None);
    lab.settle(&mut [&mut a]);
    assert_eq!(a.netmap().me.profile, None);
}

#[test]
fn the_reported_intercept_state_shows_in_the_node_detail_and_nothing_is_sent_back() {
    use i3t_model::control::{InterceptCause, InterceptReport, InterceptState, NameService};

    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut a = lab.join(network.id, &[]);
    let mut b = lab.join(network.id, &[]);
    lab.settle(&mut [&mut a, &mut b]);
    let version = a.netmap().version;

    let on = InterceptReport {
        state: InterceptState::On { udp: true, source_address: true, names: NameService::NoResolver },
        active: 3,
        up_bytes: 100,
        down_bytes: 200,
    };
    a.send(&lab.controller, ToController::Report(Box::new(Report { intercept: Some(Box::new(on.clone())), ..Report::default() })));
    assert_eq!(lab.controller.node(network.id, a.id).unwrap().intercept, Some(on));
    assert_eq!(lab.controller.node(network.id, b.id).unwrap().intercept, None, "旧版本或者还没报过的节点没有");

    let failed = InterceptReport {
        state: InterceptState::Failed { cause: InterceptCause::ProgramMissing { program: "nft".into() } },
        active: 0,
        up_bytes: 0,
        down_bytes: 0,
    };
    a.send(&lab.controller, ToController::Report(Box::new(Report { intercept: Some(Box::new(failed.clone())), ..Report::default() })));
    assert_eq!(lab.controller.node(network.id, a.id).unwrap().intercept, Some(failed));
    lab.settle(&mut [&mut a]);
    assert_eq!(a.netmap().version, version, "透明访问是本机的事，controller 不因此改 netmap");
}

#[test]
fn a_report_reaches_the_peers_almost_at_once_but_a_burst_of_reports_is_published_at_most_once_a_window() {
    use std::time::{Duration, Instant};

    let window = Duration::from_secs(1);
    let mut lab = Lab::with_timing(super::super::Timing { ack_coalesce: Duration::ZERO, report_coalesce: window, ..Default::default() });
    let network = lab.network("home");
    let mut watcher = lab.join(network.id, &[]);
    let mut first = lab.join(network.id, &[]);
    let mut second = lab.join(network.id, &[]);
    lab.settle(&mut [&mut watcher, &mut first, &mut second]);
    let endpoint = |port: u16| Report { endpoints: vec![std::net::SocketAddr::from(([203, 0, 113, 7], port))], ..Report::default() };
    let due = |lab: &Lab| lab.controller.lock().dirty.get(&watcher.id).map(|deferred| deferred.due);

    let reported = Instant::now();
    lab.controller.handle(first.session, REMOTE, ToController::Report(Box::new(endpoint(1000)))).unwrap();
    let lead = due(&lab).expect("对端的 netmap 要重算");
    assert!(lead < reported + window / 2, "离上一次够久了：不等满一个窗口");

    // 紧接着又一个节点报了：还没发的那一次带上它，不另排。
    lab.controller.handle(second.session, REMOTE, ToController::Report(Box::new(endpoint(2000)))).unwrap();
    assert_eq!(due(&lab), Some(lead));

    // 那一次发出去之后马上又有变化：等到上一次之后满一个窗口，不一阵阵地发。
    lab.controller.lock().dirty.clear();
    lab.controller.handle(first.session, REMOTE, ToController::Report(Box::new(endpoint(3000)))).unwrap();
    assert!(due(&lab).is_some_and(|next| next >= lead + window), "同一个节点两次之间至少隔一个窗口");
}

#[test]
fn a_node_whose_relays_answer_only_over_tcp_is_held_without_udp_after_a_while_and_its_peers_are_told() {
    use std::time::Duration;

    use i3t_crypto::NodeKey;
    use i3t_model::control::{LANE_DEFAULT_REVISION, RelayTry, TryOutcome};

    use crate::store::Store;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.db");
    let settle = Duration::from_millis(300);
    let timing =
        super::super::Timing { ack_coalesce: Duration::ZERO, report_coalesce: Duration::ZERO, no_udp_settle: settle, ..Default::default() };
    let open =
        || Controller::new(Store::open(&path).unwrap(), NodeKey::generate().unwrap(), "ctl.example.com:20331".to_owned(), timing).unwrap();
    let mut lab = Lab::around(open());
    let network = lab.network("home");
    let relay_hello = Hello { protocol: LANE_DEFAULT_REVISION, ..super::super::testing::hello(CAN_DO_ALL) };
    let (mut r, mut s) = (lab.join_as(network.id, &relay_hello), lab.join_as(network.id, &relay_hello));
    let mut y = join_speaking(&mut lab, network.id, LANE_DEFAULT_REVISION);
    for relay in [r.id, s.id] {
        lab.controller.enable_relay(network.id, relay, relay_settings()).unwrap();
    }
    let tries = |entries: &[(i3t_proto::NodeId, TryOutcome)]| {
        let relays = entries.iter().map(|&(relay, outcome)| RelayTry { relay, outcome, age_secs: 0, seen_as: None }).collect();
        ToController::Report(Box::new(Report { relays, ..Report::default() }))
    };
    // y 经 UDP 连得上 r（r 有 UDP），经 TCP 才连得上 s；s 连 r 也只经 TCP。
    y.send(&lab.controller, tries(&[(r.id, TryOutcome::Udp), (s.id, TryOutcome::Tcp)]));
    s.send(&lab.controller, tries(&[(r.id, TryOutcome::Tcp)]));
    lab.settle(&mut [&mut r, &mut s, &mut y]);
    let flag = |lab: &Lab, node| lab.controller.node(network.id, node).unwrap().node.no_udp;
    assert!(!flag(&lab, s.id), "证据要成立一阵才算数");
    assert!(!y.netmap().peer(s.id).unwrap().no_udp);

    std::thread::sleep(settle);
    lab.settle(&mut [&mut r, &mut s, &mut y]);
    assert!(flag(&lab, s.id), "只经 TCP 连上一个别人经 UDP 连得上的 relay，又没有一个经 UDP：认定 s 用不了 UDP");
    assert!(!flag(&lab, y.id) && !flag(&lab, r.id), "y 经 UDP 连上了 r；r 只报过它自己，没有证据");
    assert!(y.netmap().peer(s.id).unwrap().no_udp && r.netmap().peer(s.id).unwrap().no_udp, "s 的对端都知道");
    assert!(!s.netmap().peer(y.id).unwrap().no_udp);

    // controller 重启：存储里的证据还成立，马上就算数，对端的 netmap 不用先改掉再改回来。
    let version = y.netmap().version;
    drop(lab);
    let mut lab = Lab::around(open());
    assert!(flag(&lab, s.id), "重启之后不重新等");
    let mut y = lab.connect_as(y.key, &Hello { protocol: LANE_DEFAULT_REVISION, ..super::super::testing::modern_hello("0.1.19") }).unwrap();
    assert_eq!(y.netmap().version, version, "netmap 没变");
    let mut s = lab.connect_as(s.key, &relay_hello).unwrap();

    // s 又经 UDP 连上了 r：马上不算，对端随之知道。
    s.send(&lab.controller, tries(&[(r.id, TryOutcome::Udp)]));
    lab.settle(&mut [&mut s, &mut y]);
    assert!(!flag(&lab, s.id));
    assert!(!y.netmap().peer(s.id).unwrap().no_udp);
}
