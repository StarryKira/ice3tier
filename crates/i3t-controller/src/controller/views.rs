//! 给界面看的几个现算的视图：总览、访问关系图、访问检查。

use std::collections::{BTreeMap, BTreeSet};

use i3t_model::admin::{AccessCheck, AccessEdge, AccessGraph, NetworkId, NodeAccess, NodePair, NodeRef, Overview, TagMembers};
use i3t_model::control::Route;
use i3t_proto::NodeId;

use super::{Controller, Inner, count, node_ref};
use crate::access::{ALL, AccessMap};
use crate::error::{Entity, Error};
use crate::netmap::NetworkView;

impl Controller {
    /// 一个网络的总览。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn overview(&self, network: NetworkId) -> Result<Overview, Error> {
        let inner = self.lock();
        let view = inner.view(network)?;
        let now = inner.clock.now();
        let online_nodes: Vec<NodeRef> = view.nodes.values().filter(|node| inner.is_online(node.id)).map(node_ref).collect();
        Ok(Overview {
            network,
            nodes: count(view.nodes.len()),
            online: count(online_nodes.len()),
            relays: count(view.nodes.values().filter(|node| node.relay.is_some()).count()),
            exits: count(view.nodes.values().filter(|node| node.exit.is_some()).count()),
            rules: count(view.rules.len()),
            listeners: count(view.listeners.len()),
            forwards: count(view.forwards.len()),
            credentials: count(view.credentials.len()),
            pending_tokens: count(inner.store.tokens(network)?.iter().filter(|token| token.expires_at > now).count()),
            unreachable_pairs: inner.unreachable_pairs(&view),
            online_nodes,
        })
    }

    /// 访问关系图的标签视图。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn access_graph(&self, network: NetworkId) -> Result<AccessGraph, Error> {
        let inner = self.lock();
        let view = inner.view(network)?;
        let access = view.access();

        let mut tags: BTreeMap<&str, Vec<NodeRef>> = BTreeMap::new();
        tags.insert(ALL, view.nodes.values().map(node_ref).collect());
        for rule in &view.rules {
            tags.entry(&rule.src).or_default();
            tags.entry(&rule.dst).or_default();
        }
        for node in view.nodes.values() {
            for tag in &node.tags {
                tags.entry(tag).or_default().push(node_ref(node));
            }
        }

        let is_connected = |id: NodeId| view.nodes.keys().any(|&other| access.allows(id, other) || access.allows(other, id));
        Ok(AccessGraph {
            tags: tags.into_iter().map(|(tag, nodes)| TagMembers { tag: tag.to_owned(), nodes }).collect(),
            unconnected: view.nodes.values().filter(|node| !is_connected(node.id)).map(node_ref).collect(),
            unacknowledged: view.nodes.values().filter(|node| !inner.has_acknowledged(node.id)).map(node_ref).collect(),
            rules: view.rules,
        })
    }

    /// 访问关系图的节点视图：`id` 能访问谁、谁能访问它、各是哪些规则允许的。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者读存储失败。
    pub fn node_access(&self, network: NetworkId, id: NodeId) -> Result<NodeAccess, Error> {
        let inner = self.lock();
        let view = inner.view(network)?;
        let node = view.nodes.get(&id).ok_or(Error::NotFound(Entity::Node))?;
        let access = view.access();
        let edges = |outbound: bool| -> Vec<AccessEdge> {
            let others = view.nodes.values().filter(|other| other.id != id);
            others
                .filter_map(|other| {
                    let (from, to) = if outbound { (id, other.id) } else { (other.id, id) };
                    let rules: Vec<u32> = access.rules_allowing(from, to).map(|rule| rule.id).collect();
                    (!rules.is_empty()).then(|| AccessEdge { node: node_ref(other), rules })
                })
                .collect()
        };
        Ok(NodeAccess {
            node: node_ref(node),
            can_reach: edges(true),
            reachable_by: edges(false),
            acknowledged: inner.has_acknowledged(id),
        })
    }

    /// 访问检查：`from` 能不能访问 `to`，是哪些规则允许的。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，或者读存储失败。
    pub fn access_check(&self, network: NetworkId, from: NodeId, to: NodeId) -> Result<AccessCheck, Error> {
        let inner = self.lock();
        let view = inner.view(network)?;
        let node = |id: NodeId| view.nodes.get(&id).map(node_ref).ok_or(Error::NotFound(Entity::Node));
        let access: AccessMap<'_> = view.access();
        let rules: Vec<_> = access.rules_allowing(from, to).cloned().collect();
        Ok(AccessCheck { from: node(from)?, to: node(to)?, allowed: !rules.is_empty(), rules })
    }
}

impl Inner {
    fn view(&self, network: NetworkId) -> Result<NetworkView, Error> {
        NetworkView::load(&self.store, network)?.ok_or(Error::NotFound(Entity::Network))
    }

    /// 节点是不是已经确认了它当前版本的 netmap。
    fn has_acknowledged(&self, id: NodeId) -> bool {
        self.sync.get(&id).is_none_or(|sync| sync.state.acked >= sync.state.version)
    }

    /// 两端都在线、但至少一端上报"没有可用的路"的节点对。
    fn unreachable_pairs(&self, view: &NetworkView) -> Vec<NodePair> {
        let mut pairs = BTreeSet::new();
        for (&id, live) in &self.live {
            let no_route = live.routes.values().filter(|entry| entry.route == Route::None).map(|entry| entry.peer);
            pairs.extend(no_route.filter(|peer| self.is_online(*peer)).map(|peer| (id.min(peer), id.max(peer))));
        }
        let pair = |(a, b): (NodeId, NodeId)| Some(NodePair { a: node_ref(view.nodes.get(&a)?), b: node_ref(view.nodes.get(&b)?) });
        pairs.into_iter().filter_map(pair).collect()
    }
}
