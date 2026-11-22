//! 节点。

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use i3t_model::admin::{GroupId, NetworkId, NodeRef, RelaySettings, Timestamp, UpdateFailure};
use i3t_model::control::{Declared, SelfUpdate, TryOutcome};
use i3t_model::netmap::{Attachment, ExitRole, Profile, Reach, RoutingMode};
use i3t_proto::{NodeId, PublicKey};
use rusqlite::{OptionalExtension, Row, named_params, params};
use serde::{Deserialize, Serialize};

use super::codec::{Json, SqlIp, SqlKey, SqlNetwork, SqlNode, Text, Unsigned};
use super::{DIGEST_LEN, Store, StoreError};

/// 读节点时顺带取出它所在的分组（成员表里的分组编号，JSON 数组）。
const SELECT: &str = "SELECT nodes.*, (SELECT json_group_array(group_id) FROM group_members WHERE node_id = nodes.id) AS groups FROM nodes";

/// [`Store::exits_among`]：`?1` 是编号的 JSON 数组。从数组出发按主键找，问几个查几行。
/// `CROSS JOIN` 让 SQLite 照写的顺序连：库里没有统计信息时，它会以为先按网络的索引读整个网络更省。
const EXITS_AMONG: &str = "SELECT nodes.id FROM json_each(?1) AS asked CROSS JOIN nodes ON nodes.id = asked.value \
                           WHERE nodes.network_id = ?2 AND nodes.exit IS NOT NULL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRecord {
    pub id: NodeId,
    pub network: NetworkId,
    pub key: PublicKey,
    pub name: String,
    pub virtual_ip: Ipv4Addr,
    /// 它所在的分组。内置分组 `All` 不在里面：每个节点本来就在。
    /// 读的时候从成员表里取；写的时候用 [`Store::set_node_groups`]，[`Store::update_node`] 不写它。
    pub groups: BTreeSet<GroupId>,
    /// 节点自己声明的"可以承担"（relay、出口）。
    pub declared: Declared,
    /// 管理员的开关：可以作为端口转发的目标。节点不声明它。
    pub forward_target: bool,
    /// 管理员启用了 relay 时有值。
    pub relay: Option<RelaySettings>,
    /// 管理员启用了出口时有值。
    pub exit: Option<ExitRole>,
    pub dial_out_only: bool,
    pub routing: Option<RoutingMode>,
    pub reach: Option<Reach>,
    pub pinned_relays: Vec<NodeId>,
    /// 管理员给它选的性能档位；没有选时是 `None`，节点用本机配置文件里的。
    pub profile: Option<Profile>,
    /// 管理员的开关：自动更新这个节点。新节点是开着的。
    pub auto_update: bool,
    /// 节点最近上报的候选端点。
    pub endpoints: Vec<SocketAddr>,
    /// 节点最近上报的网卡地址；旧版本的节点不报，是 `None`。
    pub local_addresses: Option<Vec<IpAddr>>,
    /// controller 看到的来源 IP 加上节点自报的 UDP 端口。
    pub observed: Option<SocketAddr>,
    pub attached: Vec<Attachment>,
    /// 节点最近一次试各个 relay 的结果，按 relay 编号排，最多 [`MAX_RELAY_TRIES`](i3t_model::control::MAX_RELAY_TRIES) 项。
    pub relay_reach: Vec<ReachRecord>,
    pub platform: String,
    pub version: String,
    /// 节点上报的主机名；旧版本的节点不报，是空的。
    pub hostname: String,
    /// 节点自己报的能不能自己更新。`None` 表示它没有报过。
    pub self_update: Option<SelfUpdate>,
    /// 最近一次归因于版本的更新失败。
    pub update_failure: Option<UpdateFailure>,
    pub created_at: Timestamp,
    pub last_seen_at: Option<Timestamp>,
    pub netmap: NetmapState,
    /// 注册时所用令牌的哈希。回答丢了的节点拿同一个令牌重试时靠它认出来。
    pub enrolled_with: Option<[u8; DIGEST_LEN]>,
}

/// 节点最近一次试一个 relay 的结果，controller 存着的样子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachRecord {
    pub relay: NodeId,
    pub outcome: TryOutcome,
    /// 什么时候试的，controller 的时间。
    pub at: Timestamp,
    /// 这个 relay 看到的节点的 IP。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_as: Option<IpAddr>,
}

