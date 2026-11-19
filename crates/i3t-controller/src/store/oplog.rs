//! 操作记录：何时改了什么，说的是哪些东西。

use i3t_model::admin::{LogEntry, LogRef, NetworkId, Timestamp};
use rusqlite::types::Value;
use rusqlite::{Row, params, params_from_iter};
use serde_json::{Map, Value as JsonValue};

use super::codec::{Json, SqlNetwork, Unsigned};
use super::{Store, StoreError};

/// 查操作记录的条件。都可以不写。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogFilter {
    /// 最多多少条。
    pub limit: u32,
    /// 只要编号比它小的：翻页时写上一页最后一条的编号。
    pub before: Option<u64>,
    pub network: Option<NetworkId>,
    /// 这种操作，或者以它加一个点开头的操作：`node` 包括 `node.update`、`node.relay.enable` 等。
    pub action: Option<String>,
    /// 提到了这样东西的。
    pub reference: Option<LogRef>,
}

/// 最多留这么多条，再多就从最老的删起。个人用的 controller 一年也攒不到这个数。
const KEEP: u32 = 10_000;

fn from_row(row: &Row<'_>) -> rusqlite::Result<LogEntry> {
    Ok(LogEntry {
        id: row.get::<_, Unsigned>("id")?.0,
        at: row.get::<_, Unsigned>("at")?.0,
        network: row.get::<_, Option<SqlNetwork>>("network_id")?.map(|network| network.0),
        action: row.get("action")?,
        detail: row.get("detail")?,
        refs: row.get::<_, Json<Vec<LogRef>>>("refs")?.0,
        params: row.get::<_, Json<Map<String, JsonValue>>>("params")?.0,
    })
}

