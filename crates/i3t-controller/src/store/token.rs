//! 注册令牌。只存一次性密钥的哈希。

use std::net::Ipv4Addr;

use i3t_model::admin::{EnrollTokenInfo, NetworkId, Timestamp};
use i3t_proto::NodeId;
use rusqlite::{OptionalExtension, Row, named_params, params};

use super::codec::{Json, SqlIp, SqlNetwork, SqlNode, Unsigned, row_id};
use super::{DIGEST_LEN, Store, StoreError};

/// 存着的注册令牌。能给管理员看的部分就是 [`EnrollTokenInfo`]。
pub type TokenRecord = EnrollTokenInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToken {
    pub network: NetworkId,
    pub secret_hash: [u8; DIGEST_LEN],
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub tags: Vec<String>,
    pub virtual_ip: Option<Ipv4Addr>,
    pub successor_of: Option<NodeId>,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<TokenRecord> {
    Ok(TokenRecord {
        id: row.get("id")?,
        network: row.get::<_, SqlNetwork>("network_id")?.0,
        created_at: row.get::<_, Unsigned>("created_at")?.0,
        expires_at: row.get::<_, Unsigned>("expires_at")?.0,
        tags: row.get::<_, Json<Vec<String>>>("tags")?.0,
        virtual_ip: row.get::<_, Option<SqlIp>>("virtual_ip")?.map(|ip| ip.0),
        successor_of: row.get::<_, Option<SqlNode>>("successor_of")?.map(|node| node.0),
    })
}

impl Store {
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn insert_token(&self, new: &NewToken) -> Result<u32, StoreError> {
        self.conn.execute(
            "INSERT INTO enroll_tokens (network_id, secret_hash, created_at, expires_at, tags, virtual_ip, successor_of)
             VALUES (:network, :hash, :created_at, :expires_at, :tags, :ip, :successor_of)",
            named_params! {
                ":network": SqlNetwork(new.network),
                ":hash": new.secret_hash,
                ":created_at": Unsigned(new.created_at),
                ":expires_at": Unsigned(new.expires_at),
                ":tags": Json(&new.tags),
                ":ip": new.virtual_ip.map(SqlIp),
                ":successor_of": new.successor_of.map(SqlNode),
            },
        )?;
        Ok(row_id(self.conn.last_insert_rowid())?)
    }

    /// 一个网络里还没被用掉的令牌，包括已经过期但还没清掉的。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn tokens(&self, network: NetworkId) -> Result<Vec<TokenRecord>, StoreError> {
        let mut statement = self.conn.prepare("SELECT * FROM enroll_tokens WHERE network_id = ?1 ORDER BY id")?;
        let rows = statement.query_map(params![SqlNetwork(network)], from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// # Errors
    ///
    /// 读数据库失败。
    pub fn token(&self, id: u32) -> Result<Option<TokenRecord>, StoreError> {
        Ok(self.conn.query_row("SELECT * FROM enroll_tokens WHERE id = ?1", params![id], from_row).optional()?)
    }

    /// 按一次性密钥的哈希找令牌。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn token_by_hash(&self, secret_hash: &[u8; DIGEST_LEN]) -> Result<Option<TokenRecord>, StoreError> {
        Ok(self.conn.query_row("SELECT * FROM enroll_tokens WHERE secret_hash = ?1", params![secret_hash], from_row).optional()?)
    }

    /// 删掉一个令牌（用掉了，或者管理员收回）。返回它之前在不在。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_token(&self, id: u32) -> Result<bool, StoreError> {
        Ok(self.conn.execute("DELETE FROM enroll_tokens WHERE id = ?1", params![id])? > 0)
    }

    /// 清掉已经过期的令牌，返回清掉了几个。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn purge_expired_tokens(&self, now: Timestamp) -> Result<usize, StoreError> {
        Ok(self.conn.execute("DELETE FROM enroll_tokens WHERE expires_at <= ?1", params![Unsigned(now)])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::store_with_nodes;

    #[test]
    fn a_token_is_found_by_its_hash_until_it_is_consumed() {
        let (store, network, nodes) = store_with_nodes(1);
        let new = NewToken {
            network,
            secret_hash: [9; DIGEST_LEN],
            created_at: 100,
            expires_at: 200,
            tags: vec!["servers".to_owned()],
            virtual_ip: Some("10.66.0.50".parse().unwrap()),
            successor_of: Some(nodes[0]),
        };
        let id = store.insert_token(&new).unwrap();
        let record = store.token_by_hash(&[9; DIGEST_LEN]).unwrap().unwrap();
        assert_eq!(record.id, id);
        assert_eq!(record.tags, vec!["servers".to_owned()]);
        assert_eq!(record.virtual_ip, new.virtual_ip);
        assert_eq!(record.successor_of, Some(nodes[0]));
        assert_eq!(store.tokens(network).unwrap(), vec![record.clone()]);
        assert_eq!(store.token(id).unwrap(), Some(record));
        assert_eq!(store.token_by_hash(&[8; DIGEST_LEN]).unwrap(), None);

        assert!(store.delete_token(id).unwrap());
        assert!(!store.delete_token(id).unwrap());
        assert_eq!(store.token_by_hash(&[9; DIGEST_LEN]).unwrap(), None);
    }

    #[test]
    fn expired_tokens_are_purged_and_a_successor_token_goes_with_its_node() {
        let (store, network, nodes) = store_with_nodes(1);
        let token = |hash: u8, expires_at, successor_of| NewToken {
            network,
            secret_hash: [hash; DIGEST_LEN],
            created_at: 100,
            expires_at,
            tags: Vec::new(),
            virtual_ip: None,
            successor_of,
        };
        store.insert_token(&token(1, 200, None)).unwrap();
        let kept = store.insert_token(&token(2, 300, None)).unwrap();
        let successor = store.insert_token(&token(3, 300, Some(nodes[0]))).unwrap();
        assert_eq!(store.purge_expired_tokens(200).unwrap(), 1);
        assert_eq!(store.tokens(network).unwrap().iter().map(|token| token.id).collect::<Vec<_>>(), vec![kept, successor]);

        store.delete_node(nodes[0]).unwrap();
        assert_eq!(store.tokens(network).unwrap().iter().map(|token| token.id).collect::<Vec<_>>(), vec![kept]);
    }
}
