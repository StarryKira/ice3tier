//! 访问规则的编译：从"分组 a 里的节点可以访问分组 b 里的节点"得出"谁能访问谁"。
//!
//! 只有"允许"，规则之间没有顺序：任何一条启用的规则连通了两个节点，它们就连通。
//! 节点不会拿到分组和规则，它们拿到的是这里算出来的结果。

use std::collections::{BTreeMap, BTreeSet};

use i3t_model::admin::{Direction, GroupId, Rule};
use i3t_proto::NodeId;

/// 一个网络里"谁能访问谁"。只看同一个网络的节点和规则，所以网络之间天然互不可见。
#[derive(Debug, Clone)]
pub struct AccessMap<'a> {
    groups: BTreeMap<NodeId, &'a BTreeSet<GroupId>>,
    /// 内置分组 `All`：每个节点都在里面，不用写在节点的分组里。
    all: GroupId,
    /// 只留启用了的规则。
    rules: Vec<&'a Rule>,
}

impl<'a> AccessMap<'a> {
    /// `nodes` 是一个网络里的全部节点和它们各自所在的分组（不含 `All`），`all` 是这个网络的内置分组，
    /// `rules` 是这个网络的规则（停用的会被忽略）。
    #[must_use]
    pub fn new(nodes: impl IntoIterator<Item = (NodeId, &'a BTreeSet<GroupId>)>, all: GroupId, rules: &'a [Rule]) -> Self {
        Self { groups: nodes.into_iter().collect(), all, rules: rules.iter().filter(|rule| rule.enabled).collect() }
    }

    fn is_member(&self, node: NodeId, group: GroupId) -> bool {
        self.groups.get(&node).is_some_and(|groups| group == self.all || groups.contains(&group))
    }

    fn permits(&self, rule: &Rule, from: NodeId, to: NodeId) -> bool {
        let (src, dst) = (rule.src.id, rule.dst.id);
        let forward = self.is_member(from, src) && self.is_member(to, dst);
        let backward = || rule.direction == Direction::Both && self.is_member(from, dst) && self.is_member(to, src);
        forward || backward()
    }

    /// `from` 能不能向 `to` 发起连接。节点和自己之间不算访问关系。
    #[must_use]
    pub fn allows(&self, from: NodeId, to: NodeId) -> bool {
        from != to && self.rules.iter().any(|rule| self.permits(rule, from, to))
    }

    /// 允许 `from` 访问 `to` 的全部规则。
    pub fn rules_allowing(&self, from: NodeId, to: NodeId) -> impl Iterator<Item = &'a Rule> + '_ {
        self.rules.iter().copied().filter(move |rule| from != to && self.permits(rule, from, to))
    }

    /// `node` 可以访问的节点。
    #[must_use]
    pub fn targets_of(&self, node: NodeId) -> BTreeSet<NodeId> {
        self.groups.keys().copied().filter(|&other| self.allows(node, other)).collect()
    }

    /// 可以访问 `node` 的节点。目的节点在每条流建立时查的就是这个集合。
    #[must_use]
    pub fn sources_of(&self, node: NodeId) -> BTreeSet<NodeId> {
        self.groups.keys().copied().filter(|&other| self.allows(other, node)).collect()
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::admin::GroupRef;

    use super::*;

    const ALL: GroupId = GroupId(1);
    const LAPTOPS: GroupId = GroupId(2);
    const SERVERS: GroupId = GroupId(3);
    const ADMINS: GroupId = GroupId(4);

    fn id(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn groups(ids: &[GroupId]) -> BTreeSet<GroupId> {
        ids.iter().copied().collect()
    }

    fn rule(id: u32, src: GroupId, dst: GroupId, direction: Direction) -> Rule {
        let group = |id: GroupId| GroupRef { id, name: format!("g{id}"), builtin: id == ALL };
        Rule { id, name: String::new(), src: group(src), dst: group(dst), direction, enabled: true }
    }

    fn set(ids: &[u32]) -> BTreeSet<NodeId> {
        ids.iter().map(|&raw| id(raw)).collect()
    }

    #[test]
    fn the_default_rule_connects_everyone_but_not_a_node_to_itself() {
        let none = groups(&[]);
        let rules = [rule(1, ALL, ALL, Direction::Both)];
        let access = AccessMap::new([(id(1), &none), (id(2), &none), (id(3), &none)], ALL, &rules);
        assert!(access.allows(id(1), id(2)) && access.allows(id(2), id(1)));
        assert!(!access.allows(id(1), id(1)));
        assert_eq!(access.targets_of(id(1)), set(&[2, 3]));
        assert_eq!(access.sources_of(id(1)), set(&[2, 3]));
    }

    #[test]
    fn a_one_way_rule_only_lets_the_source_side_initiate() {
        let (laptops, servers) = (groups(&[LAPTOPS]), groups(&[SERVERS]));
        let rules = [rule(1, LAPTOPS, SERVERS, Direction::OneWay)];
        let access = AccessMap::new([(id(1), &laptops), (id(2), &servers)], ALL, &rules);
        assert!(access.allows(id(1), id(2)));
        assert!(!access.allows(id(2), id(1)));
        assert_eq!(access.targets_of(id(1)), set(&[2]));
        assert_eq!(access.sources_of(id(1)), set(&[]));
        assert_eq!(access.sources_of(id(2)), set(&[1]));
    }

    #[test]
    fn nodes_in_groups_no_rule_connects_cannot_reach_each_other() {
        let (a, b, c) = (groups(&[GroupId(10)]), groups(&[GroupId(11)]), groups(&[GroupId(12)]));
        let rules = [rule(1, GroupId(10), GroupId(12), Direction::Both)];
        let access = AccessMap::new([(id(1), &a), (id(2), &b), (id(3), &c)], ALL, &rules);
        assert!(!access.allows(id(1), id(2)) && !access.allows(id(2), id(1)));
        assert!(!access.allows(id(2), id(3)) && !access.allows(id(3), id(2)));
        assert!(access.allows(id(1), id(3)) && access.allows(id(3), id(1)));
        assert!(access.targets_of(id(2)).is_empty() && access.sources_of(id(2)).is_empty());
    }

    #[test]
    fn a_rule_within_one_group_connects_its_members_to_each_other() {
        let (servers, other) = (groups(&[SERVERS]), groups(&[]));
        let rules = [rule(1, SERVERS, SERVERS, Direction::OneWay)];
        let access = AccessMap::new([(id(1), &servers), (id(2), &servers), (id(3), &other)], ALL, &rules);
        assert!(access.allows(id(1), id(2)) && access.allows(id(2), id(1)));
        assert!(!access.allows(id(1), id(3)) && !access.allows(id(3), id(1)));
    }

    #[test]
    fn every_node_is_in_the_builtin_group_without_being_listed() {
        let (servers, none) = (groups(&[SERVERS]), groups(&[]));
        let rules = [rule(1, ALL, SERVERS, Direction::OneWay)];
        let access = AccessMap::new([(id(1), &servers), (id(2), &none)], ALL, &rules);
        assert!(access.allows(id(2), id(1)), "没有列出 All 的节点也在 All 里");
        assert!(!access.allows(id(1), id(2)));
    }

    #[test]
    fn disabled_rules_are_ignored_and_the_allowing_rules_are_reported() {
        let (laptops, servers) = (groups(&[LAPTOPS, ADMINS]), groups(&[SERVERS]));
        let mut disabled = rule(1, ALL, ALL, Direction::Both);
        disabled.enabled = false;
        let rules = [disabled, rule(2, LAPTOPS, SERVERS, Direction::OneWay), rule(3, SERVERS, ADMINS, Direction::Both)];
        let access = AccessMap::new([(id(1), &laptops), (id(2), &servers)], ALL, &rules);
        let ids = |from, to| access.rules_allowing(id(from), id(to)).map(|rule| rule.id).collect::<Vec<_>>();
        assert_eq!(ids(1, 2), vec![2, 3]);
        assert_eq!(ids(2, 1), vec![3]);
        assert!(ids(1, 1).is_empty());
    }

    #[test]
    fn a_node_outside_the_map_is_never_allowed() {
        let none = groups(&[]);
        let rules = [rule(1, ALL, ALL, Direction::Both)];
        let access = AccessMap::new([(id(1), &none)], ALL, &rules);
        assert!(!access.allows(id(1), id(9)) && !access.allows(id(9), id(1)));
    }
}