impl Store {
    /// 记一条操作。`detail` 和 `params` 里不能有令牌之类的秘密。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn log(&self, at: Timestamp, network: Option<NetworkId>, action: &str, detail: &str, params: JsonValue) -> Result<(), StoreError> {
        self.log_refs(at, network, action, detail, &[], params)
    }

    /// 记一条操作，带上它说的是哪些东西。`detail` 和 `params` 里不能有令牌之类的秘密。
    ///
    /// `params` 是一个 JSON 对象（`serde_json::json!({ … })`）：按名字给出的参数，见 `docs/internal/admin-api.md` 第 3 节。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn log_refs(
        &self,
        at: Timestamp,
        network: Option<NetworkId>,
        action: &str,
        detail: &str,
        refs: &[LogRef],
        params: JsonValue,
    ) -> Result<(), StoreError> {
        let params = match params {
            JsonValue::Object(params) => params,
            other => {
                // 调用方写错了。不为一条记录的参数让整个操作失败：记下原话，参数留空。
                debug_assert!(false, "操作记录的参数要是一个 JSON 对象，{action} 给的是 {other}");
                tracing::error!(action, params = %other, "操作记录的参数不是一个 JSON 对象，不记参数");
                Map::new()
            }
        };
        self.conn.execute(
            "INSERT INTO oplog (at, network_id, action, detail, refs, params) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![Unsigned(at), network.map(SqlNetwork), action, detail, Json(refs), Json(&params)],
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
        self.oplog_filtered(&LogFilter { limit, ..LogFilter::default() })
    }

    /// 符合条件的操作记录，新的在前，最多 `filter.limit` 条。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn oplog_filtered(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, StoreError> {
        let mut conditions: Vec<&str> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        if let Some(before) = filter.before {
            conditions.push("id < ?");
            values.push(Value::Integer(i64::try_from(before).unwrap_or(i64::MAX)));
        }
        if let Some(network) = filter.network {
            conditions.push("network_id = ?");
            values.push(Value::Integer(i64::from(network.0)));
        }
        if let Some(action) = &filter.action {
            // 不用 LIKE：操作的名字里有下划线，在 LIKE 里它是通配符。
            conditions.push("(action = ? OR substr(action, 1, length(?) + 1) = ? || '.')");
            values.extend([Value::Text(action.clone()), Value::Text(action.clone()), Value::Text(action.clone())]);
        }
        if let Some(reference) = filter.reference {
            // refs 是这里写进去的 JSON，写法固定，按子串找就是按元素找：`"id":3}` 的右括号挡住了 30、31……
            let needle = serde_json::to_string(&reference).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            conditions.push("instr(refs, ?) > 0");
            values.push(Value::Text(needle));
        }
        let condition = if conditions.is_empty() { String::new() } else { format!("WHERE {}", conditions.join(" AND ")) };
        values.push(Value::Integer(i64::from(filter.limit)));
        let mut statement = self.conn.prepare(&format!("SELECT * FROM oplog {condition} ORDER BY id DESC LIMIT ?"))?;
        let rows = statement.query_map(params_from_iter(values), from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::store::fixtures::home;

    #[test]
    fn operations_are_listed_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        store.log(10, None, "controller.start", "first start", json!({})).unwrap();
        store.log(20, Some(network), "node.rename", "node-1 -> nas-01", json!({ "node": 1, "node_name": "nas-01" })).unwrap();
        let entries = store.oplog(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].at, entries[0].network, entries[0].action.as_str()), (20, Some(network), "node.rename"));
        assert_eq!((entries[1].at, entries[1].network, entries[1].detail.as_str()), (10, None, "first start"));
        assert_eq!(serde_json::Value::Object(entries[0].params.clone()), json!({ "node": 1, "node_name": "nas-01" }));
        assert!(entries[1].params.is_empty());
        assert_eq!(store.oplog(1).unwrap().len(), 1);
    }

    #[test]
    fn entries_can_be_paged_and_filtered_by_network_action_and_reference() {
        use i3t_model::admin::RefKind;
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let node = |id| LogRef::new(RefKind::Node, id);
        store.log_refs(1, Some(network), "node.update", "a", &[node(3)], json!({})).unwrap();
        store.log_refs(2, Some(network), "node.relay.enable", "b", &[node(30)], json!({})).unwrap();
        store.log_refs(3, None, "update.force_all", "c", &[], json!({})).unwrap();
        store.log_refs(4, Some(network), "proxy.create", "d", &[LogRef::new(RefKind::Proxy, 3)], json!({})).unwrap();
        store.log(5, Some(network), "nodes.weird", "e", json!({})).unwrap();
        let ids = |filter: LogFilter| {
            store.oplog_filtered(&LogFilter { limit: 100, ..filter }).unwrap().iter().map(|entry| entry.at).collect::<Vec<_>>()
        };

        assert_eq!(ids(LogFilter::default()), [5, 4, 3, 2, 1]);
        let page = store.oplog_filtered(&LogFilter { limit: 2, ..LogFilter::default() }).unwrap();
        assert_eq!(page.iter().map(|entry| entry.at).collect::<Vec<_>>(), [5, 4]);
        assert_eq!(ids(LogFilter { before: Some(page[1].id), ..LogFilter::default() }), [3, 2, 1], "下一页从上一页最后一条之后接着");
        assert_eq!(ids(LogFilter { network: Some(network), ..LogFilter::default() }), [5, 4, 2, 1]);
        assert_eq!(ids(LogFilter { action: Some("node".into()), ..LogFilter::default() }), [2, 1], "nodes.weird 不算 node 开头的操作");
        assert_eq!(ids(LogFilter { action: Some("node.relay.enable".into()), ..LogFilter::default() }), [2]);
        assert_eq!(ids(LogFilter { action: Some("update.force_all".into()), ..LogFilter::default() }), [3]);
        assert_eq!(ids(LogFilter { reference: Some(node(3)), ..LogFilter::default() }), [1], "节点 3，不是节点 30，也不是代理 3");
        assert_eq!(store.oplog(10).unwrap()[3].refs, [node(30)]);
    }
}
