//! 每个节点的 netmap 下发到了哪一步，以及变更的生效顺序。
//!
//! 访问关系的变更要按顺序生效，否则版本交错期间会出现"发起端已经在用、目的端还不认"的失败：
//!
//! - **授权**先到目的端：目的节点确认自己已经认了发起节点之后，发起节点的 netmap 里才出现"可以访问它"。
//! - **撤销**先到发起端：发起节点确认自己已经不再访问目的节点之后，目的节点的 netmap 里才去掉"它可以访问我"。
//!
//! 所以一个节点的 netmap 里实际写着的关系可能暂时落后于规则算出来的关系。这里记的就是这两样东西：
//! 每个节点的 netmap 里现在实际写着什么（[`NodeSync::relations`]），以及它还欠着哪些确认。
//!
//! 确认有三个来源：节点发来 `Ack`；节点不在线（它没法确认，等它没有意义）；等得太久（不让一个不回话的节点卡住别人）。

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use i3t_proto::NodeId;

use crate::netmap::Relation;
use crate::store::NetmapState;

#[derive(Debug, Default)]
pub(crate) struct NodeSync {
    pub state: NetmapState,
    /// 这个节点当前版本的 netmap 里和各个对端的关系。
    pub relations: BTreeMap<NodeId, Relation>,
    /// 新认下的发起节点 → 第一次带着它的版本。节点确认到这个版本之前，对方还不能来访问。
    awaiting_accept: BTreeMap<NodeId, u64>,
    /// 刚被收回许可的目的节点 → 去掉它的那个版本。节点确认到这个版本之前，它可能还在访问对方。
    awaiting_undial: BTreeMap<NodeId, u64>,
    /// 最近一次给它发 netmap 的时刻，用来判断确认是不是等得太久了。
    pub sent_at: Option<Instant>,
}

impl NodeSync {
    pub(crate) fn with_state(state: NetmapState, relations: BTreeMap<NodeId, Relation>) -> Self {
        Self { state, relations, ..Self::default() }
    }

    /// 这个节点的 netmap 里认 `peer`，不管它确认了没有。
    pub(crate) fn accepts(&self, peer: NodeId) -> bool {
        self.relations.get(&peer).is_some_and(|relation| relation.inbound)
    }

    pub(crate) fn dials(&self, peer: NodeId) -> bool {
        self.relations.get(&peer).is_some_and(|relation| relation.outbound)
    }

    /// 这个节点的 netmap 里有没有 `peer`。
    pub(crate) fn knows(&self, peer: NodeId) -> bool {
        self.relations.contains_key(&peer)
    }

    /// `peer` 现在可以放心地来访问这个节点了：它的 netmap 里认 `peer`，而且已经确认过。
    pub(crate) fn has_confirmed_accepting(&self, peer: NodeId) -> bool {
        self.accepts(peer) && !self.awaiting_accept.contains_key(&peer)
    }

    /// 这个节点可能还在访问 `peer`：许可已经收回，但它还没确认。
    pub(crate) fn may_still_dial(&self, peer: NodeId) -> bool {
        self.awaiting_undial.contains_key(&peer)
    }

    /// 授权的第一步：这个节点从下一个版本起认 `peer`。在线的节点要确认过才算数。
    pub(crate) fn begin_accepting(&mut self, peer: NodeId, online: bool) {
        self.relations.entry(peer).or_default().inbound = true;
        if online {
            self.awaiting_accept.insert(peer, self.state.version + 1);
        }
    }

    /// 撤销的第一步：这个节点从下一个版本起不再访问 `peer`。在线的节点要确认过，`peer` 那边才能不认它。
    pub(crate) fn begin_undialing(&mut self, peer: NodeId, online: bool) {
        if online {
            self.awaiting_undial.insert(peer, self.state.version + 1);
        }
    }

