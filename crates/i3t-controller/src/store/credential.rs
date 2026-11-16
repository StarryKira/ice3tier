//! 接入凭据。只存令牌的哈希，和开头一小段给人认的前缀。
//!
//! 用量是各个入口报上来的：每个入口报的是它这次运行以来的累计字节数，这里记下每个（凭据，入口）最近报的那个数和
//! 它是哪一次运行报的，把累计换成增量加到凭据的用量上。入口重启之后计数从零开始，换了运行编号，这一次报的整个算增量。

use i3t_model::admin::{Credential, CredentialState, CredentialUsage, NetworkId, Timestamp};
use i3t_model::netmap::TokenHash;
use i3t_proto::NodeId;
use rusqlite::{OptionalExtension, Row, named_params, params};

use super::codec::{SqlNetwork, SqlNode, Unsigned, row_id};
use super::{Store, StoreError};

/// 一个凭据：给管理员看的部分，加上下发给入口节点的令牌哈希。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    /// `usage.connections` 不存：它是在线的入口报的现值，由 controller 现填。
    pub credential: Credential,
    pub token: TokenHash,
}

impl CredentialRecord {
    /// 用量到了流量上限。
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.credential.max_bytes.is_some_and(|max| self.credential.usage.bytes >= max)
    }

    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        self.credential.revoked_at.is_some()
    }
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<CredentialRecord> {
    let mut record = CredentialRecord {
        credential: Credential {
            id: row.get("id")?,
            name: row.get("name")?,
            max_concurrent: row.get("max_concurrent")?,
            created_at: row.get::<_, Unsigned>("created_at")?.0,
            prefix: row.get("prefix")?,
            max_bytes: row.get::<_, Option<Unsigned>>("max_bytes")?.map(|max| max.0),
            state: CredentialState::Active,
            revoked_at: row.get::<_, Option<Unsigned>>("revoked_at")?.map(|at| at.0),
            usage: CredentialUsage {
                bytes: row.get::<_, Unsigned>("used_bytes")?.0,
                connections: 0,
                last_used_at: row.get::<_, Option<Unsigned>>("last_used_at")?.map(|at| at.0),
            },
        },
        token: TokenHash::from_bytes(row.get("token_hash")?),
    };
    record.credential.state = if record.is_revoked() {
        CredentialState::Revoked
    } else if record.is_exhausted() {
        CredentialState::Exhausted
    } else {
        CredentialState::Active
    };
    Ok(record)
}

/// 运行编号是一个随机的 `u64`，按位存进 SQLite 的有符号整数里。只用来比相等，不比大小。
fn run_bits(run: u64) -> i64 {
    i64::from_be_bytes(run.to_be_bytes())
}

impl Store {
    /// 加一个凭据。`record.credential.id` 不用，返回新分配的编号。
    ///
    /// # Errors
    ///
    /// 写数据库失败，包括名字和同一个网络里已有的凭据重复。
    pub fn insert_credential(&self, network: NetworkId, record: &CredentialRecord) -> Result<u32, StoreError> {
        let credential = &record.credential;
        self.conn.execute(
            "INSERT INTO credentials (network_id, name, token_hash, max_concurrent, created_at, prefix, max_bytes)
             VALUES (:network, :name, :token, :max_concurrent, :created_at, :prefix, :max_bytes)",
            named_params! {
                ":network": SqlNetwork(network),
                ":name": credential.name,
                ":token": record.token.as_bytes(),
                ":max_concurrent": credential.max_concurrent,
                ":created_at": Unsigned(credential.created_at),
                ":prefix": credential.prefix,
                ":max_bytes": credential.max_bytes.map(Unsigned),
            },
        )?;
        Ok(row_id(self.conn.last_insert_rowid())?)
    }

