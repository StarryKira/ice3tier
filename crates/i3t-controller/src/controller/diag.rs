//! 主动测试（白皮书 §7.6）在 controller 这一侧：管理员发起测试，controller 检查、给两端下发同一个编号和参数，
//! 两端各自上报，controller 合成结果、存下来（每对节点留最近的 [`crate::store::KEEP_PER_PAIR`] 个），以及全网矩阵（`matrix`）。
//!
//! 规矩：
//!
//! - 同一对节点两次测试至少隔 [`MIN_INTERVAL`]，带吞吐的至少隔 [`THROUGHPUT_INTERVAL`]；同一对节点同时只有一个测试。
//!   节点自己也守着差不多的间隔，controller 被攻破时节点照样守得住。
//! - 指令发出之前就知道做不了的不发：节点不在线、版本太旧（[`TEST_REVISION`] 之前）、访问规则没有连通，整个测试直接失败，
//!   原因里说是哪个节点；只经 relay 的节点不参加直连的两项，只用直连的节点不参加 relay 的那项，在本地关掉了吞吐测试的不量吞吐，
//!   这些项标成"没有做"并说明原因。
//! - 发起方报完最后的结果之后，controller 告诉两端测试结束（`TestEnd`），接受方随即报它最后的那一份（它那个方向上两种模式的选择）。
//!   接受方 [`RESPONDER_GRACE`] 之内没有报，或者整个测试过了时间窗还没有报完，就按手上有的结束，缺的那一端在结果里写明。
//! - 节点掉线时，它参与的测试按手上有的结束。管理员可以取消进行中的测试，已经有的结果照样留着。
//! - 定时重复测试不做：这是个人用的工具，管理员需要时再点一次；历史结果留着可以对比。

mod matrix;
mod merge;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use i3t_model::admin::diag::{CreateTest, ItemState, TestItem, TestQuery, TestRequest, TestState, TestView};
use i3t_model::admin::{LogRef, NetworkId};
use i3t_model::control::diag::{TestCode, TestItems, TestProblem, TestRole};
use i3t_model::control::{TEST_REVISION, TestOrder, TestReport, ToNode};
use i3t_model::netmap::Reach;
use i3t_proto::NodeId;

pub(crate) use self::matrix::Matrix;
use self::merge::{Inputs, Skips, final_state, merge};
use serde_json::json;

use super::{Controller, Inner, node_ref};
use crate::error::{Entity, Error, InField};
use crate::store::{NetworkRecord, NodeRecord};

/// 同一对节点两次测试至少隔多久，带吞吐的至少隔多久。
const MIN_INTERVAL: Duration = Duration::from_secs(10);
const THROUGHPUT_INTERVAL: Duration = Duration::from_secs(30);
/// 发起方报完之后等接受方的最后一份最多等多久。
const RESPONDER_GRACE: Duration = Duration::from_secs(5);
/// 时间窗之外再给上报留的余量。
const REPORT_GRACE: Duration = Duration::from_secs(15);
/// 节点那边时间窗的上限是 150 秒，这里算出来的不超过它。
const MAX_WINDOW_SECS: u32 = 150;
/// 列测试时默认和最多给几个。
const DEFAULT_LIST: u32 = 20;
const MAX_LIST: u32 = 200;
/// 一个测试最多指定几条 relay 路径：和默认测的条数一样，加上直连，一个带吞吐的测试最多量三条路径、六条测量流。
const MAX_VIA: usize = 2;

/// 进行中的测试和全网矩阵，只在内存里：controller 重启之后，进行中的测试在存储里显示成失败。
#[derive(Default)]
pub(crate) struct Diag {
    running: BTreeMap<u64, Running>,
    matrices: BTreeMap<NetworkId, Matrix>,
    next_matrix: u64,
    /// 每一对节点（编号小的在前）最近一次测试开始的时刻，和最近一次带吞吐的测试开始的时刻。
    last: BTreeMap<(NodeId, NodeId), (Instant, Option<Instant>)>,
}

impl Diag {
    /// 没有进行中的测试和矩阵：后台任务不用为它们醒来。
    pub(crate) fn is_idle(&self) -> bool {
        self.running.is_empty() && self.matrices.values().all(Matrix::is_finished)
    }
}

/// 一个进行中的测试。
struct Running {
    /// 结果的骨架：请求、两个节点、指令发出前就知道做不了的项。
    base: TestView,
    skips: Skips,
    initiator: Option<TestReport>,
    responder: Option<TestReport>,
    deadline: Instant,
    /// 发起方报完、controller 发出结束的时刻。
    ended: Option<Instant>,
}

impl Running {
    fn view(&self, finished: bool) -> TestView {
        merge(&Inputs {
            base: &self.base,
            skips: &self.skips,
            initiator: self.initiator.as_ref(),
            responder: self.responder.as_ref(),
            finished,
        })
    }
}

fn pair(a: NodeId, b: NodeId) -> (NodeId, NodeId) {
    (a.min(b), a.max(b))
}

