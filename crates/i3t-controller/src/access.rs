//! 访问规则的编译：从"带标签 a 的节点可以访问带标签 b 的节点"得出"谁能访问谁"。
//!
//! 只有"允许"，规则之间没有顺序：任何一条启用的规则连通了两个节点，它们就连通。
//! 节点不会拿到标签和规则，它们拿到的是这里算出来的结果。

use std::collections::{BTreeMap, BTreeSet};

use i3t_model::admin::{Direction, Rule};
use i3t_proto::NodeId;

/// 内置标签：每个节点都带着它，不用打。
pub const ALL: &str = "all";

/// 一个网络里"谁能访问谁"。只看同一个网络的节点和规则，所以网络之间天然互不可见。
#[derive(Debug, Clone)]
pub struct AccessMap<'a> {
    tags: BTreeMap<NodeId, &'a BTreeSet<String>>,
    /// 只留启用了的规则。
    rules: Vec<&'a Rule>,
}

impl<'a> AccessMap<'a> {
    /// `nodes` 是一个网络里的全部节点和它们各自的标签，`rules` 是这个网络的规则（停用的会被忽略）。
    #[must_use]
    pub fn new(nodes: impl IntoIterator<Item = (NodeId, &'a BTreeSet<String>)>, rules: &'a [Rule]) -> Self {
        Self { tags: nodes.into_iter().collect(), rules: rules.iter().filter(|rule| rule.enabled).collect() }
    }

    fn has_tag(&self, node: NodeId, tag: &str) -> bool {
        self.tags.get(&node).is_some_and(|tags| tag == ALL || tags.contains(tag))
    }

    fn permits(&self, rule: &Rule, from: NodeId, to: NodeId) -> bool {
        let forward = self.has_tag(from, &rule.src) && self.has_tag(to, &rule.dst);
        let backward = || rule.direction == Direction::Both && self.has_tag(from, &rule.dst) && self.has_tag(to, &rule.src);
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
        self.tags.keys().copied().filter(|&other| self.allows(node, other)).collect()
    }

    /// 可以访问 `node` 的节点。目的节点在每条流建立时查的就是这个集合。
    #[must_use]
    pub fn sources_of(&self, node: NodeId) -> BTreeSet<NodeId> {
        self.tags.keys().copied().filter(|&other| self.allows(other, node)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn tags(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    fn rule(id: u32, src: &str, dst: &str, direction: Direction) -> Rule {
        Rule { id, name: String::new(), src: src.to_owned(), dst: dst.to_owned(), direction, enabled: true }
    }

    fn set(ids: &[u32]) -> BTreeSet<NodeId> {
        ids.iter().map(|&raw| id(raw)).collect()
    }

    #[test]
    fn the_default_rule_connects_everyone_but_not_a_node_to_itself() {
        let untagged = tags(&[]);
        let rules = [rule(1, ALL, ALL, Direction::Both)];
        let access = AccessMap::new([(id(1), &untagged), (id(2), &untagged), (id(3), &untagged)], &rules);
        assert!(access.allows(id(1), id(2)) && access.allows(id(2), id(1)));
        assert!(!access.allows(id(1), id(1)));
        assert_eq!(access.targets_of(id(1)), set(&[2, 3]));
        assert_eq!(access.sources_of(id(1)), set(&[2, 3]));
    }

    #[test]
    fn a_one_way_rule_only_lets_the_source_side_initiate() {
        let (laptops, servers) = (tags(&["laptops"]), tags(&["servers"]));
        let rules = [rule(1, "laptops", "servers", Direction::OneWay)];
        let access = AccessMap::new([(id(1), &laptops), (id(2), &servers)], &rules);
        assert!(access.allows(id(1), id(2)));
        assert!(!access.allows(id(2), id(1)));
        assert_eq!(access.targets_of(id(1)), set(&[2]));
        assert_eq!(access.sources_of(id(1)), set(&[]));
        assert_eq!(access.sources_of(id(2)), set(&[1]));
    }

    #[test]
    fn nodes_whose_tags_no_rule_connects_cannot_reach_each_other() {
        let (a, b, c) = (tags(&["a"]), tags(&["b"]), tags(&["c"]));
        let rules = [rule(1, "a", "c", Direction::Both)];
        let access = AccessMap::new([(id(1), &a), (id(2), &b), (id(3), &c)], &rules);
        assert!(!access.allows(id(1), id(2)) && !access.allows(id(2), id(1)));
        assert!(!access.allows(id(2), id(3)) && !access.allows(id(3), id(2)));
        assert!(access.allows(id(1), id(3)) && access.allows(id(3), id(1)));
        assert!(access.targets_of(id(2)).is_empty() && access.sources_of(id(2)).is_empty());
    }

    #[test]
    fn a_rule_within_one_tag_connects_its_members_to_each_other() {
        let (servers, other) = (tags(&["servers"]), tags(&[]));
        let rules = [rule(1, "servers", "servers", Direction::OneWay)];
        let access = AccessMap::new([(id(1), &servers), (id(2), &servers), (id(3), &other)], &rules);
        assert!(access.allows(id(1), id(2)) && access.allows(id(2), id(1)));
        assert!(!access.allows(id(1), id(3)) && !access.allows(id(3), id(1)));
    }

    #[test]
    fn disabled_rules_are_ignored_and_the_allowing_rules_are_reported() {
        let (laptops, servers) = (tags(&["laptops", "admins"]), tags(&["servers"]));
        let mut disabled = rule(1, ALL, ALL, Direction::Both);
        disabled.enabled = false;
        let rules = [disabled, rule(2, "laptops", "servers", Direction::OneWay), rule(3, "servers", "admins", Direction::Both)];
        let access = AccessMap::new([(id(1), &laptops), (id(2), &servers)], &rules);
        let ids = |from, to| access.rules_allowing(id(from), id(to)).map(|rule| rule.id).collect::<Vec<_>>();
        assert_eq!(ids(1, 2), vec![2, 3]);
        assert_eq!(ids(2, 1), vec![3]);
        assert!(ids(1, 1).is_empty());
    }

    #[test]
    fn a_node_outside_the_map_is_never_allowed() {
        let untagged = tags(&[]);
        let rules = [rule(1, ALL, ALL, Direction::Both)];
        let access = AccessMap::new([(id(1), &untagged)], &rules);
        assert!(!access.allows(id(1), id(9)) && !access.allows(id(9), id(1)));
    }
}
