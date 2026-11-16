//! 代理监听器。存的就是管理 API 里的 [`Listener`]。

use std::net::SocketAddr;

use i3t_model::admin::{Listener, ListenerInput, NetworkId};
use i3t_model::netmap::{self, DefaultAction, ListenerProtocol};
use i3t_proto::NodeId;
use ipnet::IpNet;
use rusqlite::{OptionalExtension, Row, named_params, params};

use super::codec::{Json, SqlNetwork, SqlNode, Text, row_id};
use super::{Store, StoreError};

const LOCAL: &str = "local";
const REJECT: &str = "reject";
const EXIT: &str = "exit";

/// 默认动作在表里占两列：动作的种类，和走固定出口时的节点编号。
fn default_action_columns(action: Option<DefaultAction>) -> (Option<&'static str>, Option<SqlNode>) {
    match action {
        None => (None, None),
        Some(DefaultAction::Local) => (Some(LOCAL), None),
        Some(DefaultAction::Reject) => (Some(REJECT), None),
        Some(DefaultAction::Exit { node }) => (Some(EXIT), Some(SqlNode(node))),
    }
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Listener> {
    let kind: Option<String> = row.get("default_action")?;
    let exit = row.get::<_, Option<SqlNode>>("default_exit")?.map(|node| node.0);
    let default_action = match (kind.as_deref(), exit) {
        (Some(LOCAL), _) => Some(DefaultAction::Local),
        (Some(REJECT), _) => Some(DefaultAction::Reject),
        (Some(EXIT), Some(node)) => Some(DefaultAction::Exit { node }),
        // 出口节点被删掉时两列一起清空；剩下的写法都按"没有设置"读。
        _ => None,
    };
    Ok(Listener {
        id: row.get("id")?,
        node: row.get::<_, SqlNode>("node_id")?.0,
        config: netmap::Listener {
            name: row.get("name")?,
            bind: row.get::<_, Text<SocketAddr>>("bind")?.0,
            protocol: row.get::<_, Json<ListenerProtocol>>("protocol")?.0,
            allow_from: row.get::<_, Json<Vec<IpNet>>>("allow_from")?.0,
            require_credential: row.get("require_credential")?,
            default_action,
            fixed_exit: row.get::<_, Option<SqlNode>>("fixed_exit")?.map(|node| node.0),
        },
    })
}

impl Store {
    /// # Errors
    ///
    /// 写数据库失败，包括同一个节点上已经有同名的监听器。
    pub fn insert_listener(&self, input: &ListenerInput) -> Result<u32, StoreError> {
        let config = &input.config;
        let (action, exit) = default_action_columns(config.default_action);
        self.conn.execute(
            "INSERT INTO listeners (node_id, name, bind, protocol, allow_from, require_credential, default_action, default_exit, fixed_exit)
             VALUES (:node, :name, :bind, :protocol, :allow_from, :require_credential, :action, :exit, :fixed_exit)",
            named_params! {
                ":node": SqlNode(input.node),
                ":name": config.name,
                ":bind": Text(config.bind),
                ":protocol": Json(config.protocol),
                ":allow_from": Json(&config.allow_from),
                ":require_credential": config.require_credential,
                ":action": action,
                ":exit": exit,
                ":fixed_exit": config.fixed_exit.map(SqlNode),
            },
        )?;
        Ok(row_id(self.conn.last_insert_rowid())?)
    }

    /// 一个网络里全部节点上的监听器，按编号排。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn listeners(&self, network: NetworkId) -> Result<Vec<Listener>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT listeners.* FROM listeners JOIN nodes ON nodes.id = listeners.node_id WHERE nodes.network_id = ?1 ORDER BY listeners.id",
        )?;
        let rows = statement.query_map(params![SqlNetwork(network)], from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 只在这个网络里找。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn listener(&self, network: NetworkId, id: u32) -> Result<Option<Listener>, StoreError> {
        let sql = "SELECT listeners.* FROM listeners JOIN nodes ON nodes.id = listeners.node_id
                   WHERE nodes.network_id = ?1 AND listeners.id = ?2";
        Ok(self.conn.query_row(sql, params![SqlNetwork(network), id], from_row).optional()?)
    }

    /// 整个替换一个监听器，包括它开在哪个节点上。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括和同一个节点上的另一个监听器重名。
    pub fn update_listener(&self, id: u32, input: &ListenerInput) -> Result<(), StoreError> {
        let config = &input.config;
        let (action, exit) = default_action_columns(config.default_action);
        self.conn.execute(
            "UPDATE listeners SET node_id = :node, name = :name, bind = :bind, protocol = :protocol, allow_from = :allow_from,
                                  require_credential = :require_credential, default_action = :action, default_exit = :exit,
                                  fixed_exit = :fixed_exit
             WHERE id = :id",
            named_params! {
                ":id": id,
                ":node": SqlNode(input.node),
                ":name": config.name,
                ":bind": Text(config.bind),
                ":protocol": Json(config.protocol),
                ":allow_from": Json(&config.allow_from),
                ":require_credential": config.require_credential,
                ":action": action,
                ":exit": exit,
                ":fixed_exit": config.fixed_exit.map(SqlNode),
            },
        )?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_listener(&self, id: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM listeners WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 把一个节点当出口用的监听器开在哪些节点上（固定出口或者默认动作指向它）。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn listener_nodes_using_exit(&self, exit: NodeId) -> Result<Vec<NodeId>, StoreError> {
        let mut statement = self.conn.prepare("SELECT DISTINCT node_id FROM listeners WHERE default_exit = ?1 OR fixed_exit = ?1")?;
        let rows = statement.query_map(params![SqlNode(exit)], |row| row.get::<_, SqlNode>(0).map(|node| node.0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::store_with_nodes;

    fn input(node: NodeId, name: &str, default_action: Option<DefaultAction>, fixed_exit: Option<NodeId>) -> ListenerInput {
        ListenerInput {
            node,
            config: netmap::Listener {
                name: name.to_owned(),
                bind: "192.168.1.5:1080".parse().unwrap(),
                protocol: ListenerProtocol::Socks5,
                allow_from: vec!["192.168.1.0/24".parse().unwrap()],
                require_credential: true,
                default_action,
                fixed_exit,
            },
        }
    }

    #[test]
    fn listeners_round_trip_with_every_default_action() {
        let (store, network, nodes) = store_with_nodes(2);
        let actions = [None, Some(DefaultAction::Local), Some(DefaultAction::Reject), Some(DefaultAction::Exit { node: nodes[1] })];
        for (index, action) in actions.into_iter().enumerate() {
            let new = input(nodes[0], &format!("l{index}"), action, Some(nodes[1]));
            let id = store.insert_listener(&new).unwrap();
            let stored = store.listener(network, id).unwrap().unwrap();
            assert_eq!(stored, Listener { id, node: nodes[0], config: new.config });
        }
        assert_eq!(store.listeners(network).unwrap().len(), 4);
        assert!(store.insert_listener(&input(nodes[0], "l0", None, None)).is_err(), "同一个节点上监听器的名字不能重复");
        assert_eq!(store.listener_nodes_using_exit(nodes[1]).unwrap(), vec![nodes[0]]);
    }

    #[test]
    fn updating_replaces_the_whole_listener() {
        let (store, network, nodes) = store_with_nodes(2);
        let id = store.insert_listener(&input(nodes[0], "a", Some(DefaultAction::Reject), None)).unwrap();
        let mut replacement = input(nodes[1], "b", Some(DefaultAction::Local), Some(nodes[0]));
        replacement.config.protocol = ListenerProtocol::Http;
        replacement.config.require_credential = false;
        store.update_listener(id, &replacement).unwrap();
        assert_eq!(store.listener(network, id).unwrap(), Some(Listener { id, node: nodes[1], config: replacement.config }));
        store.delete_listener(id).unwrap();
        assert_eq!(store.listener(network, id).unwrap(), None);
    }

    #[test]
    fn deleting_a_node_removes_its_listeners_and_clears_references_to_it() {
        let (store, network, nodes) = store_with_nodes(3);
        store.insert_listener(&input(nodes[0], "own", None, None)).unwrap();
        let uses_exit =
            store.insert_listener(&input(nodes[1], "via", Some(DefaultAction::Exit { node: nodes[0] }), Some(nodes[0]))).unwrap();
        store.delete_node(nodes[0]).unwrap();
        let left = store.listeners(network).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, uses_exit);
        assert_eq!(left[0].config.default_action, None);
        assert_eq!(left[0].config.fixed_exit, None);
    }

    #[test]
    fn repointing_moves_listeners_and_exit_references_to_the_successor() {
        let (store, network, nodes) = store_with_nodes(3);
        let (old, new, other) = (nodes[0], nodes[1], nodes[2]);
        let moved = store.insert_listener(&input(old, "own", None, None)).unwrap();
        let referring = store.insert_listener(&input(other, "via", Some(DefaultAction::Exit { node: old }), Some(old))).unwrap();
        store.repoint_node(old, new).unwrap();
        store.delete_node(old).unwrap();
        assert_eq!(store.listener(network, moved).unwrap().unwrap().node, new);
        let referring = store.listener(network, referring).unwrap().unwrap();
        assert_eq!(referring.config.default_action, Some(DefaultAction::Exit { node: new }));
        assert_eq!(referring.config.fixed_exit, Some(new));
    }
}
