//! 分组：名字、内置的 `All`、只能引用已有的分组、成员、删除和它的影响、访问关系图。

use i3t_model::admin::{
    CreateEnrollToken, CreateGroup, CreateRule, Direction, ErrorCode, GroupId, GroupMembers, RefKind, UpdateGroup, UpdateNode, UpdateRule,
};
use i3t_model::control::Declared;

use super::super::testing::Lab;
use super::{add_rule, drop_default_rule, peer_ids};
use crate::error::Error;
use crate::store::LogFilter;

fn create(lab: &Lab, network: i3t_model::admin::NetworkId, name: &str) -> Result<GroupId, Error> {
    lab.controller.create_group(network, &CreateGroup { name: name.to_owned() }).map(|group| group.id)
}

/// 错误的 `code`、`what`、`field`。
fn described(error: &Error) -> (ErrorCode, Option<String>, Option<&str>) {
    (error.code(), error.what(), error.field())
}

#[test]
fn group_names_follow_the_rule_and_are_unique_ignoring_case() {
    let lab = Lab::new();
    let network = lab.network("home").id;
    let servers = create(&lab, network, " Servers ").unwrap();
    assert_eq!(lab.controller.group(network, servers).unwrap().name, "Servers", "保留大小写，去掉首尾空白");
    let chinese = create(&lab, network, "家里").unwrap();

    let taken = create(&lab, network, "SERVERS").unwrap_err();
    assert_eq!(described(&taken), (ErrorCode::NameTaken, Some("group".to_owned()), Some("name")));
    assert!(taken.to_string().contains("SERVERS"), "{taken}");
    assert_eq!(create(&lab, network, "all").unwrap_err().code(), ErrorCode::NameTaken, "内置的 All 占着这个名字");
    for (bad, what) in [("", "empty"), (&*"a".repeat(33), "too_long"), ("a b", "malformed"), ("a.b", "malformed")] {
        assert_eq!(described(&create(&lab, network, bad).unwrap_err()), (ErrorCode::InvalidRequest, Some(what.to_owned()), Some("name")));
    }

    let other = lab.network("lab").id;
    create(&lab, other, "servers").unwrap();
    let names: Vec<String> = lab.controller.groups(network).unwrap().into_iter().map(|group| group.name).collect();
    assert_eq!(names, ["All", "Servers", "家里"], "All 在前，别的网络的分组不在这里");
    assert_eq!(lab.controller.group(other, chinese).unwrap_err().code(), ErrorCode::NotFound, "网络之间隔离");
}

#[test]
fn renaming_keeps_members_and_rules_because_they_hold_the_id() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    drop_default_rule(&lab, network);
    let mut laptop = lab.join(network, &["laptops"]);
    let mut server = lab.join(network, &["servers"]);
    let rule = add_rule(&lab, network, "laptops", "servers", Direction::OneWay);
    lab.settle(&mut [&mut laptop, &mut server]);
    let (version, before) = (laptop.netmap().version, super::relations(laptop.netmap()));
    let laptops = lab.group(network, "laptops");

    let renamed = lab.controller.update_group(network, laptops, &UpdateGroup { name: Some("Phones".to_owned()) }).unwrap();
    assert_eq!((renamed.name.as_str(), renamed.nodes.len(), renamed.rules.len()), ("Phones", 1, 1));
    assert_eq!(lab.controller.rules(network).unwrap()[0].src.name, "Phones");
    assert_eq!(lab.controller.node(network, laptop.id).unwrap().node.groups[0].name, "Phones");
    lab.settle(&mut [&mut laptop, &mut server]);
    assert_eq!((laptop.netmap().version, super::relations(laptop.netmap())), (version, before), "改名不下发任何东西");
    assert!(lab.controller.access_check(network, laptop.id, server.id).unwrap().rules.iter().any(|allowing| allowing.id == rule));

    let case_only = lab.controller.update_group(network, laptops, &UpdateGroup { name: Some("phones".to_owned()) }).unwrap();
    assert_eq!(case_only.name, "phones", "只改大小写也算改名");
    let unchanged = lab.controller.update_group(network, laptops, &UpdateGroup::default()).unwrap();
    assert_eq!(unchanged.name, "phones");
    let servers = lab.group(network, "servers");
    let clash = lab.controller.update_group(network, laptops, &UpdateGroup { name: Some("SERVERS".to_owned()) }).unwrap_err();
    assert_eq!(clash.code(), ErrorCode::NameTaken);
    assert_eq!(lab.controller.group(network, servers).unwrap().name, "servers");

    let renames = lab.controller.oplog_filtered(&LogFilter { limit: 10, action: Some("group.rename".to_owned()), ..LogFilter::default() });
    assert_eq!(renames.unwrap().len(), 2, "和原来一样的名字不记");
}

