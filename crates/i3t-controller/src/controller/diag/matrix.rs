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
    /// 正在跑的测试和它们的节点对，包括已经结束、结果还没存下来的：它们都存下来了矩阵才算结束。
    running: BTreeMap<u64, (NodeId, NodeId)>,
    /// 管理员取消了，还有测试没有收尾：不再开始新的；测试都收尾了才算取消完，这之前不能开始下一次。
    cancelling: bool,
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
        inner.require_network(network)?;
        if let Some(matrix) = inner.diag.matrices.get(&network) {
            if matrix.cancelling {
                return Err(Error::Busy("这个网络的全网矩阵正在取消，还有测试的结果没存下来，稍后再试".to_owned()));
            }
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
        let refs = [LogRef::new(RefKind::Network, network.0)];
        // 操作记录和它顺带的清理在一个事务里：写不进去时什么都没有变，可以再试。
        inner.store.atomically(|store| store.log_refs(run.started_at, Some(network), action::TEST_MATRIX, &detail, &refs, params))?;
        tracing::info!(network = %network, matrix = id, pairs = run.total, testing = pending.len(), "开始全网矩阵");
        let mut matrix = Matrix { run, started: Instant::now(), pending, running: BTreeMap::new(), cancelling: false };
        matrix.count_finished();
        inner.diag.matrices.insert(network, matrix);
        inner.schedule_matrix_or_later(network);
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
    /// 测试都收尾了矩阵才算取消完。有测试的结果存不下来时，它们的结果已经定成取消，后台任务接着存，存下来了矩阵才算取消完；
    /// 这期间可以再取消一次，不能开始下一次。
    ///
    /// # Errors
    ///
    /// 没有这个网络、没有矩阵（`not_found`），或者它已经结束了（`busy`）；读写存储失败。
    pub fn cancel_matrix(&self, network: NetworkId) -> Result<MatrixRun, Error> {
        let mut inner = self.lock();
        inner.require_network(network)?;
        let Some(matrix) = inner.diag.matrices.get(&network) else { return Err(Error::NotFound(Entity::Matrix)) };
        if matrix.is_finished() {
            return Err(Error::Busy("全网矩阵已经结束了".to_owned()));
        }
        // 已经在取消的是再取消一次：操作记过了，只是接着收尾。
        if !matrix.cancelling {
            let (id, at) = (matrix.run.id, inner.clock.now());
            let refs = [LogRef::new(RefKind::Network, network.0)];
            let params = json!({ "matrix": id });
            // 操作记录和它顺带的清理在一个事务里：写不进去时什么都没有变，可以再试。
            inner
                .store
                .atomically(|store| store.log_refs(at, Some(network), action::TEST_MATRIX_CANCEL, "取消了全网矩阵", &refs, params))?;
            tracing::info!(network = %network, "取消全网矩阵");
            if let Some(matrix) = inner.diag.matrices.get_mut(&network) {
                // 先标成在取消：下面结束各个测试时不会再开始新的。
                matrix.cancelling = true;
                for &(a, b) in &std::mem::take(&mut matrix.pending) {
                    if let Some(cell) = matrix.cell_mut(a, b) {
                        cell.state = CellState::Cancelled;
                    }
                }
                matrix.count_finished();
            }
        }
        let tests: Vec<u64> = inner.diag.matrices.get(&network).map(|matrix| matrix.running.keys().copied().collect()).unwrap_or_default();
        // 一个存不下来不耽误别的；存不下来的结果已经定成取消，后台任务接着存。
        let mut failed = None;
        for test in tests {
            // 前面结束的测试给节点发结束时，节点可能掉线，连带结束了这一个。
            if !inner.diag.running.contains_key(&test) {
                continue;
            }
            if let Err(error) = inner.finish_test(test, true) {
                tracing::warn!(test, "取消主动测试时存储出错，稍后再试：{error}");
                failed.get_or_insert(error);
            }
        }
        // 一个测试都没有在跑时，上面不会有测试结束来带着它收尾。
        inner.finish_matrix_if_done(network);
        self.wake_if_needed(&inner);
        match failed {
            Some(error) => Err(error),
            None => inner.matrix_run(network),
        }
    }
}

impl Inner {
    fn matrix_run(&self, network: NetworkId) -> Result<MatrixRun, Error> {
        self.diag.matrices.get(&network).map(|matrix| matrix.run.clone()).ok_or(Error::NotFound(Entity::Matrix))
    }

    /// 有空位就开始矩阵里排着的节点对。刚单独测过的对先跳过，等它过了间隔。
    /// 一个网络出错不耽误别的网络，最后交出第一个错误。
    pub(super) fn schedule_matrices(&mut self) -> Result<(), Error> {
        let networks: Vec<NetworkId> =
            self.diag.matrices.iter().filter(|(_, matrix)| !matrix.is_finished()).map(|(network, _)| *network).collect();
        let mut failed = None;
        for network in networks {
            if let Err(error) = self.schedule_network_matrix(network) {
                failed.get_or_insert(error);
            }
        }
        failed.map_or(Ok(()), Err)
    }

