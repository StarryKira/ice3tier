//! 透明访问的 UDP：内核把本机程序发往对端虚拟 IP 的报文交过来（Linux 上是 TPROXY），每个报文带着它原来要去的地址。
//!
//! - 会话按（应用的地址，它要发往的地址）分。一个会话是到那个对端的一个数据报上下文，报文的目标是 `Target::Node`（那个端口）；
//!   和 UDP 端口转发一样，对端那一侧是一个 UDP 端口，服务看到的来源是它，回的报文分得清是哪个会话的。
//! - 回给应用的报文要以它原来要发往的地址为来源，应用才认得：每个会话有一个以那个地址为源地址、连着应用的套接字
//!   （[`Diversion::reply_socket`]）。内核之后收到这个应用发往同一个地址的报文时，先找到这个连着的套接字，直接交给它，
//!   不再经过总的入口；所以会话两边都要收。
//! - 同时最多 [`MAX_SESSIONS`] 个会话，和一条 UDP 端口转发一样多（目标节点给本节点的上下文名额的一半）。
//!   会话没有报文往来超过空闲时限（默认 60 秒）就回收。
//! - 内核只把发往能访问的对端的报文交过来，这里按当前的 netmap 再核对一次；不通过的报文丢掉，UDP 没有地方回错误。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use i3t_conn::Connector;
use i3t_conn::contract::Datagrams;
use i3t_proto::NodeId;
use i3t_proto::flow::{Class, Host, Origin, Target};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time;
use tracing::Instrument;

use super::forward::MAX_SESSIONS;
use super::{MAX_UDP_PAYLOAD, OUTGOING_BACKLOG, open_context, owned};
use crate::manager::{Access, Connection};
use crate::route;
use crate::session::Shared;

/// 最多为多少个被拒绝的目标地址各记一行日志。
const MAX_LOGGED_REFUSALS: usize = 64;

/// 内核交过来的 UDP 报文从哪里收，回给应用的报文从哪里发。Linux 上由 `i3t_intercept::socket` 实现；测试里是普通的套接字。
pub trait Diversion: Send + Sync + 'static {
    /// 收下一个报文：内容写进 `buf`，返回它有多长、从哪里来、原来要发往哪里。
    fn recv(&self, buf: &mut [u8]) -> impl Future<Output = io::Result<Diverted>> + Send;

    /// 以 `local`（应用要发往的地址）为来源、发给 `client` 的套接字。要在 tokio 运行时里调用。
    ///
    /// # Errors
    ///
    /// 没有权限，或者系统不给套接字。
    fn reply_socket(&self, local: SocketAddr, client: SocketAddr) -> io::Result<UdpSocket>;
}

/// 内核交过来的一个 UDP 报文（[`Diversion::recv`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diverted {
    /// 报文的长度，内容在调用方给的缓冲区里。
    pub len: usize,
    /// 发报文的应用的地址。
    pub from: SocketAddr,
    /// 应用原来要发往的地址：对端的虚拟 IP 和端口。
    pub to: SocketAddr,
}

impl<C: Connector + Datagrams> Access<C> {
    /// 接住内核交过来的 UDP 报文，直到这个 future 被丢弃。
    pub async fn intercept_udp(&self, diversion: impl Diversion) {
        let shared = &self.shared;
        let mut sessions: HashMap<(SocketAddr, SocketAddr), mpsc::Sender<Bytes>> = HashMap::new();
        let mut refused: HashSet<SocketAddr> = HashSet::new();
        let mut buf = vec![0; MAX_UDP_PAYLOAD];
        loop {
            let Diverted { len, from: client, to: original } = match diversion.recv(&mut buf).await {
                Ok(received) => received,
                Err(error) => {
                    tracing::debug!(%error, "透明访问的 UDP 入口收报文出错");
                    continue;
                }
            };
            let key = (client, original);
            if sessions.get(&key).is_none_or(mpsc::Sender::is_closed) {
                sessions.retain(|_, queue| !queue.is_closed());
                let Some(peer) = admit(shared, &mut refused, original, sessions.len()) else { continue };
                let reply = match diversion.reply_socket(original, client) {
                    Ok(reply) => reply,
                    Err(error) => {
                        tracing::warn!(%original, %client, %error, "丢掉了一个被拦截的 UDP 报文：开不出回给应用的套接字");
                        continue;
                    }
                };
                let (queue, outgoing) = mpsc::channel(OUTGOING_BACKLOG);
                let class = shared.netmap.borrow().network.class.unwrap_or_default();
                let session = Session { reply, peer, port: original.port(), class, connection: Connection::enter(&shared.intercept) };
                let span = tracing::info_span!("intercept-udp", %client, %original);
                tokio::spawn(session.run(Arc::clone(shared), outgoing).instrument(span));
                sessions.insert(key, queue);
            }
            let queued = sessions.get(&key).is_some_and(|queue| queue.try_send(owned(&buf[..len])).is_ok());
            if !queued {
                tracing::trace!(%client, %original, "丢掉了一个被拦截的 UDP 报文：到对端的上下文还没开好，或者来不及发");
            }
        }
    }
}

