//! 节点报上来的用量和状态在 controller 这一侧（白皮书 §9.3、§9.6）：
//!
//! - **出口**：出口报它自己的计数（按归因分的失败、配额、是不是在排空）；用它的节点各自报它们看到的。两份都留着给界面交叉核对。
//!   健康只看出口自己报的、归因于它自己的失败，取最近 5 分钟。
//! - **监听**：每个节点报它的代理、本机的组网入口和端口转发的监听状态、连接数和流量，界面直接显示。
//!
//! 这些都只在内存里：节点离线之后留着它最后报的那一份，controller 重启就没有了，等节点重新上报。

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use i3t_model::admin::{
    ExitHealth, ExitObserved, ExitView, ForwardFailure, ListenStatus, NetworkId, NodeForwardListen, NodeListener, NodeListeners,
};
use i3t_model::control::{ExitObservation, ExitReport, ForwardReport, ListenerReport, ProxyReport};
use i3t_proto::NodeId;

use super::{Controller, Inner};
use crate::access::AccessMap;
use crate::error::{Entity, Error};
use crate::netmap::NetworkView;
use crate::store::NodeRecord;

/// 健康看最近这么久。
const HEALTH_WINDOW: Duration = Duration::from_secs(5 * 60);
/// 归因于出口自己的失败至少这么多次、并且占请求的这么大比例，才算不健康。偶尔一次解析器超时不算。
const DEGRADED_MIN_FAILURES: u64 = 3;
const DEGRADED_RATIO_PERCENT: u64 = 10;
/// 一个节点最多报这么多个监听、这么多个出口。乱报的节点占不了太多内存。
const MAX_LISTENS: usize = 256;
const MAX_EXITS: usize = 256;

/// 出口和用出口的节点报的计数。
#[derive(Debug, Default)]
pub(crate) struct Usage {
    exits: BTreeMap<NodeId, ExitSeen>,
    /// （出口，发起节点）→ 发起节点看到的。
    observed: BTreeMap<(NodeId, NodeId), Observed>,
}

/// 一个出口最近报的计数，和最近几分钟的样本。
#[derive(Debug)]
struct ExitSeen {
    report: ExitReport,
    at: Instant,
    run: u64,
    /// 每次上报时的累计请求数和累计的出口自己的失败数，最老的一个在窗口之外，作为起点。
    samples: VecDeque<Sample>,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    total: u64,
    failed_by_exit: u64,
}

#[derive(Debug)]
struct Observed {
    counts: ExitObservation,
    at: Instant,
}

impl ExitSeen {
    /// 最近 5 分钟里的请求数和出口自己的失败数。
    fn recent(&self) -> (u64, u64) {
        match (self.samples.front(), self.samples.back()) {
            (Some(first), Some(last)) => (last.total.saturating_sub(first.total), last.failed_by_exit.saturating_sub(first.failed_by_exit)),
            _ => (0, 0),
        }
    }
}

impl Usage {
    fn exit_reported(&mut self, exit: NodeId, run: u64, report: ExitReport, now: Instant) {
        let sample = Sample { at: now, total: report.total, failed_by_exit: report.failed_by_exit };
        let seen = self.exits.entry(exit).or_insert_with(|| ExitSeen { report: report.clone(), at: now, run, samples: VecDeque::new() });
        // 出口重启了，累计的数从头开始：之前的样本不能和之后的相减。
        if seen.run != run {
            seen.samples.clear();
            seen.run = run;
        }
        seen.report = report;
        seen.at = now;
        seen.samples.push_back(sample);
        while seen.samples.len() > 2 && seen.samples.get(1).is_some_and(|second| now.saturating_duration_since(second.at) >= HEALTH_WINDOW)
        {
            seen.samples.pop_front();
        }
    }

    fn observed(&mut self, by: NodeId, observations: Vec<ExitObservation>, now: Instant) {
        for counts in observations.into_iter().take(MAX_EXITS) {
            self.observed.insert((counts.exit, by), Observed { counts, at: now });
        }
    }

    pub(crate) fn forget(&mut self, node: NodeId) {
        self.exits.remove(&node);
        self.observed.retain(|&(exit, by), _| exit != node && by != node);
    }
}

/// 一个出口最近 5 分钟的健康。
fn health(online: bool, (requests, failures): (u64, u64)) -> ExitHealth {
    if !online || requests == 0 {
        ExitHealth::Unknown
    } else if failures >= DEGRADED_MIN_FAILURES && failures * 100 >= requests * DEGRADED_RATIO_PERCENT {
        ExitHealth::Degraded
    } else {
        ExitHealth::Ok
    }
}

