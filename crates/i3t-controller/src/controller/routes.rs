//! 路由视图（白皮书 §7.7）：各个节点上报的路由表、全网的路由矩阵，以及"到某个节点现在会怎么走"的现问现答。
//!
//! 路是发送方在本地选的，controller 不参与，所以这里的数据全部来自节点：
//!
//! - **上报**：节点连上之后的第一次上报带着完整的路由表，之后只报变化的条目。这里存每个节点最近的一份，
//!   节点离线之后留着（界面上标成过期），controller 重启就没有了。
//! - **现问**：controller 给发送方发一条查询，它用当前的测量值回答。问不到（不在线、版本太旧、没有按时回答）时退回到上报的那一份。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use i3t_model::admin::{MatrixEntry, MatrixNode, NetworkId, NodeRoute, PathLeg, PathSource, PathView, RouteMatrix, RouteTable};
use i3t_model::control::{Route, RouteAnswer, RouteEntry, ToNode};
use i3t_proto::NodeId;
use tokio::sync::oneshot;

use super::{Controller, Inner, Live, node_ref};
use crate::error::Error;
use crate::server::blocking;
use crate::store::NodeRecord;

/// 等节点回答一次路由查询最多等多久。节点是拿手上现成的结论答的，正常只要一个往返。
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);

/// 一个节点最近上报的路由表。
#[derive(Debug)]
pub(crate) struct Reported {
    pub(crate) entries: BTreeMap<NodeId, RouteEntry>,
    /// 每一条路 controller 第一次看到它的时刻：路换了才重新算，只是延迟变了不算。
    since: BTreeMap<NodeId, Instant>,
    /// 最近一次上报的时刻。没有变化的条目节点不重复报，所以整张表都是"截至这个时刻"的。
    pub(crate) at: Instant,
    /// 是节点的哪一条连接报的。不是它现在这条连接报的，就是过期的：它离线了，或者重新连上来之后还没有上报。
    conn: u64,
}

/// 一条发出去、还在等回答的路由查询。
#[derive(Debug)]
pub(crate) struct PendingQuery {
    /// 问的是谁。只有它的回答算数。
    node: NodeId,
    reply: oneshot::Sender<RouteAnswer>,
}

/// 两个方向各自在等的回答。问不了的方向是 `None`。
#[derive(Debug)]
pub struct RouteAnswers {
    forward: Option<oneshot::Receiver<RouteAnswer>>,
    reverse: Option<oneshot::Receiver<RouteAnswer>>,
}

impl Controller {
    /// 一个节点最近上报的路由表：它到各个对端现在怎么走。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个节点，或者读存储失败。
    pub fn routes(&self, network: NetworkId, id: NodeId) -> Result<RouteTable, Error> {
        let inner = self.lock();
        let record = inner.require_node(network, id)?;
        let peers = inner.store.nodes(network)?;
        let now = Instant::now();
        let routes = inner.current_routes(id).map(|reported| {
            let age_secs = now.saturating_duration_since(reported.at).as_secs();
            let known = reported.entries.values().filter_map(|entry| {
                let peer = peers.iter().find(|peer| peer.id == entry.peer)?;
                let since = reported.since.get(&entry.peer).map_or(reported.at, |since| *since);
                Some(NodeRoute {
                    peer: node_ref(peer),
                    route: entry.route.clone(),
                    rtt_ms: entry.rtt_ms,
                    age_secs,
                    since_secs: now.saturating_duration_since(since).as_secs(),
                })
            });
            known.collect()
        });
        Ok(RouteTable { node: node_ref(&record), online: inner.is_online(id), routes: routes.unwrap_or_default() })
    }

