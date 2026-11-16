//! 处理别的节点开过来的流和数据报上下文。

use std::future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use i3t_conn::contract::DatagramSender;
use i3t_conn::{RecvStream, SendStream};
use i3t_flow::datagram::{self, OutboundSocket};
use i3t_flow::{Handler, Request};
use i3t_model::Netmap;
use i3t_model::netmap::{Forward, ForwardProtocol, Peer};
use i3t_proto::NodeId;
use i3t_proto::flow::{Reason, Reply, Status, Target};
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::datagrams::{ContextLimit, IDLE_TIMEOUT, MAX_CONTEXTS_PER_PEER, Session};
use crate::outbound::Outbound;
use crate::stats::{Counters, ExitStats};
use crate::system::{Os, System};

/// 一个节点接受流的那一侧。
///
/// 每个建流请求按这个顺序检查，不通过就用对应的状态拒绝：
///
/// 1. 访问规则：发起节点在当前的 netmap 里必须被允许访问本节点。发起节点是谁来自连接的握手，这里不信任入口的判断。
/// 2. 按目标的种类查本节点的能力和策略：
///    - 互联网：本节点启用了出口，端口在允许的范围里，解析出来的每个地址都过出站策略；
///    - 局域网：本节点启用了出口并打开了局域网开关，地址在本机网卡直连的网段里；
///    - 本节点的端口：只用本机的非回环地址去连。那个端口上有监听在虚拟 IP 上的端口转发时，按那条转发处理；
///    - 端口转发：必须是下发给本节点、并且由发起节点监听的那一条。不需要本节点是出口。
/// 3. 连上目标之后回成功，开始搬运。访问权限被收回时，正在搬运的流被重置。
///
/// UDP 应用的数据走数据报上下文。建立上下文时只查访问规则和上下文的个数（每个对端最多 64 个）；
/// 目标写在每个数据报里，所以第 2 步的检查落在每个目标第一次出现的时候，规则和流的完全一样，
/// 只是端口转发要的是 UDP 的那一条。没通过的数据报被丢掉并计数，UDP 没有地方回错误。
/// 上下文没有报文往来 60 秒就回收。
///
/// 把它交给 [`i3t_flow::serve`] 就开始工作。
pub struct Exit<S = Os> {
    netmap: watch::Receiver<Arc<Netmap>>,
    outbound: Outbound<S>,
    counters: Counters,
    contexts: ContextLimit,
    datagram_idle_timeout: Duration,
}

impl<S> std::fmt::Debug for Exit<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exit").field("counters", &self.counters).finish_non_exhaustive()
    }
}

impl<S: System> Exit<S> {
    #[must_use]
    pub fn new(netmap: watch::Receiver<Arc<Netmap>>, outbound: Outbound<S>) -> Self {
        Self { netmap, outbound, counters: Counters::default(), contexts: ContextLimit::default(), datagram_idle_timeout: IDLE_TIMEOUT }
    }

    /// 数据报上下文没有报文往来多久之后回收。默认 60 秒；测试里调短，不用真的等一分钟。
    #[must_use]
    pub fn with_datagram_idle_timeout(mut self, timeout: Duration) -> Self {
        self.datagram_idle_timeout = timeout;
        self
    }

    /// 到现在为止的计数。
    #[must_use]
    pub fn stats(&self) -> ExitStats {
        self.counters.snapshot()
    }

    /// 检查一个建流请求，通过的话连上它的目标。
    async fn connect(&self, netmap: &Netmap, peer: NodeId, target: &Target) -> Result<TcpStream, Reply> {
        let Some(initiator) = netmap.peer(peer).filter(|initiator| initiator.inbound) else {
            return Err(Reply::new(Status::AccessDenied, "访问规则不允许发起节点访问这个节点"));
        };
        match target {
            Target::Internet { host, port } => self.outbound.internet(netmap, host, *port).await,
            Target::Lan { addr } => self.outbound.lan(netmap, *addr).await,
            Target::Node { port } => match forward_on_virtual_ip(netmap, *port, ForwardProtocol::Tcp) {
                Some(forward) => self.outbound.forward(virtual_forward_target(netmap, initiator, forward)?).await,
                None => self.outbound.node_port(*port).await,
            },
            Target::Forward { id } => {
                let listed = netmap
                    .me
                    .forward_targets
                    .iter()
                    .find(|entry| entry.id == *id && entry.listener == peer && entry.protocol == ForwardProtocol::Tcp);
                match listed {
                    Some(entry) => self.outbound.forward(entry.addr).await,
                    None => Err(Reply::new(Status::UnknownForward, "没有下发给这个节点、由发起节点监听的这条 TCP 端口转发")),
                }
            }
        }
    }
}

impl<S: System> Handler for Exit<S> {
    async fn handle<Snd: SendStream, Rcv: RecvStream>(&self, request: Request<Snd, Rcv>) {
        let _active = self.counters.begin();
        let peer = request.peer();
        let (class, kind) = (request.open().class, request.open().target.kind());
        // 先记下"已经看过"的版本再判断，之后的变化才不会漏掉。
        let mut netmap_changes = self.netmap.clone();
        let netmap = Arc::clone(&netmap_changes.borrow_and_update());

        let target = match self.connect(&netmap, peer, &request.open().target).await {
            Ok(target) => target,
            Err(reply) => {
                self.counters.failed(reply.status);
                tracing::info!(%peer, ?class, kind, status = ?reply.status, detail = %reply.detail, "拒绝了一个建流请求");
                return request.refuse(&reply).await;
            }
        };
        let flow = match request.accept().await {
            Ok(flow) => flow,
            Err(error) => return tracing::info!(%peer, ?class, kind, ?error, "目标已经连上，但发起方放弃了这条流"),
        };
        let summary = flow.relay(target, async move { access_revoked(&mut netmap_changes, peer).await }).await;

        self.counters.finished(&summary);
        tracing::info!(
            %peer,
            ?class,
            kind,
            to_target = summary.from_flow.bytes,
            from_target = summary.from_tcp.bytes,
            first_byte_ms = summary.from_tcp.first_byte.map(|elapsed| elapsed.as_millis()),
            end = ?summary.end,
            "流结束"
        );
    }

