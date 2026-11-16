//! 访问规则的增删改。

use std::collections::BTreeSet;

use i3t_model::admin::{CreateRule, Direction, LogRef, NetworkId, RefKind, RenameTag, RenamedTag, Rule, UpdateRule};

use super::{Controller, Inner, Seeds, count, naming};
use crate::access::ALL;
use crate::error::{Entity, Error};

impl Controller {
    /// 一个网络的全部规则，包括停用的。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn rules(&self, network: NetworkId) -> Result<Vec<Rule>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        Ok(inner.store.rules(network)?)
    }

    /// # Errors
    ///
    /// 没有这个网络，或者标签、名字的写法不对。
    pub fn create_rule(&self, network: NetworkId, request: &CreateRule) -> Result<Rule, Error> {
        let mut rule = Rule {
            id: 0,
            name: naming::title("规则", &request.name, false)?,
            src: naming::tag(&request.src)?,
            dst: naming::tag(&request.dst)?,
            direction: request.direction,
            enabled: request.enabled,
        };
        let mut inner = self.lock();
        inner.require_network(network)?;
        let now = inner.clock.now();
        rule.id = inner.store.atomically(|store| {
            let id = store.insert_rule(network, &rule)?;
            store.log_refs(now, Some(network), "rule.create", &describe(&rule), &[LogRef::new(RefKind::Rule, id)])?;
            Ok::<_, Error>(id)
        })?;
        let seeds = inner.nodes_tagged(network, &[&rule.src, &rule.dst])?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(rule)
    }

    /// 改一条规则。只改请求里写了的字段。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这条规则，或者标签、名字的写法不对。
    pub fn update_rule(&self, network: NetworkId, id: u32, request: &UpdateRule) -> Result<Rule, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let (before, after) = inner.store.atomically(|store| {
            let before = store.rule(network, id)?.ok_or(Error::NotFound(Entity::Rule))?;
            let mut rule = before.clone();
            if let Some(name) = &request.name {
                rule.name = naming::title("规则", name, false)?;
            }
            if let Some(src) = &request.src {
                rule.src = naming::tag(src)?;
            }
            if let Some(dst) = &request.dst {
                rule.dst = naming::tag(dst)?;
            }
            rule.direction = request.direction.unwrap_or(rule.direction);
            rule.enabled = request.enabled.unwrap_or(rule.enabled);
            store.update_rule(&rule)?;
            store.log_refs(now, Some(network), "rule.update", &describe(&rule), &[LogRef::new(RefKind::Rule, id)])?;
            Ok::<_, Error>((before, rule))
        })?;
        let seeds = inner.nodes_tagged(network, &[&before.src, &before.dst, &after.src, &after.dst])?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(after)
    }

    /// # Errors
    ///
    /// 这个网络里没有这条规则。
    pub fn delete_rule(&self, network: NetworkId, id: u32) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let rule = inner.store.atomically(|store| {
            let rule = store.rule(network, id)?.ok_or(Error::NotFound(Entity::Rule))?;
            store.delete_rule(id)?;
            store.log_refs(now, Some(network), "rule.delete", &describe(&rule), &[LogRef::new(RefKind::Rule, id)])?;
            Ok::<_, Error>(rule)
        })?;
        let seeds = inner.nodes_tagged(network, &[&rule.src, &rule.dst])?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

impl Controller {
    /// 在整个网络里把标签 `from` 改名为 `to`：节点上的、规则里的、还没用掉的注册令牌预设的。
    ///
    /// 节点上已经有 `to` 的，两个合成一个。改名不改变谁能访问谁（除非 `to` 原来就是一个在用的标签，那时两组合并），
    /// 所以只有带这两个标签的节点要重算。
    ///
    /// # Errors
    ///
    /// 没有这个网络；标签的写法不对；`from` 或 `to` 是内置的 `all`；两个是同一个。
    pub fn rename_tag(&self, network: NetworkId, request: &RenameTag) -> Result<RenamedTag, Error> {
        let (from, to) = (naming::tag(&request.from)?, naming::tag(&request.to)?);
        if from == ALL || to == ALL {
            return Err(Error::invalid(format!("内置标签 {ALL} 不能改名，也不能把别的标签改成它")));
        }
        if from == to {
            return Err(Error::invalid("新旧两个标签是同一个"));
        }
        let mut inner = self.lock();
        inner.require_network(network)?;
        let now = inner.clock.now();
        let seeds = inner.nodes_tagged(network, &[&from, &to])?;
        let renamed = inner.store.atomically(|store| {
            let mut nodes = Vec::new();
            for mut node in store.nodes(network)? {
                if node.tags.remove(&from) {
                    node.tags.insert(to.clone());
                    store.update_node(&node)?;
                    nodes.push(node.id);
                }
            }
            let mut rules = 0;
            for mut rule in store.rules(network)? {
                if rule.src != from && rule.dst != from {
                    continue;
                }
                for tag in [&mut rule.src, &mut rule.dst] {
                    if *tag == from {
                        tag.clone_from(&to);
                    }
                }
                store.update_rule(&rule)?;
                rules += 1;
            }
            let tokens = store.rename_token_tag(network, &from, &to)?;
            let renamed = RenamedTag { from: from.clone(), to: to.clone(), nodes: count(nodes.len()), rules, tokens };
            let detail = format!("标签 {from} 改名为 {to}：{} 个节点，{rules} 条规则，{tokens} 个注册令牌", renamed.nodes);
            let refs: Vec<LogRef> = nodes.into_iter().map(LogRef::node).collect();
            store.log_refs(now, Some(network), "tag.rename", &detail, &refs)?;
            Ok::<_, Error>(renamed)
        })?;
        tracing::info!(%network, from = %renamed.from, to = %renamed.to, nodes = renamed.nodes, rules = renamed.rules, "标签改名");
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(renamed)
    }
}

impl Inner {
    /// 带着这些标签里任何一个的节点：一条规则变了，要重算的就是它两头的节点。
    fn nodes_tagged(&self, network: NetworkId, tags: &[&String]) -> Result<Seeds, Error> {
        if tags.iter().any(|tag| *tag == ALL) {
            return Ok(Seeds::All);
        }
        let tagged = self.store.nodes(network)?.into_iter().filter(|node| tags.iter().any(|tag| node.tags.contains(*tag)));
        Ok(Seeds::Nodes(tagged.map(|node| node.id).collect::<BTreeSet<_>>()))
    }
}

fn describe(rule: &Rule) -> String {
    let arrow = match rule.direction {
        Direction::Both => "<->",
        Direction::OneWay => "->",
    };
    let state = if rule.enabled { "启用" } else { "停用" };
    format!("规则 #{} {} {arrow} {}（{state}）", rule.id, rule.src, rule.dst)
}
