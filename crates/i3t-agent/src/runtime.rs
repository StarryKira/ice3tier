//! 一个运行着的节点的各个部分，放在一处：上报、回答路由查询、本机状态接口和指标都从这里读。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use i3t_access::Access;
use i3t_conn::{Endpoint, RouteChange};
use i3t_exit::{Exit, Os, Outbound};
use i3t_model::Netmap;
use i3t_model::control::{Report, Route, RouteAnswer, RouteSince, SelfUpdate};
use i3t_model::update::UpdateStatus;
use i3t_proto::NodeId;
use tokio::sync::{broadcast, watch};

use crate::control::LinkState;
use crate::drain::DrainGate;
use crate::flows::{self, RelayActivity, Sample};
use crate::program::Program;

/// 一个运行着的节点。
pub(crate) struct Runtime {
    pub(crate) program: Program,
    /// 这次运行能不能自己更新，和报给 controller 的是同一个结论。
    pub(crate) self_update: SelfUpdate,
    pub(crate) started: Instant,
    /// controller 的地址；静态模式下没有。
    pub(crate) controller: Option<String>,
    pub(crate) endpoint: Arc<Endpoint>,
    pub(crate) exit: Arc<Exit>,
    pub(crate) gate: Arc<DrainGate<Exit>>,
    pub(crate) access: Arc<Access<Endpoint, Outbound<Os>>>,
    /// 各层实际在用的 netmap（按本机配置调整过的）。
    pub(crate) netmap: watch::Receiver<Arc<Netmap>>,
    /// 长连接的状态；静态模式下没有。
    pub(crate) link: Option<watch::Receiver<LinkState>>,
    /// 报给 controller 的更新状态。
    pub(crate) update: watch::Receiver<UpdateStatus>,
    /// 活跃的流的数量，更新等它变成零。
    pub(crate) flows: watch::Receiver<usize>,
    pub(crate) history: RouteHistory,
    relay_activity: Mutex<RelayActivity>,
}

/// [`Runtime::new`] 要的东西。
pub(crate) struct Parts {
    pub(crate) program: Program,
    pub(crate) self_update: SelfUpdate,
    pub(crate) controller: Option<String>,
    pub(crate) endpoint: Arc<Endpoint>,
    pub(crate) exit: Arc<Exit>,
    pub(crate) gate: Arc<DrainGate<Exit>>,
    pub(crate) access: Arc<Access<Endpoint, Outbound<Os>>>,
    pub(crate) netmap: watch::Receiver<Arc<Netmap>>,
    pub(crate) link: Option<watch::Receiver<LinkState>>,
    pub(crate) update: watch::Receiver<UpdateStatus>,
    pub(crate) flows: watch::Receiver<usize>,
}

impl Runtime {
    pub(crate) fn new(parts: Parts) -> Self {
        let Parts { program, self_update, controller, endpoint, exit, gate, access, netmap, link, update, flows } = parts;
        Self {
            program,
            self_update,
            started: Instant::now(),
            controller,
            endpoint,
            exit,
            gate,
            access,
            netmap,
            link,
            update,
            flows,
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
            .sum();
        let exit = usize::try_from(self.exit.stats().active).unwrap_or(usize::MAX);
        let forwarding =
            self.relay_activity.lock().expect("relay activity mutex poisoned").observe(&self.endpoint.relay_counters(), Instant::now());
        Sample { ingress, exit, sole_relay_forwarding: forwarding && flows::is_only_relay(&self.netmap.borrow()) }
    }

    /// 报给 controller 的内容。`routes` 是完整的路由表，长连接自己挑要发的条目。
    pub(crate) fn report(&self) -> Report {
        let stats = self.exit.stats();
        Report {
            endpoints: self.endpoint.endpoints(),
            mapping_varies: self.endpoint.mapping_varies(),
            attached: self.endpoint.attached(),
            flows: u32::try_from(*self.flows.borrow()).unwrap_or(u32::MAX),
            tx_bytes: stats.bytes_to_target,
            rx_bytes: stats.bytes_from_target,
            routes: self.endpoint.routes(),
            routes_full: false,
            update: Some(self.update.borrow().clone()),
        }
    }

    /// 回答"到 `peer` 现在会怎么走"：路由层手上现成的结论，不为此去探测或者建连接。
    pub(crate) fn route_to(&self, peer: NodeId) -> RouteAnswer {
        let entry = self.endpoint.routes().into_iter().find(|entry| entry.peer == peer);
        let known = self.netmap.borrow().peer(peer).is_some();
        match entry {
            Some(entry) => RouteAnswer { peer, route: Some(entry.route), rtt_ms: entry.rtt_ms, since: self.history.since(peer) },
            // 在 netmap 里、路由层还没有它的条目（刚下发）：对它还没有路。
            None if known => RouteAnswer { peer, route: Some(Route::None), rtt_ms: None, since: None },
            None => RouteAnswer { peer, route: None, rtt_ms: None, since: None },
        }
    }
}

/// 到每个对端的路上一次是什么时候、因为什么换的。路由层只在换路的那一刻说一次原因，这里把它记下来。
#[derive(Debug, Clone, Default)]
pub(crate) struct RouteHistory {
    changes: Arc<Mutex<BTreeMap<NodeId, (Instant, String)>>>,
}

impl RouteHistory {
    /// 到 `peer` 的路是多久之前换上的、为什么。这次启动以来没有换过时是 `None`。
    pub(crate) fn since(&self, peer: NodeId) -> Option<RouteSince> {
        let changes = self.changes.lock().expect("route history mutex poisoned");
        changes.get(&peer).map(|(at, reason)| RouteSince { secs_ago: at.elapsed().as_secs(), reason: reason.clone() })
    }

    fn record(&self, change: RouteChange) {
        self.changes.lock().expect("route history mutex poisoned").insert(change.peer, (Instant::now(), change.reason));
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

    use super::*;

    #[tokio::test]
    async fn the_latest_change_per_peer_is_remembered_with_its_reason() {
        let history = RouteHistory::default();
        let (tx, rx) = broadcast::channel(8);
        let following = tokio::spawn(history.clone().follow(rx));
        let peer = NodeId::new(2).unwrap();
        assert_eq!(history.since(peer), None);

        tx.send(RouteChange { peer, route: Route::None, reason: "直连超时".to_owned() }).unwrap();
        tx.send(RouteChange { peer, route: Route::Relay { relays: vec![NodeId::new(9).unwrap()] }, reason: "改走 relay".to_owned() })
            .unwrap();
        for _ in 0..100 {
            if history.since(peer).is_some_and(|since| since.reason == "改走 relay") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let since = history.since(peer).unwrap();
        assert_eq!(since.reason, "改走 relay");
        assert!(since.secs_ago <= 1);
        assert_eq!(history.since(NodeId::new(3).unwrap()), None);

        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), following).await.expect("路由层关闭之后应该停").unwrap();
    }
}