impl Controller {
    /// 发起一个测试。做不了的测试也会建起来（状态是失败，原因里说是哪个节点），这样历史里看得到。
    ///
    /// # Errors
    ///
    /// 节点不在这个网络里（`not_found`）；请求不合法（`invalid_request`）；这一对节点刚测过或者正在测（`too_soon`、`busy`）。
    pub fn start_test(&self, network: NetworkId, input: &CreateTest) -> Result<TestView, Error> {
        let mut inner = self.lock();
        let record = inner.require_network(network)?;
        let (a, b) = (inner.require_node(network, input.a)?, inner.require_node(network, input.b)?);
        let request = inner.test_request(&record, input)?;
        inner.check_interval(a.id, b.id, request.throughput)?;
        let view = inner.begin_test(&record, &a, &b, request, None)?;
        self.wake_if_needed(&inner);
        Ok(view)
    }

    /// 一个测试：进行中的现在的样子，结束了的存下来的结果。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个测试。
    pub fn test(&self, network: NetworkId, id: u64) -> Result<TestView, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        if let Some(running) = inner.diag.running.get(&id).filter(|running| running.base.network == network) {
            return Ok(running.view(false));
        }
        inner.store.test(network, id)?.map(|view| inner.settled(view)).ok_or(Error::NotFound(Entity::Test))
    }

    /// 最近的测试，新的在前。写了 `a` 和 `b` 时只要这一对的。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者只写了 `a`、`b` 中的一个。
    pub fn tests(&self, network: NetworkId, query: &TestQuery) -> Result<Vec<TestView>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        let pair = match (query.a, query.b) {
            (Some(a), Some(b)) => Some((a, b)),
            (None, None) => None,
            (Some(_), None) => return Err(Error::invalid_field("b", "missing", "要看一对节点的测试时 a 和 b 都要写")),
            (None, Some(_)) => return Err(Error::invalid_field("a", "missing", "要看一对节点的测试时 a 和 b 都要写")),
        };
        let limit = query.limit.unwrap_or(DEFAULT_LIST).clamp(1, MAX_LIST);
        let stored = inner.store.tests(network, pair, limit)?;
        Ok(stored
            .into_iter()
            .map(|view| match inner.diag.running.get(&view.id) {
                Some(running) => running.view(false),
                None => inner.settled(view),
            })
            .collect())
    }

    /// 取消一个进行中的测试。已经有的结果照样留着。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个测试（`not_found`）；它已经结束了（`busy`）。
    pub fn cancel_test(&self, network: NetworkId, id: u64) -> Result<TestView, Error> {
        let mut inner = self.lock();
        inner.require_network(network)?;
        if inner.diag.running.get(&id).is_none_or(|running| running.base.network != network) {
            return match inner.store.test(network, id)? {
                Some(_) => Err(Error::Busy(format!("测试 #{id} 已经结束了"))),
                None => Err(Error::NotFound(Entity::Test)),
            };
        }
        let view = inner.finish_test(id, true)?;
        let detail = format!("取消了节点 {} 和 {} 之间的主动测试 #{id}", view.a.name, view.b.name);
        inner.store.log_refs(
            inner.clock.now(),
            Some(network),
            "test.cancel",
            &detail,
            &[LogRef::node(view.a.id), LogRef::node(view.b.id)],
            json!({ "test": id, "a": view.a.id, "a_name": view.a.name, "b": view.b.id, "b_name": view.b.name }),
        )?;
        self.wake_if_needed(&inner);
        Ok(view)
    }
}

impl Inner {
    /// 把请求规整一遍：测试项去重排好，指定的路径要是这个网络里启用了的 relay、不超过最大中转级数。
    fn test_request(&self, network: &NetworkRecord, input: &CreateTest) -> Result<TestRequest, Error> {
        if input.a == input.b {
            return Err(Error::invalid_field("b", "same_node", "要测的是两个不同的节点"));
        }
        let mut items: Vec<TestItem> = input.items.clone().unwrap_or_else(|| TestItem::ALL.to_vec());
        if !input.via.is_empty() && !items.contains(&TestItem::RelayQuality) {
            items.push(TestItem::RelayQuality);
        }
        items.sort_unstable();
        items.dedup();
        if items.is_empty() {
            return Err(Error::invalid_field("items", "empty", "至少要做一项测试"));
        }
        if input.throughput && !items.iter().any(|item| matches!(item, TestItem::DirectQuality | TestItem::RelayQuality)) {
            return Err(Error::invalid_field("throughput", "needs_quality", "吞吐是在测质量的路径上量的：要同时测直连质量或者中继质量"));
        }
        if input.via.len() > MAX_VIA {
            return Err(Error::invalid_field("via", "too_many", format!("最多指定 {MAX_VIA} 条路径")));
        }
        for path in &input.via {
            if path.is_empty() || path.len() > usize::from(network.max_relay_hops) {
                return Err(Error::invalid_field(
                    "via",
                    "out_of_range",
                    format!("一条路径经过 1 到 {} 个中继（网络的最大中转级数）", network.max_relay_hops),
                ));
            }
            if path.iter().enumerate().any(|(index, relay)| path[..index].contains(relay)) {
                return Err(Error::invalid_field("via", "duplicate", "一条路径里同一个中继不能出现两次"));
            }
            for relay in path {
                let node = self.require_node(network.id, *relay).in_field("via")?;
                if node.relay.is_none() {
                    return Err(Error::invalid_field("via", "not_relay", format!("节点 {} 没有启用为中继，不能出现在路径里", node.name)));
                }
            }
        }
        Ok(TestRequest { items, throughput: input.throughput, via: input.via.clone() })
    }