impl Controller {
    /// 一个网络里启用了的出口：设置、它自己报的计数、健康，以及用它的节点各自看到的。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn exits(&self, network: NetworkId) -> Result<Vec<ExitView>, Error> {
        let inner = self.lock();
        let view = NetworkView::load(&inner.store, network)?.ok_or(Error::NotFound(Entity::Network))?;
        let access = view.access();
        Ok(view.nodes.values().filter_map(|node| inner.exit_view(&view, &access, node)).collect())
    }

    /// 一个出口。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者它没有启用出口。
    pub fn exit(&self, network: NetworkId, id: NodeId) -> Result<ExitView, Error> {
        let inner = self.lock();
        inner.require_node(network, id)?;
        let view = NetworkView::load(&inner.store, network)?.ok_or(Error::NotFound(Entity::Network))?;
        let node = view.nodes.get(&id).ok_or(Error::NotFound(Entity::Node))?;
        inner.exit_view(&view, &view.access(), node).ok_or(Error::NotFound(Entity::Exit))
    }

    /// 一个节点上的全部监听：controller 下发的代理、节点按本机配置开的组网入口、端口转发的监听一侧，取自它最近的上报。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者读存储失败。
    pub fn node_listeners(&self, network: NetworkId, id: NodeId) -> Result<NodeListeners, Error> {
        let inner = self.lock();
        let record = inner.require_node(network, id)?;
        let configured = inner.store.proxies(network)?;
        let live = inner.live.get(&id);
        let proxies = live.map(|live| &live.proxies[..]).unwrap_or_default().iter().filter_map(|report| {
            // 刚删掉的代理节点还在报，不列。
            let proxy = configured.iter().find(|proxy| proxy.id == report.id && proxy.node == id)?;
            Some(NodeListener {
                id: Some(proxy.id),
                builtin: false,
                name: proxy.name.clone(),
                bind: report.bind,
                protocol: proxy.protocol,
                status: ListenStatus { state: report.state.clone(), traffic: report.traffic },
            })
        });
        // 比代理旧的节点在这里也报 controller 下发的监听器，那些已经不存在了，不列。
        let local =
            live.map(|live| &live.listeners[..]).unwrap_or_default().iter().filter(|report| report.builtin).map(|report| NodeListener {
                id: None,
                builtin: true,
                name: report.name.clone(),
                bind: report.bind,
                protocol: report.protocol,
                status: ListenStatus { state: report.state.clone(), traffic: report.traffic },
            });
        let listeners = proxies.chain(local);
        let forwards = live.map(|live| &live.forwards[..]).unwrap_or_default().iter().map(|report| NodeForwardListen {
            id: report.id,
            bind: report.bind,
            protocol: report.protocol,
            status: ListenStatus { state: report.state.clone(), traffic: report.traffic },
        });
        Ok(NodeListeners {
            node: record.to_ref(),
            online: live.is_some(),
            reported_secs_ago: live.and_then(|live| live.reported_at).map(|at| at.elapsed().as_secs()),
            listeners: listeners.collect(),
            forwards: forwards.collect(),
        })
    }
}

impl Inner {
    fn exit_view(&self, view: &NetworkView, access: &AccessMap<'_>, node: &NodeRecord) -> Option<ExitView> {
        let settings = node.exit.clone()?;
        let online = self.is_online(node.id);
        let seen = self.usage.exits.get(&node.id);
        let recent = seen.map(ExitSeen::recent).unwrap_or_default();
        let observed = self.usage.observed.iter().filter(|((exit, _), _)| *exit == node.id).filter_map(|(&(_, by), observed)| {
            let by = view.nodes.get(&by)?;
            Some(ExitObserved { by: by.to_ref(), counts: observed.counts.clone(), reported_secs_ago: observed.at.elapsed().as_secs() })
        });
        Some(ExitView {
            node: node.to_ref(),
            online,
            settings,
            health: health(online, recent),
            recent_requests: recent.0,
            recent_failures: recent.1,
            usage: seen.map(|seen| seen.report.clone()),
            reported_secs_ago: seen.map(|seen| seen.at.elapsed().as_secs()),
            observed: observed.collect(),
            proxies: view
                .proxies
                .iter()
                .filter(|proxy| proxy.exit == Some(node.id))
                .map(|proxy| self.proxy_view(view, access, proxy))
                .collect(),
        })
    }

    /// 一条端口转发的监听一侧最近一次报的状态。
    pub(crate) fn forward_listen_status(&self, node: NodeId, id: u32) -> Option<ListenStatus> {
        let report = self.live.get(&node)?.forwards.iter().find(|report| report.id == id)?;
        Some(ListenStatus { state: report.state.clone(), traffic: report.traffic })
    }