#[test]
fn the_builtin_group_holds_every_node_and_cannot_be_changed() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let a = lab.join_offline(network, &[], Declared::default());
    let b = lab.join_offline(network, &["servers"], Declared::default());
    let all = lab.group(network, "all");
    let group = lab.controller.group(network, all).unwrap();
    assert!(group.builtin);
    assert_eq!(group.nodes.iter().map(|node| node.id).collect::<Vec<_>>(), [a, b], "每个节点都在 All 里");
    assert_eq!(group.rules.len(), 1, "默认规则用着它");

    let builtin = |error: Error| assert_eq!(described(&error), (ErrorCode::InvalidRequest, Some("builtin".to_owned()), Some("group")));
    builtin(lab.controller.update_group(network, all, &UpdateGroup { name: Some("Everyone".to_owned()) }).unwrap_err());
    builtin(lab.controller.delete_group(network, all).unwrap_err());
    builtin(lab.controller.remove_member(network, all, a).unwrap_err());
    builtin(lab.controller.add_member(network, all, a).unwrap_err());
    builtin(lab.controller.change_members(network, all, &GroupMembers { add: Vec::new(), remove: vec![b] }).unwrap_err());
    assert_eq!(lab.controller.groups(network).unwrap()[0].nodes.len(), 2);

    // 写了 All 的编号会被忽略：它不出现在节点的分组里，也不用存。
    let node = lab.controller.update_node(network, a, &UpdateNode { groups: Some(vec![all]), ..UpdateNode::default() }).unwrap();
    assert!(node.groups.is_empty());
}

#[test]
fn only_existing_groups_can_be_referenced_and_the_missing_one_is_named() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let other = lab.network("lab").id;
    let node = lab.join_offline(network, &[], Declared::default());
    let servers = lab.group(network, "servers");
    let elsewhere = lab.group(other, "servers");
    let missing = GroupId(999);
    let groups_before = lab.controller.groups(network).unwrap().len();

    let error = lab.controller.update_node(network, node, &UpdateNode { groups: Some(vec![servers, missing]), ..UpdateNode::default() });
    let error = error.unwrap_err();
    assert_eq!(described(&error), (ErrorCode::NotFound, Some("group".to_owned()), Some("groups[1]")));
    assert!(error.to_string().contains("#999"), "{error}");
    assert!(lab.controller.node(network, node).unwrap().node.groups.is_empty(), "什么也没改");

    let token = CreateEnrollToken { groups: vec![elsewhere], ..CreateEnrollToken::default() };
    let error = lab.controller.create_token(network, &token).unwrap_err();
    assert_eq!(described(&error), (ErrorCode::NotFound, Some("group".to_owned()), Some("groups[0]")), "别的网络的分组等于不存在");

    let rule = CreateRule { name: String::new(), src: servers, dst: missing, direction: Direction::Both, enabled: true };
    let error = lab.controller.create_rule(network, &rule).unwrap_err();
    assert_eq!(described(&error), (ErrorCode::NotFound, Some("group".to_owned()), Some("dst")));
    let default = lab.controller.rules(network).unwrap()[0].id;
    let error = lab.controller.update_rule(network, default, &UpdateRule { src: Some(missing), ..UpdateRule::default() }).unwrap_err();
    assert_eq!(described(&error), (ErrorCode::NotFound, Some("group".to_owned()), Some("src")));
    assert_eq!(lab.controller.group(network, missing).unwrap_err().field(), None, "路径里的分组没有 field");
    assert_eq!(lab.controller.groups(network).unwrap().len(), groups_before, "引用不会新建分组");
}

