//! 节点报上来的用量和状态在 controller 这一侧（白皮书 §9.2、§9.3、§9.6）：
//!
//! - **凭据**：每个入口报它这次运行以来每个凭据搬了多少字节。增量加到凭据的用量上（存在数据库里，见 `store::credential`）；
//!   用量到了流量上限，凭据在 netmap 里被标成用完，入口拒绝它的新连接并断开正在进行的。
//! - **出口**：出口报它自己的计数（按归因分的失败、配额、是不是在排空）；用它的节点各自报它们看到的。两份都留着给界面交叉核对。
//!   健康只看出口自己报的、归因于它自己的失败，取最近 5 分钟。
//! - **监听**：每个节点报它的代理监听器和端口转发的监听状态、连接数和流量，界面直接显示。
//!
//! 除了凭据的用量，这些都只在内存里：节点离线之后留着它最后报的那一份，controller 重启就没有了，等节点重新上报。

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use i3t_model::admin::{ExitHealth, ExitObserved, ExitView, ListenStatus, NetworkId, NodeForwardListen, NodeListener, NodeListeners};
use i3t_model::control::{CredentialUsage, ExitObservation, ExitReport};
use i3t_proto::NodeId;

use super::{Controller, Inner, Seeds, node_ref};
use crate::error::{Entity, Error};
use crate::store::NodeRecord;

/// 健康看最近这么久。
const HEALTH_WINDOW: Duration = Duration::from_secs(5 * 60);
/// 归因于出口自己的失败至少这么多次、并且占请求的这么大比例，才算不健康。偶尔一次解析器超时不算。
const DEGRADED_MIN_FAILURES: u64 = 3;
const DEGRADED_RATIO_PERCENT: u64 = 10;
/// 一个节点最多报这么多个监听、这么多个凭据、这么多个出口。乱报的节点占不了太多内存。
const MAX_LISTENS: usize = 256;
const MAX_CREDENTIALS: usize = 4096;
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
        inner.require_network(network)?;
        let nodes = inner.store.nodes(network)?;
        Ok(nodes.iter().filter_map(|node| inner.exit_view(&nodes, node)).collect())
    }

    /// 一个出口。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者它没有启用出口。
    pub fn exit(&self, network: NetworkId, id: NodeId) -> Result<ExitView, Error> {
        let inner = self.lock();
        inner.require_node(network, id)?;
        let nodes = inner.store.nodes(network)?;
        let node = nodes.iter().find(|node| node.id == id).ok_or(Error::NotFound(Entity::Node))?;
        inner.exit_view(&nodes, node).ok_or(Error::NotFound(Entity::Exit))
    }

    /// 一个节点上的全部监听：controller 下发的监听器、节点自己开的回环入口、端口转发的监听一侧，取自它最近的上报。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者读存储失败。
    pub fn node_listeners(&self, network: NetworkId, id: NodeId) -> Result<NodeListeners, Error> {
        let inner = self.lock();
        let record = inner.require_node(network, id)?;
        let configured = inner.store.listeners(network)?;
        let live = inner.live.get(&id);
        let listeners = live.map(|live| &live.listeners[..]).unwrap_or_default().iter().map(|report| NodeListener {
            id: configured
                .iter()
                .find(|listener| !report.builtin && listener.node == id && listener.config.name == report.name)
                .map(|listener| listener.id),
            builtin: report.builtin,
            name: report.name.clone(),
            bind: report.bind,
            protocol: report.protocol,
            status: ListenStatus { state: report.state.clone(), traffic: report.traffic },
        });
        let forwards = live.map(|live| &live.forwards[..]).unwrap_or_default().iter().map(|report| NodeForwardListen {
            id: report.id,
            bind: report.bind,
            protocol: report.protocol,
            status: ListenStatus { state: report.state.clone(), traffic: report.traffic },
        });
        Ok(NodeListeners {
            node: node_ref(&record),
            online: live.is_some(),
            reported_secs_ago: live.and_then(|live| live.reported_at).map(|at| at.elapsed().as_secs()),
            listeners: listeners.collect(),
            forwards: forwards.collect(),
        })
    }
}

impl Inner {
    fn exit_view(&self, nodes: &[NodeRecord], node: &NodeRecord) -> Option<ExitView> {
        let settings = node.exit.clone()?;
        let online = self.is_online(node.id);
        let seen = self.usage.exits.get(&node.id);
        let recent = seen.map(ExitSeen::recent).unwrap_or_default();
        let observed = self.usage.observed.iter().filter(|((exit, _), _)| *exit == node.id).filter_map(|(&(_, by), observed)| {
            let by = nodes.iter().find(|candidate| candidate.id == by)?;
            Some(ExitObserved { by: node_ref(by), counts: observed.counts.clone(), reported_secs_ago: observed.at.elapsed().as_secs() })
        });
        Some(ExitView {
            node: node_ref(node),
            online,
            settings,
            health: health(online, recent),
            recent_requests: recent.0,
            recent_failures: recent.1,
            usage: seen.map(|seen| seen.report.clone()),
            reported_secs_ago: seen.map(|seen| seen.at.elapsed().as_secs()),
            observed: observed.collect(),
        })
    }

