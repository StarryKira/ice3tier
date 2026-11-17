//! 模型的规则：网络隔离、编号和地址的分配、能力的声明与启用、继任。

use std::net::Ipv4Addr;

use i3t_model::admin::{
    CreateCredential, CreateEnrollToken, CreateNetwork, Direction, ErrorCode, ForwardInput, ForwardStatus, ListenerInput, PairInput,
    RelaySettings, UpdateNetwork, UpdateNode,
};
use i3t_model::control::{Declared, EnrollToken, ToController};
use i3t_model::netmap::{self, DefaultAction, ExitRole, ForwardProtocol, LaneMode, ListenerProtocol};
use i3t_proto::NodeId;

use super::super::COOLDOWN_SECS;
use super::super::testing::{CAN_DO_ALL, Lab, REMOTE, hello};
use super::{add_rule, drop_default_rule, rename};
use crate::error::{Error, Rejection};

fn ip(text: &str) -> Ipv4Addr {
    text.parse().unwrap()
}

fn relay_at(address: &str) -> RelaySettings {
    RelaySettings { udp: vec![address.parse().unwrap()], ..RelaySettings::default() }
}

fn listener(node: NodeId, name: &str, bind: &str, require_credential: bool) -> ListenerInput {
    ListenerInput {
        node,
        config: netmap::Listener {
            name: name.to_owned(),
            bind: bind.parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            allow_from: Vec::new(),
            require_credential,
            default_action: None,
            fixed_exit: None,
            class: None,
            optimistic: None,
        },
    }
}

fn forward(listen_node: NodeId, target_node: NodeId) -> ForwardInput {
    ForwardInput {
        name: "web".to_owned(),
        protocol: ForwardProtocol::Tcp,
        listen_node,
        bind: "127.0.0.1:8080".parse().unwrap(),
        target_node,
        target_addr: "127.0.0.1:80".parse().unwrap(),
        allow_from: Vec::new(),
        enabled: true,
        class: None,
        proxy_protocol: None,
    }
}

#[test]
fn a_new_network_comes_with_the_builtin_group_and_the_all_to_all_rule() {
    let lab = Lab::new();
    let network = lab.network("home");
    assert_eq!(network.domain, "home.i3t");
    let groups = lab.controller.groups(network.id).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!((groups[0].name.as_str(), groups[0].builtin), ("All", true));
    let rules = lab.controller.rules(network.id).unwrap();
    assert_eq!(rules.len(), 1);
    let all = &groups[0].id;
    assert_eq!((&rules[0].src.id, &rules[0].dst.id, rules[0].direction, rules[0].enabled), (all, all, Direction::Both, true));
    assert_eq!(rules[0].src.name, "All");
}

#[test]
fn address_ranges_of_different_networks_must_not_overlap() {
    let lab = Lab::new();
    let home = lab.network("home");
    assert_eq!(home.cidr, "10.66.0.0/16".parse().unwrap());

    let overlapping = CreateNetwork { name: "lab".to_owned(), cidr: Some("10.66.128.0/17".parse().unwrap()), ..CreateNetwork::default() };
    let error = lab.controller.create_network(&overlapping).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RangeOverlaps);
    assert!(error.to_string().contains("home"), "{error}");

    let other = lab.network("lab");
    assert_eq!(other.cidr, "10.67.0.0/16".parse().unwrap(), "默认地址段要避开已有的网络");
    let grow_into_home = UpdateNetwork { cidr: Some("10.64.0.0/14".parse().unwrap()), ..UpdateNetwork::default() };
    assert_eq!(lab.controller.update_network(other.id, &grow_into_home).unwrap_err().code(), ErrorCode::RangeOverlaps);

    let duplicate = lab.controller.create_network(&CreateNetwork { name: "home".to_owned(), ..CreateNetwork::default() });
    assert_eq!(duplicate.unwrap_err().code(), ErrorCode::NameTaken);
}

#[test]
fn a_node_joins_the_network_its_token_belongs_to() {
    let mut lab = Lab::new();
    let (home, office) = (lab.network("home"), lab.network("office"));
    let in_home = lab.join_offline(home.id, &[], Declared::default());
    let in_office = lab.join_offline(office.id, &[], Declared::default());

    let home_nodes = lab.controller.nodes(home.id).unwrap();
    assert_eq!(home_nodes.iter().map(|node| node.id).collect::<Vec<_>>(), vec![in_home]);
    assert_eq!(home_nodes[0].network, home.id);
    assert!(home.cidr.contains(&home_nodes[0].virtual_ip));
    assert_eq!(home_nodes[0].fqdn, format!("{}.home.i3t", home_nodes[0].name));

    let office_nodes = lab.controller.nodes(office.id).unwrap();
    assert_eq!(office_nodes.iter().map(|node| node.id).collect::<Vec<_>>(), vec![in_office]);
    assert!(office.cidr.contains(&office_nodes[0].virtual_ip));
}

