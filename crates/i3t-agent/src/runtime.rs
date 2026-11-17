//! 一个运行着的节点的各个部分，放在一处：上报、回答路由查询、本机状态接口和指标都从这里读。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use i3t_access::Access;
use i3t_conn::{Endpoint, RouteChange};
use i3t_exit::{Exit, ExitStats, Os, Outbound};
use i3t_model::Netmap;
use i3t_model::control::{
    BackboneRow, CredentialUsage, ExitObservation, ExitReport, ForwardReport, InterceptReport, InterceptState, ListenTraffic,
    ListenerReport, PerformanceReport, QuotaUsage, Report, Route, RouteAnswer, RouteSince, SelfUpdate,
};
use i3t_model::update::UpdateStatus;
use i3t_proto::NodeId;
use tokio::sync::{broadcast, watch};

use crate::control::LinkState;
use crate::flows::{self, RelayActivity, Sample};
use crate::lanes::{self, NodeLanes};
use crate::node::BUILTIN_LISTENERS;
use crate::program::Program;
use crate::route_detail;
use crate::status::{listen_state, target_failure};

/// 节点里的出口：流经共用的连接或者 lane 进来。
pub(crate) type NodeExit = Exit<Os, lanes::Connector>;
/// 节点里的入口：TCP 流按节点对的设置走 lane 或者共用的连接。
pub(crate) type NodeAccess = Access<lanes::Connector, Outbound<Os>>;

/// 一个运行着的节点。
pub(crate) struct Runtime {
    pub(crate) program: Program,
    /// 这次运行能不能自己更新，和报给 controller 的是同一个结论。
    pub(crate) self_update: SelfUpdate,
    pub(crate) started: Instant,
    /// controller 的地址；静态模式下没有。
    pub(crate) controller: Option<String>,
    pub(crate) endpoint: Arc<Endpoint>,
    pub(crate) lanes: Arc<NodeLanes>,
    pub(crate) exit: Arc<NodeExit>,
    pub(crate) access: Arc<NodeAccess>,
    /// 各层实际在用的 netmap（按本机配置调整过的）。
    pub(crate) netmap: watch::Receiver<Arc<Netmap>>,
    /// 这次运行的编号，上报里带着，controller 靠它知道累计的计数是不是从头开始了。
    pub(crate) run: u64,
    /// 长连接的状态；静态模式下没有。
    pub(crate) link: Option<watch::Receiver<LinkState>>,
    /// 报给 controller 的更新状态。
    pub(crate) update: watch::Receiver<UpdateStatus>,
    /// 活跃的流的数量，更新等它变成零。
    pub(crate) flows: watch::Receiver<usize>,
    /// 正在用的性能档位。
    pub(crate) performance: watch::Receiver<PerformanceReport>,
    /// 透明访问的状态。
    pub(crate) intercept: watch::Receiver<InterceptState>,
    pub(crate) history: RouteHistory,
    relay_activity: Mutex<RelayActivity>,
}

/// [`Runtime::new`] 要的东西。
pub(crate) struct Parts {
    pub(crate) program: Program,
    pub(crate) self_update: SelfUpdate,
    pub(crate) controller: Option<String>,
    pub(crate) endpoint: Arc<Endpoint>,
    pub(crate) lanes: Arc<NodeLanes>,
    pub(crate) exit: Arc<NodeExit>,
    pub(crate) access: Arc<NodeAccess>,
    pub(crate) netmap: watch::Receiver<Arc<Netmap>>,
    pub(crate) link: Option<watch::Receiver<LinkState>>,
    pub(crate) update: watch::Receiver<UpdateStatus>,
    pub(crate) flows: watch::Receiver<usize>,
    pub(crate) performance: watch::Receiver<PerformanceReport>,
    pub(crate) intercept: watch::Receiver<InterceptState>,
}

impl Runtime {
    pub(crate) fn new(parts: Parts) -> Self {
        let Parts { program, self_update, controller, endpoint, lanes, exit, access, netmap, link, update, flows, performance, intercept } =
            parts;
        Self {
            program,
            self_update,
            started: Instant::now(),
            controller,
            endpoint,
            lanes,
            exit,
            access,
            netmap,
            // 0 留给"旧版本的节点没有报"。
            run: rand::random::<u64>().max(1),
            link,
            update,
            flows,
            performance,
            intercept,
            history: RouteHistory::default(),
            relay_activity: Mutex::default(),
        }
    }

    /// 现在的入口连接数、出口流数，以及（本节点是唯一的 relay 时）是不是在替人转发。
    pub(crate) fn sample_flows(&self) -> Sample {
        let status = self.access.status();
        let ingress: usize = status
            .listeners
            .iter()
            .map(|listener| listener.active.get())
            .chain(status.forwards.iter().map(|forward| forward.active.get()))
            .sum::<usize>()
            + self.access.intercept_status().active.get();
        let exit = usize::try_from(self.exit.stats().active).unwrap_or(usize::MAX);
        let lanes = usize::try_from(self.lanes.status().total.in_use).unwrap_or(usize::MAX);
        let forwarding =
            self.relay_activity.lock().expect("relay activity mutex poisoned").observe(&self.endpoint.relay_counters(), Instant::now());
        Sample { ingress, exit, lanes, sole_relay_forwarding: forwarding && flows::is_only_relay(&self.netmap.borrow()) }
    }