    async fn handle_datagrams<Snd: SendStream, Rcv: RecvStream, T: DatagramSender>(&self, request: datagram::Request<Snd, Rcv, T>) {
        let _active = self.counters.begin();
        let peer = request.peer();
        let class = request.open().class;
        // 先记下"已经看过"的版本再判断，之后的变化才不会漏掉。
        let mut netmap_changes = self.netmap.clone();
        let allowed = netmap_changes.borrow_and_update().peer(peer).is_some_and(|initiator| initiator.inbound);

        // 名额和端口占到上下文结束。
        let admitted = if allowed {
            match self.contexts.acquire(peer) {
                Some(slot) => OutboundSocket::bind()
                    .map(|socket| (slot, socket))
                    .map_err(|error| Reply::new(Status::InternalError, format!("绑不出 UDP 端口：{error}"))),
                None => Err(Reply::new(Status::PolicyDenied, format!("发起节点开着的数据报上下文到了上限（{MAX_CONTEXTS_PER_PEER} 个）"))),
            }
        } else {
            Err(Reply::new(Status::AccessDenied, "访问规则不允许发起节点访问这个节点"))
        };
        let (_slot, socket) = match admitted {
            Ok(admitted) => admitted,
            Err(reply) => {
                self.counters.failed(reply.status);
                tracing::info!(%peer, ?class, status = ?reply.status, detail = %reply.detail, "拒绝了一个数据报上下文");
                return request.refuse(&reply).await;
            }
        };
        let context = match request.accept().await {
            Ok(context) => context,
            Err(error) => return tracing::info!(%peer, ?class, ?error, "数据报上下文没有建成"),
        };
        tracing::debug!(%peer, ?class, "数据报上下文建立");

        let session = Session {
            outbound: &self.outbound,
            netmap: netmap_changes,
            counters: &self.counters,
            peer,
            socket,
            idle_timeout: self.datagram_idle_timeout,
        };
        let (end, tally) = session.run(context).await;
        if end.is_abnormal() {
            self.counters.aborted();
        }
        tracing::info!(
            %peer,
            ?class,
            to_target = tally.to_target,
            from_target = tally.from_target,
            refused = tally.refused,
            ?end,
            "数据报上下文结束"
        );
    }
}

/// 等到 netmap 不再允许 `peer` 访问本节点。那时它已经建立的流和数据报上下文要被重置，不只是拒绝新的。
///
/// 可以放在 `select!` 里反复调用：被取消时不会漏掉变化。
pub(crate) async fn access_revoked(netmap: &mut watch::Receiver<Arc<Netmap>>, peer: NodeId) -> Reason {
    loop {
        if netmap.changed().await.is_err() {
            // 发送端没了说明节点在关闭，不是权限变了。流留给连接的关闭去结束。
            return future::pending().await;
        }
        if !netmap.borrow_and_update().peer(peer).is_some_and(|peer| peer.inbound) {
            tracing::info!(%peer, "访问规则不再允许这个节点访问本节点，重置它已经建立的流和数据报上下文");
            return Reason::AccessRevoked;
        }
    }
}

/// 本节点上监听在自己的虚拟 IP 的 `port` 端口上的端口转发。这种转发不占操作系统的端口，别的节点访问这个端口时在这里接住。
pub(crate) fn forward_on_virtual_ip(netmap: &Netmap, port: u16, protocol: ForwardProtocol) -> Option<&Forward> {
    let bind = SocketAddr::new(IpAddr::V4(netmap.me.virtual_ip), port);
    netmap.me.forwards.iter().find(|forward| forward.protocol == protocol && forward.bind == bind)
}

/// 监听在虚拟 IP 上的端口转发最后要连的地址。
///
/// 只支持目标节点就是本节点的情况（监听节点和目标节点是同一个，这是这种转发的通常用法）：
/// 目标是别的节点时要在两条流之间接力，现在没有实现。
pub(crate) fn virtual_forward_target(netmap: &Netmap, initiator: &Peer, forward: &Forward) -> Result<SocketAddr, Reply> {
    let from = IpAddr::V4(initiator.virtual_ip);
    if !(forward.allow_from.is_empty() || forward.allow_from.iter().any(|net| net.contains(&from))) {
        return Err(Reply::new(Status::PolicyDenied, "发起节点不在这条端口转发的来源白名单里"));
    }
    if forward.target != netmap.me.id {
        return Err(Reply::new(Status::UnknownForward, "这条端口转发监听在虚拟 IP 上而目标是别的节点，这种用法还不支持"));
    }
    netmap
        .me
        .forward_targets
        .iter()
        .find(|entry| entry.id == forward.id && entry.listener == netmap.me.id && entry.protocol == forward.protocol)
        .map(|entry| entry.addr)
        .ok_or_else(|| Reply::new(Status::UnknownForward, "这条端口转发的目标地址没有下发给这个节点"))
}