#[test]
fn things_in_another_network_are_not_found_through_this_one() {
    let mut lab = Lab::new();
    let (home, office) = (lab.network("home"), lab.network("office"));
    let ours = lab.join_offline(home.id, &[], CAN_DO_ALL);
    let theirs = lab.join_offline(office.id, &[], CAN_DO_ALL);
    let controller = &lab.controller;
    let not_found = |error: Error| assert_eq!(error.code(), ErrorCode::NotFound, "{error}");

    not_found(controller.node(home.id, theirs).unwrap_err());
    not_found(controller.delete_node(home.id, theirs).unwrap_err());
    not_found(controller.update_node(home.id, theirs, &UpdateNode::default()).unwrap_err());
    not_found(controller.enable_exit(home.id, theirs, ExitRole::default()).unwrap_err());
    not_found(controller.routes(home.id, theirs).unwrap_err());
    not_found(controller.node_access(home.id, theirs).unwrap_err());
    not_found(controller.access_check(home.id, ours, theirs).unwrap_err());
    not_found(controller.succeed(home.id, ours, theirs).unwrap_err());

    let their_rule = controller.rules(office.id).unwrap()[0].id;
    not_found(controller.delete_rule(home.id, their_rule).unwrap_err());
    let their_credential =
        controller.create_credential(office.id, &CreateCredential { name: "c".to_owned(), max_concurrent: None, max_bytes: None }).unwrap();
    not_found(controller.delete_credential(home.id, their_credential.credential.id).unwrap_err());
    let their_token = controller.create_token(office.id, &CreateEnrollToken::default()).unwrap();
    not_found(controller.delete_token(home.id, their_token.info.id).unwrap_err());
    let their_listener = controller.create_listener(office.id, listener(theirs, "l", "127.0.0.1:1080", false)).unwrap();
    not_found(controller.delete_listener(home.id, their_listener.id).unwrap_err());
    let their_forward = controller.create_forward(office.id, forward(theirs, theirs)).unwrap();
    not_found(controller.delete_forward(home.id, their_forward.id).unwrap_err());

    // 这个网络里的配置也引用不了别的网络的节点。
    not_found(controller.create_listener(home.id, listener(theirs, "l", "127.0.0.1:1080", false)).unwrap_err());
    let mut exit_elsewhere = listener(ours, "l", "127.0.0.1:1080", false);
    exit_elsewhere.config.fixed_exit = Some(theirs);
    not_found(controller.create_listener(home.id, exit_elsewhere).unwrap_err());
    let mut default_elsewhere = listener(ours, "l", "127.0.0.1:1080", false);
    default_elsewhere.config.default_action = Some(DefaultAction::Exit { node: theirs });
    not_found(controller.create_listener(home.id, default_elsewhere).unwrap_err());
    not_found(controller.create_forward(home.id, forward(ours, theirs)).unwrap_err());
    not_found(controller.create_forward(home.id, forward(theirs, ours)).unwrap_err());
    not_found(controller.set_pair(home.id, ours, theirs, PairInput { keep_alive: Some(true), lane: None }).unwrap_err());
    not_found(
        controller.create_token(home.id, &CreateEnrollToken { successor_of: Some(theirs), ..CreateEnrollToken::default() }).unwrap_err(),
    );
    let pin = UpdateNode { pinned_relays: Some(vec![theirs]), ..UpdateNode::default() };
    not_found(controller.update_node(home.id, ours, &pin).unwrap_err());

    assert!(controller.listeners(home.id, None).unwrap().is_empty());
    assert!(controller.forwards(home.id).unwrap().is_empty());
    assert!(controller.credentials(home.id).unwrap().is_empty());
    assert!(controller.tokens(home.id, false).unwrap().is_empty());
}

#[test]
fn node_ids_are_not_reused_after_deletion() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let first = lab.join_offline(network.id, &[], Declared::default());
    let second = lab.join_offline(network.id, &[], Declared::default());
    lab.controller.delete_node(network.id, second).unwrap();
    let third = lab.join_offline(network.id, &[], Declared::default());
    let elsewhere = lab.network("office");
    let fourth = lab.join_offline(elsewhere.id, &[], Declared::default());
    assert!(first < second && second < third && third < fourth, "{first} {second} {third} {fourth}");
}

#[test]
fn virtual_ips_are_the_lowest_free_and_stay_with_the_node() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let address = |lab: &Lab, node| lab.controller.node(network.id, node).unwrap().node.virtual_ip;

    let first = lab.join(network.id, &[]);
    let second = lab.join_offline(network.id, &[], Declared::default());
    let third = lab.join_offline(network.id, &[], Declared::default());
    assert_eq!(address(&lab, first.id), ip("10.66.0.1"), "网络地址 10.66.0.0 不分配");
    assert_eq!(address(&lab, second), ip("10.66.0.2"));
    assert_eq!(address(&lab, third), ip("10.66.0.3"));

    // 重新连上来、改名、改分组都不换地址。
    lab.controller.disconnect(first.session);
    let again = lab.connect(first.key, Declared::default()).unwrap();
    assert_eq!(again.id, first.id);
    rename(&lab, network.id, first.id, "laptop");
    assert_eq!(address(&lab, first.id), ip("10.66.0.1"));
    assert_eq!(again.netmap().me.virtual_ip, ip("10.66.0.1"));

    // 删掉中间一个：它的地址在冷却，新节点拿下一个，而不是回填。
    lab.controller.delete_node(network.id, second).unwrap();
    let fourth = lab.join_offline(network.id, &[], Declared::default());
    assert_eq!(address(&lab, fourth), ip("10.66.0.4"));
}

#[test]
fn the_broadcast_address_is_never_handed_out() {
    let mut lab = Lab::new();
    let tiny = CreateNetwork { name: "tiny".to_owned(), cidr: Some("10.90.0.0/30".parse().unwrap()), ..CreateNetwork::default() };
    let network = lab.controller.create_network(&tiny).unwrap();
    let first = lab.join_offline(network.id, &[], Declared::default());
    let second = lab.join_offline(network.id, &[], Declared::default());
    let addresses: Vec<_> = [first, second].iter().map(|node| lab.controller.node(network.id, *node).unwrap().node.virtual_ip).collect();
    assert_eq!(addresses, vec![ip("10.90.0.1"), ip("10.90.0.2")]);

    let secret = lab.secret(network.id, &CreateEnrollToken::default());
    let key = lab.fresh_key();
    assert_eq!(lab.enroll(key, &secret, Declared::default()).err(), Some(Rejection::RangeExhausted));
    assert_eq!(lab.controller.tokens(network.id, false).unwrap().len(), 1, "没用成的令牌还在");
}

