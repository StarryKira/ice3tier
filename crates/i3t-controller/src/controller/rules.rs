//! 访问规则的增删改。

use std::collections::BTreeSet;

use i3t_model::admin::{CreateRule, Direction, NetworkId, Rule, UpdateRule};

use super::{Controller, Inner, Seeds, naming};
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
            store.log(now, Some(network), "rule.create", &describe(&rule))?;
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
            store.log(now, Some(network), "rule.update", &describe(&rule))?;
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
            store.log(now, Some(network), "rule.delete", &describe(&rule))?;
            Ok::<_, Error>(rule)
        })?;
        let seeds = inner.nodes_tagged(network, &[&rule.src, &rule.dst])?;
        inner.refresh(network, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(())
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
