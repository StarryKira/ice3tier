//! 每个节点拿到的 netmap：最小权限，内容变了才动版本号，只发给受影响的节点。

use i3t_model::admin::{
    CreateCredential, Direction, ForwardInput, LaneBlock, LaneOpening, ListenerInput, PairInput, RelaySettings, UpdateNode,
};
use i3t_model::control::{Declared, Hello, Report, ToController};
use i3t_model::netmap::{self, Attachment, ExitRole, ForwardProtocol, LaneMode, ListenerProtocol, Reach, TokenHash};

use super::super::Controller;
use super::super::testing::{CAN_DO_ALL, Lab, REMOTE};
use super::{add_rule, drop_default_rule, peer_ids, relations, rename};
use crate::secret;

fn relay_at(address: &str) -> RelaySettings {
    RelaySettings {
        udp: vec![address.parse().unwrap()],
        tcp: vec![address.parse().unwrap()],
        region: "hk".to_owned(),
        bandwidth_mbps: Some(100),
    }
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
        assert!(netmap.relays.is_empty() && netmap.relay_clients.is_empty() && netmap.credentials.is_empty());
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
    lab.controller.enable_relay(office.id, their_relay, relay_at("203.0.113.9:20330")).unwrap();
    lab.controller.create_credential(office.id, &CreateCredential { name: "c".to_owned(), max_concurrent: None, max_bytes: None }).unwrap();
    add_rule(&lab, home.id, "shared", "shared", Direction::Both);

    let netmap = lab.netmap(ours);
    assert!(netmap.peers.is_empty() && netmap.relays.is_empty() && netmap.relay_clients.is_empty() && netmap.credentials.is_empty());
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
    lab.controller.enable_relay(network.id, relay, relay_at("203.0.113.7:20330")).unwrap();
    let relay_view = lab.controller.node(network.id, relay).unwrap().node;

    for node in [a, b] {
        let netmap = lab.netmap(node);
        assert!(netmap.peers.is_empty(), "没有任何规则：relay 不是对端");
        assert_eq!(netmap.relays.len(), 1);
        let listed = &netmap.relays[0];
        assert_eq!(
            (listed.id, listed.key, listed.name.as_str(), listed.region.as_str()),
            (relay, relay_view.key, relay_view.name.as_str(), "hk")
        );
        assert_eq!(listed.udp, vec!["203.0.113.7:20330".parse().unwrap()]);
        assert_eq!(listed.tcp, vec!["203.0.113.7:20330".parse().unwrap()]);
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
fn credentials_go_only_to_nodes_with_a_listener_that_requires_them() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let gateway = lab.join_offline(network.id, &[], Declared::default());
    let plain = lab.join_offline(network.id, &[], Declared::default());
    let listener = |node, bind: &str, require_credential| ListenerInput {
        node,
        config: netmap::Listener {
            name: "l".to_owned(),
            bind: bind.parse().unwrap(),
            protocol: ListenerProtocol::Http,
            allow_from: vec!["192.168.1.0/24".parse().unwrap()],
            require_credential,
            default_action: None,
            fixed_exit: Some(plain),
            class: None,
            optimistic: None,
        },
    };
    let created = lab
        .controller
        .create_credential(network.id, &CreateCredential { name: "phone".to_owned(), max_concurrent: Some(4), max_bytes: None })
        .unwrap();
    assert!(lab.netmap(gateway).credentials.is_empty());

    let on_gateway = lab.controller.create_listener(network.id, listener(gateway, "192.168.1.5:8080", true)).unwrap();
    lab.controller.create_listener(network.id, listener(plain, "127.0.0.1:8080", false)).unwrap();
    let netmap = lab.netmap(gateway);
    assert_eq!(netmap.me.listeners, vec![on_gateway.config.clone()]);
    assert_eq!(netmap.credentials.len(), 1);
    let delivered = &netmap.credentials[0];
    assert_eq!((delivered.name.as_str(), delivered.max_concurrent), ("phone", Some(4)));
    assert_eq!(delivered.token, TokenHash::from_bytes(secret::sha256(created.token.as_bytes())), "入口拿到的是令牌文本的 SHA-256");
    assert!(!serde_json::to_string(&netmap).unwrap().contains(&created.token), "令牌本身不下发");
    assert!(lab.netmap(plain).credentials.is_empty() && lab.netmap(plain).me.listeners.len() == 1);

    lab.controller.delete_credential(network.id, created.credential.id).unwrap();
    assert!(lab.netmap(gateway).credentials.is_empty());
    lab.controller.delete_listener(network.id, on_gateway.id).unwrap();
    assert!(lab.netmap(gateway).me.listeners.is_empty());
}

#[test]
fn a_forward_is_delivered_to_both_ends_only_while_the_listener_may_access_the_target() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    let listen = lab.join_offline(network.id, &["laptops"], Declared::default());
    let target = lab.join_offline(network.id, &["servers"], CAN_DO_ALL);
    let bystander = lab.join_offline(network.id, &["laptops"], CAN_DO_ALL);
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
        vec![netmap::ForwardTarget { id: created.id, protocol: ForwardProtocol::Udp, addr: input.target_addr, listener: listen }]
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
    lab.controller.enable_relay(network.id, relay, relay_at("203.0.113.7:20330")).unwrap();
    lab.controller.enable_exit(network.id, exit.id, ExitRole { lan: true, ports: None, ..ExitRole::default() }).unwrap();
    lab.controller.set_pair(network.id, user, exit.id, PairInput { keep_alive: Some(true), lane: None }).unwrap();
    let report = Report {
        endpoints: vec!["192.168.1.5:20330".parse().unwrap(), "203.0.113.50:20330".parse().unwrap(), "192.168.1.5:20330".parse().unwrap()],
        // 挂靠关系里只有真的是 relay 的才下发，并且按延迟排。
        attached: vec![Attachment { relay: user, rtt_ms: 1 }, Attachment { relay, rtt_ms: 30 }],
        ..Report::default()
    };
    lab.controller.handle(exit.session, REMOTE, ToController::Report(report)).unwrap();
    lab.settle(&mut [&mut exit]);

    let seen_by_user = lab.netmap(user).peer(exit.id).cloned().unwrap();
    assert_eq!(seen_by_user.exit, Some(netmap::PeerExit { lan: true, class: None }));
    assert!(seen_by_user.keep_alive);
    assert_eq!(seen_by_user.attached, vec![Attachment { relay, rtt_ms: 30 }]);
    let expected: Vec<std::net::SocketAddr> =
        vec!["192.168.1.5:20330".parse().unwrap(), "203.0.113.50:20330".parse().unwrap(), std::net::SocketAddr::new(REMOTE, 20330)];
    assert_eq!(seen_by_user.endpoints, expected);
    let me = &exit.netmap().me;
    assert_eq!(me.exit, Some(ExitRole { lan: true, ports: None, ..ExitRole::default() }));

    // 出口只能被 users 访问；它自己看 user 时没有出口的提示，user 也不是出口。
    let seen_by_exit = exit.netmap().peer(user).cloned().unwrap();
    assert!(seen_by_exit.inbound && !seen_by_exit.outbound && seen_by_exit.exit.is_none() && seen_by_exit.keep_alive);

    // 只走 relay 的节点不向对端暴露地址；只拨出的节点带着标记。
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
    lab.controller.enable_relay(network.id, relay.id, relay_at("203.0.113.7:20330")).unwrap();
    let lane = PairInput { keep_alive: None, lane: Some(LaneMode::UdpFallback) };
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
        ToController::Report(Report {
            lanes: Some(Box::new(LaneReport { peers: vec![lanes], ..LaneReport::default() })),
            ..Report::default()
        })
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

    lab.controller.set_pair(network.id, relay.id, member.id, PairInput { keep_alive: None, lane: Some(LaneMode::Always) }).unwrap();
    assert!(lab.controller.pairs(network.id).unwrap()[0].lane_effect.as_ref().unwrap().opening.is_empty(), "只在 udp_fallback 时说");
}