    /// 一条端口转发的流最近一次在目标那里失败，监听节点报的。时间换算成 controller 的时钟。
    pub(crate) fn forward_target_failure(&self, node: NodeId, id: u32) -> Option<ForwardFailure> {
        let live = self.live.get(&node)?;
        let failure = live.forwards.iter().find(|report| report.id == id)?.target_failure?;
        let since_report = live.reported_at.map_or(0, |at| at.elapsed().as_secs());
        Some(ForwardFailure { cause: failure.cause, at: self.clock.now().saturating_sub(failure.secs_ago.saturating_add(since_report)) })
    }

    /// 记下一个节点上报里的用量和状态。
    pub(super) fn usage_reported(&mut self, node: &NodeRecord, report: UsageReport) {
        let UsageReport { run, exit, exits, proxies, listeners, forwards } = report;
        let now = Instant::now();
        if let Some(live) = self.live.get_mut(&node.id) {
            live.reported_at = Some(now);
            live.proxies = proxies.into_iter().take(MAX_LISTENS).collect();
            live.listeners = listeners.into_iter().take(MAX_LISTENS).collect();
            live.forwards = forwards.into_iter().take(MAX_LISTENS).collect();
        }
        if let Some(exit) = exit {
            self.usage.exit_reported(node.id, run, *exit, now);
        }
        self.usage.observed(node.id, exits, now);
    }
}

/// 一次上报里和用量、状态有关的部分。
pub(super) struct UsageReport {
    pub(super) run: u64,
    pub(super) exit: Option<Box<ExitReport>>,
    pub(super) exits: Vec<ExitObservation>,
    pub(super) proxies: Vec<ProxyReport>,
    pub(super) listeners: Vec<ListenerReport>,
    pub(super) forwards: Vec<ForwardReport>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(total: u64, failed_by_exit: u64) -> ExitReport {
        ExitReport { total, failed_by_exit, ..ExitReport::default() }
    }

    #[test]
    fn health_counts_only_failures_of_the_exit_itself_over_the_last_five_minutes() {
        let mut usage = Usage::default();
        let exit = NodeId::new(2).unwrap();
        let start = Instant::now();
        usage.exit_reported(exit, 1, report(100, 0), start);
        usage.exit_reported(exit, 1, report(120, 4), start + Duration::from_secs(10));
        let seen = &usage.exits[&exit];
        assert_eq!(seen.recent(), (20, 4));
        assert_eq!(health(true, seen.recent()), ExitHealth::Degraded, "20 个请求里 4 个是出口自己的问题");
        assert_eq!(health(false, seen.recent()), ExitHealth::Unknown, "不在线时说不上来");
        assert_eq!(health(true, (100, 2)), ExitHealth::Ok, "偶尔两次不算");
        assert_eq!(health(true, (1000, 30)), ExitHealth::Ok, "不到一成不算");
        assert_eq!(health(true, (0, 0)), ExitHealth::Unknown, "没有请求就说不上来");

        // 5 分钟之后，早先的失败滑出窗口。
        usage.exit_reported(exit, 1, report(200, 4), start + HEALTH_WINDOW + Duration::from_secs(20));
        usage.exit_reported(exit, 1, report(300, 4), start + HEALTH_WINDOW + Duration::from_secs(30));
        assert_eq!(usage.exits[&exit].recent(), (180, 0));

        // 出口重启：累计的数从头开始，不和重启之前的相减。
        usage.exit_reported(exit, 2, report(5, 0), start + HEALTH_WINDOW + Duration::from_secs(40));
        assert_eq!(usage.exits[&exit].recent(), (0, 0));
    }

    #[test]
    fn observations_are_kept_per_exit_and_initiator_and_forgotten_with_either() {
        let mut usage = Usage::default();
        let (gw, laptop, phone) = (NodeId::new(2).unwrap(), NodeId::new(1).unwrap(), NodeId::new(3).unwrap());
        let seen = |exit, flows| ExitObservation {
            exit,
            flows,
            bytes_up: 0,
            bytes_down: 0,
            failed_by_target: 0,
            refused: 0,
            failed_by_exit: 0,
            unreachable: 0,
        };
        usage.observed(laptop, vec![seen(gw, 3)], Instant::now());
        usage.observed(phone, vec![seen(gw, 5)], Instant::now());
        usage.observed(laptop, vec![seen(gw, 4)], Instant::now());
        assert_eq!(usage.observed.len(), 2);
        assert_eq!(usage.observed[&(gw, laptop)].counts.flows, 4, "同一个发起节点的新报的顶替旧的");
        usage.forget(phone);
        assert_eq!(usage.observed.len(), 1);
        usage.forget(gw);
        assert!(usage.observed.is_empty());
    }
}