    /// 一个网络的全部凭据，包括吊销了的，按编号排。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn credentials(&self, network: NetworkId) -> Result<Vec<CredentialRecord>, StoreError> {
        let mut statement = self.conn.prepare("SELECT * FROM credentials WHERE network_id = ?1 ORDER BY id")?;
        let rows = statement.query_map(params![SqlNetwork(network)], from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 只在这个网络里找。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn credential(&self, network: NetworkId, id: u32) -> Result<Option<CredentialRecord>, StoreError> {
        let sql = "SELECT * FROM credentials WHERE network_id = ?1 AND id = ?2";
        Ok(self.conn.query_row(sql, params![SqlNetwork(network), id], from_row).optional()?)
    }

    /// 改上限。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn set_credential_limits(&self, id: u32, max_concurrent: Option<u32>, max_bytes: Option<u64>) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE credentials SET max_concurrent = ?2, max_bytes = ?3 WHERE id = ?1",
            params![id, max_concurrent, max_bytes.map(Unsigned)],
        )?;
        Ok(())
    }

    /// 用量清零。各个入口的累计数照旧记着，之后只把新的增量加上来。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn reset_credential_usage(&self, id: u32) -> Result<(), StoreError> {
        self.conn.execute("UPDATE credentials SET used_bytes = 0 WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 吊销：行留着，入口不再认它。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn revoke_credential(&self, id: u32, at: Timestamp) -> Result<(), StoreError> {
        self.conn.execute("UPDATE credentials SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL", params![id, Unsigned(at)])?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_credential(&self, id: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM credentials WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 入口 `node` 报了凭据 `id` 在它这次运行（`run`）里一共搬了 `bytes` 字节。把新增的部分加到凭据的用量上，返回加了多少。
    ///
    /// `last_used_at` 有值时，凭据的最近使用时间取它和原来的较晚者。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn record_credential_usage(
        &self,
        id: u32,
        node: NodeId,
        run: u64,
        bytes: u64,
        last_used_at: Option<Timestamp>,
    ) -> Result<u64, StoreError> {
        let previous: Option<(i64, Unsigned)> = self
            .conn
            .query_row(
                "SELECT run, bytes FROM credential_counters WHERE credential_id = ?1 AND node_id = ?2",
                params![id, SqlNode(node)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let delta = match previous {
            // 同一次运行里计数只增不减；变小了说明入口的计数被重置过，这一次报的整个算新的。
            Some((stored, Unsigned(before))) if stored == run_bits(run) && bytes >= before => bytes - before,
            _ => bytes,
        };
        self.conn.execute(
            "INSERT INTO credential_counters (credential_id, node_id, run, bytes) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (credential_id, node_id) DO UPDATE SET run = excluded.run, bytes = excluded.bytes",
            params![id, SqlNode(node), run_bits(run), Unsigned(bytes)],
        )?;
        if delta > 0 {
            self.conn.execute("UPDATE credentials SET used_bytes = used_bytes + ?2 WHERE id = ?1", params![id, Unsigned(delta)])?;
        }
        if let Some(at) = last_used_at {
            self.conn.execute(
                "UPDATE credentials SET last_used_at = MAX(COALESCE(last_used_at, 0), ?2) WHERE id = ?1",
                params![id, Unsigned(at)],
            )?;
        }
        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::{home, store_with_nodes};

    fn record(name: &str) -> CredentialRecord {
        CredentialRecord {
            credential: Credential {
                id: 0,
                name: name.to_owned(),
                max_concurrent: Some(8),
                created_at: 500,
                prefix: "I7bRtu".to_owned(),
                max_bytes: None,
                state: CredentialState::Active,
                revoked_at: None,
                usage: CredentialUsage::default(),
            },
            token: TokenHash::from_bytes([3; 32]),
        }
    }

    #[test]
    fn credentials_round_trip_with_their_hash() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let mut record = record("phone");
        record.credential.id = store.insert_credential(network, &record).unwrap();
        assert_eq!(store.credentials(network).unwrap(), vec![record.clone()]);
        assert_eq!(store.credential(network, record.credential.id).unwrap(), Some(record.clone()));
        assert!(store.insert_credential(network, &record).is_err(), "同一个网络里凭据的名字不能重复");

        store.delete_credential(record.credential.id).unwrap();
        assert!(store.credentials(network).unwrap().is_empty());
    }

    #[test]
    fn usage_is_turned_from_running_totals_into_increments_per_ingress_and_run() {
        let (store, network, nodes) = store_with_nodes(2);
        let id = store.insert_credential(network, &record("phone")).unwrap();
        let used = || store.credential(network, id).unwrap().unwrap().credential.usage.bytes;

        assert_eq!(store.record_credential_usage(id, nodes[0], 7, 1000, Some(600)).unwrap(), 1000);
        assert_eq!(store.record_credential_usage(id, nodes[0], 7, 1500, None).unwrap(), 500, "同一次运行：只加新增的");
        assert_eq!(store.record_credential_usage(id, nodes[0], 7, 1500, None).unwrap(), 0, "没有变化就不加");
        assert_eq!(store.record_credential_usage(id, nodes[1], 3, 200, Some(700)).unwrap(), 200, "另一个入口各算各的");
        assert_eq!(used(), 1700);
        assert_eq!(store.record_credential_usage(id, nodes[0], 8, 50, Some(650)).unwrap(), 50, "入口重启了：从零开始的计数整个算新的");
        assert_eq!(store.record_credential_usage(id, nodes[0], u64::MAX, 10, None).unwrap(), 10, "运行编号用满 64 位也存得下");
        assert_eq!(used(), 1760);
        assert_eq!(store.credential(network, id).unwrap().unwrap().credential.usage.last_used_at, Some(700), "取较晚的那个时间");

        store.reset_credential_usage(id).unwrap();
        assert_eq!(used(), 0);
        assert_eq!(store.record_credential_usage(id, nodes[0], u64::MAX, 30, None).unwrap(), 20, "清零之后只加清零之后的增量");
        assert_eq!(used(), 20);
    }

    #[test]
    fn limits_revocation_and_state_are_kept_in_the_row() {
        let (store, network, nodes) = store_with_nodes(1);
        let id = store.insert_credential(network, &record("phone")).unwrap();
        let read = || store.credential(network, id).unwrap().unwrap();
        store.set_credential_limits(id, None, Some(100)).unwrap();
        assert_eq!(
            (read().credential.max_concurrent, read().credential.max_bytes, read().credential.state),
            (None, Some(100), CredentialState::Active)
        );
        store.record_credential_usage(id, nodes[0], 1, 100, None).unwrap();
        assert!(read().is_exhausted());
        assert_eq!(read().credential.state, CredentialState::Exhausted);
        store.revoke_credential(id, 900).unwrap();
        store.revoke_credential(id, 950).unwrap();
        assert_eq!((read().credential.state, read().credential.revoked_at), (CredentialState::Revoked, Some(900)), "吊销的时间是第一次的");
        assert_eq!(read().credential.prefix, "I7bRtu");
    }
}
