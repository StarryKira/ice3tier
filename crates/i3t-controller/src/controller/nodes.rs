//! 节点：查看、改名、打标签、启用能力、删除、继任。

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};

use i3t_model::admin::{LogRef, NetworkId, Node, NodeDetail, RelaySettings, Timestamp, UpdateNode};
use i3t_model::control::ToNode;
use i3t_model::netmap::ExitRole;
use i3t_proto::NodeId;

use super::{COOLDOWN_SECS, Controller, Inner, Seeds, address, listed, naming, switch};
use crate::error::{Capability, Entity, Error};
use crate::store::{CooldownKind, NetworkRecord, NodeRecord, Store, is_pending};

/// 一个 relay 每种协议最多填几个公网地址，一个节点最多固定几个挂靠 relay。都只是防止填错。
const MAX_RELAY_ADDRESSES: usize = 8;
const MAX_PINNED_RELAYS: usize = 8;

impl Controller {
    /// 一个网络里的全部节点。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn nodes(&self, network: NetworkId) -> Result<Vec<Node>, Error> {
        let inner = self.lock();
        let network = inner.require_network(network)?;
        let now = inner.clock.now();
        Ok(inner.store.nodes(network.id)?.iter().map(|record| inner.node_view(&network, record, now)).collect())
    }

    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者读存储失败。
    pub fn node(&self, network: NetworkId, id: NodeId) -> Result<NodeDetail, Error> {
        let inner = self.lock();
        let network = inner.require_network(network)?;
        let record = inner.require_node(network.id, id)?;
        let live = inner.live.get(&id);
        Ok(NodeDetail {
            node: inner.node_view(&network, &record, inner.clock.now()),
            endpoints: record.endpoints,
            observed_endpoint: record.observed,
            mapping_varies: live.and_then(|live| live.mapping_varies),
            flows: live.map_or(0, |live| live.flows),
            tx_bytes: live.map_or(0, |live| live.tx_bytes),
            rx_bytes: live.map_or(0, |live| live.rx_bytes),
            lanes: live.and_then(|live| live.lanes.clone()),
        })
    }

    /// 改节点的名字、标签、虚拟 IP 和各个开关。只改请求里写了的字段。
    ///
    /// # Errors
    ///
    /// 名字或虚拟 IP 不合法、被占用、在冷却期；固定的挂靠 relay 不是这个网络里启用了的 relay。
    pub fn update_node(&self, network: NetworkId, id: NodeId, request: &UpdateNode) -> Result<Node, Error> {
        let mut inner = self.lock();
        let network = inner.require_network(network)?;
        let now = inner.clock.now();
        let (before, after) = inner.store.atomically(|store| {
            let before = require_node(store, network.id, id)?;
            let mut node = before.clone();
            if let Some(name) = &request.name {
                node.name = naming::label("节点", name)?;
                if node.name != before.name {
                    check_node_name(store, network.id, &node.name, now)?;
                }
            }
            if let Some(tags) = &request.tags {
                node.tags = naming::node_tags(tags)?;
            }
            if let Some(virtual_ip) = request.virtual_ip.filter(|ip| *ip != before.virtual_ip) {
                check_address(store, &network, virtual_ip, now)?;
                node.virtual_ip = virtual_ip;
            }
            if let Some(pinned) = &request.pinned_relays {
                node.pinned_relays = check_pinned_relays(store, &node, pinned)?;
            }
            node.dial_out_only = request.dial_out_only.unwrap_or(node.dial_out_only);
            node.routing = request.routing.unwrap_or(node.routing);
            node.reach = request.reach.unwrap_or(node.reach);
            store.update_node(&node)?;
            store.log_refs(now, Some(network.id), "node.update", &describe_update(&before, &node), &[LogRef::node(id)])?;
            Ok::<_, Error>((before, node))
        })?;
        // relay 的名字写在全网每个节点的 netmap 里；别的改动只影响 netmap 里有它的节点。
        let seeds = if after.relay.is_some() && before.name != after.name { Seeds::All } else { inner.holders_and(id) };
        inner.refresh(network.id, seeds, None)?;
        self.wake_if_needed(&inner);
        Ok(inner.node_view(&network, &after, now))
    }

    /// 把节点启用为 relay，或者改它的 relay 设置。
    ///
    /// # Errors
    ///
    /// 节点没有声明可以作为 relay；没有填公网可达的地址，或者地址不合法。
    pub fn enable_relay(&self, network: NetworkId, id: NodeId, settings: RelaySettings) -> Result<Node, Error> {
        let settings = check_relay_settings(settings)?;
        self.set_capability(network, id, Affects::WholeNetwork, "node.relay.enable", move |node| {
            if !node.declared.relay {
                return Err(Error::NotDeclared { node: node.name.clone(), capability: Capability::Relay });
            }
            node.relay = Some(settings);
            Ok(())
        })
    }

    /// # Errors
    ///
    /// 这个网络里没有这个节点。
    pub fn disable_relay(&self, network: NetworkId, id: NodeId) -> Result<Node, Error> {
        self.set_capability(network, id, Affects::WholeNetwork, "node.relay.disable", |node| {
            node.relay = None;
            Ok(())
        })
    }

    /// 把节点启用为出口，或者改它的出口设置。
    ///
    /// # Errors
    ///
    /// 节点没有声明可以作为出口；端口或上游代理的地址不合法。
    pub fn enable_exit(&self, network: NetworkId, id: NodeId, settings: ExitRole) -> Result<Node, Error> {
        let settings = check_exit_settings(settings)?;
        self.set_capability(network, id, Affects::Holders, "node.exit.enable", move |node| {
            if !node.declared.exit {
                return Err(Error::NotDeclared { node: node.name.clone(), capability: Capability::Exit });
            }
            node.exit = Some(settings);
            Ok(())
        })
    }

    /// # Errors
    ///
    /// 这个网络里没有这个节点。
    pub fn disable_exit(&self, network: NetworkId, id: NodeId) -> Result<Node, Error> {
        self.set_capability(network, id, Affects::Holders, "node.exit.disable", |node| {
            node.exit = None;
            Ok(())
        })
    }

    fn set_capability(
        &self,
        network: NetworkId,
        id: NodeId,
        affects: Affects,
        action: &str,
        change: impl FnOnce(&mut NodeRecord) -> Result<(), Error>,
    ) -> Result<Node, Error> {
        let mut inner = self.lock();
        let network = inner.require_network(network)?;
        let now = inner.clock.now();
        let node = inner.store.atomically(|store| {
            let mut node = require_node(store, network.id, id)?;
            change(&mut node)?;
            store.update_node(&node)?;
            store.log_refs(now, Some(network.id), action, &format!("节点 {}", node.name), &[LogRef::node(id)])?;
            Ok::<_, Error>(node)
        })?;
        tracing::info!(node = %id, name = %node.name, action, "修改节点的能力");
        let seeds = match affects {
            Affects::WholeNetwork => Seeds::All,
            Affects::Holders => inner.holders_and(id),
        };
        inner.refresh(network.id, seeds, None)?;
        // relay 的更新要排队，是不是 relay 变了，该轮到谁也跟着变。
        inner.roll_out()?;
        self.wake_if_needed(&inner);
        Ok(inner.node_view(&network, &node, now))
    }

    /// 删掉一个节点：吊销它的公钥，立即把它从所有节点的 netmap 里去掉。
    /// 它的虚拟 IP 和名字进入冷却期，到期之前不分给别的节点。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者写存储失败。
    pub fn delete_node(&self, network: NetworkId, id: NodeId) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let node = inner.store.atomically(|store| {
            let node = require_node(store, network, id)?;
            let until = now.saturating_add(COOLDOWN_SECS);
            store.add_cooldown(network, CooldownKind::Address, &node.virtual_ip.to_string(), until)?;
            store.add_cooldown(network, CooldownKind::Name, &node.name, until)?;
            unpin_everywhere(store, network, id)?;
            store.delete_node(id)?;
            store.log_refs(
                now,
                Some(network),
                "node.delete",
                &format!("删除节点 {}（{}）", node.name, node.virtual_ip),
                &[LogRef::node(id)],
            )?;
            Ok::<_, Error>(node)
        })?;
        tracing::info!(node = %id, name = %node.name, "删除节点，吊销它的公钥");
        // 先告诉它自己，再从别人那里去掉它。它的连接随着发送队列的关闭而结束。
        inner.send(id, ToNode::Revoked);
        inner.forget_node(id);
        inner.refresh(network, Seeds::All, None)?;
        inner.roll_out()?;
        self.wake_if_needed(&inner);
        Ok(())
    }

    /// 把 `id` 指定为已下线节点 `predecessor` 的继任：继承它的虚拟 IP、名字、标签，
    /// 以及指向它的端口转发、监听器和节点对设置；然后删掉 `predecessor`。
    ///
    /// relay、出口和端口转发目标不继承：要在新机器上重新声明，再重新启用。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，或者 `predecessor` 还在线。
    pub fn succeed(&self, network: NetworkId, id: NodeId, predecessor: NodeId) -> Result<Node, Error> {
        if id == predecessor {
            return Err(Error::invalid("节点不能继任它自己"));
        }
        let mut inner = self.lock();
        let network = inner.require_network(network)?;
        let now = inner.clock.now();
        if inner.is_online(predecessor) {
            return Err(Error::PredecessorOnline(inner.require_node(network.id, predecessor)?.name));
        }
        let node = inner.store.atomically(|store| {
            let old = require_node(store, network.id, predecessor)?;
            let mut node = require_node(store, network.id, id)?;
            let former = format!("{}（{}）", node.name, node.virtual_ip);
            store.park_node(predecessor)?;
            node.name.clone_from(&old.name);
            node.virtual_ip = old.virtual_ip;
            node.tags.extend(old.tags.iter().cloned());
            store.update_node(&node)?;
            inherit(store, network.id, &old, &node)?;
            let detail = format!("节点 {former} 继任 {}（{}）", old.name, old.virtual_ip);
            store.log_refs(now, Some(network.id), "node.succeed", &detail, &[LogRef::node(id), LogRef::node(predecessor)])?;
            Ok::<_, Error>(node)
        })?;
        tracing::info!(node = %id, %predecessor, name = %node.name, "节点继任");
        inner.forget_node(predecessor);
        inner.refresh(network.id, Seeds::All, None)?;
        self.wake_if_needed(&inner);
        Ok(inner.node_view(&network, &node, now))
    }
}

