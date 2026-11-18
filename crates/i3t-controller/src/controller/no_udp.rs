//! 认定哪些节点用不了 UDP：管理接口里节点的 `no_udp`，别的节点的 netmap 里它那一项的 `no_udp`。
//!
//! 用处：没有设置的节点对（`udp_fallback`）只在路上没有 UDP 时用 ice3proxy，可节点只看得见自己到对端的那一段。
//! 普通节点经一个 UDP 的中继到一个没有 UDP 的中继时，自己这一段有 UDP，看不出路上那一段 TCP；controller 看得出来，告诉它。
//!
//! - 证据是节点上报的、试过的 relay 连没连上、经什么连上的（[`relay_reach::without_udp`]）。只在有节点报的结果变了之后、
//!   在后台的下一轮重新算那个网络。
//! - 证据成立满 [`Timing::no_udp_settle`](super::Timing) 才算数：有 UDP 的节点也会一阵子只有 TCP（到 relay 的会话让 UDP
//!   先走一秒，没握上手就先用 TCP，30 秒之后再试 UDP）。等的这段时间里不动任何 netmap，所以这样一阵一阵的变化不会让它的
//!   对端的 netmap 来回重算。证据不成立了马上不算：节点上的按需拨号自己还会多拨一个保持时间，不会因此拆了重拨。
//! - controller 启动时，存储里证据已经成立的节点直接算数：重启之前多半就是这样，不让它的对端的 netmap 先改掉再改回来。
//! - 算数和不算数之间换了的时候，网络里知道它的节点（它的对端）的 netmap 重算，按上报引起的重算一样合并。

use std::collections::BTreeSet;
use std::collections::btree_map::Entry;
use std::time::Instant;

use i3t_model::admin::NetworkId;
use i3t_proto::NodeId;

use super::Inner;
use crate::error::Error;
use crate::relay_reach;
use crate::store::NodeRecord;

/// 一个证据成立着的节点。
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoUdp {
    network: NetworkId,
    /// 证据从什么时候起一直成立。
    since: Instant,
    /// 已经算数（写进了它的对端的 netmap）。
    settled: bool,
}

impl Inner {
    /// 认定 `node` 用不了 UDP。
    pub(crate) fn no_udp(&self, node: NodeId) -> bool {
        self.no_udp.get(&node).is_some_and(|entry| entry.settled)
    }

    /// 启动时：一个网络里存储中证据已经成立的节点直接算数。
    pub(crate) fn load_no_udp<'a>(&mut self, nodes: impl IntoIterator<Item = &'a NodeRecord> + Clone, now: Instant) {
        let without = relay_reach::without_udp(nodes.clone());
        for node in nodes.into_iter().filter(|node| without.contains(&node.id)) {
            self.no_udp.insert(node.id, NoUdp { network: node.network, since: now, settled: true });
        }
    }

    /// 后台的一轮：重新算报的结果变了的网络，证据成立满了等待时间的算数，换了的通知它的对端。返回下一个要算数的时刻。
    ///
    /// # Errors
    ///
    /// 读存储失败。没算完的网络留到下一轮。
    pub(crate) fn no_udp_tick(&mut self, now: Instant) -> Result<Option<Instant>, Error> {
        let mut changed: Vec<(NetworkId, NodeId)> = Vec::new();
        while let Some(network) = self.no_udp_dirty.pop_first() {
            let nodes = match self.store.nodes(network) {
                Ok(nodes) => nodes,
                Err(error) => {
                    self.no_udp_dirty.insert(network);
                    return Err(error.into());
                }
            };
            let without: BTreeSet<NodeId> = relay_reach::without_udp(&nodes);
            for node in &nodes {
                match self.no_udp.entry(node.id) {
                    Entry::Vacant(slot) if without.contains(&node.id) => {
                        tracing::debug!(node = %node.id, "节点连得上的 relay 没有一个经 UDP，等一阵再认定它用不了 UDP");
                        slot.insert(NoUdp { network, since: now, settled: false });
                    }
                    Entry::Occupied(slot) if !without.contains(&node.id) => {
                        if slot.remove().settled {
                            tracing::info!(node = %node.id, "节点又经 UDP 连上了 relay，不再认定它用不了 UDP");
                            changed.push((network, node.id));
                        }
                    }
                    Entry::Vacant(_) | Entry::Occupied(_) => {}
                }
            }
            // 删掉了的节点不再记着。
            let present: BTreeSet<NodeId> = nodes.iter().map(|node| node.id).collect();
            self.no_udp.retain(|id, entry| entry.network != network || present.contains(id));
        }
        let settle = self.timing.no_udp_settle;
        for (id, entry) in &mut self.no_udp {
            if !entry.settled && now.saturating_duration_since(entry.since) >= settle {
                entry.settled = true;
                tracing::info!(node = %id, ?settle, "节点连得上的 relay 这么久都只经 TCP，认定它用不了 UDP");
                changed.push((entry.network, *id));
            }
        }
        for (network, node) in changed {
            let holders = self.holders_of(node);
            self.defer_report(network, holders);
        }
        Ok(self.no_udp.values().filter(|entry| !entry.settled).map(|entry| entry.since + settle).min())
    }
}