#[test]
fn an_admin_can_set_a_virtual_ip_explicitly_within_the_rules() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let first = lab.join_offline(network.id, &[], Declared::default());
    let second = lab.join_offline(network.id, &[], Declared::default());
    let set = |node, address: &str| {
        lab.controller.update_node(network.id, node, &UpdateNode { virtual_ip: Some(ip(address)), ..UpdateNode::default() })
    };

    assert_eq!(set(first, "10.66.7.7").unwrap().virtual_ip, ip("10.66.7.7"));
    assert_eq!(set(first, "10.66.7.7").unwrap().virtual_ip, ip("10.66.7.7"), "设成它自己现在的地址不算冲突");
    assert_eq!(set(second, "10.66.7.7").unwrap_err().code(), ErrorCode::AddressTaken);
    assert_eq!(set(second, "10.67.0.1").unwrap_err().code(), ErrorCode::AddressOutOfRange);
    assert_eq!(set(second, "10.66.0.0").unwrap_err().code(), ErrorCode::AddressOutOfRange);
    assert_eq!(set(second, "10.66.255.255").unwrap_err().code(), ErrorCode::AddressOutOfRange);

    // 令牌可以预设地址；预设着的地址不会被自动分给别人。
    let preset = CreateEnrollToken { virtual_ip: Some(ip("10.66.0.1")), ..CreateEnrollToken::default() };
    let secret = lab.secret(network.id, &preset);
    assert_eq!(lab.controller.create_token(network.id, &preset).unwrap_err().code(), ErrorCode::Conflict);
    let taken = CreateEnrollToken { virtual_ip: Some(ip("10.66.7.7")), ..CreateEnrollToken::default() };
    assert_eq!(lab.controller.create_token(network.id, &taken).unwrap_err().code(), ErrorCode::AddressTaken);

    let automatic = lab.join_offline(network.id, &[], Declared::default());
    assert_eq!(lab.controller.node(network.id, automatic).unwrap().node.virtual_ip, ip("10.66.0.3"), "10.66.0.1 留给令牌，10.66.0.2 在用");
    let key = lab.fresh_key();
    let with_preset = lab.enroll(key, &secret, Declared::default()).unwrap();
    assert_eq!(with_preset.netmap().me.virtual_ip, ip("10.66.0.1"));
}

#[test]
fn node_names_are_unique_within_a_network_only() {
    let mut lab = Lab::new();
    let (home, office) = (lab.network("home"), lab.network("office"));
    let first = lab.join_offline(home.id, &[], Declared::default());
    let second = lab.join_offline(home.id, &[], Declared::default());
    let elsewhere = lab.join_offline(office.id, &[], Declared::default());
    let name = |network, node, name: &str| {
        lab.controller.update_node(network, node, &UpdateNode { name: Some(name.to_owned()), ..UpdateNode::default() })
    };

    assert_eq!(name(home.id, first, "NAS-01").unwrap().name, "nas-01");
    assert_eq!(name(home.id, second, "nas-01").unwrap_err().code(), ErrorCode::NameTaken);
    assert_eq!(name(home.id, first, "nas-01").unwrap().name, "nas-01", "改成它自己现在的名字不算冲突");
    assert_eq!(name(office.id, elsewhere, "nas-01").unwrap().fqdn, "nas-01.office.i3t");
    assert_eq!(name(home.id, second, "not a name").unwrap_err().code(), ErrorCode::InvalidRequest);
}

#[test]
fn a_deleted_nodes_name_and_address_cool_down_for_ninety_days() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let gone = lab.join_offline(network.id, &[], Declared::default());
    let kept = lab.join_offline(network.id, &[], Declared::default());
    rename(&lab, network.id, gone, "nas-01");
    lab.controller.delete_node(network.id, gone).unwrap();
    let take = |lab: &Lab| {
        let request = UpdateNode { name: Some("nas-01".to_owned()), virtual_ip: Some(ip("10.66.0.1")), ..UpdateNode::default() };
        lab.controller.update_node(network.id, kept, &request)
    };
    let take_name =
        |lab: &Lab| lab.controller.update_node(network.id, kept, &UpdateNode { name: Some("nas-01".to_owned()), ..UpdateNode::default() });

    assert_eq!(take_name(&lab).unwrap_err().code(), ErrorCode::NameCoolingDown);
    let address_only = UpdateNode { virtual_ip: Some(ip("10.66.0.1")), ..UpdateNode::default() };
    assert_eq!(lab.controller.update_node(network.id, kept, &address_only).unwrap_err().code(), ErrorCode::AddressCoolingDown);
    let preset = CreateEnrollToken { virtual_ip: Some(ip("10.66.0.1")), ..CreateEnrollToken::default() };
    assert_eq!(lab.controller.create_token(network.id, &preset).unwrap_err().code(), ErrorCode::AddressCoolingDown);

    lab.controller.advance_clock(COOLDOWN_SECS - 60);
    assert_eq!(take_name(&lab).unwrap_err().code(), ErrorCode::NameCoolingDown);
    lab.controller.advance_clock(61);
    let node = take(&lab).unwrap();
    assert_eq!((node.name.as_str(), node.virtual_ip), ("nas-01", ip("10.66.0.1")));
}