    /// 整个网络的路由矩阵，取自各个节点最近的上报。离线的节点留着它最后报的那一份，标成过期。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn route_matrix(&self, network: NetworkId) -> Result<RouteMatrix, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        let nodes = inner.store.nodes(network)?;
        let members: BTreeSet<NodeId> = nodes.iter().map(|node| node.id).collect();
        let now = Instant::now();
        let entries = nodes.iter().filter_map(|node| Some((node.id, inner.routes.get(&node.id)?))).flat_map(|(from, reported)| {
            let age_secs = now.saturating_duration_since(reported.at).as_secs();
            let stale = inner.current_routes(from).is_none();
            let listed = reported.entries.values().filter(|entry| members.contains(&entry.peer));
            listed.map(move |entry| MatrixEntry { from, to: entry.peer, route: entry.route.clone(), rtt_ms: entry.rtt_ms, age_secs, stale })
        });
        Ok(RouteMatrix {
            entries: entries.collect(),
            nodes: nodes.iter().map(|node| MatrixNode { node: node_ref(node), online: inner.is_online(node.id) }).collect(),
        })
    }

    /// 两个节点之间两个方向的当前路径：先问两端，问不到的方向退回到它最近的上报。
    ///
    /// 两个方向各由发送方决定走哪条路，所以两端都要问。最多等几秒。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，是同一个节点，或者读存储失败。
    pub async fn path(self: &Arc<Self>, network: NetworkId, from: NodeId, to: NodeId) -> Result<PathView, Error> {
        let asked = blocking(self, move |controller| controller.ask_path(network, from, to)).await?;
        let wait = async |pending: Option<oneshot::Receiver<RouteAnswer>>| match pending {
            Some(pending) => tokio::time::timeout(ANSWER_TIMEOUT, pending).await.ok().and_then(Result::ok),
            None => None,
        };
        let (forward, reverse) = tokio::join!(wait(asked.forward), wait(asked.reverse));
        blocking(self, move |controller| controller.path_view(network, from, to, forward, reverse)).await
    }

    /// [`Controller::path`] 的前一半：给两端各发一条查询。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，或者是同一个节点。
    pub fn ask_path(&self, network: NetworkId, from: NodeId, to: NodeId) -> Result<RouteAnswers, Error> {
        if from == to {
            return Err(Error::invalid("要查的是两个不同的节点之间的路径"));
        }
        let mut inner = self.lock();
        inner.require_node(network, from)?;
        inner.require_node(network, to)?;
        Ok(RouteAnswers { forward: inner.ask_route(from, to), reverse: inner.ask_route(to, from) })
    }

    /// [`Controller::path`] 的后一半：把问到的回答和上报的数据拼成结果。没有问到的方向传 `None`。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里。
    pub fn path_view(
        &self,
        network: NetworkId,
        from: NodeId,
        to: NodeId,
        forward: Option<RouteAnswer>,
        reverse: Option<RouteAnswer>,
    ) -> Result<PathView, Error> {
        let inner = self.lock();
        let (from, to) = (inner.require_node(network, from)?, inner.require_node(network, to)?);
        Ok(PathView { forward: inner.path_leg(&from, &to, forward), reverse: inner.path_leg(&to, &from, reverse) })
    }
}

impl Inner {
    /// 节点这次连接上报过的路由表。它不在线、或者重新连上来之后还没有上报时是 `None`。
    pub(crate) fn current_routes(&self, id: NodeId) -> Option<&Reported> {
        let conn = self.live.get(&id)?.conn;
        self.routes.get(&id).filter(|reported| reported.conn == conn)
    }