    /// 新算出来的 netmap 已经定下版本号：记下里面的关系，丢掉已经没有意义的等待。
    ///
    /// 返回因此不用再等的对端。正常情况下是空的；它们的 netmap 要重新算一遍。
    pub(crate) fn settle(&mut self, relations: BTreeMap<NodeId, Relation>) -> BTreeSet<NodeId> {
        let version = self.state.version;
        self.relations = relations;
        let relations = &self.relations;
        let mut released = BTreeSet::new();
        // 等待登记的是"下一个版本"。内容其实没变、版本号没有加的话，那个版本永远不会被确认，不能留着。
        // 关系又变回去的（刚认下又不认了、刚收回又给了）也不用再等。
        self.awaiting_accept.retain(|peer, awaited| {
            let keep = *awaited <= version && relations.get(peer).is_some_and(|relation| relation.inbound);
            if !keep {
                released.insert(*peer);
            }
            keep
        });
        self.awaiting_undial.retain(|peer, awaited| {
            let keep = *awaited <= version && !relations.get(peer).is_some_and(|relation| relation.outbound);
            if !keep {
                released.insert(*peer);
            }
            keep
        });
        released
    }

    /// 节点确认了 `version`。返回因此可以继续下一步的对端：它们的 netmap 要重新算。
    pub(crate) fn confirm(&mut self, version: u64) -> BTreeSet<NodeId> {
        let mut released = BTreeSet::new();
        for awaiting in [&mut self.awaiting_accept, &mut self.awaiting_undial] {
            awaiting.retain(|peer, awaited| {
                let confirmed = *awaited <= version;
                if confirmed {
                    released.insert(*peer);
                }
                !confirmed
            });
        }
        released
    }

    /// 不再等这个节点确认（它下线了，或者等得太久）。返回可以继续下一步的对端。
    pub(crate) fn confirm_all(&mut self) -> BTreeSet<NodeId> {
        self.confirm(u64::MAX)
    }

    pub(crate) fn is_waiting(&self) -> bool {
        !self.awaiting_accept.is_empty() || !self.awaiting_undial.is_empty()
    }

    /// `peer` 被删掉了：和它有关的一切都作废。
    pub(crate) fn forget(&mut self, peer: NodeId) {
        self.relations.remove(&peer);
        self.awaiting_accept.remove(&peer);
        self.awaiting_undial.remove(&peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn inbound() -> Relation {
        Relation { inbound: true, outbound: false }
    }

    #[test]
    fn an_online_node_must_confirm_before_the_peer_may_come() {
        let mut sync = NodeSync::default();
        sync.begin_accepting(id(2), true);
        assert!(sync.accepts(id(2)) && !sync.has_confirmed_accepting(id(2)));

        sync.state.version = 1;
        assert!(sync.settle([(id(2), inbound())].into()).is_empty());
        assert!(sync.confirm(0).is_empty());
        assert!(!sync.has_confirmed_accepting(id(2)));
        assert_eq!(sync.confirm(1), [id(2)].into());
        assert!(sync.has_confirmed_accepting(id(2)) && !sync.is_waiting());
    }

    #[test]
    fn an_offline_node_counts_as_confirmed_at_once() {
        let mut sync = NodeSync::default();
        sync.begin_accepting(id(2), false);
        assert!(sync.has_confirmed_accepting(id(2)));
        sync.begin_undialing(id(3), false);
        assert!(!sync.may_still_dial(id(3)));
    }

    #[test]
    fn a_revoked_destination_is_released_only_by_a_late_enough_ack() {
        let mut sync = NodeSync::with_state(
            NetmapState { version: 4, ..NetmapState::default() },
            [(id(2), Relation { inbound: false, outbound: true })].into(),
        );
        sync.begin_undialing(id(2), true);
        sync.state.version = 5;
        assert!(sync.settle(BTreeMap::new()).is_empty());
        assert!(sync.may_still_dial(id(2)));
        assert!(sync.confirm(4).is_empty());
        assert_eq!(sync.confirm(7), [id(2)].into());
        assert!(!sync.may_still_dial(id(2)));
    }

    #[test]
    fn waiting_for_a_version_that_was_never_issued_is_dropped() {
        let mut sync = NodeSync::default();
        sync.begin_accepting(id(2), true);
        // 内容没变，版本号没有加：还是 0。
        assert_eq!(sync.settle([(id(2), inbound())].into()), [id(2)].into());
        assert!(sync.has_confirmed_accepting(id(2)));
    }

    #[test]
    fn going_offline_releases_everything_and_a_deleted_peer_is_forgotten() {
        let mut sync = NodeSync::default();
        sync.begin_accepting(id(2), true);
        sync.begin_accepting(id(3), true);
        sync.begin_undialing(id(4), true);
        sync.forget(id(3));
        assert!(!sync.knows(id(3)));
        assert_eq!(sync.confirm_all(), [id(2), id(4)].into());
        assert!(!sync.is_waiting());
    }
}
