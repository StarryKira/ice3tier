//! UDP 的端口转发：监听一个 UDP 端口，每个客户端地址是一个会话，会话的报文走到目标节点的一个数据报上下文。
//!
//! - 会话按客户端的地址（IP 加端口）分。一个会话没有报文往来超过空闲时限（默认 60 秒）就回收，客户端再发报文时重新建。
//! - 一条转发同时最多 [`MAX_SESSIONS`] 个会话。UDP 的来源地址可以随便伪造，没有这个上限的话，
//!   往监听端口上乱发报文就能让本节点和目标节点开出无数个上下文。到了上限，新来的客户端要等旧的会话回收。
//! - 目标节点按转发编号找到要发往的地址；它回来的报文从监听端口发回那个客户端。
//! - 来源白名单逐个报文检查。UDP 没有地方回错误，不通过的报文只是丢掉。

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use i3t_conn::Connector;
use i3t_conn::contract::Datagrams;
use i3t_model::netmap::{Forward, ForwardProtocol};
use i3t_proto::flow::{Class, Origin, Target};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time;
use tracing::Instrument;

use super::{MAX_UDP_PAYLOAD, OUTGOING_BACKLOG, open_context, owned};
use crate::manager::allowed;
use crate::session::{Shared, origin_address};

/// 一条转发同时最多多少个会话。
pub(crate) const MAX_SESSIONS: usize = 256;

/// 最多为多少个被拒绝的来源地址各记一行日志。
const MAX_LOGGED_REFUSALS: usize = 64;

/// 在 `socket` 上收报文，按客户端地址分给各个会话，直到任务被取消（监听被关掉）。
///
/// `active` 是这条转发现在有多少个会话。转发的设置每个报文都从当前的 netmap 里取，所以改白名单不用重新绑定。
pub(crate) async fn serve<C: Connector + Datagrams, E: Send + Sync + 'static>(
    shared: Arc<Shared<C, E>>,
    id: u32,
    bind: SocketAddr,
    socket: UdpSocket,
    active: Arc<AtomicUsize>,
) {
    let socket = Arc::new(socket);
    let mut sessions: HashMap<SocketAddr, mpsc::Sender<Bytes>> = HashMap::new();
    let mut refused: HashSet<IpAddr> = HashSet::new();
    let mut buf = vec![0; MAX_UDP_PAYLOAD];
    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(error) => {
                // 有的系统把上一次发送引起的"端口不可达"报在这里。它不影响这个端口继续用。
                tracing::debug!(%error, "端口转发的 UDP 端口收报文时出错");
                continue;
            }
        };
        let netmap = Arc::clone(&shared.netmap.borrow());
        let current = netmap.me.forwards.iter().find(|forward| forward.id == id && forward.bind == bind);
        // 找不到说明它刚从 netmap 里删掉，监听马上就会关。
        let Some(forward) = current.filter(|forward| forward.protocol == ForwardProtocol::Udp) else { continue };
        if !allowed(&forward.allow_from, from) {
            if refused.len() < MAX_LOGGED_REFUSALS && refused.insert(from.ip()) {
                tracing::info!(%from, "丢掉了一个 UDP 报文：来源不在这条端口转发的白名单里（这个来源之后的不再记）");
            }
            continue;
        }

        if sessions.get(&from).is_none_or(mpsc::Sender::is_closed) {
            sessions.retain(|_, queue| !queue.is_closed());
            if sessions.len() >= MAX_SESSIONS {
                tracing::debug!(%from, "丢掉了一个 UDP 报文：这条端口转发的会话到了上限（{MAX_SESSIONS} 个），等旧的会话空闲回收");
                continue;
            }
            if !netmap.peer(forward.target).is_some_and(|peer| peer.outbound) {
                tracing::debug!(target = %forward.target, "丢掉了一个 UDP 报文：目标节点不在 netmap 里，或者访问规则不允许本节点访问它");
                continue;
            }
            let (queue, outgoing) = mpsc::channel(OUTGOING_BACKLOG);
            let session = Session { socket: Arc::clone(&socket), client: from, active: Active::enter(&active) };
            let span = tracing::info_span!("udp-session", client = %from);
            tokio::spawn(session.run(Arc::clone(&shared), forward.clone(), outgoing).instrument(span));
            sessions.insert(from, queue);
        }
        let queued = sessions.get(&from).is_some_and(|queue| queue.try_send(owned(&buf[..len])).is_ok());
        if !queued {
            tracing::trace!(%from, "丢掉了一个 UDP 报文：到目标节点的上下文还没开好，或者来不及发");
        }
    }
}

/// 一个会话占着的计数，会话的任务结束时减回去。
struct Active(Arc<AtomicUsize>);

impl Active {
    fn enter(active: &Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(active))
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 一个客户端地址的会话。
struct Session {
    /// 监听的那个端口：回客户端的报文也从它发，客户端才认得。
    socket: Arc<UdpSocket>,
    client: SocketAddr,
    active: Active,
}

impl Session {
    /// 开到目标节点的上下文，然后在客户端和上下文之间搬运，直到空闲太久或者上下文结束。
    async fn run<C: Connector + Datagrams, E>(self, shared: Arc<Shared<C, E>>, forward: Forward, mut outgoing: mpsc::Receiver<Bytes>) {
        let _active = self.active;
        let origin = Origin { credential: None, client: origin_address(self.client) };
        let Some(mut context) = open_context(&shared, forward.target, Class::default(), origin, &mut outgoing).await else { return };
        let sender = context.sender();
        let target = Target::Forward { id: forward.id };
        tracing::debug!(target = %forward.target, "UDP 端口转发的会话建立");

        let idle = time::sleep(shared.udp_idle_timeout);
        tokio::pin!(idle);
        let (mut sent, mut received) = (0u64, 0u64);
        let end = loop {
            tokio::select! {
                queued = outgoing.recv() => match queued {
                    Some(payload) => match sender.send(&target, &payload).await {
                        Ok(()) => sent += 1,
                        Err(error) => tracing::debug!(%error, "一个 UDP 报文没有发给目标节点"),
                    },
                    // 监听被关掉了。
                    None => break "监听关闭".to_owned(),
                },
                arrived = context.recv() => match arrived {
                    Ok(datagram) => match self.socket.send_to(&datagram.payload, self.client).await {
                        Ok(_) => received += 1,
                        Err(error) => tracing::debug!(%error, "一个 UDP 报文没有发回客户端"),
                    },
                    Err(end) => {
                        tracing::info!(target = %forward.target, sent, received, ?end, "UDP 端口转发的会话结束：到目标节点的上下文结束了");
                        return;
                    }
                },
                () = &mut idle => break format!("{:?} 没有报文往来", shared.udp_idle_timeout),
            }
            idle.as_mut().reset(time::Instant::now() + shared.udp_idle_timeout);
        };
        context.close().await;
        tracing::info!(target = %forward.target, sent, received, end, "UDP 端口转发的会话结束");
    }
}