    /// 记下节点上报的路由。`full` 时整个替换，否则只更新报上来的条目。
    pub(super) fn routes_reported(&mut self, id: NodeId, routes: Vec<RouteEntry>, full: bool) {
        let (Some(sync), Some(live)) = (self.sync.get(&id), self.live.get(&id)) else { return };
        let now = Instant::now();
        let reported = self.routes.entry(id).or_insert_with(|| Reported {
            entries: BTreeMap::new(),
            since: BTreeMap::new(),
            at: now,
            conn: live.conn,
        });
        let before: BTreeMap<NodeId, Route> = reported.entries.iter().map(|(peer, entry)| (*peer, entry.route.clone())).collect();
        // 上一条连接留下的表，节点没有说完整的也要先清掉：增量是相对于这条连接上报过的内容说的。
        if full || reported.conn != live.conn {
            reported.entries.clear();
        }
        reported.conn = live.conn;
        // 只记到它 netmap 里的对端的路由：别的编号它本来就不该知道，记下来只会让表无限长。
        reported.entries.retain(|peer, _| sync.knows(*peer));
        for entry in routes.into_iter().filter(|entry| sync.knows(entry.peer)) {
            if before.get(&entry.peer) == Some(&entry.route) {
                reported.since.entry(entry.peer).or_insert(now);
            } else {
                reported.since.insert(entry.peer, now);
            }
            reported.entries.insert(entry.peer, entry);
        }
        let entries = &reported.entries;
        reported.since.retain(|peer, _| entries.contains_key(peer));
        reported.at = now;
    }

    /// 问 `from`"到 `to` 现在会怎么走"。问不了（不在线，或者它的版本不懂这条消息）时返回 `None`。
    fn ask_route(&mut self, from: NodeId, to: NodeId) -> Option<oneshot::Receiver<RouteAnswer>> {
        if !self.live.get(&from).is_some_and(Live::understands_updates) {
            return None;
        }
        // 没有人等的查询顺手清掉：节点一直不回答的话，它们不会自己消失。
        self.route_queries.retain(|_, pending| !pending.reply.is_closed());
        let request = self.next_request();
        let (reply, answer) = oneshot::channel();
        if !self.send(from, ToNode::RouteQuery { request, peer: to }) {
            return None;
        }
        self.counters.route_queries += 1;
        self.route_queries.insert(request, PendingQuery { node: from, reply });
        Some(answer)
    }

    /// 节点回答了一条路由查询。
    pub(super) fn route_answered(&mut self, node: NodeId, request: u64, answer: RouteAnswer) {
        match self.route_queries.remove(&request) {
            Some(pending) if pending.node == node => {
                // 问的人等不及已经走了也没有关系。
                let _dropped = pending.reply.send(answer);
            }
            Some(pending) => {
                tracing::debug!(%node, asked = %pending.node, request, "丢掉一条路由查询的回答：不是被问的那个节点答的");
                self.route_queries.insert(request, pending);
            }
            None => tracing::debug!(%node, request, "丢掉一条路由查询的回答：没有在等这个编号（多半是等得太久已经放弃了）"),
        }
    }

    /// 节点不在线了：发给它的查询不会有回答了。
    pub(super) fn drop_route_queries(&mut self, node: NodeId) {
        self.route_queries.retain(|_, pending| pending.node != node);
    }

    fn path_leg(&self, from: &NodeRecord, to: &NodeRecord, answer: Option<RouteAnswer>) -> PathLeg {
        let leg = |source, route, rtt_ms, age_secs, since| PathLeg {
            from: node_ref(from),
            to: node_ref(to),
            source,
            route,
            rtt_ms,
            age_secs,
            since,
        };
        match answer {
            Some(RouteAnswer { route: Some(route), rtt_ms, since, .. }) => leg(PathSource::Query, Some(route), rtt_ms, Some(0), since),
            Some(RouteAnswer { route: None, .. }) => leg(PathSource::NotPeers, None, None, Some(0), None),
            None => {
                let reported = self.routes.get(&from.id).and_then(|reported| Some((reported.entries.get(&to.id)?, reported.at)));
                match reported {
                    Some((entry, at)) => {
                        leg(PathSource::Report, Some(entry.route.clone()), entry.rtt_ms, Some(at.elapsed().as_secs()), None)
                    }
                    None => leg(PathSource::None, None, None, None, None),
                }
            }
        }
    }
}