#[test]
fn a_network_range_can_only_grow_once_it_has_nodes_and_a_populated_network_cannot_be_deleted() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let resize = |lab: &Lab, cidr: &str| {
        lab.controller.update_network(network.id, &UpdateNetwork { cidr: Some(cidr.parse().unwrap()), ..UpdateNetwork::default() })
    };

    assert_eq!(resize(&lab, "10.80.0.0/16").unwrap().cidr, "10.80.0.0/16".parse().unwrap(), "空的网络可以换地址段");
    let node = lab.join_offline(network.id, &[], Declared::default());
    assert_eq!(resize(&lab, "10.81.0.0/16").unwrap_err().code(), ErrorCode::RangeNotSuperset);
    assert_eq!(resize(&lab, "10.80.0.0/24").unwrap_err().code(), ErrorCode::RangeNotSuperset);
    assert_eq!(resize(&lab, "10.80.0.0/12").unwrap().cidr, "10.80.0.0/12".parse().unwrap());
    assert_eq!(lab.netmap(node).network.cidr, "10.80.0.0/12".parse().unwrap());

    assert_eq!(lab.controller.delete_network(network.id).unwrap_err().code(), ErrorCode::NetworkNotEmpty);
    lab.controller.delete_node(network.id, node).unwrap();
    lab.controller.delete_network(network.id).unwrap();
    assert!(lab.controller.networks().unwrap().is_empty());
    assert_eq!(lab.network("home").cidr, "10.66.0.0/16".parse().unwrap(), "删掉的网络让出了名字和地址段");
}

#[test]
fn enabling_an_undeclared_capability_is_refused_and_names_the_command_to_run() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let plain = lab.join_offline(network.id, &[], Declared::default());
    let other = lab.join_offline(network.id, &[], Declared::default());
    rename(&lab, network.id, plain, "nas-01");

    let cases = [
        (lab.controller.enable_relay(network.id, plain, relay_at("203.0.113.7:20330")).map(drop), "i3t set can-relay on"),
        (lab.controller.enable_exit(network.id, plain, ExitRole::default()).map(drop), "i3t set can-exit on"),
        (lab.controller.create_forward(network.id, forward(other, plain)).map(drop), "i3t set can-forward on"),
    ];
    for (result, command) in cases {
        let error = result.unwrap_err();
        assert_eq!(error.code(), ErrorCode::NotDeclared);
        assert!(error.to_string().contains(command) && error.to_string().contains("nas-01"), "{error}");
    }
    let node = lab.controller.node(network.id, plain).unwrap().node;
    assert!(node.relay.is_none() && node.exit.is_none());
    assert!(lab.controller.forwards(network.id).unwrap().is_empty());
}

#[test]
fn a_relay_needs_a_publicly_reachable_address() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let node = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let enable = |settings| lab.controller.enable_relay(network.id, node, settings);

    assert_eq!(enable(RelaySettings::default()).unwrap_err().code(), ErrorCode::InvalidRequest);
    assert_eq!(enable(relay_at("0.0.0.0:20330")).unwrap_err().code(), ErrorCode::InvalidRequest);
    assert_eq!(enable(relay_at("203.0.113.7:0")).unwrap_err().code(), ErrorCode::InvalidRequest);
    let tcp_only = RelaySettings { tcp: vec!["203.0.113.7:20330".parse().unwrap()], region: "hk".to_owned(), ..RelaySettings::default() };
    assert_eq!(enable(tcp_only.clone()).unwrap().relay, Some(tcp_only));
    assert_eq!(lab.controller.disable_relay(network.id, node).unwrap().relay, None);
}

#[test]
fn withdrawing_a_declaration_turns_the_capability_off() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut node = lab.join(network.id, &[]);
    lab.controller.disconnect(node.session);
    // 第一次连接时什么都没声明；这里带着声明重新连上来。
    node = lab.connect(node.key, CAN_DO_ALL).unwrap();
    let listener_node = lab.join_offline(network.id, &[], Declared::default());
    lab.controller.enable_relay(network.id, node.id, relay_at("203.0.113.7:20330")).unwrap();
    lab.controller.enable_exit(network.id, node.id, ExitRole::default()).unwrap();
    let forward_id = lab.controller.create_forward(network.id, forward(listener_node, node.id)).unwrap().id;
    lab.settle(&mut [&mut node]);
    assert!(node.netmap().me.relay.is_some() && node.netmap().me.exit.is_some());
    assert_eq!(node.netmap().me.forward_targets.len(), 1);
    assert_eq!(lab.netmap(listener_node).relays.len(), 1);

    // 节点在同一条连接上发来新的声明：出口撤回了，别的还在。
    let still_relay = Declared { exit: false, ..CAN_DO_ALL };
    lab.controller.handle(node.session, REMOTE, ToController::Hello(hello(still_relay))).unwrap();
    lab.settle(&mut [&mut node]);
    let view = lab.controller.node(network.id, node.id).unwrap().node;
    assert!(view.exit.is_none() && view.relay.is_some());
    assert!(node.netmap().me.exit.is_none() && node.netmap().me.relay.is_some());
    assert!(lab.netmap(listener_node).peers[0].exit.is_none());

    // 重新连上来时什么都不声明了：relay 关掉，指向它的转发停用。
    lab.controller.disconnect(node.session);
    let mut node = lab.connect(node.key, Declared::default()).unwrap();
    lab.settle(&mut [&mut node]);
    let view = lab.controller.node(network.id, node.id).unwrap().node;
    assert!(view.relay.is_none() && view.exit.is_none());
    assert!(node.netmap().me.relay.is_none() && node.netmap().relay_clients.is_empty() && node.netmap().me.forward_targets.is_empty());
    assert!(lab.netmap(listener_node).relays.is_empty() && lab.netmap(listener_node).me.forwards.is_empty());
    let stored = &lab.controller.forwards(network.id).unwrap()[0];
    assert_eq!((stored.id, stored.enabled, stored.status), (forward_id, false, ForwardStatus::TargetNotDeclared));

    // 再声明回来，能力也不会自己打开：启用是管理员的事。
    lab.controller.disconnect(node.session);
    let node = lab.connect(node.key, CAN_DO_ALL).unwrap();
    assert!(node.netmap().me.relay.is_none() && node.netmap().me.forward_targets.is_empty());
    assert_eq!(lab.controller.forwards(network.id).unwrap()[0].status, ForwardStatus::Disabled);
    let actions: Vec<String> = lab.controller.oplog(50).unwrap().into_iter().map(|entry| entry.action).collect();
    for action in ["node.relay.withdrawn", "node.exit.withdrawn", "node.forward.withdrawn"] {
        assert!(actions.iter().any(|logged| logged == action), "{action} 不在操作记录里：{actions:?}");
    }
}

