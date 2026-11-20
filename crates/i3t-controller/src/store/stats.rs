//! 给界面画图的时间序列：每个点一行。

use i3t_model::admin::stats::Series;
use i3t_model::admin::{NetworkId, Timestamp};
use i3t_proto::NodeId;
use rusqlite::params;

use super::codec::{SqlNetwork, Unsigned};
use super::{Store, StoreError};

/// 时间序列里的一个点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatPoint {
    pub network: NetworkId,
    pub series: Series,
    /// 节点的序列是这个节点；网络的序列是 `None`。
    pub node: Option<NodeId>,
    /// 这一段的开始，按步长对齐。
    pub at: Timestamp,
    pub value: f64,
}

/// 序列在表里的编号。写进数据库的数，已经用过的不能改，也不能给别的序列。
const fn code(series: Series) -> i64 {
    match series {
        Series::Online => 1,
        Series::Flows => 2,
        Series::Tx => 3,
        Series::Rx => 4,
        Series::NodeTx => 5,
        Series::NodeRx => 6,
        Series::NodeFlows => 7,
        Series::RelayAttached => 8,
        Series::ExitTx => 9,
        Series::ExitRx => 10,
    }
}

fn node_column(node: Option<NodeId>) -> u32 {
    node.map_or(0, NodeId::get)
}

impl Store {
    /// 写下一批点，步长都是 `step`。同一段已经有点时换成新的。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn insert_stats(&self, step: u32, points: &[StatPoint]) -> Result<(), StoreError> {
        let mut statement = self.conn.prepare_cached(
            "INSERT OR REPLACE INTO stats (network_id, series, node_id, step, at, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for point in points {
            statement.execute(params![
                SqlNetwork(point.network),
                code(point.series),
                node_column(point.node),
                step,
                Unsigned(point.at),
                point.value
            ])?;
        }
        Ok(())
    }

    /// 把 `[start, start + to_step)` 里步长是 `from_step` 的点按序列平均成一个步长是 `to_step` 的点。
    /// 这一段里一个点都没有的序列不写：图上是断开的。返回写了几个点。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn roll_up_stats(&self, from_step: u32, to_step: u32, start: Timestamp) -> Result<usize, StoreError> {
        let end = start.saturating_add(u64::from(to_step));
        Ok(self.conn.execute(
            "INSERT OR REPLACE INTO stats (network_id, series, node_id, step, at, value)
             SELECT network_id, series, node_id, ?2, ?3, AVG(value) FROM stats
             WHERE step = ?1 AND at >= ?3 AND at < ?4
             GROUP BY network_id, series, node_id",
            params![from_step, to_step, Unsigned(start), Unsigned(end)],
        )?)
    }

    /// 删掉步长是 `step`、时间在 `before` 之前的点。返回删了几个。
    ///
    /// # Errors
    ///
    /// 写数据库失败。
    pub fn prune_stats(&self, step: u32, before: Timestamp) -> Result<usize, StoreError> {
        Ok(self.conn.execute("DELETE FROM stats WHERE step = ?1 AND at < ?2", params![step, Unsigned(before)])?)
    }

    /// 一条序列在 `[from, to]` 里步长是 `step` 的点，按时间排。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn stats(
        &self,
        network: NetworkId,
        series: Series,
        node: Option<NodeId>,
        step: u32,
        range: (Timestamp, Timestamp),
    ) -> Result<Vec<(Timestamp, f64)>, StoreError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT at, value FROM stats WHERE network_id = ?1 AND series = ?2 AND node_id = ?3 AND step = ?4 AND at >= ?5 AND at <= ?6
             ORDER BY at",
        )?;
        let rows = statement.query_map(
            params![SqlNetwork(network), code(series), node_column(node), step, Unsigned(range.0), Unsigned(range.1)],
            |row| Ok((row.get::<_, Unsigned>(0)?.0, row.get::<_, f64>(1)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 表里一共有多少个点。测试和日志用。
    ///
    /// # Errors
    ///
    /// 读数据库失败。
    pub fn stats_count(&self) -> Result<u64, StoreError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM stats", [], |row| row.get::<_, Unsigned>(0))?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixtures::home;

    fn point(network: NetworkId, series: Series, node: Option<u32>, at: Timestamp, value: f64) -> StatPoint {
        StatPoint { network, series, node: node.and_then(NodeId::new), at, value }
    }

    #[test]
    fn every_series_has_its_own_code() {
        let mut codes: Vec<i64> = Series::ALL.into_iter().map(code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Series::ALL.len());
    }

    #[test]
    fn points_are_read_back_per_series_node_and_step_and_rolled_up_into_averages() {
        let store = Store::open_in_memory().unwrap();
        let network = store.insert_network(&home()).unwrap();
        let points = [
            point(network, Series::NodeTx, Some(3), 3_600, 100.0),
            point(network, Series::NodeTx, Some(3), 3_900, 300.0),
            point(network, Series::NodeTx, Some(4), 3_600, 7.0),
            point(network, Series::Online, None, 3_600, 2.0),
            point(network, Series::NodeTx, Some(3), 7_200, 9.0),
        ];
        store.insert_stats(300, &points).unwrap();
        let node3 = NodeId::new(3);
        assert_eq!(store.stats(network, Series::NodeTx, node3, 300, (0, 4_000)).unwrap(), [(3_600, 100.0), (3_900, 300.0)]);
        assert_eq!(store.stats(network, Series::Online, None, 300, (0, 9_000)).unwrap(), [(3_600, 2.0)]);
        assert!(store.stats(network, Series::NodeRx, node3, 300, (0, 9_000)).unwrap().is_empty());

        assert_eq!(store.roll_up_stats(300, 3600, 3_600).unwrap(), 3, "三条序列在这一小时里有点");
        assert_eq!(store.stats(network, Series::NodeTx, node3, 3600, (0, 9_000)).unwrap(), [(3_600, 200.0)], "7200 的点在下一小时");
        store.insert_stats(300, &[point(network, Series::NodeTx, Some(3), 3_900, 500.0)]).unwrap();
        assert_eq!(store.stats(network, Series::NodeTx, node3, 300, (3_900, 3_900)).unwrap(), [(3_900, 500.0)], "同一段写两次留后一次");

        assert_eq!(store.prune_stats(300, 3_900).unwrap(), 3, "3600 的三个点");
        assert_eq!(store.stats_count().unwrap(), 2 + 3);
        store.delete_network(network).unwrap();
        assert_eq!(store.stats_count().unwrap(), 0, "删掉网络连同它的点");
    }
}