#[test]
fn a_lane_setting_reaches_both_nodes_and_the_pair_view_says_why_it_does_not_work_yet() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let entry = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let exit = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let lane = |mode| PairInput { keep_alive: None, lane: Some(mode) };

    let view = lab.controller.set_pair(network.id, exit, entry, lane(LaneMode::Always)).unwrap();
    assert_eq!((view.setting.a, view.setting.b, view.setting.keep_alive, view.setting.lane), (entry, exit, false, LaneMode::Always));
    let effect = view.lane_effect.unwrap();
    assert!(!effect.effective);
    assert_eq!(effect.blocked, vec![LaneBlock::NoRelay], "两边都没有启用 relay，没有 TCP 端口可拨");
    assert_eq!(lab.netmap(entry).peer(exit).unwrap().lane, LaneMode::Always, "两边的 netmap 里是同一个设置");
    assert_eq!(lab.netmap(exit).peer(entry).unwrap().lane, LaneMode::Always);

    // entry 启用了 relay：exit 拨它。
    lab.controller.enable_relay(network.id, entry, relay_at("203.0.113.7:20330")).unwrap();
    let pairs = lab.controller.pairs(network.id).unwrap();
    let effect = pairs[0].lane_effect.clone().unwrap();
    assert!(effect.effective && effect.blocked.is_empty(), "{effect:?}");
    assert_eq!(effect.dialers, vec![exit]);
    assert!(effect.summary.contains("共用的连接"), "要说明共用的连接和它的路照常在：{}", effect.summary);

    // 只改常驻，独占 TCP 的设置不变。
    let view = lab.controller.set_pair(network.id, entry, exit, PairInput { keep_alive: Some(true), lane: None }).unwrap();
    assert_eq!((view.setting.keep_alive, view.setting.lane), (true, LaneMode::Always));
    let view = lab.controller.set_pair(network.id, entry, exit, lane(LaneMode::UdpFallback)).unwrap();
    assert_eq!((view.setting.keep_alive, view.setting.lane), (true, LaneMode::UdpFallback));
    assert_eq!(lab.netmap(exit).peer(entry).unwrap().lane, LaneMode::UdpFallback);

    // 在线的旧版本节点不懂独占 TCP。
    let mut old = lab.join(network.id, &[]);
    lab.settle(&mut [&mut old]);
    let effect = lab.controller.set_pair(network.id, old.id, entry, lane(LaneMode::Always)).unwrap().lane_effect.unwrap();
    assert_eq!(effect.blocked, vec![LaneBlock::NodeTooOld { node: old.id }]);

    // 访问规则不再连通它们。
    drop_default_rule(&lab, network.id);
    let pairs = lab.controller.pairs(network.id).unwrap();
    let pair = pairs.iter().find(|pair| (pair.setting.a, pair.setting.b) == (entry, exit)).unwrap();
    assert_eq!(pair.lane_effect.as_ref().unwrap().blocked, vec![LaneBlock::NotConnected]);

    // 去掉设置回到 off；节点删掉时它的设置一起没了。
    add_rule(&lab, network.id, "all", "all", Direction::Both);
    lab.controller.delete_pair(network.id, entry, exit).unwrap();
    assert_eq!(lab.netmap(exit).peer(entry).unwrap().lane, LaneMode::Off);
    lab.controller.set_pair(network.id, entry, exit, lane(LaneMode::Always)).unwrap();
    lab.controller.delete_node(network.id, exit).unwrap();
    let left: Vec<_> = lab.controller.pairs(network.id).unwrap().into_iter().map(|pair| (pair.setting.a, pair.setting.b)).collect();
    assert_eq!(left, vec![(entry.min(old.id), entry.max(old.id))]);
}