#[test]
fn a_forward_can_only_be_enabled_towards_a_node_that_declared_it() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let listen = lab.join_offline(network.id, &[], Declared::default());
    let target = lab.join_offline(network.id, &[], Declared { forward: true, ..Declared::default() });
    let undeclared = lab.join_offline(network.id, &[], Declared::default());

    let created = lab.controller.create_forward(network.id, forward(listen, target)).unwrap();
    assert_eq!(created.status, ForwardStatus::ListenerOffline);
    // 监听节点自己不需要声明什么。
    assert_eq!(lab.netmap(listen).me.forwards.len(), 1);
    assert_eq!(lab.netmap(target).me.forward_targets.len(), 1);

    let retarget = ForwardInput { target_node: undeclared, ..forward(listen, target) };
    assert_eq!(lab.controller.replace_forward(network.id, created.id, retarget.clone()).unwrap_err().code(), ErrorCode::NotDeclared);
    // 停用着的转发可以指向没声明的节点，但它不会被下发，也启用不了。
    let parked = lab.controller.replace_forward(network.id, created.id, ForwardInput { enabled: false, ..retarget }).unwrap();
    assert_eq!(parked.status, ForwardStatus::TargetNotDeclared);
    assert!(lab.netmap(listen).me.forwards.is_empty() && lab.netmap(undeclared).me.forward_targets.is_empty());
    assert!(lab.netmap(target).me.forward_targets.is_empty());

    let same_bind = lab.controller.create_forward(network.id, forward(listen, target));
    assert_eq!(same_bind.unwrap_err().code(), ErrorCode::Conflict, "同一个监听地址上不能有两条同协议的转发");
}

#[test]
fn a_successor_enrolled_by_token_inherits_identity_and_configuration() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut old = lab.join_with(network.id, &["servers"], CAN_DO_ALL);
    let other = lab.join_offline(network.id, &[], CAN_DO_ALL);
    rename(&lab, network.id, old.id, "nas-01");
    lab.controller.enable_relay(network.id, old.id, relay_at("203.0.113.7:20330")).unwrap();
    lab.controller.enable_exit(network.id, old.id, ExitRole::default()).unwrap();
    let own_listener = lab.controller.create_listener(network.id, listener(old.id, "local", "127.0.0.1:1080", false)).unwrap();
    let mut via_exit = listener(other, "via", "127.0.0.1:1081", false);
    via_exit.config.fixed_exit = Some(old.id);
    let referring_listener = lab.controller.create_listener(network.id, via_exit).unwrap();
    let to_old = lab.controller.create_forward(network.id, forward(other, old.id)).unwrap();
    let from_old = lab.controller.create_forward(network.id, forward(old.id, other)).unwrap();
    lab.controller.set_pair(network.id, old.id, other, PairInput { keep_alive: Some(true), lane: None }).unwrap();
    let pin = UpdateNode { pinned_relays: Some(vec![old.id]), ..UpdateNode::default() };
    lab.controller.update_node(network.id, other, &pin).unwrap();
    lab.settle(&mut [&mut old]);
    let old_view = lab.controller.node(network.id, old.id).unwrap().node;

    let rebuilt = lab.group(network.id, "rebuilt");
    let request = CreateEnrollToken { successor_of: Some(old.id), groups: vec![rebuilt], ..CreateEnrollToken::default() };
    let secret = lab.secret(network.id, &request);
    let new_key = lab.fresh_key();
    assert_eq!(lab.enroll(new_key, &secret, Declared::default()).err(), Some(Rejection::PredecessorOnline));
    lab.controller.disconnect(old.session);

    // 新机器只声明了可以作为端口转发的目标。
    let mut new = lab.enroll(new_key, &secret, Declared { forward: true, ..Declared::default() }).unwrap();
    lab.settle(&mut [&mut new]);
    assert!(new.id > old.id, "继任者是一个新节点，编号不沿用");
    let view = lab.controller.node(network.id, new.id).unwrap().node;
    assert_eq!((view.name.as_str(), view.virtual_ip), ("nas-01", old_view.virtual_ip));
    assert_eq!(group_names(&view.groups), ["rebuilt", "servers"], "令牌预设的分组和原节点所在的分组合在一起");
    assert!(view.relay.is_none() && view.exit.is_none(), "relay 和出口要重新声明、重新启用");
    assert_eq!(lab.controller.node(network.id, old.id).unwrap_err().code(), ErrorCode::NotFound);
    assert_eq!(lab.connect(old.key, CAN_DO_ALL).err(), Some(Rejection::UnknownKey), "原来的公钥被吊销了");

    let listeners = lab.controller.listeners(network.id, None).unwrap();
    assert_eq!(listeners.iter().find(|l| l.id == own_listener.id).unwrap().node, new.id);
    assert_eq!(listeners.iter().find(|l| l.id == referring_listener.id).unwrap().config.fixed_exit, Some(new.id));
    let forwards = lab.controller.forwards(network.id).unwrap();
    let inherited_target = forwards.iter().find(|f| f.id == to_old.id).unwrap();
    assert_eq!((inherited_target.target_node, inherited_target.enabled), (new.id, true), "新机器声明了，转发照常");
    assert_eq!(forwards.iter().find(|f| f.id == from_old.id).unwrap().listen_node, new.id);
    let pairs = lab.controller.pairs(network.id).unwrap();
    let settings: Vec<_> = pairs.into_iter().map(|pair| pair.setting).collect();
    assert_eq!(settings, vec![i3t_model::admin::PairSetting { a: other, b: new.id, keep_alive: true, lane: LaneMode::Off }]);
    assert!(lab.controller.node(network.id, other).unwrap().node.pinned_relays.is_empty(), "原来的 relay 没了，固定它的节点解除固定");

    assert_eq!(new.netmap().me.forward_targets.len(), 1);
    assert_eq!(new.netmap().me.listeners.len(), 1);
    assert!(new.netmap().peers[0].keep_alive);
    let seen_by_other = lab.netmap(other);
    assert_eq!(seen_by_other.peers.len(), 1);
    assert_eq!(
        (seen_by_other.peers[0].id, seen_by_other.peers[0].key, seen_by_other.peers[0].virtual_ip),
        (new.id, new_key, old_view.virtual_ip)
    );
    assert!(seen_by_other.relays.is_empty());
    assert!(lab.controller.tokens(network.id, false).unwrap().is_empty());
}

