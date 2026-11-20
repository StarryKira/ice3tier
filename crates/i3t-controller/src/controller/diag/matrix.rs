//! 全网矩阵：对网络里所有的节点对做直连能力和延迟测试，分批进行，同时最多 [`CONCURRENCY`] 个。
//! 吞吐不进矩阵。每一对是一个普通的测试（历史里看得到），矩阵只记它们的进度和摘要。
//!
//! 节点不在线、版本太旧、访问规则没有连通的节点对不测，格子里写明原因。同一对节点刚单独测过的，等它过了间隔再测。
//! 矩阵只在内存里：controller 重启之后没有了，每一对的结果还在各自的测试里。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use i3t_model::admin::diag::{CellState, ItemState, MatrixCell, MatrixRun, MatrixState, TestItem, TestRequest, TestState, TestView};
use i3t_model::admin::{LogRef, NetworkId, RefKind, action};
use i3t_proto::NodeId;
use serde_json::json;

use super::{MIN_INTERVAL, pair};
use crate::controller::{Controller, Inner, count};
use crate::error::{Entity, Error};
use crate::store::NodeRecord;

/// 同时最多跑几个测试。一个节点同时出现在几对里也不会超过节点自己的上限（8 个）。
pub(crate) const CONCURRENCY: usize = 4;
/// 两次矩阵至少隔多久开始。
const MATRIX_INTERVAL: Duration = Duration::from_secs(60);

/// 一次矩阵。
pub(crate) struct Matrix {
    run: MatrixRun,
    started: Instant,
    /// 还没开始的节点对，按顺序。
    pending: Vec<(NodeId, NodeId)>,
    /// 正在跑的测试和它们的节点对。
    running: BTreeMap<u64, (NodeId, NodeId)>,
}

impl Matrix {
    pub(crate) fn is_finished(&self) -> bool {
        self.run.state != MatrixState::Running
    }

    fn cell_mut(&mut self, a: NodeId, b: NodeId) -> Option<&mut MatrixCell> {
        self.run.cells.iter_mut().find(|cell| (cell.a, cell.b) == pair(a, b))
    }

    fn count_finished(&mut self) {
        let finished = self.run.cells.iter().filter(|cell| !matches!(cell.state, CellState::Pending | CellState::Running)).count();
        self.run.finished = count(finished);
    }
}

impl Controller {
    /// 开始一次全网矩阵。
    ///
    /// # Errors
    ///
    /// 没有这个网络；这个网络正在跑一次矩阵（`busy`），或者上一次刚开始不久（`too_soon`）。
    pub fn start_matrix(&self, network: NetworkId) -> Result<MatrixRun, Error> {
        let mut inner = self.lock();
        let record = inner.require_network(network)?;
        if let Some(matrix) = inner.diag.matrices.get(&network) {
            if !matrix.is_finished() {
                return Err(Error::Busy("这个网络正在跑全网矩阵，等它结束或者取消它".to_owned()));
            }
            let wait = MATRIX_INTERVAL.saturating_sub(matrix.started.elapsed());
            if !wait.is_zero() {
                return Err(Error::TooSoon(format!("上一次全网矩阵刚开始不久，{} 秒之后再试", wait.as_secs().max(1))));
            }
        }
        let nodes = inner.store.nodes(network)?;
        inner.diag.next_matrix += 1;
        let id = inner.diag.next_matrix;
        let mut cells = Vec::new();
        let mut pending = Vec::new();
        for (index, a) in nodes.iter().enumerate() {
            for b in &nodes[index + 1..] {
                let blocker = inner.test_blocker(a, b);
                let state = if blocker.is_some() { CellState::Skipped } else { CellState::Pending };
                if blocker.is_none() {
                    pending.push((a.id, b.id));
                }
                cells.push(MatrixCell {
                    a: a.id,
                    b: b.id,
                    state,
                    test: None,
                    direct_ok: None,
                    direct_kind: None,
                    direct_elapsed_ms: None,
                    route: None,
                    latency: None,
                    problem: blocker,
                });
            }
        }
        let run = MatrixRun {
            id,
            network,
            state: MatrixState::Running,
            started_at: inner.clock.now(),
            finished_at: None,
            concurrency: count(CONCURRENCY),
            total: count(cells.len()),
            finished: 0,
            nodes: nodes.iter().map(NodeRecord::to_ref).collect(),
            cells,
        };
        let detail = format!("开始全网矩阵：{} 个节点，{} 对，其中 {} 对要测", nodes.len(), run.total, pending.len());
        let params = json!({ "matrix": id, "nodes": nodes.len(), "pairs": run.total, "testing": pending.len() });
        inner.store.log_refs(
            run.started_at,
            Some(network),
            action::TEST_MATRIX,
            &detail,
            &[LogRef::new(RefKind::Network, network.0)],
            params,
        )?;
        tracing::info!(network = %network, matrix = id, pairs = run.total, testing = pending.len(), "开始全网矩阵");
        let mut matrix = Matrix { run, started: Instant::now(), pending, running: BTreeMap::new() };
        matrix.count_finished();
        inner.diag.matrices.insert(network, matrix);
        inner.schedule_matrix(&record)?;
        let run = inner.matrix_run(network)?;
        self.wake_if_needed(&inner);
        Ok(run)
    }