/// 一项能力的变化要重算哪些节点。
#[derive(Clone, Copy)]
enum Affects {
    /// relay 列在全网每个节点的 netmap 里。
    WholeNetwork,
    /// 出口只写在能访问它的节点的 netmap 里。
    Holders,
}

impl Inner {
    /// `id` 自己，加上 netmap 里有它的节点。
    pub(crate) fn holders_and(&self, id: NodeId) -> Seeds {
        let mut seeds = self.holders_of(id);
        seeds.insert(id);
        Seeds::Nodes(seeds)
    }

    /// 一个节点不存在了：丢掉它的下发状态和排着的工作，别的节点也不再记着它。
    pub(crate) fn forget_node(&mut self, id: NodeId) {
        self.live.remove(&id);
        self.sync.remove(&id);
        self.dirty.remove(&id);
        self.routes.remove(&id);
        self.usage.forget(id);
        self.drop_route_queries(id);
        self.updates.forget(id);
        for sync in self.sync.values_mut() {
            sync.forget(id);
        }
    }
}

pub(super) fn require_node(store: &Store, network: NetworkId, id: NodeId) -> Result<NodeRecord, Error> {
    store.node(id)?.filter(|node| node.network == network).ok_or(Error::NotFound(Entity::Node))
}

/// 一个名字现在能不能给节点用：没有别的节点在用，也不在冷却期。
pub(super) fn check_node_name(store: &Store, network: NetworkId, name: &str, now: Timestamp) -> Result<(), Error> {
    if store.nodes(network)?.iter().any(|node| node.name == name) {
        return Err(Error::NameTaken(name.to_owned()));
    }
    match store.cooldown_until(network, CooldownKind::Name, name, now)? {
        Some(until) => Err(Error::NameCoolingDown { name: name.to_owned(), until }),
        None => Ok(()),
    }
}

