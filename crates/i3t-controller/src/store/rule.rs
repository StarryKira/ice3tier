//! 访问规则。存的就是管理 API 里的 [`Rule`]。

use i3t_model::admin::{Direction, NetworkId, Rule};
use rusqlite::{OptionalExtension, Row, params};

use super::codec::{Json, SqlNetwork, row_id};
use super::{Store, StoreError};

fn from_row(row: &Row<'_>) -> rusqlite::Result<Rule> {
    Ok(Rule {
        id: row.get("id")?,
        name: row.get("name")?,
        src: row.get("src")?,
        dst: row.get("dst")?,
        direction: row.get::<_, Json<Direction>>("direction")?.0,
        enabled: row.get("enabled")?,
    })
}

impl Store {
    /// 加一条规则。`rule.id` 不用，返回新分配的编号。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn insert_rule(&self, network: NetworkId, rule: &Rule) -> Result<u32, StoreError> {
        self.conn.execute(
            "INSERT INTO rules (network_id, name, src, dst, direction, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![SqlNetwork(network), rule.name, rule.src, rule.dst, Json(rule.direction), rule.enabled],
        )?;
        Ok(row_id(self.conn.last_insert_rowid())?)
    }

    /// 一个网络的全部规则，按编号排。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn rules(&self, network: NetworkId) -> Result<Vec<Rule>, StoreError> {
        let mut statement = self.conn.prepare("SELECT * FROM rules WHERE network_id = ?1 ORDER BY id")?;
        let rows = statement.query_map(params![SqlNetwork(network)], from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 只在这个网络里找：别的网络的规则编号在这里等于不存在。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn rule(&self, network: NetworkId, id: u32) -> Result<Option<Rule>, StoreError> {
        let sql = "SELECT * FROM rules WHERE network_id = ?1 AND id = ?2";
        Ok(self.conn.query_row(sql, params![SqlNetwork(network), id], from_row).optional()?)
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn update_rule(&self, rule: &Rule) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE rules SET name = ?2, src = ?3, dst = ?4, direction = ?5, enabled = ?6 WHERE id = ?1",
            params![rule.id, rule.name, rule.src, rule.dst, Json(rule.direction), rule.enabled],
        )?;
        Ok(())
    }

    /// # Errors
    ///
    /// 写数据库失败。
    pub fn delete_rule(&self, id: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM rules WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewNetwork;
    use crate::store::fixtures::home;

    #[test]
    fn rules_round_trip_and_stay_inside_their_network() {
        let store = Store::open_in_memory().unwrap();
        let home_id = store.insert_network(&home()).unwrap();
        let lab_id = store.insert_network(&NewNetwork { name: "lab".to_owned(), cidr: "10.67.0.0/16".parse().unwrap(), ..home() }).unwrap();
        let mut rule = Rule {
            id: 0,
            name: "laptops reach servers".to_owned(),
            src: "laptops".to_owned(),
            dst: "servers".to_owned(),
            direction: Direction::OneWay,
            enabled: true,
        };
        rule.id = store.insert_rule(home_id, &rule).unwrap();
        assert_eq!(store.rules(home_id).unwrap(), vec![rule.clone()]);
        assert_eq!(store.rule(home_id, rule.id).unwrap(), Some(rule.clone()));
        assert_eq!(store.rule(lab_id, rule.id).unwrap(), None);
        assert!(store.rules(lab_id).unwrap().is_empty());

        rule.direction = Direction::Both;
        rule.enabled = false;
        store.update_rule(&rule).unwrap();
        assert_eq!(store.rule(home_id, rule.id).unwrap(), Some(rule.clone()));

        store.delete_rule(rule.id).unwrap();
        assert!(store.rules(home_id).unwrap().is_empty());
    }
}