    /// 这一对节点能不能现在测：没有正在进行的测试，离上一次足够久。
    fn check_interval(&self, a: NodeId, b: NodeId, throughput: bool) -> Result<(), Error> {
        let key = pair(a, b);
        if self.diag.running.values().any(|running| pair(running.base.a.id, running.base.b.id) == key) {
            return Err(Error::Busy("这一对节点正在测试，等它结束或者取消它".to_owned()));
        }
        let now = Instant::now();
        let Some(&(started, with_throughput)) = self.diag.last.get(&key) else { return Ok(()) };
        let wait = MIN_INTERVAL.saturating_sub(now.saturating_duration_since(started));
        let wait = match with_throughput.filter(|_| throughput) {
            Some(at) => wait.max(THROUGHPUT_INTERVAL.saturating_sub(now.saturating_duration_since(at))),
            None => wait,
        };
        if wait.is_zero() {
            return Ok(());
        }
        Err(Error::TooSoon(format!("这一对节点刚测过，{} 秒之后再试", wait.as_secs().max(1))))
    }

    /// 整个测试做不了的原因：有一端不在线、版本太旧，或者访问规则没有把它们连通。
    fn test_blocker(&self, a: &NodeRecord, b: &NodeRecord) -> Option<TestProblem> {
        for node in [a, b] {
            match self.live.get(&node.id) {
                None => return Some(TestProblem::new(TestCode::NodeOffline, Some(node.id))),
                Some(live) if live.protocol < TEST_REVISION => return Some(TestProblem::new(TestCode::NotSupported, Some(node.id))),
                Some(_) => {}
            }
        }
        let knows = |from: &NodeRecord, to: &NodeRecord| self.sync.get(&from.id).is_some_and(|sync| sync.knows(to.id));
        (!knows(a, b) || !knows(b, a)).then(|| TestProblem::new(TestCode::NotConnected, None))
    }

    /// 指令发出前就知道做不了的项。
    fn test_skips(&self, network: &NetworkRecord, a: &NodeRecord, b: &NodeRecord, request: &TestRequest) -> Result<Skips, Error> {
        let reach = |node: &NodeRecord| node.reach.unwrap_or(network.reach);
        let blamed = |wanted: Reach, code: TestCode| {
            [a, b].into_iter().find(|node| reach(node) == wanted).map(|node| TestProblem::new(code, Some(node.id)))
        };
        let has_relay = self.store.nodes(network.id)?.iter().any(|node| node.relay.is_some());
        let relay = blamed(Reach::DirectOnly, TestCode::DirectOnly)
            .or_else(|| (request.via.is_empty() && !has_relay).then(|| TestProblem::new(TestCode::NoRelay, None)));
        let throughput = [a, b]
            .into_iter()
            .find(|node| self.live.get(&node.id).is_some_and(|live| !live.throughput_tests))
            .map(|node| TestProblem::new(TestCode::ThroughputOff, Some(node.id)));
        Ok(Skips { direct: blamed(Reach::RelayOnly, TestCode::RelayOnly), relay, throughput })
    }