#[test]
fn an_existing_node_can_be_made_the_successor_of_an_offline_one() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut old = lab.join_with(network.id, &["servers"], CAN_DO_ALL);
    let other = lab.join_offline(network.id, &[], Declared::default());
    rename(&lab, network.id, old.id, "nas-01");
    lab.controller.create_forward(network.id, forward(other, old.id)).unwrap();
    lab.settle(&mut [&mut old]);
    let old_address = lab.controller.node(network.id, old.id).unwrap().node.virtual_ip;

    // 重装之后的机器先作为一个普通的新节点加入，没有声明任何能力。
    let replacement = lab.join_offline(network.id, &["new"], Declared::default());
    assert_eq!(lab.controller.succeed(network.id, replacement, old.id).unwrap_err().code(), ErrorCode::PredecessorOnline);
    assert_eq!(lab.controller.succeed(network.id, replacement, replacement).unwrap_err().code(), ErrorCode::InvalidRequest);
    lab.controller.disconnect(old.session);

    let node = lab.controller.succeed(network.id, replacement, old.id).unwrap();
    assert_eq!((node.id, node.name.as_str(), node.virtual_ip), (replacement, "nas-01", old_address));
    assert_eq!(group_names(&node.groups), ["new", "servers"]);
    assert_eq!(lab.controller.nodes(network.id).unwrap().len(), 2);
    let inherited = &lab.controller.forwards(network.id).unwrap()[0];
    assert_eq!((inherited.target_node, inherited.enabled, inherited.status), (replacement, false, ForwardStatus::TargetNotDeclared));
    assert_eq!(lab.netmap(other).peers[0].virtual_ip, old_address);
    assert_eq!(lab.netmap(other).peers[0].id, replacement);
}

#[test]
fn tokens_are_one_time_expire_and_can_be_withdrawn() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let secret = lab.secret(network.id, &CreateEnrollToken::default());
    let first_key = lab.fresh_key();
    let first = lab.enroll(first_key, &secret, Declared::default()).unwrap();
    let second_key = lab.fresh_key();
    assert_eq!(lab.enroll(second_key, &secret, Declared::default()).err(), Some(Rejection::BadToken), "令牌只能用一次");
    assert_eq!(lab.enroll(second_key, "no-such-secret", Declared::default()).err(), Some(Rejection::BadToken));

    // 回答没送到的节点拿同一个令牌、同一把公钥重试：认出它来，不当成第二次使用。
    let retried = lab.enroll(first_key, &secret, Declared::default()).unwrap();
    assert_eq!(retried.id, first.id);
    let other_secret = lab.secret(network.id, &CreateEnrollToken::default());
    assert_eq!(lab.enroll(first_key, &other_secret, Declared::default()).err(), Some(Rejection::KeyAlreadyEnrolled));

    let expired = lab.secret(network.id, &CreateEnrollToken { expires_in_secs: Some(0), ..CreateEnrollToken::default() });
    assert_eq!(lab.enroll(second_key, &expired, Declared::default()).err(), Some(Rejection::BadToken));
    assert_eq!(lab.controller.tokens(network.id, false).unwrap().len(), 1, "过期的令牌不再列出，other_secret 还在");

    let withdrawn = lab.controller.create_token(network.id, &CreateEnrollToken::default()).unwrap();
    lab.controller.delete_token(network.id, withdrawn.info.id).unwrap();
    let withdrawn_secret = withdrawn.token.parse::<EnrollToken>().unwrap();
    assert_eq!(withdrawn_secret.controller, "ctl.example.com:20331");
    assert_eq!(withdrawn_secret.key, lab.controller.public_key());
    assert_eq!(lab.enroll(second_key, &withdrawn_secret.secret, Declared::default()).err(), Some(Rejection::BadToken));
    assert_eq!(lab.controller.nodes(network.id).unwrap().len(), 1);
}

#[test]
fn a_token_presets_groups_and_groups_decide_access() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    add_rule(&lab, network.id, "laptops", "servers", Direction::OneWay);
    let laptop = lab.join_offline(network.id, &["Laptops", "all"], Declared::default());
    let server = lab.join_offline(network.id, &["servers"], Declared::default());
    assert_eq!(group_names(&lab.controller.node(network.id, laptop).unwrap().node.groups), ["laptops"], "All 不列出");
    let check = lab.controller.access_check(network.id, laptop, server).unwrap();
    assert!(check.allowed && check.rules.len() == 1);
    assert!(!lab.controller.access_check(network.id, server, laptop).unwrap().allowed);

    let regroup = UpdateNode { groups: Some(vec![lab.group(network.id, "servers")]), ..UpdateNode::default() };
    lab.controller.update_node(network.id, laptop, &regroup).unwrap();
    assert!(!lab.controller.access_check(network.id, laptop, server).unwrap().allowed);
    assert!(lab.netmap(laptop).peers.is_empty() && lab.netmap(server).peers.is_empty());
}