    /// 报给 controller 的内容。`routes` 是完整的路由表，长连接自己挑要发的条目。
    pub(crate) fn report(&self) -> Report {
        let stats = self.exit.stats();
        let access = self.access.status();
        let configured = &self.netmap.borrow().me.listeners;
        let listeners: Vec<ListenerReport> = access
            .listeners
            .iter()
            .map(|listener| ListenerReport {
                name: listener.name.clone(),
                bind: listener.bind,
                protocol: listener.protocol,
                // 名字是本机入口的名字、地址也是它的：本机配置文件开的那一个（controller 在同一个地址上配了监听器时，那一个不会被加进来）。
                builtin: BUILTIN_LISTENERS.contains(&listener.name.as_str())
                    && configured.iter().filter(|config| config.bind == listener.bind).count() <= 1,
                state: listen_state(&listener.state),
                traffic: ListenTraffic {
                    connections: count(listener.active.get()),
                    bytes_up: listener.traffic.up(),
                    bytes_down: listener.traffic.down(),
                },
            })
            .collect();
        let forwards: Vec<ForwardReport> = access
            .forwards
            .iter()
            .map(|forward| ForwardReport {
                id: forward.id,
                bind: forward.bind,
                protocol: forward.protocol,
                state: listen_state(&forward.state),
                traffic: ListenTraffic {
                    connections: count(forward.active.get()),
                    bytes_up: forward.traffic.up(),
                    bytes_down: forward.traffic.down(),
                },
                target_failure: forward.target_failure.as_ref().map(target_failure),
            })
            .collect();
        let intercept = self.intercept_report();
        let ingress_up: u64 = listeners
            .iter()
            .map(|listener| listener.traffic.bytes_up)
            .chain(forwards.iter().map(|forward| forward.traffic.bytes_up))
            .sum::<u64>()
            .saturating_add(intercept.up_bytes);
        let ingress_down: u64 = listeners
            .iter()
            .map(|listener| listener.traffic.bytes_down)
            .chain(forwards.iter().map(|forward| forward.traffic.bytes_down))
            .sum::<u64>()
            .saturating_add(intercept.down_bytes);
        Report {
            endpoints: self.endpoint.endpoints(),
            mapping_varies: self.endpoint.mapping_varies(),
            attached: self.endpoint.attached(),
            flows: u32::try_from(*self.flows.borrow()).unwrap_or(u32::MAX),
            // 入口一侧客户端发出的、出口一侧发往目标的，都算本节点经流发出的字节；反过来是收到的。
            tx_bytes: stats.bytes_to_target.saturating_add(ingress_up),
            rx_bytes: stats.bytes_from_target.saturating_add(ingress_down),
            routes: self.endpoint.routes(),
            routes_full: false,
            update: Some(self.update.borrow().clone()),
            run: self.run,
            credentials: self.credential_usage(),
            exit: Some(Box::new(exit_report(&stats))),
            exits: self.exit_observations(),
            listeners,
            forwards,
            route_kinds: self.endpoint.path_reports().iter().filter_map(route_detail::kind).collect(),
            lanes: Some(Box::new(self.lanes.report())),
            performance: Some(Box::new(self.performance.borrow().clone())),
            intercept: Some(Box::new(intercept)),
        }
    }

    /// 透明访问的状态和这一路的计数。
    pub(crate) fn intercept_report(&self) -> InterceptReport {
        let status = self.access.intercept_status();
        InterceptReport {
            state: self.intercept.borrow().clone(),
            active: count(status.active.get()),
            up_bytes: status.traffic.up(),
            down_bytes: status.traffic.down(),
        }
    }

    /// 各个接入凭据的用量。没有编号的（旧版本的 controller 下发的）报不了，跳过。
    pub(crate) fn credential_usage(&self) -> Vec<CredentialUsage> {
        let usage = self.access.credential_usage().into_iter().filter(|usage| usage.id != 0);
        usage
            .map(|usage| CredentialUsage {
                id: usage.id,
                bytes: usage.bytes_up.saturating_add(usage.bytes_down),
                connections: usage.connections,
                last_used_secs_ago: usage.last_used.map(|ago| ago.as_secs()),
            })
            .collect()
    }

    /// 本节点作为发起方，经各个出口出去的流。
    pub(crate) fn exit_observations(&self) -> Vec<ExitObservation> {
        let observed = self.access.exit_usage().into_iter();
        observed
            .map(|seen| ExitObservation {
                exit: seen.exit,
                flows: seen.flows,
                bytes_up: seen.bytes_up,
                bytes_down: seen.bytes_down,
                failed_by_target: seen.failed_by_target,
                refused: seen.refused,
                failed_by_exit: seen.failed_by_exit,
                unreachable: seen.unreachable,
            })
            .collect()
    }

