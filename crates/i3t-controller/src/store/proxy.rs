//! 代理：在节点 A 上监听，经出口节点 B 出去，认一对用户名和密码。
//!
//! 用户名和密码原样存着：管理员要能再看、再复制（数据库文件只有属主读得了，管理接口要管理令牌）。
//! 节点拿到的只是按这一行的盐算出来的校验值（[`ProxyRecord::verifier`]）。

use std::net::SocketAddr;

use i3t_model::admin::NetworkId;
use i3t_model::netmap::{ListenerProtocol, Verifier};
use i3t_proto::NodeId;
use i3t_proto::flow::Class;
use ipnet::IpNet;
use rusqlite::{OptionalExtension, Row, named_params, params};

use super::codec::{Json, SqlNetwork, SqlNode, Text, row_id};
use super::{Store, StoreError};

/// 存着的一个代理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRecord {
    /// 新建时不看它，由数据库分。
    pub id: u32,
    /// 监听的节点。
    pub node: NodeId,
    pub name: String,
    pub bind: SocketAddr,
    pub protocol: ListenerProtocol,
    /// 只有升级前就有、当时没有出口的代理，和出口节点被删掉了的代理是 `None`。
    pub exit: Option<NodeId>,
    /// 都是空的表示不认证。
    pub username: String,
    pub password: String,
    /// 算校验值用的盐，用户名或者密码变了就换一个。
    pub salt: [u8; Verifier::SALT_LEN],
    pub enabled: bool,
    pub allow_from: Vec<IpNet>,
    pub class: Option<Class>,
    pub optimistic: Option<bool>,
    /// 这个代理的 TCP 流走 ice3proxy（白皮书 §9.7 第 9 条）。
    pub lane: bool,
}

impl ProxyRecord {
    /// 节点核对用户名和密码用的校验值；不认证的代理没有。
    #[must_use]
    pub fn verifier(&self) -> Option<Verifier> {
        let anonymous = self.username.is_empty() && self.password.is_empty();
        (!anonymous).then(|| Verifier::derive(self.salt, self.username.as_bytes(), self.password.as_bytes()))
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            node: row.get::<_, SqlNode>("node_id")?.0,
            name: row.get("name")?,
            bind: row.get::<_, Text<SocketAddr>>("bind")?.0,
            protocol: row.get::<_, Json<ListenerProtocol>>("protocol")?.0,
            exit: row.get::<_, Option<SqlNode>>("exit_id")?.map(|node| node.0),
            username: row.get("username")?,
            password: row.get("password")?,
            salt: row.get("salt")?,
            enabled: row.get("enabled")?,
            allow_from: row.get::<_, Json<Vec<IpNet>>>("allow_from")?.0,
            class: row.get::<_, Option<Json<Class>>>("class")?.map(|json| json.0),
            optimistic: row.get("optimistic")?,
            lane: row.get("lane")?,
        })
    }
}

impl Store {
    /// 新建一个代理，返回它的编号。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括同一个节点上已经有同名的代理。
    pub fn insert_proxy(&self, proxy: &ProxyRecord) -> Result<u32, StoreError> {
        self.conn.execute(
            "INSERT INTO proxies (node_id, name, bind, protocol, exit_id, username, password, salt, enabled, allow_from, class, optimistic, lane)
             VALUES (:node, :name, :bind, :protocol, :exit, :username, :password, :salt, :enabled, :allow_from, :class, :optimistic, :lane)",
            named_params! {
                ":node": SqlNode(proxy.node),
                ":name": proxy.name,
                ":bind": Text(proxy.bind),
                ":protocol": Json(proxy.protocol),
                ":exit": proxy.exit.map(SqlNode),
                ":username": proxy.username,
                ":password": proxy.password,
                ":salt": &proxy.salt[..],
                ":enabled": proxy.enabled,
                ":allow_from": Json(&proxy.allow_from),
                ":class": proxy.class.map(Json),
                ":optimistic": proxy.optimistic,
                ":lane": proxy.lane,
            },
        )?;
        Ok(row_id(self.conn.last_insert_rowid())?)
    }