    /// 排一个网络的矩阵。出错时记下日志：排着的还排着，下次再排。
    fn schedule_network_matrix(&mut self, network: NetworkId) -> Result<(), Error> {
        let scheduled = match self.store.network(network) {
            Ok(Some(record)) => self.schedule_matrix(&record),
            Ok(None) => Ok(()),
            Err(error) => Err(error.into()),
        };
        if let Err(error) = &scheduled {
            tracing::warn!(network = %network, "全网矩阵开始下一对时出错，稍后再试：{error}");
        }
        scheduled
    }

    /// 出错时排着的还排着：这一对开始了（或者知道它不用测了）才从排着的里面拿掉。
    ///
    /// 开始不了的一对排到最后，这一轮就停下：下一轮先排别的对，一对一直开始不了也挡不住后面的；
    /// 存储整个写不进去时一轮也只多试一次，不会把排着的每一对都试一遍。
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
            let Some(&(a, b)) = matrix.pending.iter().find(|&&(a, b)| !busy(a, b)) else {
                self.finish_matrix_if_done(network.id);
                return Ok(());
            };
            let matrix_id = matrix.run.id;
            let view = match self.start_pair(network, matrix_id, a, b) {
                Ok(Some(view)) => view,
                Ok(None) => {
                    // 节点在矩阵开始之后被删掉了。
                    if let Some(matrix) = self.diag.matrices.get_mut(&network.id) {
                        matrix.pending.retain(|&queued| queued != (a, b));
                        if let Some(cell) = matrix.cell_mut(a, b) {
                            cell.state = CellState::Skipped;
                        }
                    }
                    continue;
                }
                Err(error) => {
                    if let Some(matrix) = self.diag.matrices.get_mut(&network.id) {
                        matrix.pending.retain(|&queued| queued != (a, b));
                        matrix.pending.push((a, b));
                    }
                    return Err(error);
                }
            };
            // 还在进行中的（包括结束了、结果还没存下来的）记进矩阵，存下来时矩阵才把它拿掉。
            let unfinished = self.diag.running.contains_key(&view.id);
            if let Some(matrix) = self.diag.matrices.get_mut(&network.id) {
                // 按值拿掉：指令发不出去时节点掉线、测试当场结束，矩阵会在这中间接着排别的对，位置可能已经变了。
                matrix.pending.retain(|&queued| queued != (a, b));
                if unfinished {
                    matrix.running.insert(view.id, (a, b));
                }
                if let Some(cell) = matrix.cell_mut(a, b) {
                    fill_cell(cell, &view);
                }
                matrix.count_finished();
            }
        }
    }

    /// 开始矩阵里的一对。两个节点有一个已经被删掉时返回 `None`。出错时什么都没有变。
    fn start_pair(&mut self, network: &crate::store::NetworkRecord, matrix: u64, a: NodeId, b: NodeId) -> Result<Option<TestView>, Error> {
        let (Some(a), Some(b)) = (self.store.node(a)?, self.store.node(b)?) else { return Ok(None) };
        // 矩阵只测直连能力和延迟：有直连时量直连，没有时量默认会经的 relay。不量吞吐，不算选路。
        let request = TestRequest {
            items: vec![TestItem::Direct, TestItem::DirectQuality, TestItem::RelayQuality],
            throughput: false,
            via: Vec::new(),
        };
        self.begin_test(network, &a, &b, request, Some(matrix)).map(Some)
    }

    /// 已经做成了的事（矩阵开始了、它的一个测试结束了）之后接着排。排不上时它们不算失败：排着的还排着，后台任务下次再排。
    fn schedule_matrix_or_later(&mut self, network: NetworkId) {
        // 出错已经记了日志。
        let _ = self.schedule_network_matrix(network);
    }

    /// 矩阵里的一个测试结束了：记下摘要，空出来的位置给排着的。
    pub(super) fn matrix_test_finished(&mut self, network: NetworkId, matrix: u64, view: &TestView) {
        let Some(state) = self.diag.matrices.get_mut(&network).filter(|state| state.run.id == matrix) else { return };
        state.running.remove(&view.id);
        if let Some(cell) = state.cell_mut(view.a.id, view.b.id) {
            fill_cell(cell, view);
        }
        state.count_finished();
        // 最后一个测试存下来了，矩阵就结束了（在取消的就算取消完了），不用等下面读存储。
        self.finish_matrix_if_done(network);
        self.schedule_matrix_or_later(network);
    }

    fn finish_matrix_if_done(&mut self, network: NetworkId) {
        let now = self.clock.now();
        let Some(matrix) = self.diag.matrices.get_mut(&network) else { return };
        if matrix.is_finished() || !matrix.pending.is_empty() || !matrix.running.is_empty() {
            return;
        }
        matrix.run.state = if matrix.cancelling { MatrixState::Cancelled } else { MatrixState::Done };
        matrix.run.finished_at = Some(now);
        matrix.count_finished();
        tracing::info!(network = %network, matrix = matrix.run.id, state = ?matrix.run.state, "全网矩阵结束了");
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