    /// 建起一个测试并下发指令。调用前已经查过间隔。
    pub(super) fn begin_test(
        &mut self,
        network: &NetworkRecord,
        a: &NodeRecord,
        b: &NodeRecord,
        request: TestRequest,
        matrix: Option<u64>,
    ) -> Result<TestView, Error> {
        let started_at = self.clock.now();
        let mut view = TestView {
            id: 0,
            network: network.id,
            a: node_ref(a),
            b: node_ref(b),
            state: TestState::Running,
            request,
            started_at,
            finished_at: None,
            matrix,
            error: None,
            direct: None,
            direct_quality: None,
            relay_quality: Vec::new(),
            route: None,
            notes: Vec::new(),
        };
        let wants = |item| view.request.items.contains(&item);
        let skips = self.test_skips(network, a, b, &view.request)?;
        let items = TestItems {
            direct: wants(TestItem::Direct) && skips.direct.is_none(),
            direct_quality: wants(TestItem::DirectQuality) && skips.direct.is_none(),
            relay_quality: wants(TestItem::RelayQuality) && skips.relay.is_none(),
            route: wants(TestItem::Route),
            throughput: false,
        };
        let items = TestItems {
            throughput: view.request.throughput && skips.throughput.is_none() && (items.direct_quality || items.relay_quality),
            ..items
        };
        let blocker = self.test_blocker(a, b);
        let nothing_to_do = !(items.direct || items.direct_quality || items.relay_quality || items.route);
        if blocker.is_some() || nothing_to_do {
            // 指令不用发：要么做不了，要么每一项都事先知道做不了。
            view.state = if blocker.is_some() { TestState::Failed } else { TestState::Done };
            view.error.clone_from(&blocker);
            view.finished_at = Some(started_at);
            view.id = self.store.insert_test(&view)?;
            // 整个测试做不了时，每一项都是"没有做"，原因就是那一条。
            let skips = match &blocker {
                Some(problem) => Skips { direct: Some(problem.clone()), relay: Some(problem.clone()), throughput: Some(problem.clone()) },
                None => skips,
            };
            let running = Running { base: view.clone(), skips, initiator: None, responder: None, deadline: Instant::now(), ended: None };
            let mut view = TestView { state: view.state, error: view.error.clone(), finished_at: view.finished_at, ..running.view(false) };
            if let Some(route) = view.route.as_mut().filter(|route| route.state == ItemState::Pending) {
                route.state = ItemState::Skipped;
                route.problem.clone_from(&blocker);
            }
            self.store.update_test(&view)?;
            self.log_test(&view)?;
            return Ok(view);
        }
        view.id = self.store.insert_test(&view)?;
        let window = window_secs(&items, &view.request);
        let (id, paths) = (view.id, view.request.via.clone());
        let order = |peer: NodeId, role| TestOrder { test: id, peer, role, items, paths: paths.clone(), window_secs: window };
        tracing::info!(test = view.id, a = %a.id, b = %b.id, ?items, window_secs = window, "开始主动测试");
        let now = Instant::now();
        let deadline = now + Duration::from_secs(u64::from(window)) + REPORT_GRACE;
        let running = Running { base: view, skips, initiator: None, responder: None, deadline, ended: None };
        let started = running.view(false);
        self.log_test(&started)?;
        self.diag.last.insert(pair(a.id, b.id), (now, items.throughput.then_some(now)));
        self.diag.running.insert(id, running);
        // 接受方先拿到指令：发起方的第一个探测到的时候，它的时间窗已经开着了。
        let sent = self.send(b.id, ToNode::Test(Box::new(order(a.id, TestRole::Responder))))
            && self.send(a.id, ToNode::Test(Box::new(order(b.id, TestRole::Initiator))));
        if !sent && self.diag.running.contains_key(&id) {
            // 发的时候节点刚好掉线（发送失败会把它记为离线，测试也就随之结束了；这里只是兜底）。
            return self.finish_test(id, false);
        }
        Ok(self.diag.running.get(&id).map_or(started, |running| running.view(false)))
    }

    fn log_test(&self, view: &TestView) -> Result<(), Error> {
        let items: Vec<&str> = view
            .request
            .items
            .iter()
            .map(|item| match item {
                TestItem::Direct => "直连能力",
                TestItem::DirectQuality => "直连质量",
                TestItem::RelayQuality => "中继质量",
                TestItem::Route => "选路",
            })
            .collect();
        let throughput = if view.request.throughput { "，含吞吐" } else { "" };
        let detail = format!("节点 {} 和 {} 之间的主动测试 #{}：{}{throughput}", view.a.name, view.b.name, view.id, items.join("、"));
        let refs = [LogRef::node(view.a.id), LogRef::node(view.b.id)];
        let params = json!({
            "test": view.id,
            "a": view.a.id,
            "a_name": view.a.name,
            "b": view.b.id,
            "b_name": view.b.name,
            "items": view.request.items,
            "throughput": view.request.throughput,
            "matrix": view.matrix,
        });
        self.store.log_refs(view.started_at, Some(view.network), "test.start", &detail, &refs, params)?;
        Ok(())
    }

    /// 节点报来一个测试的结果。不是这个测试的两端、或者角色不对的不算数。
    pub(super) fn test_reported(&mut self, node: NodeId, report: TestReport) -> Result<(), Error> {
        let id = report.test;
        let Some(running) = self.diag.running.get_mut(&id) else {
            tracing::debug!(%node, test = id, "丢掉一份测试结果：没有在进行这个测试（多半已经结束了）");
            return Ok(());
        };
        let role = if node == running.base.a.id {
            TestRole::Initiator
        } else if node == running.base.b.id {
            TestRole::Responder
        } else {
            tracing::debug!(%node, test = id, "丢掉一份测试结果：报的节点不是这个测试的两端");
            return Ok(());
        };
        if report.role.is_some_and(|reported| reported != role) {
            tracing::debug!(%node, test = id, "丢掉一份测试结果：角色对不上");
            return Ok(());
        }
        let refused = report.refused.is_some();
        let last = report.last;
        match role {
            TestRole::Initiator => running.initiator = Some(report),
            // 接受方的最后一份之后再来的（不该有）不覆盖它。
            TestRole::Responder if running.responder.as_ref().is_none_or(|known| !known.last) => running.responder = Some(report),
            TestRole::Responder => {}
        }
        let initiator_done = running.initiator.as_ref().is_some_and(|report| report.last);
        let responder_done = running.responder.as_ref().is_some_and(|report| report.last);
        tracing::info!(test = id, %node, ?role, last, refused, "收到主动测试的结果");
        if refused || (initiator_done && responder_done) {
            self.finish_test(id, false)?;
        } else if initiator_done && running.ended.is_none() {
            // 发起方量完了：告诉接受方结束，它随即报最后的一份。
            running.ended = Some(Instant::now());
            let responder = running.base.b.id;
            self.send(responder, ToNode::TestEnd { test: id });
        } else {
            self.store_progress(id)?;
        }
        Ok(())
    }