/// 一个节点的 netmap 下发到了哪一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetmapState {
    /// 当前内容的版本号。内容变了才加一。
    pub version: u64,
    /// 当前内容（不含版本号）的摘要，用来判断内容变没变。
    pub digest: Option<[u8; DIGEST_LEN]>,
    /// 最近一次发给节点的版本。
    pub sent: u64,
    /// 节点确认已经应用的版本。
    pub acked: u64,
}

impl NodeRecord {
    /// 管理 API 里指代它的写法：编号、现在的名字和虚拟 IP。
    #[must_use]
    pub fn to_ref(&self) -> NodeRef {
        NodeRef { id: self.id, name: self.name.clone(), virtual_ip: self.virtual_ip }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get::<_, SqlNode>("id")?.0,
            network: row.get::<_, SqlNetwork>("network_id")?.0,
            key: row.get::<_, SqlKey>("public_key")?.0,
            name: row.get("name")?,
            virtual_ip: row.get::<_, SqlIp>("virtual_ip")?.0,
            groups: row.get::<_, Json<BTreeSet<GroupId>>>("groups")?.0,
            declared: row.get::<_, Json<Declared>>("declared")?.0,
            forward_target: row.get("forward_target")?,
            relay: row.get::<_, Option<Json<RelaySettings>>>("relay")?.map(|json| json.0),
            exit: row.get::<_, Option<Json<ExitRole>>>("exit")?.map(|json| json.0),
            dial_out_only: row.get("dial_out_only")?,
            routing: row.get::<_, Option<Json<RoutingMode>>>("routing")?.map(|json| json.0),
            reach: row.get::<_, Option<Json<Reach>>>("reach")?.map(|json| json.0),
            pinned_relays: row.get::<_, Json<Vec<NodeId>>>("pinned_relays")?.0,
            profile: row.get::<_, Option<Json<Profile>>>("profile")?.map(|json| json.0),
            auto_update: row.get("auto_update")?,
            endpoints: row.get::<_, Json<Vec<SocketAddr>>>("endpoints")?.0,
            local_addresses: row.get::<_, Option<Json<Vec<IpAddr>>>>("local_addresses")?.map(|json| json.0),
            observed: row.get::<_, Option<Text<SocketAddr>>>("observed")?.map(|text| text.0),
            attached: row.get::<_, Json<Vec<Attachment>>>("attached")?.0,
            relay_reach: row.get::<_, Json<Vec<ReachRecord>>>("relay_reach")?.0,
            platform: row.get("platform")?,
            version: row.get("version")?,
            hostname: row.get("hostname")?,
            self_update: row.get::<_, Option<Json<SelfUpdate>>>("self_update")?.map(|json| json.0),
            update_failure: row.get::<_, Option<Json<UpdateFailure>>>("update_failure")?.map(|json| json.0),
            created_at: row.get::<_, Unsigned>("created_at")?.0,
            last_seen_at: row.get::<_, Option<Unsigned>>("last_seen_at")?.map(|at| at.0),
            netmap: NetmapState {
                version: row.get::<_, Unsigned>("netmap_version")?.0,
                digest: row.get("netmap_digest")?,
                sent: row.get::<_, Unsigned>("sent_version")?.0,
                acked: row.get::<_, Unsigned>("acked_version")?.0,
            },
            enrolled_with: row.get("enrolled_with")?,
        })
    }
}

/// 注册一个新节点时要写下的东西。其余的列从空值开始。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNode {
    pub network: NetworkId,
    pub key: PublicKey,
    /// 留空时用 `node-<编号>`：编号要插入之后才知道。
    pub name: Option<String>,
    pub virtual_ip: Ipv4Addr,
    /// 它一加入就在的分组（注册令牌预设的）。
    pub groups: BTreeSet<GroupId>,
    pub declared: Declared,
    pub platform: String,
    pub version: String,
    pub created_at: Timestamp,
    pub enrolled_with: Option<[u8; DIGEST_LEN]>,
}

