//! 操作记录：何时改了什么。

use i3t_model::admin::{LogEntry, NetworkId, Timestamp};
use rusqlite::{Row, params};

use super::codec::{SqlNetwork, Unsigned};
use super::{Store, StoreError};

/// 最多留这么多条，再多就从最老的删起。个人用的 controller 一年也攒不到这个数。
const KEEP: u32 = 10_000;

fn from_row(row: &Row<'_>) -> rusqlite::Result<LogEntry> {
    Ok(LogEntry {
        id: row.get::<_, Unsigned>("id")?.0,
        at: row.get::<_, Unsigned>("at")?.0,
        network: row.get::<_, Option<SqlNetwork>>("network_id")?.map(|network| network.0),
        action: row.get("action")?,
        detail: row.get("detail")?,
    })
}

impl Store {
    /// 记一条操作。`detail` 里不能有令牌之类的秘密。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn log(&self, at: Timestamp, network: Option<NetworkId>, action: &str, detail: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO oplog (at, network_id, action, detail) VALUES (?1, ?2, ?3, ?4)",
            params![Unsigned(at), network.map(SqlNetwork), action, detail],
        )?;
        self.conn.execute("DELETE FROM oplog WHERE id <= ?1 - ?2", params![self.conn.last_insert_rowid(), KEEP])?;
        Ok(())
    }

    /// 最近的 `limit` 条操作记录，新的在前。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn oplog(&self, limit: u32) -> Result<Vec<LogEntry>, StoreError> {
        let mut statement = self.conn.prepare("SELECT * FROM oplog ORDER BY id DESC LIMIT ?1")?;
        let rows = statement.query_map(params![limit], from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::home;

    #[test]
    fn operations_are_listed_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        store.log(10, None, "controller.start", "first start").unwrap();
        store.log(20, Some(network), "node.rename", "node-1 -> nas-01").unwrap();
        let entries = store.oplog(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].at, entries[0].network, entries[0].action.as_str()), (20, Some(network), "node.rename"));
        assert_eq!((entries[1].at, entries[1].network, entries[1].detail.as_str()), (10, None, "first start"));
        assert_eq!(store.oplog(1).unwrap().len(), 1);
    }
}
