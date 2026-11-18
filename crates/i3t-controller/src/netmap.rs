//! 给一个节点算它的 netmap。
//!
//! 最小权限：对端只列和它有访问关系的节点；relay 列全网的（relay 不受访问规则约束）；
//! 全网节点的编号和公钥只给 relay（认证中转会话要用）；代理只给它监听的那个节点，带着用户名和密码的校验值，不带密码。
//!
//! 这里算出来的内容不带版本号。版本号由下发的一方管：内容的摘要变了才加一。

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;

use i3t_model::Netmap;
use i3t_model::admin::{GroupId, NetworkId, PairSetting, Rule};
use i3t_model::netmap::{
    Forward, ForwardTarget, LaneMode, LocalNode, Network, Peer, PeerExit, Proxy, Reach, Relay, RelayClient, RelayRole,
};
use i3t_proto::NodeId;

use crate::access::AccessMap;
use crate::store::{ForwardRecord, GroupRecord, NetworkRecord, NodeRecord, ProxyRecord, Store, StoreError};
use crate::{node_addresses, relay_reach};

/// 下发给对端的候选端点最多这么多个。正常的节点只有几个；多出来的没有用，只会让每个对端都去探测。
pub(crate) const MAX_ENDPOINTS: usize = 16;

/// 算 netmap 要用的、存储里没有的事：controller 从节点的连接和上报里知道的。
pub trait NodeFacts {
    /// 节点懂不懂按需拨 ice3proxy：没有设置的节点对，两个节点都懂才是默认的 [`LaneMode::PAIR_DEFAULT`]，否则是 `off`
    /// （旧的节点在 `udp_fallback` 下一直拨着，不能让它因为 controller 升级就拨起来）。
    fn dials_on_demand(&self, node: NodeId) -> bool;
    /// 认定这个节点用不了 UDP，写进别的节点的 netmap 里它那一项（[`Peer::no_udp`]）。
    fn has_no_udp(&self, node: NodeId) -> bool;
}

/// 一个节点的 netmap 里，它和某个对端之间的访问关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Relation {
    /// 对端可以访问本节点。
    pub inbound: bool,
    /// 本节点可以访问对端。
    pub outbound: bool,
}

impl Relation {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.inbound && !self.outbound
    }
}

/// 一个网络在某一刻的全部内容。算 netmap 只读它，所以同一批节点看到的是同一份数据。
#[derive(Debug, Clone)]
pub struct NetworkView {
    pub network: NetworkRecord,
    pub nodes: BTreeMap<NodeId, NodeRecord>,
    /// 全部分组，内置的在最前面，其余按名字排。
    pub groups: Vec<GroupRecord>,
    pub rules: Vec<Rule>,
    pub proxies: Vec<ProxyRecord>,
    pub forwards: Vec<ForwardRecord>,
    pub pairs: Vec<PairSetting>,
}