    /// 这个网络最近一次全网矩阵。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者 controller 这次启动以来还没有跑过矩阵。
    pub fn matrix(&self, network: NetworkId) -> Result<MatrixRun, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        inner.matrix_run(network)
    }

    /// 取消正在跑的全网矩阵：正在跑的测试取消，还没开始的不再开始。
    ///
    /// # Errors
    ///
    /// 没有这个网络、没有矩阵（`not_found`），或者它已经结束了（`busy`）。
    pub fn cancel_matrix(&self, network: NetworkId) -> Result<MatrixRun, Error> {
        let mut inner = self.lock();
        inner.require_network(network)?;
        let Some(matrix) = inner.diag.matrices.get_mut(&network) else { return Err(Error::NotFound(Entity::Matrix)) };
        if matrix.is_finished() {
            return Err(Error::Busy("全网矩阵已经结束了".to_owned()));
        }
        // 先标成取消：下面结束各个测试时不会再开始新的。
        matrix.run.state = MatrixState::Cancelled;
        for &(a, b) in &std::mem::take(&mut matrix.pending) {
            if let Some(cell) = matrix.cell_mut(a, b) {
                cell.state = CellState::Cancelled;
            }
        }
        let running: Vec<u64> = matrix.running.keys().copied().collect();
        for test in running {
            inner.finish_test(test, true)?;
        }
        let now = inner.clock.now();
        let mut id = None;
        if let Some(matrix) = inner.diag.matrices.get_mut(&network) {
            matrix.run.finished_at = Some(now);
            matrix.count_finished();
            id = Some(matrix.run.id);
        }
        let refs = [LogRef::new(RefKind::Network, network.0)];
        inner.store.log_refs(now, Some(network), action::TEST_MATRIX_CANCEL, "取消了全网矩阵", &refs, json!({ "matrix": id }))?;
        tracing::info!(network = %network, "取消全网矩阵");
        inner.matrix_run(network)
    }
}

impl Inner {
    fn matrix_run(&self, network: NetworkId) -> Result<MatrixRun, Error> {
        self.diag.matrices.get(&network).map(|matrix| matrix.run.clone()).ok_or(Error::NotFound(Entity::Matrix))
    }

    /// 有空位就开始矩阵里排着的节点对。刚单独测过的对先跳过，等它过了间隔。
    pub(super) fn schedule_matrices(&mut self) -> Result<(), Error> {
        let networks: Vec<NetworkId> =
            self.diag.matrices.iter().filter(|(_, matrix)| !matrix.is_finished()).map(|(network, _)| *network).collect();
        for network in networks {
            if let Some(record) = self.store.network(network)? {
                self.schedule_matrix(&record)?;
            }
        }
        Ok(())
    }