#[test]
fn members_are_added_and_removed_one_at_a_time_or_in_a_batch() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    drop_default_rule(&lab, network);
    add_rule(&lab, network, "admins", "all", Direction::OneWay);
    let mut a = lab.join(network, &[]);
    let mut b = lab.join(network, &[]);
    let mut c = lab.join(network, &[]);
    lab.settle(&mut [&mut a, &mut b, &mut c]);
    assert!(peer_ids(a.netmap()).is_empty());
    let admins = lab.group(network, "admins");

    let batch = GroupMembers { add: vec![a.id, b.id], remove: Vec::new() };
    let group = lab.controller.change_members(network, admins, &batch).unwrap();
    assert_eq!(group.nodes.iter().map(|node| node.id).collect::<Vec<_>>(), [a.id, b.id]);
    lab.settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(peer_ids(a.netmap()), [b.id, c.id], "加入之后得到这个分组的规则给的访问");
    assert!(!lab.controller.access_check(network, c.id, a.id).unwrap().allowed, "单向的");

    let group = lab.controller.remove_member(network, admins, b.id).unwrap();
    assert_eq!(group.nodes.len(), 1);
    lab.controller.remove_member(network, admins, b.id).unwrap();
    lab.controller.add_member(network, admins, a.id).unwrap();
    lab.settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(peer_ids(b.netmap()), [a.id], "移出之后只剩别人给它的访问");

    let unknown = GroupMembers { add: vec![c.id], remove: vec![a.id, i3t_proto::NodeId::new(999).unwrap()] };
    let error = lab.controller.change_members(network, admins, &unknown).unwrap_err();
    assert_eq!(described(&error), (ErrorCode::NotFound, Some("node".to_owned()), Some("remove[1]")));
    let both = GroupMembers { add: vec![c.id], remove: vec![c.id] };
    let error = lab.controller.change_members(network, admins, &both).unwrap_err();
    assert_eq!(described(&error), (ErrorCode::InvalidRequest, Some("duplicate".to_owned()), Some("remove")));
    assert_eq!(lab.controller.group(network, admins).unwrap().nodes.len(), 1, "出错时一个都不改");

    let members = lab.controller.oplog_filtered(&LogFilter { limit: 10, action: Some("group.members".to_owned()), ..LogFilter::default() });
    assert_eq!(members.unwrap().len(), 2, "什么也没变的增减不记");
}

#[test]
fn deleting_a_group_removes_it_everywhere_with_its_rules_in_one_go() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    drop_default_rule(&lab, network);
    let mut laptop = lab.join(network, &["laptops", "home"]);
    let mut server = lab.join(network, &["servers"]);
    let ssh = add_rule(&lab, network, "laptops", "servers", Direction::OneWay);
    let kept = add_rule(&lab, network, "home", "home", Direction::Both);
    let laptops = lab.group(network, "laptops");
    let token = lab.controller.create_token(network, &CreateEnrollToken { groups: vec![laptops], ..CreateEnrollToken::default() }).unwrap();
    lab.settle(&mut [&mut laptop, &mut server]);
    assert_eq!(peer_ids(laptop.netmap()), [server.id]);

    let impact = lab.controller.group(network, laptops).unwrap();
    assert_eq!((impact.nodes.len(), impact.rules.iter().map(|rule| rule.id).collect::<Vec<_>>()), (1, vec![ssh]));
    let deleted = lab.controller.delete_group(network, laptops).unwrap();
    assert_eq!(deleted.nodes.iter().map(|node| node.id).collect::<Vec<_>>(), [laptop.id]);
    assert_eq!((deleted.rules.iter().map(|rule| rule.id).collect::<Vec<_>>(), deleted.tokens), (vec![ssh], vec![token.info.id]));
    assert_eq!(lab.controller.rules(network).unwrap().iter().map(|rule| rule.id).collect::<Vec<_>>(), [kept]);
    assert_eq!(lab.controller.node(network, laptop.id).unwrap().node.groups[0].name, "home");
    assert!(lab.controller.tokens(network, false).unwrap()[0].groups.is_empty());
    assert_eq!(lab.controller.group(network, laptops).unwrap_err().code(), ErrorCode::NotFound);
    lab.settle(&mut [&mut laptop, &mut server]);
    assert!(peer_ids(laptop.netmap()).is_empty() && peer_ids(server.netmap()).is_empty(), "只经由它的访问撤销了");

    let entry = &lab.controller.oplog_filtered(&LogFilter { limit: 1, action: Some("group.delete".to_owned()), ..LogFilter::default() });
    let entry = &entry.as_ref().unwrap()[0];
    assert!(entry.refs.iter().any(|reference| reference.kind == RefKind::Rule && reference.id == ssh));
    assert!(entry.refs.iter().any(|reference| reference.kind == RefKind::Group && reference.id == laptops.0));
}

#[test]
fn the_access_graph_shows_every_group_with_its_members() {
    let mut lab = Lab::new();
    let network = lab.network("home").id;
    let laptop = lab.join_offline(network, &["laptops"], Declared::default());
    let empty = lab.group(network, "empty");
    let graph = lab.controller.access_graph(network).unwrap();
    let names: Vec<&str> = graph.groups.iter().map(|group| group.name.as_str()).collect();
    assert_eq!(names, ["All", "empty", "laptops"]);
    assert_eq!(graph.groups[0].nodes.len(), 1);
    assert_eq!(graph.groups[2].nodes[0].id, laptop);
    assert!(graph.groups.iter().find(|group| group.id == empty).unwrap().nodes.is_empty());
    assert_eq!(graph.groups[0].rules, graph.rules, "默认规则挂在 All 上");
}