    /// 一个监听器最近一次报的状态。
    pub(crate) fn listener_status(&self, node: NodeId, name: &str) -> Option<ListenStatus> {
        let live = self.live.get(&node)?;
        let report = live.listeners.iter().find(|report| !report.builtin && report.name == name)?;
        Some(ListenStatus { state: report.state.clone(), traffic: report.traffic })
    }

    /// 一条端口转发的监听一侧最近一次报的状态。
    pub(crate) fn forward_listen_status(&self, node: NodeId, id: u32) -> Option<ListenStatus> {
        let report = self.live.get(&node)?.forwards.iter().find(|report| report.id == id)?;
        Some(ListenStatus { state: report.state.clone(), traffic: report.traffic })
    }

    /// 一个凭据现在在各个在线的入口上一共有多少条连接。
    pub(crate) fn credential_connections(&self, network: NetworkId, id: u32) -> u32 {
        let live = self.live.values().filter(|live| live.network == network);
        live.filter_map(|live| live.credential_connections.get(&id)).fold(0, |total, connections| total.saturating_add(*connections))
    }

    /// 记下一个节点上报里的用量和状态。凭据因此用完（或者不再用完）时，下发给入口的 netmap 随之更新。
    pub(super) fn usage_reported(&mut self, node: &NodeRecord, report: UsageReport) -> Result<(), Error> {
        let UsageReport { run, credentials, exit, exits, listeners, forwards } = report;
        let now = Instant::now();
        if let Some(live) = self.live.get_mut(&node.id) {
            live.reported_at = Some(now);
            live.listeners = listeners.into_iter().take(MAX_LISTENS).collect();
            live.forwards = forwards.into_iter().take(MAX_LISTENS).collect();
            live.credential_connections = credentials.iter().take(MAX_CREDENTIALS).map(|usage| (usage.id, usage.connections)).collect();
        }
        if let Some(exit) = exit {
            self.usage.exit_reported(node.id, run, *exit, now);
        }
        self.usage.observed(node.id, exits, now);
        // 旧版本的节点不报运行编号，它的累计数没法安全地换成增量，不算。
        if run == 0 || credentials.is_empty() {
            return Ok(());
        }
        let wall = self.clock.now();
        let mut flipped = false;
        for usage in credentials.into_iter().take(MAX_CREDENTIALS) {
            flipped |= self.credential_used(node, run, &usage, wall)?;
        }
        if flipped {
            let seeds = self.nodes_requiring_credentials(node.network)?;
            self.refresh(node.network, seeds, None)?;
        }
        Ok(())
    }

    /// 把一个凭据的用量记上。返回它是不是因此用完了（或者不再用完）。
    fn credential_used(&mut self, node: &NodeRecord, run: u64, usage: &CredentialUsage, now: u64) -> Result<bool, Error> {
        // 入口只能报它自己网络里的凭据。
        let Some(before) = self.store.credential(node.network, usage.id)? else { return Ok(false) };
        let last_used = usage.last_used_secs_ago.map(|ago| now.saturating_sub(ago));
        let added = self.store.record_credential_usage(usage.id, node.id, run, usage.bytes, last_used)?;
        if added == 0 {
            return Ok(false);
        }
        let after = self.store.credential(node.network, usage.id)?.ok_or(Error::NotFound(Entity::Credential))?;
        let flipped = before.is_exhausted() != after.is_exhausted();
        if flipped {
            tracing::info!(credential = %after.credential.name, used = after.credential.usage.bytes, max = ?after.credential.max_bytes, "凭据的流量上限用完了，通知各个入口");
            let refs = [i3t_model::admin::LogRef::new(i3t_model::admin::RefKind::Credential, usage.id)];
            let detail = format!("凭据 {} 的流量上限用完了（已用 {} 字节）", after.credential.name, after.credential.usage.bytes);
            self.store.log_refs(now, Some(node.network), "credential.exhausted", &detail, &refs)?;
        }
        Ok(flipped)
    }

    /// 凭据只下发给有监听器要求凭据的节点，所以凭据变了只重算它们。
    pub(crate) fn nodes_requiring_credentials(&self, network: NetworkId) -> Result<Seeds, Error> {
        let listeners = self.store.listeners(network)?;
        Ok(Seeds::of(listeners.into_iter().filter(|listener| listener.config.require_credential).map(|listener| listener.node)))
    }
}

/// 一次上报里和用量、状态有关的部分。
pub(super) struct UsageReport {
    pub(super) run: u64,
    pub(super) credentials: Vec<CredentialUsage>,
    pub(super) exit: Option<Box<ExitReport>>,
    pub(super) exits: Vec<ExitObservation>,
    pub(super) listeners: Vec<i3t_model::control::ListenerReport>,
    pub(super) forwards: Vec<i3t_model::control::ForwardReport>,
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