    /// 进行中的测试的中间结果也存一份：界面轮询的是内存里的，存储里的只是为了 controller 意外退出时少丢一点。
    fn store_progress(&self, id: u64) -> Result<(), Error> {
        if let Some(running) = self.diag.running.get(&id) {
            self.store.update_test(&running.view(false))?;
        }
        Ok(())
    }

    /// 结束一个测试：合成最后的结果、存下来、告诉两端结束。`cancelled` 是管理员取消的。
    pub(super) fn finish_test(&mut self, id: u64, cancelled: bool) -> Result<TestView, Error> {
        let Some(running) = self.diag.running.remove(&id) else { return Err(Error::NotFound(Entity::Test)) };
        let (state, error) = final_state(cancelled, running.initiator.as_ref(), running.responder.as_ref());
        let finished_at = self.clock.now();
        let error = error.or_else(|| (state == TestState::Failed).then(|| TestProblem::new(TestCode::NoReport, Some(running.base.a.id))));
        let view = TestView { state, error, finished_at: Some(finished_at), ..running.view(true) };
        self.store.update_test(&view)?;
        tracing::info!(test = id, ?state, "主动测试结束");
        for node in [running.base.a.id, running.base.b.id] {
            if self.live.contains_key(&node) {
                self.send(node, ToNode::TestEnd { test: id });
            }
        }
        if let Some(matrix) = view.matrix {
            self.matrix_test_finished(view.network, matrix, &view)?;
        }
        Ok(view)
    }

    /// 存储里的一个测试：进行中的却不在内存里，是 controller 重启前没做完的，显示成失败。
    fn settled(&self, view: TestView) -> TestView {
        if view.state != TestState::Running {
            return view;
        }
        let problem =
            TestProblem { code: TestCode::Internal, node: None, message: "controller 重启过，这个测试没有做完".to_owned() };
        TestView { state: TestState::Failed, error: Some(problem), ..view }
    }

    /// 到点了的：过了时间窗还没报完的测试按手上有的结束；接受方迟迟不报最后一份的也结束；矩阵里排着的接着开始。
    /// 返回下一次最晚什么时候再来看。
    pub(super) fn tests_tick(&mut self, now: Instant) -> Result<Option<Instant>, Error> {
        let due: Vec<u64> = self
            .diag
            .running
            .iter()
            .filter(|(_, running)| now >= running.deadline || running.ended.is_some_and(|ended| now >= ended + RESPONDER_GRACE))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            tracing::info!(test = id, "主动测试到时间了，按手上的结果结束");
            self.finish_test(id, false)?;
        }
        self.schedule_matrices()?;
        let next = self
            .diag
            .running
            .values()
            .map(|running| running.ended.map_or(running.deadline, |ended| (ended + RESPONDER_GRACE).min(running.deadline)))
            .min();
        let cooling = self.matrix_wake(now);
        Ok([next, cooling].into_iter().flatten().min())
    }

    /// 节点掉线了：它参与的测试不会再有它的结果，按手上有的结束。
    pub(crate) fn tests_node_gone(&mut self, node: NodeId) {
        let involved: Vec<u64> = self
            .diag
            .running
            .iter()
            .filter(|(_, running)| running.base.a.id == node || running.base.b.id == node)
            .map(|(id, _)| *id)
            .collect();
        for id in involved {
            tracing::info!(test = id, %node, "节点掉线，主动测试按手上的结果结束");
            if let Err(error) = self.finish_test(id, false) {
                tracing::warn!(test = id, "结束主动测试时存储出错：{error}");
            }
        }
    }
}

/// 给节点的时间窗多长：每一项大概要多久，加起来，不超过节点的上限。
fn window_secs(items: &TestItems, request: &TestRequest) -> u32 {
    // 直连尝试 8 秒；一列探测 2 秒，等路径就绪最多 6 秒；吞吐上下行各 5 秒加收尾。
    let throughput = if items.throughput { 20 } else { 0 };
    let paths = if request.via.is_empty() { 2 } else { u32::try_from(request.via.len()).unwrap_or(4) };
    let mut window = 10;
    if items.direct || items.direct_quality {
        window += 10;
    }
    if items.direct_quality {
        window += 4 + throughput;
    }
    if items.relay_quality {
        window += 6 + paths * (4 + throughput);
    }
    if items.route {
        window += 2;
    }
    window.min(MAX_WINDOW_SECS)
}

#[cfg(test)]
mod tests {
    use i3t_model::admin::UpdateNode;
    use i3t_model::admin::diag::{CellState, ItemState, MatrixState, TestItem};
    use i3t_model::control::diag::{Choice, ChoiceCode, Decision, DirectSide, Latency, PathMeasure, PathPick};
    use i3t_model::control::{Declared, DirectKind, Hello, Route, ToController};