/// 一个新的（应用，目标）能不能开会话，能的话是到哪个对端。
fn admit<C: Datagrams>(shared: &Shared<C>, refused: &mut HashSet<SocketAddr>, original: SocketAddr, open: usize) -> Option<NodeId> {
    if open >= MAX_SESSIONS {
        tracing::debug!(%original, "丢掉了一个被拦截的 UDP 报文：会话到了上限（{MAX_SESSIONS} 个），等旧的会话空闲回收");
        return None;
    }
    let netmap = Arc::clone(&shared.netmap.borrow());
    let peer = match route::mesh_node(&netmap, &Host::Ip(original.ip())) {
        Some(Ok(peer)) => peer.id,
        Some(Err(reason)) => {
            if refused.len() < MAX_LOGGED_REFUSALS && refused.insert(original) {
                tracing::info!(%original, %reason, "丢掉了一个被拦截的 UDP 报文（这个目标之后的不再记）");
            }
            return None;
        }
        None => {
            tracing::debug!(%original, "丢掉了一个发到透明访问入口的 UDP 报文：原目标不是组网的地址");
            return None;
        }
    };
    if shared.refuse_while_draining("被拦截的 UDP 会话") {
        return None;
    }
    Some(peer)
}

/// 一个（应用，目标）的会话。
struct Session {
    /// 以目标为来源、连着应用的套接字。应用之后发往这个目标的报文也从它收。
    reply: UdpSocket,
    peer: NodeId,
    port: u16,
    class: Class,
    connection: Connection,
}

impl Session {
    async fn run<C: Connector + Datagrams>(self, shared: Arc<Shared<C>>, mut outgoing: mpsc::Receiver<Bytes>) {
        let Self { reply, peer, port, class, connection } = self;
        // 客户端是本机的程序，不写来源地址。
        let origin = Origin { credential: None, client: None };
        let Ok(mut context) = open_context(&shared, peer, class, origin, &mut outgoing).await else { return };
        let sender = context.sender();
        let target = Target::Node { port };
        tracing::debug!(%peer, port, "被拦截的 UDP 会话建立");

        let idle = time::sleep(shared.udp_idle_timeout);
        tokio::pin!(idle);
        let mut buf = vec![0; MAX_UDP_PAYLOAD];
        let (mut sent, mut received) = (0u64, 0u64);
        let end = loop {
            // 从应用来的：会话刚建的时候经总的入口排队过来，之后内核直接交给这个会话的套接字。
            let from_app = tokio::select! {
                queued = outgoing.recv() => match queued {
                    Some(payload) => Some(payload),
                    None => break "入口关闭".to_owned(),
                },
                read = reply.recv(&mut buf) => match read {
                    Ok(len) => Some(owned(&buf[..len])),
                    Err(error) => {
                        tracing::debug!(%error, "被拦截的 UDP 会话收应用的报文出错");
                        None
                    }
                },
                arrived = context.recv() => match arrived {
                    Ok(datagram) => {
                        match reply.send(&datagram.payload).await {
                            Ok(_) => {
                                received += 1;
                                connection.traffic.add(false, datagram.payload.len());
                            }
                            Err(error) => tracing::debug!(%error, "一个 UDP 报文没有发回应用"),
                        }
                        None
                    }
                    Err(end) => {
                        tracing::info!(%peer, port, sent, received, ?end, "被拦截的 UDP 会话结束：到对端的上下文结束了");
                        return;
                    }
                },
                () = &mut idle => break format!("{:?} 没有报文往来", shared.udp_idle_timeout),
            };
            if let Some(payload) = from_app {
                match sender.send(&target, &payload).await {
                    Ok(()) => {
                        sent += 1;
                        connection.traffic.add(true, payload.len());
                    }
                    Err(error) => tracing::debug!(%error, "一个被拦截的 UDP 报文没有发给对端"),
                }
            }
            idle.as_mut().reset(time::Instant::now() + shared.udp_idle_timeout);
        };
        context.close().await;
        tracing::info!(%peer, port, sent, received, end, "被拦截的 UDP 会话结束");
    }
}