impl NetworkView {
    /// 从存储里读出一个网络。网络不存在时返回 `None`。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn load(store: &Store, id: NetworkId) -> Result<Option<Self>, StoreError> {
        let Some(network) = store.network(id)? else { return Ok(None) };
        Ok(Some(Self {
            network,
            nodes: store.nodes(id)?.into_iter().map(|node| (node.id, node)).collect(),
            groups: store.groups(id)?,
            rules: store.rules(id)?,
            proxies: store.proxies(id)?,
            forwards: store.forwards(id)?,
            pairs: store.pairs(id)?,
        }))
    }

    /// 这个网络的内置分组 `All`。
    ///
    /// 每个网络建好时就有它，迁移也给旧的网络建了。真的没有时用一个不存在的编号（编号从 1 起）：
    /// 规则的两头有外键，没有规则能指向一个不存在的分组，所以它不会让任何节点多出访问关系。
    #[must_use]
    pub fn all_group(&self) -> GroupId {
        self.groups.iter().find(|group| group.builtin).map_or(GroupId(0), |group| group.id)
    }

    /// 这个网络里谁能访问谁。
    #[must_use]
    pub fn access(&self) -> AccessMap<'_> {
        AccessMap::new(self.nodes.values().map(|node| (node.id, &node.groups)), self.all_group(), &self.rules)
    }

    /// 规则算出来的访问关系：`node` 和网络里每个别的节点之间。没有关系的不在结果里。
    #[must_use]
    pub fn target_relations(&self, access: &AccessMap<'_>, node: NodeId) -> BTreeMap<NodeId, Relation> {
        let others = self.nodes.keys().copied().filter(|&other| other != node);
        others
            .map(|other| (other, Relation { inbound: access.allows(other, node), outbound: access.allows(node, other) }))
            .filter(|(_, relation)| !relation.is_empty())
            .collect()
    }

    /// 一条端口转发现在该不该下发：启用了，目标节点的开关（可以作为端口转发的目标）打开着，并且监听节点被允许访问目标节点。
    #[must_use]
    pub fn forward_is_deliverable(&self, access: &AccessMap<'_>, forward: &ForwardRecord) -> bool {
        let config = &forward.config;
        let switched_on = self.nodes.get(&config.target_node).is_some_and(|target| target.forward_target);
        let permitted = config.listen_node == config.target_node || access.allows(config.listen_node, config.target_node);
        config.enabled && switched_on && self.nodes.contains_key(&config.listen_node) && permitted
    }

    fn reach_of(&self, node: &NodeRecord) -> Reach {
        node.reach.unwrap_or(self.network.reach)
    }

    fn relay_ids(&self) -> BTreeSet<NodeId> {
        self.nodes.values().filter(|node| node.relay.is_some()).map(|node| node.id).collect()
    }

    /// 算 `id` 这个节点的 netmap，版本号留 0。节点不在这个网络里时返回 `None`。
    ///
    /// `relations` 决定它的对端列表：键是对端，值是两个方向各通不通。它通常就是规则算出来的
    /// [`NetworkView::target_relations`]，变更还在按顺序生效的途中时会暂时落后于规则。
    /// `access` 是 [`NetworkView::access`] 的结果，由调用方传进来是为了给一批节点算的时候只建一次。
    /// `facts` 是存储里没有、controller 知道的事（见 [`NodeFacts`]）。
    #[must_use]
    pub fn compile(
        &self,
        access: &AccessMap<'_>,
        id: NodeId,
        relations: &BTreeMap<NodeId, Relation>,
        facts: &impl NodeFacts,
    ) -> Option<Netmap> {
        let me = self.nodes.get(&id)?;
        let relays = self.relay_ids();

        let peers = relations
            .iter()
            .filter(|&(&peer, relation)| peer != id && !relation.is_empty())
            .filter_map(|(peer, &relation)| Some(self.peer(me, self.nodes.get(peer)?, relation, &relays, facts)))
            .collect();

        let deliverable = || self.forwards.iter().filter(|forward| self.forward_is_deliverable(access, forward));
        let forwards = deliverable()
            .filter(|forward| forward.config.listen_node == id)
            .map(|forward| Forward {
                id: forward.id,
                protocol: forward.config.protocol,
                bind: forward.config.bind,
                target: forward.config.target_node,
                allow_from: forward.config.allow_from.clone(),
                class: forward.config.class,
            })
            .collect();
        let forward_targets = deliverable()
            .filter(|forward| forward.config.target_node == id)
            .map(|forward| ForwardTarget {
                id: forward.id,
                protocol: forward.config.protocol,
                addr: forward.config.target_addr,
                listener: forward.config.listen_node,
                proxy_protocol: forward.config.proxy_protocol,
            })
            .collect();

        // 启用了的、有出口的才下发。出口没有启用、访问规则不让访问出口的照样下发：节点照样监听，
        // 不在组网里的目标被它拒绝，代理的状态说明为什么（见 `controller::proxies`）。
        let proxies = self
            .proxies
            .iter()
            .filter(|proxy| proxy.node == id && proxy.enabled)
            .filter_map(|proxy| {
                Some(Proxy {
                    id: proxy.id,
                    name: proxy.name.clone(),
                    bind: proxy.bind,
                    protocol: proxy.protocol,
                    exit: proxy.exit?,
                    auth: proxy.verifier(),
                    allow_from: proxy.allow_from.clone(),
                    class: proxy.class,
                    optimistic: proxy.optimistic,
                })
            })
            .collect();

        let other_nodes = || self.nodes.values().filter(|node| node.id != id);
        // relay 的地址是 controller 找出来的（白皮书 §7.2），UDP 和 TCP 是同一组：节点自己挑连得上的那个。
        let relay_list = other_nodes()
            .filter(|node| node.relay.is_some())
            .map(|node| {
                let addresses: Vec<SocketAddr> =
                    node_addresses::relay_addresses(node, self.network.cidr).into_iter().map(|entry| entry.address).collect();
                Relay { id: node.id, key: node.key, name: node.name.clone(), udp: addresses.clone(), tcp: addresses }
            })
            .collect();
        let relay_clients =
            if me.relay.is_some() { other_nodes().map(|node| RelayClient { id: node.id, key: node.key }).collect() } else { Vec::new() };
        // 不用 relay 的节点不挂靠，也不为它找；对端不用 relay 的也不算。
        let shared_relays = if self.reach_of(me) == Reach::DirectOnly {
            Vec::new()
        } else {
            let peers = relations.keys().filter_map(|peer| self.nodes.get(peer)).filter(|peer| self.reach_of(peer) != Reach::DirectOnly);
            relay_reach::shared_relays(me, peers, &relays)
        };

        Some(Netmap {
            version: 0,
            network: Network {
                name: self.network.name.clone(),
                domain: self.network.domain(),
                cidr: self.network.cidr,
                routing: self.network.routing,
                max_relay_hops: self.network.max_relay_hops,
                reach: self.network.reach,
                class: self.network.class,
            },
            me: LocalNode {
                id,
                name: me.name.clone(),
                virtual_ip: me.virtual_ip,
                routing: me.routing,
                reach: me.reach,
                dial_out_only: me.dial_out_only,
                relay: me.relay.as_ref().map(|settings| RelayRole { bandwidth_mbps: settings.bandwidth_mbps }),
                exit: me.exit.clone(),
                proxies,
                forwards,
                forward_targets,
                // 固定的 relay 里已经不是 relay 的那些不下发：节点连不上它们，只会白等。
                pinned_relays: me.pinned_relays.iter().copied().filter(|relay| *relay != id && relays.contains(relay)).collect(),
                profile: me.profile,
                shared_relays,
            },
            peers,
            relays: relay_list,
            relay_clients,
        })
    }

    fn peer(&self, me: &NodeRecord, peer: &NodeRecord, relation: Relation, relays: &BTreeSet<NodeId>, facts: &impl NodeFacts) -> Peer {
        let mut attached: Vec<_> = peer.attached.iter().copied().filter(|attachment| relays.contains(&attachment.relay)).collect();
        attached.sort_by_key(|attachment| (attachment.rtt_ms, attachment.relay));
        let (low, high) = (me.id.min(peer.id), me.id.max(peer.id));
        let stored = self.pairs.iter().find(|pair| pair.a == low && pair.b == high);
        let lane = match stored {
            Some(pair) => pair.lane,
            None if facts.dials_on_demand(me.id) && facts.dials_on_demand(peer.id) => LaneMode::PAIR_DEFAULT,
            None => LaneMode::Off,
        };
        Peer {
            id: peer.id,
            key: peer.key,
            name: peer.name.clone(),
            virtual_ip: peer.virtual_ip,
            inbound: relation.inbound,
            outbound: relation.outbound,
            // 只走 relay 的节点不向对端暴露自己的地址。
            endpoints: if self.reach_of(peer) == Reach::RelayOnly { Vec::new() } else { candidate_endpoints(peer) },
            attached,
            // 出口的提示只给能用它的节点。
            exit: peer.exit.as_ref().filter(|_| relation.outbound).map(|exit| PeerExit {
                lan: exit.lan,
                class: exit.class,
                optimistic: exit.optimistic,
            }),
            dial_out_only: peer.dial_out_only,
            keep_alive: stored.is_some_and(|pair| pair.keep_alive),
            lane,
            congestion: stored.and_then(|pair| pair.congestion),
            no_udp: facts.has_no_udp(peer.id),
        }
    }
}

/// 一个节点的候选端点：它自己上报的，加上 controller 看到的来源地址。去重，保持顺序。
pub(crate) fn candidate_endpoints(node: &NodeRecord) -> Vec<SocketAddr> {
    let mut endpoints: Vec<SocketAddr> = Vec::with_capacity(node.endpoints.len() + 1);
    for endpoint in node.endpoints.iter().chain(&node.observed) {
        if !endpoints.contains(endpoint) && endpoints.len() < MAX_ENDPOINTS {
            endpoints.push(*endpoint);
        }
    }
    endpoints
}