impl Store {
    /// # Errors
    ///
    /// 写数据库失败，包括公钥、虚拟 IP 或名字和已有的节点重复。
    pub fn insert_node(&self, new: &NewNode) -> Result<NodeId, StoreError> {
        // 名字有唯一约束，编号又要插入之后才知道，所以先占个位，再改成默认名字。
        // 占位的名字带冒号，合法的名字里没有冒号，所以不会和任何节点撞。
        let placeholder = format!("new:{}", new.key);
        self.conn.execute(
            "INSERT INTO nodes (network_id, public_key, name, virtual_ip, declared, dial_out_only, pinned_relays, endpoints,
                                attached, platform, version, created_at, netmap_version, sent_version, acked_version, enrolled_with)
             VALUES (:network, :key, :name, :ip, :declared, 0, '[]', '[]', '[]', :platform, :version, :created_at, 0, 0, 0, :enrolled_with)",
            named_params! {
                ":network": SqlNetwork(new.network),
                ":key": SqlKey(new.key),
                ":name": new.name.as_deref().unwrap_or(&placeholder),
                ":ip": SqlIp(new.virtual_ip),
                ":declared": Json(new.declared),
                ":platform": new.platform,
                ":version": new.version,
                ":created_at": Unsigned(new.created_at),
                ":enrolled_with": new.enrolled_with,
            },
        )?;
        let raw = self.conn.last_insert_rowid();
        let id = u32::try_from(raw).ok().and_then(NodeId::new).ok_or(rusqlite::Error::IntegralValueOutOfRange(0, raw))?;
        if new.name.is_none() {
            self.conn.execute("UPDATE nodes SET name = ?2 WHERE id = ?1", params![SqlNode(id), format!("node-{id}")])?;
        }
        self.set_node_groups(id, &new.groups)?;
        Ok(id)
    }

    /// # Errors
    ///
    /// 读数据库失败。
    pub fn node(&self, id: NodeId) -> Result<Option<NodeRecord>, StoreError> {
        Ok(self.conn.query_row(&format!("{SELECT} WHERE id = ?1"), params![SqlNode(id)], NodeRecord::from_row).optional()?)
    }

    /// 按公钥找节点，不分网络：一把公钥在整个 controller 里只对应一个节点。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn node_by_key(&self, key: &PublicKey) -> Result<Option<NodeRecord>, StoreError> {
        let sql = format!("{SELECT} WHERE public_key = ?1");
        Ok(self.conn.query_row(&sql, params![SqlKey(*key)], NodeRecord::from_row).optional()?)
    }

    /// 一个网络里的全部节点，按编号排。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn nodes(&self, network: NetworkId) -> Result<Vec<NodeRecord>, StoreError> {
        let mut statement = self.conn.prepare(&format!("{SELECT} WHERE network_id = ?1 ORDER BY id"))?;
        let rows = statement.query_map(params![SqlNetwork(network)], NodeRecord::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 全部启用了 relay 的节点：所在的网络和节点编号。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn relay_nodes(&self) -> Result<Vec<(NetworkId, NodeId)>, StoreError> {
        let mut statement = self.conn.prepare("SELECT network_id, id FROM nodes WHERE relay IS NOT NULL ORDER BY id")?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, SqlNetwork>(0)?.0, row.get::<_, SqlNode>(1)?.0)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `ids` 里哪些是 `network` 里启用了出口的节点。一条语句按编号查，不读整个网络。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn exits_among(&self, network: NetworkId, ids: impl IntoIterator<Item = NodeId>) -> Result<BTreeSet<NodeId>, StoreError> {
        let ids: Vec<u32> = ids.into_iter().map(NodeId::get).collect();
        if ids.is_empty() {
            return Ok(BTreeSet::new());
        }
        let mut statement = self.conn.prepare_cached(EXITS_AMONG)?;
        let rows = statement.query_map(params![Json(&ids), SqlNetwork(network)], |row| Ok(row.get::<_, SqlNode>(0)?.0))?;
        Ok(rows.collect::<rusqlite::Result<BTreeSet<_>>>()?)
    }

    /// # Errors
    ///
    /// 读数据库失败。
    pub fn node_count(&self, network: Option<NetworkId>) -> Result<u32, StoreError> {
        let network = network.map(SqlNetwork);
        Ok(self.conn.query_row("SELECT COUNT(*) FROM nodes WHERE ?1 IS NULL OR network_id = ?1", params![network], |row| row.get(0))?)
    }

    /// 写回一个节点的身份和设置：名字、虚拟 IP、声明、启用的能力（含端口转发的目标）、各个开关，以及它自我介绍里的
    /// 平台、版本、主机名和能不能自己更新。
    /// 所在的分组、上报的内容、最近在线时间、netmap 的进度和更新失败的记录各有各的方法，不在这里写。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括名字或虚拟 IP 和别的节点重复。
    pub fn update_node(&self, node: &NodeRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE nodes SET name = :name, virtual_ip = :ip, declared = :declared, forward_target = :forward_target, relay = :relay, exit = :exit,
                              dial_out_only = :dial_out_only, routing = :routing, reach = :reach, pinned_relays = :pinned,
                              profile = :profile, auto_update = :auto_update, platform = :platform, version = :version,
                              hostname = :hostname, self_update = :self_update
             WHERE id = :id",
            named_params! {
                ":id": SqlNode(node.id),
                ":name": node.name,
                ":ip": SqlIp(node.virtual_ip),
                ":declared": Json(node.declared),
                ":forward_target": node.forward_target,
                ":relay": node.relay.as_ref().map(Json),
                ":exit": node.exit.as_ref().map(Json),
                ":dial_out_only": node.dial_out_only,
                ":routing": node.routing.map(Json),
                ":reach": node.reach.map(Json),
                ":pinned": Json(&node.pinned_relays),
                ":profile": node.profile.map(Json),
                ":auto_update": node.auto_update,
                ":platform": node.platform,
                ":version": node.version,
                ":hostname": node.hostname,
                ":self_update": node.self_update.map(Json),
            },
        )?;
        Ok(())
    }

    /// 只改节点"可以作为端口转发的目标"的开关。继任时用：那时这一行别的列刚被别处改过，不能拿手上的旧记录整个写回去。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_forward_target(&self, id: NodeId, on: bool) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET forward_target = ?2 WHERE id = ?1", params![SqlNode(id), on])?;
        Ok(())
    }

    /// 只改节点"自动更新"的开关。继任时用，理由同 [`Store::set_node_forward_target`]。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_auto_update(&self, id: NodeId, on: bool) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET auto_update = ?2 WHERE id = ?1", params![SqlNode(id), on])?;
        Ok(())
    }

    /// 记下（或者清掉）节点最近一次归因于版本的更新失败。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_update_failure(&self, id: NodeId, failure: Option<&UpdateFailure>) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET update_failure = ?2 WHERE id = ?1", params![SqlNode(id), failure.map(Json)])?;
        Ok(())
    }

    /// 记下节点上报的候选端点和挂靠关系。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_report(&self, id: NodeId, endpoints: &[SocketAddr], attached: &[Attachment]) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE nodes SET endpoints = ?2, attached = ?3 WHERE id = ?1",
            params![SqlNode(id), Json(endpoints), Json(attached)],
        )?;
        Ok(())
    }

    /// 记下节点试各个 relay 的结果。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_relay_reach(&self, id: NodeId, reach: &[ReachRecord]) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET relay_reach = ?2 WHERE id = ?1", params![SqlNode(id), Json(reach)])?;
        Ok(())
    }

    /// 记下节点上报的网卡地址。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_local_addresses(&self, id: NodeId, addresses: &[IpAddr]) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET local_addresses = ?2 WHERE id = ?1", params![SqlNode(id), Json(addresses)])?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_observed(&self, id: NodeId, observed: Option<SocketAddr>) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET observed = ?2 WHERE id = ?1", params![SqlNode(id), observed.map(Text)])?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_last_seen(&self, id: NodeId, at: Timestamp) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET last_seen_at = ?2 WHERE id = ?1", params![SqlNode(id), Unsigned(at)])?;
        Ok(())
    }

    /// 节点的 netmap 内容变了：记下新的版本号和摘要。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_netmap(&self, id: NodeId, version: u64, digest: &[u8; DIGEST_LEN]) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE nodes SET netmap_version = ?2, netmap_digest = ?3 WHERE id = ?1",
            params![SqlNode(id), Unsigned(version), digest],
        )?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_sent(&self, id: NodeId, version: u64) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET sent_version = ?2 WHERE id = ?1", params![SqlNode(id), Unsigned(version)])?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_node_acked(&self, id: NodeId, version: u64) -> Result<(), StoreError> {
        self.conn.execute("UPDATE nodes SET acked_version = ?2 WHERE id = ?1", params![SqlNode(id), Unsigned(version)])?;
        Ok(())
    }

    /// 删掉一个节点，连同它上面的代理、和它有关的端口转发、节点对设置、指名继任它的注册令牌。
    /// 别的节点上经它出去的代理没有了出口，停用，等管理员选一个新的。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_node(&self, id: NodeId) -> Result<(), StoreError> {
        self.conn.execute("UPDATE proxies SET exit_id = NULL, enabled = 0 WHERE exit_id = ?1", params![SqlNode(id)])?;
        self.conn.execute("DELETE FROM nodes WHERE id = ?1", params![SqlNode(id)])?;
        Ok(())
    }

    /// 让出一个节点的名字和虚拟 IP，好让继任者在同一个事务里接手。之后这一行读不出来，只能删掉。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn park_node(&self, id: NodeId) -> Result<(), StoreError> {
        // 两列都有唯一约束，所以换成不会和任何正常值相撞的东西：带冒号的名字，负数的地址。
        self.conn.execute("UPDATE nodes SET name = 'parked:' || id, virtual_ip = -id WHERE id = ?1", params![SqlNode(id)])?;
        Ok(())
    }

    /// 把指向 `old` 的东西改成指向 `new`：`old` 上的代理、经它出去的代理、它参与的端口转发和节点对设置，
    /// 它所在的分组（和 `new` 已经在的合并）。继任时用，之后 `old` 可以删掉而这些配置还在。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn repoint_node(&self, old: NodeId, new: NodeId) -> Result<(), StoreError> {
        let ids = params![SqlNode(old), SqlNode(new)];
        // 新节点自己已经有同名的代理、或者和同一个对端已经有设置时，保留新节点的，旧的随旧节点一起删掉。
        self.conn.execute("UPDATE OR IGNORE proxies SET node_id = ?2 WHERE node_id = ?1", ids)?;
        self.conn.execute("UPDATE proxies SET exit_id = ?2 WHERE exit_id = ?1", ids)?;
        // 出口成了监听的节点自己：不支持，当作没有出口。
        self.conn.execute("UPDATE proxies SET exit_id = NULL, enabled = 0 WHERE exit_id = node_id", [])?;
        self.conn.execute("UPDATE forwards SET listen_node = ?2 WHERE listen_node = ?1", ids)?;
        self.conn.execute("UPDATE forwards SET target_node = ?2 WHERE target_node = ?1", ids)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO pair_settings (node_a, node_b, keep_alive, congestion)
             SELECT MIN(peer, ?2), MAX(peer, ?2), keep_alive, congestion
             FROM (SELECT CASE WHEN node_a = ?1 THEN node_b ELSE node_a END AS peer, keep_alive, congestion
                   FROM pair_settings WHERE node_a = ?1 OR node_b = ?1)
             WHERE peer <> ?2",
            ids,
        )?;
        self.conn.execute("DELETE FROM pair_settings WHERE node_a = ?1 OR node_b = ?1", params![SqlNode(old)])?;
        self.conn.execute(
            "INSERT OR IGNORE INTO group_members (group_id, node_id) SELECT group_id, ?2 FROM group_members WHERE node_id = ?1",
            ids,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::update::{FailureKind, SelfUpdateBlocker};

    use super::*;
    use crate::store::NewNetwork;
    use crate::store::fixtures::{home, key, new_node};

    #[test]
    fn a_node_round_trips_with_everything_set() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let id = store.insert_node(&new_node(network, 1)).unwrap();
        let mut node = store.node(id).unwrap().unwrap();
        assert_eq!(node.name, format!("node-{id}"));
        assert_eq!(node.netmap, NetmapState::default());
        assert_eq!(node.last_seen_at, None);

        node.name = "nas-01".to_owned();
        node.groups = [store.insert_group(network, "servers").unwrap(), store.insert_group(network, "home").unwrap()].into();
        store.set_node_groups(id, &node.groups).unwrap();
        node.declared = Declared { exit: true, relay: true, relay_address: Some("203.0.113.9:40330".parse().unwrap()) };
        node.forward_target = true;
        node.relay = Some(RelaySettings { bandwidth_mbps: Some(200) });
        node.exit = Some(ExitRole { ports: None, lan: true, ..ExitRole::default() });
        node.dial_out_only = true;
        node.routing = Some(RoutingMode::LowestCost);
        node.reach = Some(Reach::DirectOnly);
        node.pinned_relays = vec![NodeId::new(9).unwrap()];
        node.profile = Some(Profile::LowFootprint);
        node.auto_update = false;
        node.hostname = "nas-01.lan".to_owned();
        node.self_update = Some(SelfUpdate::from(Some(SelfUpdateBlocker::Container)));
        store.update_node(&node).unwrap();

        let endpoints: Vec<SocketAddr> = vec!["192.168.1.5:20330".parse().unwrap(), "[2001:db8::1]:20330".parse().unwrap()];
        let attached = vec![Attachment { relay: NodeId::new(9).unwrap(), rtt_ms: 12, shared: false }];
        store.set_node_report(id, &endpoints, &attached).unwrap();
        store.set_node_observed(id, Some("198.51.100.4:20330".parse().unwrap())).unwrap();
        let local: Vec<IpAddr> = vec!["192.168.1.5".parse().unwrap(), "10.66.3.7".parse().unwrap()];
        store.set_node_local_addresses(id, &local).unwrap();
        let reach = vec![
            ReachRecord {
                relay: NodeId::new(9).unwrap(),
                outcome: TryOutcome::Tcp,
                at: 1_500,
                seen_as: Some("198.51.100.4".parse().unwrap()),
            },
            ReachRecord { relay: NodeId::new(11).unwrap(), outcome: TryOutcome::Unreached, at: 1_400, seen_as: None },
        ];
        store.set_node_relay_reach(id, &reach).unwrap();
        store.set_node_last_seen(id, 2_000).unwrap();
        store.set_node_netmap(id, 5, &[7; DIGEST_LEN]).unwrap();
        store.set_node_sent(id, 5).unwrap();
        store.set_node_acked(id, 4).unwrap();
        let failure = UpdateFailure {
            version: "0.3.0".parse().unwrap(),
            kind: FailureKind::VersionRejected,
            reason: "自检没有通过".to_owned(),
            at: 3_000,
            cause: Some(i3t_model::update::UpdateCause::SelfCheck { exit_code: Some(1), output: "坏了".to_owned() }),
        };
        store.set_node_update_failure(id, Some(&failure)).unwrap();

        node.update_failure = Some(failure);
        node.endpoints = endpoints;
        node.attached = attached;
        node.observed = Some("198.51.100.4:20330".parse().unwrap());
        node.local_addresses = Some(local);
        node.relay_reach = reach;
        node.last_seen_at = Some(2_000);
        node.netmap = NetmapState { version: 5, digest: Some([7; DIGEST_LEN]), sent: 5, acked: 4 };
        assert_eq!(store.node(id).unwrap(), Some(node.clone()));
        assert_eq!(store.node_by_key(&key(1)).unwrap(), Some(node.clone()));
        assert_eq!(store.nodes(network).unwrap(), vec![node]);
        assert_eq!(store.node_count(Some(network)).unwrap(), 1);
        assert_eq!(store.node_count(None).unwrap(), 1);

        store.set_node_update_failure(id, None).unwrap();
        assert_eq!(store.node(id).unwrap().unwrap().update_failure, None);
    }

    #[test]
    fn a_new_node_has_no_hostname_and_has_not_said_whether_it_can_update_itself() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let node = store.node(store.insert_node(&new_node(network, 1)).unwrap()).unwrap().unwrap();
        assert_eq!((node.hostname.as_str(), node.self_update, node.update_failure), ("", None, None));
        assert!(node.auto_update, "新节点的自动更新是开着的");
        assert_eq!(node.local_addresses, None, "没报过网卡地址和报了一个空表不一样");
    }

    #[test]
    fn exits_among_are_the_given_nodes_of_one_network_with_the_exit_switched_on() {
        let store = Store::open_in_memory().unwrap();
        let ours = store.insert_network(&home()).unwrap();
        let theirs = store.insert_network(&NewNetwork { name: "office".to_owned(), ..home() }).unwrap();
        let switch_on = |id| {
            let mut node = store.node(id).unwrap().unwrap();
            node.exit = Some(ExitRole::default());
            store.update_node(&node).unwrap();
        };
        let (plain, exit, other) = (
            store.insert_node(&new_node(ours, 1)).unwrap(),
            store.insert_node(&new_node(ours, 2)).unwrap(),
            store.insert_node(&new_node(theirs, 3)).unwrap(),
        );
        switch_on(exit);
        switch_on(other);
        let unknown = NodeId::new(999).unwrap();
        let asked = [plain, exit, other, unknown];
        assert_eq!(store.exits_among(ours, asked).unwrap(), [exit].into(), "{plain} 没有启用出口，{other} 在别的网络，{unknown} 没有");
        assert_eq!(store.exits_among(theirs, asked).unwrap(), [other].into());
        assert!(store.exits_among(ours, [plain]).unwrap().is_empty(), "只看问到的");
    }

    #[test]
    fn exits_among_looks_up_the_asked_ids_by_key_and_never_reads_the_whole_network() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        for seed in 1..=200 {
            store.insert_node(&new_node(network, seed)).unwrap();
        }
        let plan = || -> Vec<String> {
            let mut statement = store.conn.prepare(&format!("EXPLAIN QUERY PLAN {EXITS_AMONG}")).unwrap();
            let rows = statement.query_map(params![Json(&[1u32, 2]), SqlNetwork(network)], |row| row.get::<_, String>(3)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        let by_key = |plan: &[String]| matches!(plan, [outer, inner] if outer.starts_with("SCAN asked") && inner.starts_with("SEARCH nodes USING INTEGER PRIMARY KEY"));
        // controller 不跑 ANALYZE，库里没有统计信息：这时 SQLite 会以为按网络的索引更省，要靠写法把顺序定住。
        let plan_without_stats = plan();
        assert!(by_key(&plan_without_stats), "{plan_without_stats:?}");
        store.conn.execute_batch("ANALYZE").unwrap();
        let plan_with_stats = plan();
        assert!(by_key(&plan_with_stats), "有了统计信息也一样：{plan_with_stats:?}");
    }

    #[test]
    fn node_ids_are_never_reused() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let first = store.insert_node(&new_node(network, 1)).unwrap();
        let second = store.insert_node(&new_node(network, 2)).unwrap();
        store.delete_node(second).unwrap();
        let third = store.insert_node(&new_node(network, 3)).unwrap();
        assert!(first < second && second < third, "{first} {second} {third}");
    }

    #[test]
    fn key_address_and_name_are_unique() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        store.insert_node(&NewNode { name: Some("a".to_owned()), ..new_node(network, 1) }).unwrap();
        assert!(store.insert_node(&NewNode { virtual_ip: "10.66.0.99".parse().unwrap(), ..new_node(network, 1) }).is_err(), "同一把公钥");
        assert!(store.insert_node(&NewNode { key: key(2), ..new_node(network, 1) }).is_err(), "同一个虚拟 IP");
        assert!(store.insert_node(&NewNode { name: Some("a".to_owned()), ..new_node(network, 3) }).is_err(), "同一个名字");
    }

    #[test]
    fn a_parked_node_gives_up_its_name_and_address_to_a_successor() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let old = store.insert_node(&NewNode { name: Some("nas-01".to_owned()), ..new_node(network, 1) }).unwrap();
        let taken = NewNode { name: Some("nas-01".to_owned()), virtual_ip: "10.66.0.1".parse().unwrap(), ..new_node(network, 2) };
        assert!(store.insert_node(&taken).is_err());
        let (servers, home) = (store.insert_group(network, "servers").unwrap(), store.insert_group(network, "home").unwrap());
        store.set_node_groups(old, &[servers].into()).unwrap();
        store.park_node(old).unwrap();
        let new = store.insert_node(&NewNode { groups: [home].into(), ..taken }).unwrap();
        store.repoint_node(old, new).unwrap();
        store.delete_node(old).unwrap();
        let nodes = store.nodes(network).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!((nodes[0].id, nodes[0].name.as_str(), nodes[0].virtual_ip), (new, "nas-01", "10.66.0.1".parse().unwrap()));
        assert_eq!(nodes[0].groups, [servers, home].into(), "继任者在两边的分组里");
    }

    #[test]
    fn nodes_are_listed_per_network_only() {
        let store = Store::open_in_memory().unwrap();
        let home_id = store.insert_network(&home()).unwrap();
        let lab_id = store.insert_network(&NewNetwork { name: "lab".to_owned(), cidr: "10.67.0.0/16".parse().unwrap(), ..home() }).unwrap();
        let in_home = store.insert_node(&new_node(home_id, 1)).unwrap();
        let in_lab = store.insert_node(&NewNode { virtual_ip: "10.67.0.1".parse().unwrap(), ..new_node(lab_id, 2) }).unwrap();
        assert_eq!(store.nodes(home_id).unwrap().iter().map(|node| node.id).collect::<Vec<_>>(), vec![in_home]);
        assert_eq!(store.nodes(lab_id).unwrap().iter().map(|node| node.id).collect::<Vec<_>>(), vec![in_lab]);
    }
}