#[test]
fn a_listener_outside_loopback_must_require_a_credential() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let node = lab.join_offline(network.id, &[], Declared::default());
    let create = |input| lab.controller.create_listener(network.id, input);

    let open_to_lan = create(listener(node, "lan", "192.168.1.5:1080", false)).unwrap_err();
    assert_eq!(open_to_lan.code(), ErrorCode::InvalidRequest);
    assert!(open_to_lan.to_string().contains("require_credential"), "{open_to_lan}");
    assert_eq!(create(listener(node, "any", "0.0.0.0:1080", false)).unwrap_err().code(), ErrorCode::InvalidRequest);
    create(listener(node, "lan", "192.168.1.5:1080", true)).unwrap();
    create(listener(node, "local", "127.0.0.1:1080", false)).unwrap();
    assert_eq!(create(listener(node, "local", "127.0.0.1:1090", false)).unwrap_err().code(), ErrorCode::NameTaken);
    assert_eq!(create(listener(node, "again", "127.0.0.1:1080", false)).unwrap_err().code(), ErrorCode::Conflict);
    assert_eq!(lab.controller.listeners(network.id, Some(node)).unwrap().len(), 2);
}

#[test]
fn changes_are_written_to_the_operation_log_without_secrets() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let created = lab.controller.create_token(network.id, &CreateEnrollToken::default()).unwrap();
    let secret = created.token.parse::<EnrollToken>().unwrap().secret;
    let key = lab.fresh_key();
    let node = lab.enroll(key, &secret, Declared::default()).unwrap();
    let credential = lab
        .controller
        .create_credential(network.id, &CreateCredential { name: "phone".to_owned(), max_concurrent: None, max_bytes: None })
        .unwrap();
    rename(&lab, network.id, node.id, "laptop");
    lab.controller.delete_node(network.id, node.id).unwrap();

    let log = lab.controller.oplog(100).unwrap();
    let actions: Vec<&str> = log.iter().rev().map(|entry| entry.action.as_str()).collect();
    assert_eq!(actions, ["network.create", "token.create", "node.enroll", "credential.create", "node.update", "node.delete"]);
    assert!(log.iter().all(|entry| entry.network == Some(network.id)));
    for entry in &log {
        let params = serde_json::Value::Object(entry.params.clone()).to_string();
        for text in [&entry.detail, &params] {
            assert!(!text.contains(&secret) && !text.contains(&credential.token) && !text.contains(&created.token), "{entry:?}");
        }
    }
}