    use super::*;
    use crate::controller::testing::{Lab, Member, hello, modern_hello};

    fn two_nodes() -> (Lab, NetworkId, Member, Member) {
        let mut lab = Lab::new();
        let network = lab.network("home").id;
        let mut a = lab.join_as(network, &modern_hello("0.2.0"));
        let mut b = lab.join_as(network, &modern_hello("0.2.0"));
        lab.settle(&mut [&mut a, &mut b]);
        (lab, network, a, b)
    }

    fn request(a: &Member, b: &Member) -> CreateTest {
        CreateTest { a: a.id, b: b.id, items: None, throughput: false, via: Vec::new() }
    }

    fn decision() -> Decision {
        let choice = Choice {
            route: Some(Route::Direct { addr: "203.0.113.7:20330".parse().unwrap() }),
            reason: ChoiceCode::Direct,
            cost_us: Some(12_000),
            cheaper: None,
        };
        Decision { mode: None, current: None, direct_first: choice.clone(), lowest_cost: choice, candidates: Vec::new() }
    }

    fn initiator_report(test: u64) -> TestReport {
        let side = DirectSide { ok: true, kind: Some(DirectKind::Punched), elapsed_ms: Some(640), ..DirectSide::default() };
        let path = PathMeasure {
            route: Route::Direct { addr: "203.0.113.7:20330".parse().unwrap() },
            picked_by: vec![PathPick::Direct],
            failure: None,
            latency: Some(Latency { probes: 100, answered: 100, avg_us: Some(12_000), ..Latency::default() }),
            cost_us: Some(12_000),
            hops: Vec::new(),
            relay_leg: None,
            throughput: None,
        };
        TestReport {
            test,
            role: Some(TestRole::Initiator),
            last: true,
            direct: Some(side),
            paths: vec![path],
            decision: Some(decision()),
            ..TestReport::default()
        }
    }

    fn responder_report(test: u64, last: bool) -> TestReport {
        let side = DirectSide { ok: true, kind: Some(DirectKind::Punched), ..DirectSide::default() };
        TestReport {
            test,
            role: Some(TestRole::Responder),
            last,
            direct: Some(side),
            decision: last.then(decision),
            ..TestReport::default()
        }
    }

    fn send(lab: &Lab, member: &Member, report: TestReport) {
        member.send(&lab.controller, ToController::TestReport(Box::new(report)));
    }

    #[test]
    fn a_test_gives_both_ends_the_same_id_and_merges_what_they_report() {
        let (lab, network, mut a, mut b) = two_nodes();
        let view = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        assert_eq!((view.state, view.request.items.len()), (TestState::Running, 4));
        assert_eq!(view.direct.as_ref().map(|direct| direct.state), Some(ItemState::Pending));
        a.receive();
        b.receive();
        let (to_a, to_b) = (&a.tests[0], &b.tests[0]);
        assert_eq!((to_a.test, to_a.peer, to_a.role), (view.id, b.id, TestRole::Initiator));
        assert_eq!((to_b.test, to_b.peer, to_b.role), (view.id, a.id, TestRole::Responder));
        assert_eq!(to_a.items, to_b.items);
        assert!(to_a.items.direct && to_a.items.route && !to_a.items.throughput);

        send(&lab, &b, responder_report(view.id, false));
        assert_eq!(lab.controller.test(network, view.id).unwrap().state, TestState::Running);
        send(&lab, &a, initiator_report(view.id));
        b.receive();
        assert_eq!(b.test_ends, [view.id], "发起方报完之后接受方被告知结束");
        send(&lab, &b, responder_report(view.id, true));
        let done = lab.controller.test(network, view.id).unwrap();
        assert_eq!(done.state, TestState::Done);
        assert!(done.finished_at.is_some());
        let direct = done.direct.unwrap();
        assert!(direct.ok && direct.kind == Some(DirectKind::Punched) && direct.elapsed_ms == Some(640));
        assert_eq!(done.direct_quality.unwrap().latency.and_then(|latency| latency.avg_us), Some(12_000));
        let route = done.route.unwrap();
        assert!(route.forward.is_some() && route.reverse.is_some());
        a.receive();
        assert_eq!(a.test_ends, [view.id]);

        let listed = lab.controller.tests(network, &TestQuery { a: Some(b.id), b: Some(a.id), limit: None }).unwrap();
        assert_eq!(listed.iter().map(|view| view.id).collect::<Vec<_>>(), [view.id], "先后无所谓");
        let log = lab.controller.oplog(5).unwrap();
        assert!(log.iter().any(|entry| entry.action == "test.start"), "{log:?}");
        // 别人冒充着报的、报错了测试的，都不算数。
        send(&lab, &a, initiator_report(view.id));
        assert_eq!(lab.controller.test(network, view.id).unwrap().direct_quality.unwrap().state, ItemState::Done);
    }