    /// 一个网络里全部节点上的代理，按编号排。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn proxies(&self, network: NetworkId) -> Result<Vec<ProxyRecord>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT proxies.* FROM proxies JOIN nodes ON nodes.id = proxies.node_id WHERE nodes.network_id = ?1 ORDER BY proxies.id",
        )?;
        let rows = statement.query_map(params![SqlNetwork(network)], ProxyRecord::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 只在这个网络里找。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn proxy(&self, network: NetworkId, id: u32) -> Result<Option<ProxyRecord>, StoreError> {
        let sql = "SELECT proxies.* FROM proxies JOIN nodes ON nodes.id = proxies.node_id WHERE nodes.network_id = ?1 AND proxies.id = ?2";
        Ok(self.conn.query_row(sql, params![SqlNetwork(network), id], ProxyRecord::from_row).optional()?)
    }

    /// 整个替换编号是 `proxy.id` 的代理，包括它监听在哪个节点上。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括和同一个节点上的另一个代理重名。
    pub fn update_proxy(&self, proxy: &ProxyRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE proxies SET node_id = :node, name = :name, bind = :bind, protocol = :protocol, exit_id = :exit, username = :username,
                                password = :password, salt = :salt, enabled = :enabled, allow_from = :allow_from, class = :class,
                                optimistic = :optimistic, lane = :lane
             WHERE id = :id",
            named_params! {
                ":id": proxy.id,
                ":node": SqlNode(proxy.node),
                ":name": proxy.name,
                ":bind": Text(proxy.bind),
                ":protocol": Json(proxy.protocol),
                ":exit": proxy.exit.map(SqlNode),
                ":username": proxy.username,
                ":password": proxy.password,
                ":salt": &proxy.salt[..],
                ":enabled": proxy.enabled,
                ":allow_from": Json(&proxy.allow_from),
                ":class": proxy.class.map(Json),
                ":optimistic": proxy.optimistic,
                ":lane": proxy.lane,
            },
        )?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_proxy(&self, id: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM proxies WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::store_with_nodes;

    fn record(node: NodeId, name: &str, exit: Option<NodeId>) -> ProxyRecord {
        ProxyRecord {
            id: 0,
            node,
            name: name.to_owned(),
            bind: "192.168.1.5:1080".parse().unwrap(),
            protocol: ListenerProtocol::Mixed,
            exit,
            username: "alice".to_owned(),
            password: "s3cret".to_owned(),
            salt: [5; Verifier::SALT_LEN],
            enabled: true,
            allow_from: vec!["192.168.1.0/24".parse().unwrap()],
            class: Some(Class::Reliable),
            optimistic: Some(false),
            lane: true,
        }
    }

    #[test]
    fn proxies_round_trip_and_names_are_unique_per_node() {
        let (store, network, nodes) = store_with_nodes(3);
        let id = store.insert_proxy(&record(nodes[0], "lan", Some(nodes[1]))).unwrap();
        assert_eq!(store.proxy(network, id).unwrap(), Some(ProxyRecord { id, ..record(nodes[0], "lan", Some(nodes[1])) }));
        assert!(store.insert_proxy(&record(nodes[0], "lan", Some(nodes[2]))).is_err(), "同一个节点上代理的名字不能重复");
        store.insert_proxy(&record(nodes[1], "lan", Some(nodes[2]))).unwrap();
        assert_eq!(store.proxies(network).unwrap().len(), 2);

        let mut replacement =
            ProxyRecord { id, protocol: ListenerProtocol::Http, enabled: false, class: None, lane: false, ..record(nodes[2], "b", None) };
        replacement.username.clear();
        replacement.password.clear();
        store.update_proxy(&replacement).unwrap();
        assert_eq!(store.proxy(network, id).unwrap(), Some(replacement));
        store.delete_proxy(id).unwrap();
        assert_eq!(store.proxy(network, id).unwrap(), None);
    }

    #[test]
    fn the_verifier_follows_the_salt_and_an_anonymous_proxy_has_none() {
        let proxy = record(NodeId::new(1).unwrap(), "p", None);
        let verifier = proxy.verifier().unwrap();
        assert_eq!(verifier, Verifier::derive([5; Verifier::SALT_LEN], b"alice", b"s3cret"));
        assert_eq!(ProxyRecord { username: String::new(), password: String::new(), ..proxy }.verifier(), None);
    }

    #[test]
    fn deleting_a_node_removes_its_proxies_and_leaves_the_ones_that_used_it_without_an_exit() {
        let (store, network, nodes) = store_with_nodes(3);
        store.insert_proxy(&record(nodes[0], "own", Some(nodes[1]))).unwrap();
        let via = store.insert_proxy(&record(nodes[1], "via", Some(nodes[0]))).unwrap();
        store.delete_node(nodes[0]).unwrap();
        let left = store.proxies(network).unwrap();
        assert_eq!(left.iter().map(|proxy| proxy.id).collect::<Vec<_>>(), [via]);
        assert_eq!((left[0].exit, left[0].enabled), (None, false), "出口没了：停用，等选新的出口");
    }

    #[test]
    fn repointing_moves_proxies_and_exit_references_to_the_successor() {
        let (store, network, nodes) = store_with_nodes(3);
        let (old, new, other) = (nodes[0], nodes[1], nodes[2]);
        let moved = store.insert_proxy(&record(old, "own", Some(other))).unwrap();
        let referring = store.insert_proxy(&record(other, "via", Some(old))).unwrap();
        store.repoint_node(old, new).unwrap();
        store.delete_node(old).unwrap();
        assert_eq!(store.proxy(network, moved).unwrap().unwrap().node, new);
        let referring = store.proxy(network, referring).unwrap().unwrap();
        assert_eq!((referring.exit, referring.enabled), (Some(new), true));
    }
}