/// 每种操作的记录都带着按名字给出的参数，名字是当时的名字。
#[test]
fn every_log_entry_carries_the_named_values_its_sentence_needs() {
    use i3t_model::admin::{CreateGroup, CreateRule, GroupMembers, PairInput, UpdateCredential, UpdateGroup, UpdateRule};
    use serde_json::{Value, json};

    let mut lab = Lab::new();
    let network = lab.network("home");
    let token = lab.controller.create_token(network.id, &CreateEnrollToken::default()).unwrap();
    lab.controller.delete_token(network.id, token.info.id).unwrap();
    let nas = lab.join_with(network.id, &["servers"], CAN_DO_ALL).id;
    let laptop = lab.join(network.id, &[]).id;
    rename(&lab, network.id, laptop, "laptop");
    let rule = lab
        .controller
        .create_rule(
            network.id,
            &CreateRule {
                name: "ssh".into(),
                src: lab.group(network.id, "all"),
                dst: lab.group(network.id, "servers"),
                direction: Direction::Both,
                enabled: true,
            },
        )
        .unwrap();
    lab.controller.update_rule(network.id, rule.id, &UpdateRule { enabled: Some(false), ..UpdateRule::default() }).unwrap();
    lab.controller.delete_rule(network.id, rule.id).unwrap();
    let servers = lab.group(network.id, "servers");
    lab.controller.update_group(network.id, servers, &UpdateGroup { name: Some("boxes".into()) }).unwrap();
    let spare = lab.controller.create_group(network.id, &CreateGroup { name: "spare".into() }).unwrap().id;
    lab.controller.change_members(network.id, spare, &GroupMembers { add: vec![nas, laptop], remove: Vec::new() }).unwrap();
    lab.controller.remove_member(network.id, spare, laptop).unwrap();
    lab.controller.delete_group(network.id, spare).unwrap();
    let regroup = UpdateNode { groups: Some(vec![servers]), ..UpdateNode::default() };
    lab.controller.update_node(network.id, laptop, &regroup).unwrap();
    let credential = lab
        .controller
        .create_credential(network.id, &CreateCredential { name: "phone".into(), max_concurrent: Some(4), max_bytes: None })
        .unwrap()
        .credential;
    let limits = UpdateCredential { max_concurrent: Some(None), reset_usage: true, ..UpdateCredential::default() };
    lab.controller.update_credential(network.id, credential.id, &limits).unwrap();
    lab.controller.delete_credential(network.id, credential.id).unwrap();
    lab.controller.delete_credential(network.id, credential.id).unwrap();
    let entry = lab.controller.create_listener(network.id, listener(laptop, "local", "127.0.0.1:1080", false)).unwrap();
    lab.controller.replace_listener(network.id, entry.id, listener(laptop, "local", "127.0.0.1:1081", false)).unwrap();
    lab.controller.delete_listener(network.id, entry.id).unwrap();
    let web = lab.controller.create_forward(network.id, forward(laptop, nas)).unwrap();
    lab.controller.replace_forward(network.id, web.id, ForwardInput { enabled: false, ..forward(laptop, nas) }).unwrap();
    lab.controller.delete_forward(network.id, web.id).unwrap();
    lab.controller.set_pair(network.id, nas, laptop, PairInput { keep_alive: Some(true), lane: Some(LaneMode::Always) }).unwrap();
    lab.controller.delete_pair(network.id, laptop, nas).unwrap();
    lab.controller.enable_relay(network.id, nas, relay_at("203.0.113.7:20330")).unwrap();
    lab.controller.disable_relay(network.id, nas).unwrap();
    lab.controller.enable_exit(network.id, nas, ExitRole::default()).unwrap();
    lab.controller.disable_exit(network.id, nas).unwrap();
    lab.controller.update_network(network.id, &UpdateNetwork { max_relay_hops: Some(3), ..UpdateNetwork::default() }).unwrap();
    let other = lab.network("lab");
    lab.controller.delete_network(other.id).unwrap();
    lab.controller.delete_node(network.id, laptop).unwrap();

    let log = lab.controller.oplog(200).unwrap();
    // 最早的那一条：同一种操作有几条时（两个节点加入、两次建网）说的是先做的那个。
    let params = |action: &str| {
        let entry = log.iter().rev().find(|entry| entry.action == action).unwrap_or_else(|| panic!("没有 {action} 的记录"));
        Value::Object(entry.params.clone())
    };
    let (nas_id, laptop_id) = (json!(nas), json!(laptop));
    assert_eq!(params("network.create")["network_name"], "home");
    assert_eq!(
        (params("network.update")["network_name"].clone(), params("network.update")["max_relay_hops"].clone()),
        (json!("home"), json!(3))
    );
    assert_eq!(params("network.delete"), json!({ "network": other.id, "network_name": "lab" }));
    assert_eq!(params("token.create")["token"], json!(token.info.id));
    assert_eq!(params("token.delete")["prefix"], json!(token.info.prefix));
    assert_eq!(params("node.enroll")["node"], nas_id);
    assert!(params("node.enroll")["predecessor"].is_null());
    let update = params("node.update");
    assert_eq!((&update["node"], &update["changes"]["name"]["to"]), (&laptop_id, &json!("laptop")));
    assert_eq!(params("node.delete")["node_name"], "laptop");
    let rule_created = params("rule.create");
    assert_eq!(
        (&rule_created["rule"], &rule_created["rule_name"], &rule_created["dst"], &rule_created["dst_name"], &rule_created["src_name"]),
        (&json!(rule.id), &json!("ssh"), &json!(servers), &json!("servers"), &json!("All"))
    );
    assert_eq!(params("rule.update")["enabled"], false);
    assert_eq!(params("rule.delete")["rule"], json!(rule.id));
    assert_eq!(params("group.rename"), json!({ "group": servers, "from": "servers", "to": "boxes" }));
    assert_eq!(params("group.create"), json!({ "group": servers, "group_name": "servers" }), "最早建的是 servers");
    assert_eq!(
        params("group.members"),
        json!({ "group": spare, "group_name": "spare", "added": [{ "node": nas, "node_name": params("node.enroll")["node_name"] }, { "node": laptop, "node_name": "laptop" }], "removed": [] })
    );
    let removed = log.iter().find(|entry| entry.action == "group.members").unwrap();
    assert_eq!(Value::Object(removed.params.clone())["removed"], json!([{ "node": laptop, "node_name": "laptop" }]));
    assert_eq!(params("group.delete"), json!({ "group": spare, "group_name": "spare", "nodes": 1, "rules": 0, "tokens": 0 }));
    let regrouped = &log.iter().find(|entry| entry.action == "node.update").unwrap().params;
    assert_eq!(regrouped["changes"], json!({ "groups": [{ "group": servers, "group_name": "boxes" }] }));
    assert_eq!(params("credential.create")["credential_name"], "phone");
    let limits = params("credential.update");
    assert_eq!((&limits["max_concurrent"], &limits["reset_usage"]), (&Value::Null, &json!(true)));
    assert_eq!(params("credential.revoke")["credential"], json!(credential.id));
    assert_eq!(params("credential.delete")["credential_name"], "phone");
    let listened = params("listener.create");
    assert_eq!(
        (&listened["node_name"], &listened["bind"], &listened["listener_name"]),
        (&json!("laptop"), &json!("127.0.0.1:1080"), &json!("local"))
    );
    assert_eq!(params("listener.update")["bind"], "127.0.0.1:1081");
    assert_eq!(params("listener.delete")["listener"], json!(entry.id));
    let forwarded = params("forward.create");
    assert_eq!(
        (&forwarded["listen_node_name"], &forwarded["target_node"], &forwarded["enabled"]),
        (&json!("laptop"), &nas_id, &json!(true))
    );
    assert_eq!(params("forward.update")["enabled"], false);
    assert_eq!(params("forward.delete")["forward"], json!(web.id));
    let pair = params("pair.set");
    let (a, b) = (nas.min(laptop), nas.max(laptop));
    assert_eq!((&pair["a"], &pair["b"], &pair["lane"], &pair["keep_alive"]), (&json!(a), &json!(b), &json!("always"), &json!(true)));
    assert_eq!(params("pair.delete")["a_name"], if a == laptop { json!("laptop") } else { params("node.enroll")["node_name"].clone() });
    for action in ["node.relay.enable", "node.relay.disable", "node.exit.enable", "node.exit.disable"] {
        assert_eq!(params(action)["node"], nas_id, "{action}");
    }
}

fn group_names(groups: &[i3t_model::admin::GroupRef]) -> Vec<&str> {
    groups.iter().map(|group| group.name.as_str()).collect()
}