    #[test]
    fn a_test_that_cannot_run_fails_at_once_and_says_which_node() {
        let (mut lab, network, a, b) = two_nodes();
        let mut old = lab.join_as(network, &hello(Declared::default()));
        lab.settle(&mut [&mut old]);
        let view = lab.controller.start_test(network, &CreateTest { b: old.id, ..request(&a, &b) }).unwrap();
        assert_eq!(view.direct.as_ref().map(|direct| direct.state), Some(ItemState::Skipped), "每一项都没有做，原因同上");
        assert_eq!(view.route.as_ref().and_then(|route| route.problem.as_ref()).map(|problem| problem.code), Some(TestCode::NotSupported));
        assert_eq!(
            (view.state, view.error.map(|problem| (problem.code, problem.node))),
            (TestState::Failed, Some((TestCode::NotSupported, Some(old.id))))
        );
        old.receive();
        assert!(old.tests.is_empty(), "旧节点不懂这条消息，不发给它");

        let offline = lab.join_offline(network, &[], Declared::default());
        let view = lab.controller.start_test(network, &CreateTest { b: offline, ..request(&a, &b) }).unwrap();
        assert_eq!(view.error.map(|problem| (problem.code, problem.node)), Some((TestCode::NodeOffline, Some(offline))));
        assert!(view.finished_at.is_some());
    }

    #[test]
    fn nodes_the_access_rules_keep_apart_are_not_tested() {
        let (lab, network, mut a, mut b) = two_nodes();
        for rule in lab.controller.rules(network).unwrap() {
            lab.controller.delete_rule(network, rule.id).unwrap();
        }
        lab.settle(&mut [&mut a, &mut b]);
        let view = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        assert_eq!(view.error.map(|problem| problem.code), Some(TestCode::NotConnected));
    }

    #[test]
    fn items_a_node_would_refuse_are_skipped_with_the_reason_and_left_out_of_the_order() {
        let mut lab = Lab::new();
        let network = lab.network("home").id;
        let mut a = lab.join_as(network, &modern_hello("0.2.0"));
        let mut b = lab.join_as(network, &Hello { throughput_tests: false, ..modern_hello("0.2.0") });
        lab.settle(&mut [&mut a, &mut b]);
        lab.controller.update_node(network, b.id, &UpdateNode { reach: Some(Some(Reach::RelayOnly)), ..UpdateNode::default() }).unwrap();
        lab.settle(&mut [&mut a, &mut b]);
        let view = lab.controller.start_test(network, &CreateTest { throughput: true, ..request(&a, &b) }).unwrap();
        let direct = view.direct.unwrap();
        assert_eq!(
            (direct.state, direct.problem.map(|problem| (problem.code, problem.node))),
            (ItemState::Skipped, Some((TestCode::RelayOnly, Some(b.id))))
        );
        assert_eq!(view.relay_quality[0].problem.as_ref().map(|problem| problem.code), Some(TestCode::NoRelay), "网络里没有 relay");
        a.receive();
        let order = &a.tests[0].items;
        assert!(!order.direct && !order.direct_quality && !order.relay_quality && order.route && !order.throughput, "{order:?}");
        let skips = lab.controller.lock().diag.running[&view.id].skips.clone();
        assert_eq!(skips.throughput.map(|problem| (problem.code, problem.node)), Some((TestCode::ThroughputOff, Some(b.id))));
    }

    #[test]
    fn the_same_pair_waits_between_tests_and_a_running_test_blocks_another() {
        let (lab, network, a, b) = two_nodes();
        let view = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        assert!(matches!(lab.controller.start_test(network, &request(&b, &a)), Err(Error::Busy(_))));
        lab.controller.cancel_test(network, view.id).unwrap();
        assert!(matches!(lab.controller.start_test(network, &request(&a, &b)), Err(Error::TooSoon(_))));
        // 十秒之后可以了；带吞吐的要三十秒。
        let earlier = Instant::now().checked_sub(Duration::from_secs(11)).unwrap();
        lab.controller.lock().diag.last.insert(pair(a.id, b.id), (earlier, Some(earlier)));
        assert!(matches!(lab.controller.start_test(network, &CreateTest { throughput: true, ..request(&a, &b) }), Err(Error::TooSoon(_))));
        let view = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        assert_eq!(view.state, TestState::Running);
        assert!(matches!(
            lab.controller.start_test(network, &CreateTest { a: a.id, b: a.id, ..request(&a, &b) }),
            Err(Error::InField { .. })
        ));
    }