    fn schedule_matrix(&mut self, network: &crate::store::NetworkRecord) -> Result<(), Error> {
        let now = Instant::now();
        loop {
            let Some(matrix) = self.diag.matrices.get(&network.id).filter(|matrix| !matrix.is_finished()) else { return Ok(()) };
            if matrix.running.len() >= CONCURRENCY {
                return Ok(());
            }
            let busy = |a: NodeId, b: NodeId| {
                let key = pair(a, b);
                self.diag.running.values().any(|running| pair(running.base.a.id, running.base.b.id) == key)
                    || self.diag.last.get(&key).is_some_and(|(started, _)| now.saturating_duration_since(*started) < MIN_INTERVAL)
            };
            let Some(index) = matrix.pending.iter().position(|&(a, b)| !busy(a, b)) else {
                self.finish_matrix_if_done(network.id);
                return Ok(());
            };
            let matrix_id = matrix.run.id;
            let (a, b) = self
                .diag
                .matrices
                .get_mut(&network.id)
                .map(|matrix| matrix.pending.remove(index))
                .ok_or(Error::NotFound(Entity::Matrix))?;
            let (Some(a_record), Some(b_record)) = (self.store.node(a)?, self.store.node(b)?) else {
                // 节点在矩阵开始之后被删掉了。
                if let Some(cell) = self.diag.matrices.get_mut(&network.id).and_then(|matrix| matrix.cell_mut(a, b)) {
                    cell.state = CellState::Skipped;
                }
                continue;
            };
            // 矩阵只测直连能力和延迟：有直连时量直连，没有时量默认会经的 relay。不量吞吐，不算选路。
            let request = TestRequest {
                items: vec![TestItem::Direct, TestItem::DirectQuality, TestItem::RelayQuality],
                throughput: false,
                via: Vec::new(),
            };
            let view = self.begin_test(network, &a_record, &b_record, request, Some(matrix_id))?;
            if let Some(matrix) = self.diag.matrices.get_mut(&network.id) {
                if view.state == TestState::Running {
                    matrix.running.insert(view.id, (a, b));
                }
                if let Some(cell) = matrix.cell_mut(a, b) {
                    fill_cell(cell, &view);
                }
                matrix.count_finished();
            }
        }
    }

    /// 矩阵里的一个测试结束了：记下摘要，空出来的位置给排着的。
    pub(super) fn matrix_test_finished(&mut self, network: NetworkId, matrix: u64, view: &TestView) -> Result<(), Error> {
        let Some(state) = self.diag.matrices.get_mut(&network).filter(|state| state.run.id == matrix) else { return Ok(()) };
        state.running.remove(&view.id);
        if let Some(cell) = state.cell_mut(view.a.id, view.b.id) {
            fill_cell(cell, view);
        }
        state.count_finished();
        if let Some(record) = self.store.network(network)? {
            self.schedule_matrix(&record)?;
        }
        Ok(())
    }

    fn finish_matrix_if_done(&mut self, network: NetworkId) {
        let now = self.clock.now();
        let Some(matrix) = self.diag.matrices.get_mut(&network) else { return };
        if matrix.is_finished() || !matrix.pending.is_empty() || !matrix.running.is_empty() {
            return;
        }
        matrix.run.state = MatrixState::Done;
        matrix.run.finished_at = Some(now);
        matrix.count_finished();
        tracing::info!(network = %network, matrix = matrix.run.id, "全网矩阵做完了");
    }

    /// 矩阵里有排着的节点对在等间隔过去时，什么时候再来看。
    pub(super) fn matrix_wake(&self, now: Instant) -> Option<Instant> {
        let waiting = self.diag.matrices.values().any(|matrix| !matrix.is_finished() && !matrix.pending.is_empty());
        waiting.then(|| now + Duration::from_secs(1))
    }
}

/// 一个测试的摘要写进矩阵的格子：能不能直连，延迟测在哪条路径上（有直连时是直连，否则是默认会经的 relay）。
fn fill_cell(cell: &mut MatrixCell, view: &TestView) {
    cell.test = Some(view.id);
    cell.state = match view.state {
        TestState::Running => CellState::Running,
        TestState::Done => CellState::Done,
        TestState::Failed => CellState::Failed,
        TestState::Cancelled => CellState::Cancelled,
        TestState::Unknown => CellState::Unknown,
    };
    if let Some(direct) = &view.direct {
        cell.direct_ok = (direct.state == ItemState::Done).then_some(direct.ok);
        cell.direct_kind = direct.kind;
        cell.direct_elapsed_ms = direct.elapsed_ms;
    }
    let measured =
        view.direct_quality.iter().chain(&view.relay_quality).find(|path| path.state == ItemState::Done && path.latency.is_some());
    cell.route = measured.and_then(|path| path.route.clone());
    cell.latency = measured.and_then(|path| path.latency);
    cell.problem = view.error.clone().or_else(|| {
        let direct = view.direct.as_ref().and_then(|direct| direct.problem.clone());
        if measured.is_some() { direct } else { direct.or_else(|| view.relay_quality.iter().find_map(|path| path.problem.clone())) }
    });
}
