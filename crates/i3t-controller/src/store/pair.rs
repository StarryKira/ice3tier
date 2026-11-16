//! 节点对设置。一对节点只有一行，编号小的在前。

use i3t_model::admin::{NetworkId, PairSetting};
use i3t_proto::NodeId;
use rusqlite::{Row, params};

use super::codec::{SqlNetwork, SqlNode};
use super::{Store, StoreError};

fn from_row(row: &Row<'_>) -> rusqlite::Result<PairSetting> {
    Ok(PairSetting { a: row.get::<_, SqlNode>("node_a")?.0, b: row.get::<_, SqlNode>("node_b")?.0, keep_alive: row.get("keep_alive")? })
}

fn ordered(a: NodeId, b: NodeId) -> (SqlNode, SqlNode) {
    (SqlNode(a.min(b)), SqlNode(a.max(b)))
}

impl Store {
    /// 写下一对节点的设置，已经有就覆盖。两个节点的先后顺序无所谓。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括两个编号相同。
    pub fn set_pair(&self, a: NodeId, b: NodeId, keep_alive: bool) -> Result<(), StoreError> {
        let (low, high) = ordered(a, b);
        self.conn.execute(
            "INSERT INTO pair_settings (node_a, node_b, keep_alive) VALUES (?1, ?2, ?3)
             ON CONFLICT (node_a, node_b) DO UPDATE SET keep_alive = excluded.keep_alive",
            params![low, high, keep_alive],
        )?;
        Ok(())
    }

    /// 一个网络里全部的节点对设置。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn pairs(&self, network: NetworkId) -> Result<Vec<PairSetting>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT pair_settings.* FROM pair_settings JOIN nodes ON nodes.id = pair_settings.node_a
             WHERE nodes.network_id = ?1 ORDER BY node_a, node_b",
        )?;
        let rows = statement.query_map(params![SqlNetwork(network)], from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 返回这一对之前有没有设置。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_pair(&self, a: NodeId, b: NodeId) -> Result<bool, StoreError> {
        let (low, high) = ordered(a, b);
        Ok(self.conn.execute("DELETE FROM pair_settings WHERE node_a = ?1 AND node_b = ?2", params![low, high])? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::store_with_nodes;

    #[test]
    fn a_pair_has_one_row_whichever_way_round_it_is_named() {
        let (store, network, nodes) = store_with_nodes(2);
        store.set_pair(nodes[1], nodes[0], true).unwrap();
        assert_eq!(store.pairs(network).unwrap(), vec![PairSetting { a: nodes[0], b: nodes[1], keep_alive: true }]);
        store.set_pair(nodes[0], nodes[1], false).unwrap();
        assert_eq!(store.pairs(network).unwrap(), vec![PairSetting { a: nodes[0], b: nodes[1], keep_alive: false }]);
        assert!(store.set_pair(nodes[0], nodes[0], true).is_err(), "一个节点和自己不成一对");
        assert!(store.delete_pair(nodes[1], nodes[0]).unwrap());
        assert!(!store.delete_pair(nodes[1], nodes[0]).unwrap());
    }

    #[test]
    fn repointing_carries_pair_settings_over_to_the_successor() {
        let (store, network, nodes) = store_with_nodes(4);
        let (old, new, low_peer, high_peer) = (nodes[1], nodes[2], nodes[0], nodes[3]);
        store.set_pair(old, low_peer, true).unwrap();
        store.set_pair(old, high_peer, true).unwrap();
        // 旧节点和继任者之间的设置没有意义，不带过去。
        store.set_pair(old, new, true).unwrap();
        store.repoint_node(old, new).unwrap();
        store.delete_node(old).unwrap();
        assert_eq!(
            store.pairs(network).unwrap(),
            vec![PairSetting { a: low_peer, b: new, keep_alive: true }, PairSetting { a: new, b: high_peer, keep_alive: true }]
        );
    }
}