    #[test]
    fn a_test_ends_with_what_it_has_when_time_runs_out_a_node_goes_away_or_it_is_cancelled() {
        let (mut lab, network, mut a, b) = two_nodes();
        let view = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        send(&lab, &a, initiator_report(view.id));
        // 接受方一直不报最后一份：过了宽限就按手上有的结束，结果里写明缺了谁。
        let past = Instant::now().checked_sub(RESPONDER_GRACE).unwrap();
        lab.controller.lock().diag.running.get_mut(&view.id).unwrap().ended = Some(past);
        lab.controller.tick().unwrap();
        let done = lab.controller.test(network, view.id).unwrap();
        assert_eq!(done.state, TestState::Done);
        assert_eq!(done.notes.iter().map(|note| (note.code, note.node)).collect::<Vec<_>>(), [(TestCode::NoReport, Some(b.id))]);

        lab.controller.lock().diag.last.clear();
        let view = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        lab.controller.disconnect(b.session);
        let gone = lab.controller.test(network, view.id).unwrap();
        assert_eq!((gone.state, gone.error.map(|problem| problem.code)), (TestState::Failed, Some(TestCode::NoReport)), "发起方什么都没报");

        let b = lab.connect_as(b.key, &modern_hello("0.2.0")).unwrap();
        lab.controller.lock().diag.last.clear();
        a.receive();
        let started = lab.controller.start_test(network, &request(&a, &b)).unwrap();
        let cancelled = lab.controller.cancel_test(network, started.id).unwrap();
        assert_eq!(cancelled.state, TestState::Cancelled);
        a.receive();
        assert!(a.test_ends.contains(&started.id), "取消时两端都被告知结束");
        assert!(matches!(lab.controller.cancel_test(network, started.id), Err(Error::Busy(_))), "已经结束了");
        assert!(matches!(lab.controller.cancel_test(network, 9999), Err(Error::NotFound(Entity::Test))));
    }

    #[test]
    fn the_matrix_tests_every_reachable_pair_never_more_than_four_at_a_time() {
        let mut lab = Lab::new();
        let network = lab.network("home").id;
        let mut members: Vec<Member> = (0..6).map(|_| lab.join_as(network, &modern_hello("0.2.0"))).collect();
        let offline = lab.join_offline(network, &[], Declared::default());
        lab.settle(&mut members.iter_mut().collect::<Vec<_>>());
        let run = lab.controller.start_matrix(network).unwrap();
        assert_eq!((run.total, run.concurrency), (21, 4), "七个节点二十一对");
        let skipped: Vec<&i3t_model::admin::diag::MatrixCell> = run.cells.iter().filter(|cell| cell.state == CellState::Skipped).collect();
        assert_eq!(skipped.len(), 6, "不在线的节点那六对不测");
        assert!(skipped.iter().all(|cell| cell.problem.as_ref().is_some_and(|problem| problem.node == Some(offline))));
        assert!(matches!(lab.controller.start_matrix(network), Err(Error::Busy(_))));

        let mut rounds = 0;
        loop {
            let running: Vec<u64> = lab.controller.lock().diag.running.keys().copied().collect();
            assert!(running.len() <= 4, "同时最多四个：{}", running.len());
            if running.is_empty() {
                break;
            }
            for test in running {
                let view = lab.controller.test(network, test).unwrap();
                assert_eq!(view.matrix, Some(run.id));
                assert!(!view.request.throughput && !view.request.items.contains(&TestItem::Route), "矩阵不量吞吐");
                let initiator = members.iter().find(|member| member.id == view.a.id).unwrap();
                let responder = members.iter().find(|member| member.id == view.b.id).unwrap();
                send(&lab, responder, responder_report(test, false));
                send(&lab, initiator, initiator_report(test));
                send(&lab, responder, responder_report(test, true));
            }
            rounds += 1;
            // 刚测过的对要等间隔：测试里把它拨过去。
            let earlier = Instant::now().checked_sub(MIN_INTERVAL).unwrap();
            lab.controller.lock().diag.last.values_mut().for_each(|(started, _)| *started = earlier);
            lab.controller.tick().unwrap();
        }
        let run = lab.controller.matrix(network).unwrap();
        assert_eq!((run.state, run.finished, rounds), (MatrixState::Done, 21, 4), "十五对、每批四对");
        let done: Vec<_> = run.cells.iter().filter(|cell| cell.state == CellState::Done).collect();
        assert_eq!(done.len(), 15);
        assert!(
            done.iter()
                .all(|cell| cell.direct_ok == Some(true) && cell.latency.is_some() && matches!(cell.route, Some(Route::Direct { .. })))
        );
        assert!(matches!(lab.controller.start_matrix(network), Err(Error::TooSoon(_))));
        assert!(matches!(lab.controller.cancel_matrix(network), Err(Error::Busy(_))));
    }

    #[test]
    fn a_cancelled_matrix_stops_its_tests_and_starts_no_more() {
        let mut lab = Lab::new();
        let network = lab.network("home").id;
        let mut members: Vec<Member> = (0..4).map(|_| lab.join_as(network, &modern_hello("0.2.0"))).collect();
        lab.settle(&mut members.iter_mut().collect::<Vec<_>>());
        lab.controller.start_matrix(network).unwrap();
        let cancelled = lab.controller.cancel_matrix(network).unwrap();
        assert_eq!(cancelled.state, MatrixState::Cancelled);
        assert!(cancelled.cells.iter().all(|cell| cell.state == CellState::Cancelled), "{:?}", cancelled.cells);
        assert!(lab.controller.lock().diag.running.is_empty());
        lab.controller.tick().unwrap();
        assert!(lab.controller.lock().diag.running.is_empty(), "取消之后不再开始新的");
    }
}