    /// 回答"到 `peer` 现在会怎么走"：路由层手上现成的结论，不为此去探测或者建连接。
    pub(crate) fn route_to(&self, peer: NodeId) -> RouteAnswer {
        let entry = self.endpoint.routes().into_iter().find(|entry| entry.peer == peer);
        let known = self.netmap.borrow().peer(peer).is_some();
        match entry {
            Some(entry) => RouteAnswer {
                peer,
                route: Some(entry.route),
                rtt_ms: entry.rtt_ms,
                since: self.history.since(peer),
                detail: self.endpoint.path_report(peer).as_ref().map(route_detail::detail),
            },
            // 在 netmap 里、路由层还没有它的条目（刚下发）：对它还没有路。
            None if known => RouteAnswer { peer, route: Some(Route::None), rtt_ms: None, since: None, detail: None },
            None => RouteAnswer { peer, route: None, rtt_ms: None, since: None, detail: None },
        }
    }

    /// 回答"手上的骨干矩阵"：路由层现有的每一行。
    pub(crate) fn backbone(&self) -> Vec<BackboneRow> {
        route_detail::backbone(&self.endpoint.backbone())
    }
}

/// 出口一侧的计数写成上报的样子。
pub(crate) fn exit_report(stats: &ExitStats) -> ExitReport {
    ExitReport {
        active: stats.active,
        total: stats.total,
        bytes_to_target: stats.bytes_to_target,
        bytes_from_target: stats.bytes_from_target,
        failed_by_target: stats.failed_by_target,
        refused: stats.refused,
        failed_by_exit: stats.failed_by_exit,
        aborted: stats.aborted,
        draining: stats.draining,
        quota: stats.quota.map(|quota| QuotaUsage {
            limit_bytes: quota.limit_bytes,
            used_bytes: quota.used_bytes,
            period_start: quota.period.start,
            period_end: quota.period.end,
        }),
    }
}

/// 上报里的计数都是 `u32`。真的数不下时停在最大值。
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// 到每个对端的路上一次是什么时候、因为什么换的。路由层只在换路的那一刻说一次原因，这里把它记下来。
#[derive(Debug, Clone, Default)]
pub(crate) struct RouteHistory {
    changes: Arc<Mutex<BTreeMap<NodeId, (Instant, RouteChange)>>>,
}

impl RouteHistory {
    /// 到 `peer` 的路是多久之前换上的、为什么。这次启动以来没有换过时是 `None`。
    pub(crate) fn since(&self, peer: NodeId) -> Option<RouteSince> {
        let changes = self.changes.lock().expect("route history mutex poisoned");
        changes.get(&peer).map(|(at, change)| RouteSince {
            secs_ago: at.elapsed().as_secs(),
            reason: change.reason.clone(),
            cause: Some(change.cause.clone()),
        })
    }

    fn record(&self, change: RouteChange) {
        self.changes.lock().expect("route history mutex poisoned").insert(change.peer, (Instant::now(), change));
    }

    /// 一直记录换路事件，直到路由层关闭。
    pub(crate) async fn follow(self, mut changes: broadcast::Receiver<RouteChange>) {
        loop {
            match changes.recv().await {
                Ok(change) => self.record(change),
                // 落后太多被丢掉了几条：只是少记了几次原因，接着记后面的。
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use i3t_model::control::{RouteCause, RouteHint};

    use super::*;

    #[tokio::test]
    async fn the_latest_change_per_peer_is_remembered_with_its_reason() {
        let history = RouteHistory::default();
        let (tx, rx) = broadcast::channel(8);
        let following = tokio::spawn(history.clone().follow(rx));
        let peer = NodeId::new(2).unwrap();
        assert_eq!(history.since(peer), None);

        let lost = RouteCause::Unreachable { hint: RouteHint::DirectTimeouts };
        tx.send(RouteChange { peer, route: Route::None, reason: "直连超时".to_owned(), cause: lost }).unwrap();
        let relayed = Route::Relay { relays: vec![NodeId::new(9).unwrap()] };
        tx.send(RouteChange { peer, route: relayed, reason: "改走 relay".to_owned(), cause: RouteCause::RelayReady }).unwrap();
        for _ in 0..100 {
            if history.since(peer).is_some_and(|since| since.reason == "改走 relay") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let since = history.since(peer).unwrap();
        assert_eq!(since.reason, "改走 relay");
        assert_eq!(since.cause, Some(RouteCause::RelayReady), "原因的代码跟着原话一起记下");
        assert!(since.secs_ago <= 1);
        assert_eq!(history.since(NodeId::new(3).unwrap()), None);

        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), following).await.expect("路由层关闭之后应该停").unwrap();
    }
}
