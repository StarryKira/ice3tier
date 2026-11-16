//! 规则、netmap 和下发顺序的测试。都跑在 [`super::testing::Lab`] 上，不经过网络。

mod groups;
mod limits;
mod model;
mod netmaps;
mod ordering;
mod routes;
mod update;

use i3t_model::Netmap;
use i3t_model::admin::{CreateRule, Direction, NetworkId, UpdateNode};
use i3t_proto::NodeId;

use super::testing::Lab;

/// netmap 里的对端和两个方向的关系：`(对端, 它能访问我, 我能访问它)`。
fn relations(netmap: &Netmap) -> Vec<(NodeId, bool, bool)> {
    netmap.peers.iter().map(|peer| (peer.id, peer.inbound, peer.outbound)).collect()
}

fn peer_ids(netmap: &Netmap) -> Vec<NodeId> {
    netmap.peers.iter().map(|peer| peer.id).collect()
}

/// 去掉网络自带的 `All <-> All`，之后谁能访问谁全看测试自己加的规则。
fn drop_default_rule(lab: &Lab, network: NetworkId) {
    for rule in lab.controller.rules(network).unwrap() {
        lab.controller.delete_rule(network, rule.id).unwrap();
    }
}

/// 加一条规则，两头按分组的名字写（没有的先建出来，`all` 是 `All`）。
fn add_rule(lab: &Lab, network: NetworkId, src: &str, dst: &str, direction: Direction) -> u32 {
    let (src, dst) = (lab.group(network, src), lab.group(network, dst));
    let request = CreateRule { name: String::new(), src, dst, direction, enabled: true };
    lab.controller.create_rule(network, &request).unwrap().id
}

fn rename(lab: &Lab, network: NetworkId, node: NodeId, name: &str) {
    lab.controller.update_node(network, node, &UpdateNode { name: Some(name.to_owned()), ..UpdateNode::default() }).unwrap();
}