/// 一个虚拟 IP 现在能不能分给节点：在地址段里，没有别的节点在用，也不在冷却期。
pub(super) fn check_address(store: &Store, network: &NetworkRecord, address: Ipv4Addr, now: Timestamp) -> Result<(), Error> {
    if !address::is_assignable(network.cidr, address) {
        return Err(Error::AddressOutOfRange { address, cidr: network.cidr });
    }
    if store.nodes(network.id)?.iter().any(|node| node.virtual_ip == address) {
        return Err(Error::AddressTaken(address));
    }
    match store.cooldown_until(network.id, CooldownKind::Address, &address.to_string(), now)? {
        Some(until) => Err(Error::AddressCoolingDown { address, until }),
        None => Ok(()),
    }
}

/// 给新节点挑虚拟 IP：地址段里最小的空闲地址。在用的、冷却期里的、还没用掉的注册令牌预设的都不算空闲。
pub(super) fn allocate_address(store: &Store, network: &NetworkRecord, now: Timestamp) -> Result<Ipv4Addr, Error> {
    let in_use = store.nodes(network.id)?.into_iter().map(|node| node.virtual_ip);
    let reserved = store.tokens(network.id)?.into_iter().filter(|token| is_pending(token, now)).filter_map(|token| token.virtual_ip);
    let unavailable: BTreeSet<Ipv4Addr> = in_use.chain(reserved).chain(store.cooling_addresses(network.id, now)?).collect();
    address::lowest_free(network.cidr, &unavailable).ok_or(Error::RangeExhausted(network.cidr))
}

