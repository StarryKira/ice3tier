//! 透明访问的入口一侧：内核在建立连接的那一刻把本机程序发往对端虚拟 IP 的连接改到本机入口（见 `i3t_intercept`），
//! 这里把每一条变成到那个对端的一条流。
//!
//! 一条被拦截的连接和端口转发上的连接一样：客户端以为自己已经连上了，没有地方告诉它错误码，所以不等对端的 REPLY，
//! 首批数据随 OPEN 一起发出；对端拒绝（端口没开、访问规则不允许）时连接被重置。目标是 `Target::Node`，
//! 和本机的 SOCKS5 入口访问 `<虚拟 IP>:<端口>` 开的流一样，在对端按同样的访问规则、同样的每对端上限处理。
//!
//! 内核只把发往"本节点能访问的对端"的连接改过来（地址段里别的地址当场被拒绝），这里还要按当前的 netmap 再核对一次：
//! 规则刚变、内核里的集合还没换的那一瞬间，或者有人直接连了入口的端口（原目标就是入口自己）。

use std::future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use i3t_conn::Connector;
use i3t_conn::contract::Datagrams;
use i3t_proto::flow::{Host, Open, Origin, Target};
use tokio::net::{TcpListener, TcpStream};
use tokio::time;
use tracing::Instrument;

use crate::egress::LocalEgress;
use crate::first_data::{self, FIRST_BYTE_WAIT};
use crate::manager::{ACCEPT_BACKOFF, Access, Active, Connection, Traffic};
use crate::route;
use crate::routes::RouteAtStart;
use crate::session::{self, Shared};
use crate::usage::IngressMeter;

/// 透明访问这一路现在的计数。读到的是调用那一刻的值。
#[derive(Debug, Clone)]
pub struct InterceptStatus {
    /// 正在进行的连接（UDP 是会话）。
    pub active: Active,
    /// 这次运行以来的流量。
    pub traffic: Traffic,
}

impl<C: Connector + Datagrams, E: LocalEgress> Access<C, E> {
    /// 接住内核改过来的 TCP 连接，直到这个 future 被丢弃。`original` 取回一条连接原来要去的地址
    /// （Linux 上是 `SO_ORIGINAL_DST`）。
    pub async fn intercept_tcp(
        &self,
        listener: TcpListener,
        original: impl Fn(&TcpStream) -> io::Result<SocketAddr> + Send + Sync + 'static,
    ) {
        loop {
            match listener.accept().await {
                Ok((stream, from)) => {
                    let connection = Connection::enter(&self.shared.intercept);
                    let target = original(&stream);
                    let span = tracing::info_span!("intercept", %from);
                    tokio::spawn(carry(Arc::clone(&self.shared), stream, target, connection).instrument(span));
                }
                Err(error) => {
                    tracing::warn!(%error, "透明访问的入口接受连接出错");
                    time::sleep(ACCEPT_BACKOFF).await;
                }
            }
        }
    }

    /// 透明访问这一路的计数。
    #[must_use]
    pub fn intercept_status(&self) -> InterceptStatus {
        InterceptStatus {
            active: Active(Arc::clone(&self.shared.intercept.active)),
            traffic: Traffic(Arc::clone(&self.shared.intercept.traffic)),
        }
    }
}

/// 把一条被拦截的连接接到它原来要去的那个对端。
async fn carry<C: Connector + Datagrams, E>(
    shared: Arc<Shared<C, E>>,
    mut stream: TcpStream,
    original: io::Result<SocketAddr>,
    connection: Connection,
) {
    let original = match original {
        Ok(original) => original,
        Err(error) => {
            tracing::info!(%error, "重置了一条被拦截的连接：取不到它原来要去的地址");
            return session::reset(stream);
        }
    };
    let netmap = Arc::clone(&shared.netmap.borrow());
    let peer = match route::mesh_node(&netmap, &Host::Ip(original.ip())) {
        Some(Ok(peer)) => peer.id,
        Some(Err(reason)) => {
            tracing::info!(%original, %reason, "重置了一条被拦截的连接");
            return session::reset(stream);
        }
        None => {
            tracing::info!(%original, "重置了一条连到透明访问入口的连接：原目标不是组网的地址（有人直接连了入口的端口）");
            return session::reset(stream);
        }
    };
    if shared.refuse_while_draining("被拦截的连接") {
        return session::reset(stream);
    }
    if let Err(error) = stream.set_nodelay(true) {
        tracing::debug!(%error, "设置不了 TCP_NODELAY");
    }
    if let Err(error) = i3t_flow::keep_alive(&stream) {
        tracing::debug!(%error, "客户端连接打不开内核保活");
    }
    let first_data = match first_data::read(&mut stream, Vec::new(), FIRST_BYTE_WAIT).await {
        Ok(first_data) => first_data,
        Err(error) => return tracing::debug!(%error, "客户端在发出数据之前断开了"),
    };
    let meter = IngressMeter { listen: &connection.traffic, credential: None, exit: None };
    meter.record(true, first_data.len());
    // 客户端是本机的程序，和回环上的代理入口一样不写来源地址。
    let origin = Origin { credential: None, client: None };
    let request = Open { target: Target::Node { port: original.port() }, class: netmap.network.class.unwrap_or_default(), origin };
    tracing::debug!(%peer, port = original.port(), "被拦截的连接经对端节点出去");
    let route = RouteAtStart::now(&*shared.routes, peer);
    match i3t_flow::open(&*shared.connector, peer, &request, &first_data).await {
        Ok(pending) => {
            let summary = pending.relay_metered(stream, future::pending(), &meter).await;
            session::log_summary(peer, &request, &summary, &route.ended(&*shared.routes), &shared.netmap.borrow());
        }
        Err(error) => {
            tracing::info!(%peer, ?error, "到对端节点建流失败，重置了被拦截的连接");
            session::reset(stream);
        }
    }
}
