//! 节点对设置：连接常驻和拥塞控制。一对节点只有一行，编号小的在前。
//!
//! ice3proxy 以前也是这里的一项，现在是代理的选项（`proxies.lane`），见迁移 `0016_proxy_lane.sql`。

use i3t_model::admin::{NetworkId, PairSetting};
use i3t_model::netmap::Congestion;
use i3t_proto::NodeId;
use rusqlite::{OptionalExtension, Row, params};

use super::codec::{SqlNetwork, SqlNode};
use super::{Store, StoreError};

fn from_row(row: &Row<'_>) -> rusqlite::Result<PairSetting> {
    let congestion: Option<String> = row.get("congestion")?;
    Ok(PairSetting {
        a: row.get::<_, SqlNode>("node_a")?.0,
        b: row.get::<_, SqlNode>("node_b")?.0,
        keep_alive: row.get("keep_alive")?,
        // 只有本程序写这一列；读到不认识的值（比本程序新的版本写的）按没有设置（节点的默认）。
        congestion: congestion.and_then(|text| text.parse().ok()),
    })
}

fn ordered(a: NodeId, b: NodeId) -> (SqlNode, SqlNode) {
    (SqlNode(a.min(b)), SqlNode(a.max(b)))
}

/// 一对节点的各项设置（不含两个节点是谁）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PairOptions {
    pub keep_alive: bool,
    pub congestion: Option<Congestion>,
}

impl Store {
    /// 写下一对节点的设置，已经有就覆盖。两个节点的先后顺序无所谓。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括两个编号相同。
    pub fn set_pair(&self, a: NodeId, b: NodeId, setting: PairOptions) -> Result<(), StoreError> {
        let (low, high) = ordered(a, b);
        let PairOptions { keep_alive, congestion } = setting;
        self.conn.execute(
            "INSERT INTO pair_settings (node_a, node_b, keep_alive, congestion) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (node_a, node_b) DO UPDATE SET keep_alive = excluded.keep_alive, congestion = excluded.congestion",
            params![low, high, keep_alive, congestion.map(Congestion::as_str)],
        )?;
        Ok(())
    }

    /// 一对节点的设置；没有设置时是 `None`。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn pair(&self, a: NodeId, b: NodeId) -> Result<Option<PairSetting>, StoreError> {
        let (low, high) = ordered(a, b);
        Ok(self.conn.query_row("SELECT * FROM pair_settings WHERE node_a = ?1 AND node_b = ?2", params![low, high], from_row).optional()?)
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

    fn options(keep_alive: bool, congestion: Option<Congestion>) -> PairOptions {
        PairOptions { keep_alive, congestion }
    }

    fn setting(a: NodeId, b: NodeId, options: PairOptions) -> PairSetting {
        PairSetting { a, b, keep_alive: options.keep_alive, congestion: options.congestion }
    }

    #[test]
    fn a_pair_has_one_row_whichever_way_round_it_is_named() {
        let (store, network, nodes) = store_with_nodes(2);
        let first = options(true, None);
        store.set_pair(nodes[1], nodes[0], first).unwrap();
        assert_eq!(store.pairs(network).unwrap(), vec![setting(nodes[0], nodes[1], first)]);
        let second = options(false, Some(Congestion::Cubic));
        store.set_pair(nodes[0], nodes[1], second).unwrap();
        let expected = setting(nodes[0], nodes[1], second);
        assert_eq!(store.pairs(network).unwrap(), vec![expected]);
        assert_eq!(store.pair(nodes[1], nodes[0]).unwrap(), Some(expected));
        assert!(store.set_pair(nodes[0], nodes[0], first).is_err(), "一个节点和自己不成一对");
        assert!(store.delete_pair(nodes[1], nodes[0]).unwrap());
        assert!(!store.delete_pair(nodes[1], nodes[0]).unwrap());
    }

    #[test]
    fn repointing_carries_pair_settings_over_to_the_successor() {
        let (store, network, nodes) = store_with_nodes(4);
        let (old, new, low_peer, high_peer) = (nodes[1], nodes[2], nodes[0], nodes[3]);
        let cubic = options(true, Some(Congestion::Cubic));
        let plain = options(true, None);
        store.set_pair(old, low_peer, cubic).unwrap();
        store.set_pair(old, high_peer, plain).unwrap();
        // 旧节点和继任者之间的设置没有意义，不带过去。
        store.set_pair(old, new, options(false, Some(Congestion::Bbr))).unwrap();
        store.repoint_node(old, new).unwrap();
        store.delete_node(old).unwrap();
        assert_eq!(store.pairs(network).unwrap(), vec![setting(low_peer, new, cubic), setting(new, high_peer, plain)]);
    }
}