/// 继任的后半段：`successor` 已经拿到了 `old` 的名字、虚拟 IP 和标签，这里把指向 `old` 的配置转给它，再删掉 `old`。
pub(super) fn inherit(store: &Store, network: NetworkId, old: &NodeRecord, successor: &NodeRecord) -> Result<(), Error> {
    store.repoint_node(old.id, successor.id)?;
    // 让出了名字和地址的那一行已经读不出来，先删掉它，后面才能再遍历这个网络的节点。
    store.delete_node(old.id)?;
    unpin_everywhere(store, network, old.id)?;
    // 端口转发的目标要节点自己声明过才行。继任者没有声明时，继承来的转发先停用。
    if !successor.declared.forward {
        store.disable_forwards_to(successor.id)?;
    }
    Ok(())
}

/// 别的节点不再把 `relay` 固定为挂靠 relay。
fn unpin_everywhere(store: &Store, network: NetworkId, relay: NodeId) -> Result<(), Error> {
    for mut node in store.nodes(network)? {
        if node.pinned_relays.contains(&relay) {
            node.pinned_relays.retain(|pinned| *pinned != relay);
            store.update_node(&node)?;
        }
    }
    Ok(())
}

fn check_pinned_relays(store: &Store, node: &NodeRecord, pinned: &[NodeId]) -> Result<Vec<NodeId>, Error> {
    if pinned.len() > MAX_PINNED_RELAYS {
        return Err(Error::invalid(format!("最多固定 {MAX_PINNED_RELAYS} 个挂靠 relay")));
    }
    let mut checked = Vec::with_capacity(pinned.len());
    for &relay in pinned {
        let record = require_node(store, node.network, relay)?;
        if relay == node.id || record.relay.is_none() {
            return Err(Error::invalid(format!("节点 {} 不是可以挂靠的 relay", record.name)));
        }
        if !checked.contains(&relay) {
            checked.push(relay);
        }
    }
    Ok(checked)
}

fn check_relay_settings(mut settings: RelaySettings) -> Result<RelaySettings, Error> {
    if settings.udp.is_empty() && settings.tcp.is_empty() {
        return Err(Error::invalid("relay 必须公网可达：至少填一个从公网能连上的 UDP 或 TCP 地址"));
    }
    if settings.udp.len() > MAX_RELAY_ADDRESSES || settings.tcp.len() > MAX_RELAY_ADDRESSES {
        return Err(Error::invalid(format!("每种协议最多填 {MAX_RELAY_ADDRESSES} 个地址")));
    }
    if let Some(bad) = settings.udp.iter().chain(&settings.tcp).find(|address| !is_connectable(address)) {
        return Err(Error::invalid(format!("{bad} 不是一个能连上的地址")));
    }
    if settings.bandwidth_mbps == Some(0) {
        return Err(Error::invalid("带宽上限不能是 0；不限制就不要填"));
    }
    settings.region = naming::title("区域", &settings.region, false)?;
    Ok(settings)
}

fn check_exit_settings(mut settings: ExitRole) -> Result<ExitRole, Error> {
    if let Some(ports) = &mut settings.ports {
        if ports.contains(&0) {
            return Err(Error::invalid("端口不能是 0"));
        }
        ports.sort_unstable();
        ports.dedup();
    }
    if settings.bandwidth_mbps == Some(0) {
        return Err(Error::invalid("带宽上限不能是 0；不限制就不要填"));
    }
    if settings.upstream_socks5.is_some_and(|address| !is_connectable(&address)) {
        return Err(Error::invalid("上游代理的地址不是一个能连上的地址"));
    }
    Ok(settings)
}

/// 一个地址能不能当连接的目的地：不是全零地址，端口不是 0。
pub(super) fn is_connectable(address: &SocketAddr) -> bool {
    !address.ip().is_unspecified() && address.port() != 0
}

/// 操作记录里的一句话：这次改了什么。
fn describe_update(before: &NodeRecord, after: &NodeRecord) -> String {
    let mut changes = Vec::new();
    if before.name != after.name {
        changes.push(format!("名字 {} → {}", before.name, after.name));
    }
    if before.virtual_ip != after.virtual_ip {
        changes.push(format!("虚拟 IP {} → {}", before.virtual_ip, after.virtual_ip));
    }
    if before.tags != after.tags {
        changes.push(format!("标签 {}", listed(after.tags.iter())));
    }
    if before.dial_out_only != after.dial_out_only {
        changes.push(format!("只拨出 {}", switch(after.dial_out_only)));
    }
    if before.routing != after.routing || before.reach != after.reach {
        changes.push("选路设置".to_owned());
    }
    if before.pinned_relays != after.pinned_relays {
        changes.push(format!("固定的挂靠 relay {}", listed(after.pinned_relays.iter().map(|relay| format!("#{relay}")))));
    }
    if changes.is_empty() {
        format!("节点 {}：没有变化", after.name)
    } else {
        format!("节点 {}：{}", before.name, changes.join("，"))
    }
}
