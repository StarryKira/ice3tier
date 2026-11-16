//! 接入凭据。只存令牌的哈希。

use i3t_model::admin::{Credential, NetworkId};
use i3t_model::netmap::TokenHash;
use rusqlite::{OptionalExtension, Row, params};

use super::codec::{SqlNetwork, Unsigned, row_id};
use super::{Store, StoreError};

/// 一个凭据：给管理员看的部分，加上下发给入口节点的令牌哈希。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    pub credential: Credential,
    pub token: TokenHash,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<CredentialRecord> {
    Ok(CredentialRecord {
        credential: Credential {
            id: row.get("id")?,
            name: row.get("name")?,
            max_concurrent: row.get("max_concurrent")?,
            created_at: row.get::<_, Unsigned>("created_at")?.0,
        },
        token: TokenHash::from_bytes(row.get("token_hash")?),
    })
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
            "INSERT INTO credentials (network_id, name, token_hash, max_concurrent, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                SqlNetwork(network),
                credential.name,
                record.token.as_bytes(),
                credential.max_concurrent,
                Unsigned(credential.created_at)
            ],
        )?;
        Ok(row_id(self.conn.last_insert_rowid())?)
    }

    /// 一个网络的全部凭据，按编号排。
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

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_credential(&self, id: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM credentials WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::home;

    #[test]
    fn credentials_round_trip_with_their_hash() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let mut record = CredentialRecord {
            credential: Credential { id: 0, name: "phone".to_owned(), max_concurrent: Some(8), created_at: 500 },
            token: TokenHash::from_bytes([3; 32]),
        };
        record.credential.id = store.insert_credential(network, &record).unwrap();
        assert_eq!(store.credentials(network).unwrap(), vec![record.clone()]);
        assert_eq!(store.credential(network, record.credential.id).unwrap(), Some(record.clone()));
        assert!(store.insert_credential(network, &record).is_err(), "同一个网络里凭据的名字不能重复");

        store.delete_credential(record.credential.id).unwrap();
        assert!(store.credentials(network).unwrap().is_empty());
    }
}
