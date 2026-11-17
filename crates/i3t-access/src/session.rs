//! 一条客户端连接，从握手到搬运结束。

use std::future;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_conn::Connector;
use i3t_conn::contract::Datagrams;
use i3t_flow::datagram::Opener;
use i3t_flow::{End, FlowError, Framing, Summary};
use i3t_model::Netmap;
use i3t_model::netmap::{Credential, Forward, Listener, ListenerProtocol, TokenHash};
use i3t_proto::NodeId;
use i3t_proto::flow::{Host, Open, Origin, Reason, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time;

use crate::credential;
use crate::egress::{self, EgressPolicy, LocalEgress};
use crate::failure::Failure;
use crate::first_data::{self, FIRST_BYTE_WAIT};
use crate::forward_health::{ForwardHealth, TargetCause};
use crate::manager::{Connection, Counters, PortBook};
use crate::message::{Parsed, Parser, ProxyAuth};
use crate::route::{self, Route};
use crate::routes::{RouteAtStart, RouteSpan, SharedRoutes};
use crate::selector::Selector;
use crate::usage::{ExitTally, IngressMeter, Usage};
use crate::{http, socks5, udp};

/// 客户端要在这么长时间之内完成代理握手。握手只有几个小报文，这个时限只是不让不说话的连接一直占着。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 握手阶段每次最多从客户端读这么多。
const READ_CHUNK: usize = 4096;

/// 回了失败的应答之后，最多再等客户端这么久才关连接。
const CLOSE_LINGER: Duration = Duration::from_millis(500);

/// 所有连接共用的东西。
pub(crate) struct Shared<C: Datagrams, E> {
    pub connector: Arc<C>,
    /// 本节点开的数据报上下文。对端发回来的数据报由它分发，所以 [`crate::Access::run`] 要让它一直跑着。
    pub opener: Arc<Opener<C>>,
    pub egress: E,
    pub netmap: watch::Receiver<Arc<Netmap>>,
    /// 按凭据、按出口的用量，和凭据的并发名额。
    pub usage: Usage,
    /// 端口写 0 的端口转发挑定的端口记在哪里。
    pub ports: Arc<dyn PortBook>,
    /// 节点正在排空时有值：为什么排空，写进拒绝的日志。
    pub draining: Mutex<Option<String>>,
    /// 因为排空而拒绝的连接和 UDP 会话。
    pub drained: AtomicU64,
    /// UDP 端口转发的会话没有报文往来多久之后回收。
    pub udp_idle_timeout: Duration,
    /// 透明访问这一路的连接数和流量。
    pub intercept: Counters,
    /// 端口转发的目标最近连不上的记录。
    pub forward_health: ForwardHealth,
    /// 到各个对端现在走哪条路，写进每条流结束时的摘要。
    pub routes: SharedRoutes,
}

impl<C: Datagrams, E> Shared<C, E> {
    /// 节点正在排空时记一次拒绝，返回 `true`。
    pub(crate) fn refuse_while_draining(&self, what: &str) -> bool {
        let Some(why) = self.draining.lock().expect("draining mutex poisoned").clone() else { return false };
        self.drained.fetch_add(1, Ordering::Relaxed);
        tracing::info!(what, why, "拒绝了一个新的请求：节点正在排空");
        true
    }
}

/// 握手阶段的客户端连接：带着一个缓冲区，里面是已经读进来、还没有被解析走的字节。
struct Client {
    stream: TcpStream,
    buffer: Vec<u8>,
}

/// 握手阶段读一个报文失败的原因。
enum Broken<E> {
    Io(io::Error),
    /// 客户端在报文发完之前关掉了连接。
    Closed,
    Malformed(E),
}

impl Client {
    /// 读到 `parse` 认出一个完整的报文为止。缓冲区里它后面的字节留给下一次。
    ///
    /// 这里不限制缓冲区的长度：两种协议的解析器都保证"还不完整"不会无限持续（见各自的模块文档）。
    async fn read<T, E>(&mut self, parse: Parser<T, E>) -> Result<T, Broken<E>> {
        loop {
            if let Some(Parsed { value, len }) = parse(&self.buffer).map_err(Broken::Malformed)? {
                self.buffer.drain(..len);
                return Ok(value);
            }
            self.buffer.reserve(READ_CHUNK);
            // usize 不会比 u64 宽，这里不会截断。
            let read = (&mut self.stream).take(READ_CHUNK as u64).read_buf(&mut self.buffer).await.map_err(Broken::Io)?;
            if read == 0 {
                return Err(Broken::Closed);
            }
        }
    }

    /// 告诉客户端请求失败了，然后关闭连接。写不出去说明客户端已经走了，没有人需要这个应答。
    async fn fail(mut self, front: Front, failure: Failure) {
        let _ = self.stream.write_all(&front.failure(failure)).await;
        self.close().await;
    }

    /// 发完失败的应答之后关闭连接，并且让应答送得到。
    ///
    /// 客户端可能还在发数据（比如没读完的超长请求头）。带着没读的数据直接关，内核发的是 RST，
    /// 已经写出去的应答可能被冲掉。所以先只关写的一半，把客户端发来的东西读掉，等它关了或者过了一小会儿再关。
    async fn close(mut self) {
        let discard = async {
            self.stream.shutdown().await?;
            let mut sink = [0; 1024];
            while self.stream.read(&mut sink).await? > 0 {}
            io::Result::Ok(())
        };
        // 超时和出错都只是不再等了，连接反正要关。
        let _ = time::timeout(CLOSE_LINGER, discard).await;
    }

    /// 重置客户端的连接：已经告诉过它"连上了"，之后的失败只能这样表现。
    fn reset(self) {
        reset(self.stream);
    }
}

/// 用 RST 关掉一条连接。设不上 SO_LINGER 时退化成普通的关闭，没有更好的办法。
pub(crate) fn reset(stream: TcpStream) {
    let _ = stream.set_zero_linger();
    drop(stream);
}

/// 客户端说的是哪种代理协议。
#[derive(Debug, Clone, Copy)]
enum Front {
    Socks5,
    Http,
}

impl Front {
    fn established(self) -> Vec<u8> {
        match self {
            Self::Socks5 => socks5::reply(socks5::ReplyCode::Succeeded).to_vec(),
            Self::Http => http::Response::Established.as_bytes().to_vec(),
        }
    }

    fn failure(self, failure: Failure) -> Vec<u8> {
        match self {
            Self::Socks5 => socks5::reply(failure.socks5()).to_vec(),
            Self::Http => failure.http().as_bytes().to_vec(),
        }
    }
}

/// 客户端要做什么。
enum Command {
    /// 一条隧道：SOCKS5 或 HTTP 的 CONNECT。
    Connect,
    /// SOCKS5 的 UDP ASSOCIATE。
    UdpAssociate,
    /// 转发一个明文 HTTP 请求。
    Forward(Plain),
}

/// 一个要转发的明文 HTTP 请求，已经准备好随 OPEN 发出。
struct Plain {
    /// 改写好的请求头，加上握手时一起读进来的那部分请求体。
    first: Vec<u8>,
    /// 请求体剩下的部分到哪里为止。
    body: http::BodyFraming,
}

/// 握手得到的结果：客户端要做什么、去哪里，认证里写了什么。
struct Ask {
    command: Command,
    /// CONNECT 和明文 HTTP 的目标；UDP ASSOCIATE 时是客户端自称会用来发报文的地址。
    host: Host,
    port: u16,
    /// 用户名；没有做认证时是空的。
    username: String,
    /// 监听器要求凭据时，密码字段对上的那个凭据。
    credential: Option<Credential>,
}

/// 核对认证里的密码。监听器不要求凭据时一律通过；要求时必须对上 netmap 里的某个凭据。
fn authorize(credentials: &[Credential], required: bool, auth: Option<&ProxyAuth>) -> Result<Option<Credential>, Unauthorized> {
    if !required {
        return Ok(None);
    }
    let auth = auth.ok_or(Unauthorized)?;
    credential::find(credentials, &auth.password).cloned().map(Some).ok_or(Unauthorized)
}

struct Unauthorized;

/// 非回环的监听器一定要凭据，不管 netmap 里怎么写：能连到这个端口的不只是本机的程序。
fn requires_credential(listener: &Listener) -> bool {
    listener.require_credential || !listener.bind.ip().is_loopback()
}

/// SOCKS5 握手：问候、（可选的）用户名密码认证、请求（CONNECT 或者 UDP ASSOCIATE）。失败时已经回了相应的应答，返回 `None`。
async fn socks5_handshake(client: &mut Client, credentials: &[Credential], required: bool) -> Option<Ask> {
    let greeting = match client.read(socks5::Greeting::parse).await {
        Ok(greeting) => greeting,
        Err(broken) => return give_up("问候", &broken),
    };
    let method = greeting.choose(required);
    client.stream.write_all(&socks5::method_selection(method)).await.ok()?;
    let Some(method) = method else {
        tracing::info!("拒绝了一条 SOCKS5 连接：客户端没有提供可用的认证方法（这个监听器要求凭据）");
        return None;
    };

    let auth = match method {
        socks5::Method::None => None,
        socks5::Method::Password => match client.read(socks5::parse_auth).await {
            Ok(auth) => Some(auth),
            Err(broken) => return give_up("认证", &broken),
        },
    };
    let credential = authorize(credentials, required, auth.as_ref());
    if auth.is_some() {
        client.stream.write_all(&socks5::auth_status(credential.is_ok())).await.ok()?;
    }
    let Ok(credential) = credential else {
        tracing::info!("拒绝了一条 SOCKS5 连接：密码字段里的凭据不对");
        return None;
    };

    match client.read(socks5::Request::parse).await {
        Ok(socks5::Request { command, host, port }) => {
            let command = match command {
                socks5::Command::Connect => Command::Connect,
                socks5::Command::UdpAssociate => Command::UdpAssociate,
            };
            Some(Ask { command, host, port, username: auth.map(|auth| auth.username).unwrap_or_default(), credential })
        }
        Err(Broken::Malformed(error)) => {
            tracing::info!(%error, "拒绝了一个 SOCKS5 请求");
            // 写不出去说明客户端已经走了。
            let _ = client.stream.write_all(&socks5::reply(error.reply())).await;
            None
        }
        Err(broken) => give_up("请求", &broken),
    }
}

/// HTTP 握手：一个 CONNECT 请求，或者一个要转发的明文 HTTP 请求。失败时已经回了相应的应答，返回 `None`。
async fn http_handshake(client: &mut Client, credentials: &[Credential], required: bool) -> Option<Ask> {
    let request = match client.read(http::Request::parse).await {
        Ok(request) => request,
        Err(Broken::Malformed(error)) => {
            tracing::info!(%error, "拒绝了一个 HTTP 代理请求");
            // 写不出去说明客户端已经走了。
            let _ = client.stream.write_all(error.response().as_bytes()).await;
            return None;
        }
        Err(broken) => return give_up("请求", &broken),
    };
    let (command, host, port, auth) = match request {
        http::Request::Connect(http::Connect { host, port, auth }) => (Command::Connect, host, port, auth),
        http::Request::Forward(http::Forward { host, port, auth, head, body }) => {
            // 握手时多读进来的是请求体的开头，属于这个请求的那一截随请求头一起发出；后面的（流水线里的下一个请求）丢掉。
            let mut body = body.framing();
            let buffered = std::mem::take(&mut client.buffer);
            let Ok(taken) = body.take(&buffered) else {
                tracing::info!("拒绝了一个明文 HTTP 请求：chunked 的请求体写得不对");
                // 写不出去说明客户端已经走了。
                let _ = client.stream.write_all(http::Response::BadRequest.as_bytes()).await;
                return None;
            };
            let first = [head.as_slice(), &buffered[..taken]].concat();
            (Command::Forward(Plain { first, body }), host, port, auth)
        }
    };
    let Ok(credential) = authorize(credentials, required, auth.as_ref()) else {
        tracing::info!(presented = auth.is_some(), "拒绝了一个 HTTP 代理请求：没有带凭据，或者凭据不对");
        // 写不出去说明客户端已经走了。
        let _ = client.stream.write_all(http::Response::ProxyAuthenticationRequired.as_bytes()).await;
        return None;
    };
    let username = auth.map(|auth| auth.username).unwrap_or_default();
    Some(Ask { command, host, port, username, credential })
}

/// 握手读到一半读不下去了：连接出错、客户端走了，或者发来的根本不是这种协议。没有什么可回的，记一行就放弃。
fn give_up<E: std::fmt::Display>(stage: &str, broken: &Broken<E>) -> Option<Ask> {
    match broken {
        Broken::Io(error) => tracing::debug!(stage, %error, "握手时连接出错"),
        Broken::Closed => tracing::debug!(stage, "客户端在握手完成之前关掉了连接"),
        Broken::Malformed(error) => tracing::info!(stage, %error, "拒绝了一条连接：握手报文不合法"),
    }
    None
}

/// 处理代理监听器上的一条连接。
pub(crate) async fn proxy<C: Connector + Datagrams, E: LocalEgress>(
    shared: &Arc<Shared<C, E>>,
    netmap: &Netmap,
    listener: &Listener,
    stream: TcpStream,
    from: SocketAddr,
    connection: &Connection,
) {
    let mut client = Client { stream, buffer: Vec::new() };
    let required = requires_credential(listener);
    let (front, handshake) = match listener.protocol {
        ListenerProtocol::Socks5 => {
            (Front::Socks5, time::timeout(HANDSHAKE_TIMEOUT, socks5_handshake(&mut client, &netmap.credentials, required)).await)
        }
        ListenerProtocol::Http => {
            (Front::Http, time::timeout(HANDSHAKE_TIMEOUT, http_handshake(&mut client, &netmap.credentials, required)).await)
        }
    };
    let ask = match handshake {
        Ok(Some(ask)) => ask,
        Ok(None) => return client.close().await,
        Err(_) => {
            tracing::info!("关掉了一条连接：{HANDSHAKE_TIMEOUT:?} 之内没有完成代理握手");
            return;
        }
    };
    // 排空时也先把握手做完：客户端要拿到一个看得懂的拒绝，而不是连接莫名其妙地断了。认证在拒绝之前，
    // 没有凭据的人看不出节点在排空。
    if shared.refuse_while_draining("代理请求") {
        return client.fail(front, Failure::Draining).await;
    }

    let selector = match ask.username.parse::<Selector>() {
        Ok(selector) => selector,
        Err(error) => {
            tracing::info!(%error, "拒绝了一个代理请求：用户名不合法");
            return client.fail(front, Failure::Denied).await;
        }
    };
    if let Some(credential) = ask.credential.as_ref().filter(|credential| credential.exhausted) {
        tracing::info!(credential = %credential.name, "拒绝了一个代理请求：这个凭据的流量上限已经用完");
        return client.fail(front, Failure::LimitReached).await;
    }
    // 名额占到这条连接结束。
    let slot = match &ask.credential {
        Some(credential) => match shared.usage.acquire(credential) {
            Some(slot) => Some(slot),
            None => {
                tracing::info!(credential = %credential.name, "拒绝了一个代理请求：这个凭据的并发连接数到了上限");
                return client.fail(front, Failure::TooManyConnections).await;
            }
        },
        None => None,
    };
    let limit = Limit { netmap: shared.netmap.clone(), credential: ask.credential.as_ref().map(|credential| credential.token) };
    if matches!(ask.command, Command::UdpAssociate) {
        // 每个报文各有各的目标，往哪里去等报文来了再判断。
        let origin = Origin { credential: ask.credential.map(|credential| credential.name), client: origin_address(from) };
        let meter = IngressMeter { listen: &connection.traffic, credential: slot.as_ref().map(|slot| slot.traffic()), exit: None };
        let request = udp::association::Ask { declared: (ask.host, ask.port), selector, origin, meter };
        let association = udp::association::run(shared, listener, client.stream, from, request);
        return tokio::select! {
            () = association => {}
            reason = limit.reached() => tracing::info!(?reason, "凭据的流量上限用完了，结束这个 UDP 关联"),
        };
    }
    let route = match route::decide(netmap, listener, &selector, &ask.host, ask.port) {
        Ok(route) => route,
        Err(reason) => {
            tracing::info!(%reason, "拒绝了一个代理请求");
            return client.fail(front, Failure::Denied).await;
        }
    };

    let class = route::class_for(netmap, selector.class, listener.class, &route);
    let plain = match ask.command {
        Command::Forward(plain) => Some(plain),
        Command::Connect | Command::UdpAssociate => None,
    };
    match route {
        Route::Flow { peer, target, optimistic } => {
            let optimistic = route::optimistic_for(netmap, listener.optimistic, peer, &target, optimistic);
            let exit = target.needs_exit().then(|| shared.usage.exit(peer));
            if let Some(exit) = &exit {
                exit.started();
            }
            let meter = IngressMeter {
                listen: &connection.traffic,
                credential: slot.as_ref().map(|slot| slot.traffic()),
                exit: exit.as_deref().map(ExitTally::traffic),
            };
            let origin = Origin { credential: ask.credential.map(|credential| credential.name), client: origin_address(from) };
            let request = Open { target, class, origin };
            tracing::debug!(%peer, kind = request.target.kind(), ?class, optimistic, "代理请求经对端节点出去");
            let flow = Outbound { peer, request: &request, meter, exit: exit.as_deref(), limit };
            // 明文 HTTP 总是等 REPLY：目标的应答本来就在 REPLY 之后才到，等它不多花时间，被拒绝时客户端还能拿到状态码。
            if optimistic && plain.is_none() {
                without_waiting(shared, client, front, flow).await;
            } else {
                after_reply(shared, client, front, flow, plain).await;
            }
        }
        Route::Local { policy } => {
            tracing::debug!(?policy, "代理请求就地出站");
            let meter = IngressMeter { listen: &connection.traffic, credential: slot.as_ref().map(|slot| slot.traffic()), exit: None };
            locally(shared, client, front, Destination { host: &ask.host, port: ask.port, policy, plain }, meter, limit).await;
        }
    }
}

/// 一个连接受的流量上限：凭据的流量用完时（netmap 里它被标成用完了），正在进行的连接也要断开。
struct Limit {
    netmap: watch::Receiver<Arc<Netmap>>,
    credential: Option<TokenHash>,
}

impl Limit {
    /// 等到这个连接的凭据被标成用完。没有凭据，或者节点在关闭时一直等下去。
    ///
    /// 凭据被吊销（从 netmap 里消失）不断开已经建立的连接：吊销只管新的连接，和别的配置变更一样不打断手上的流。
    async fn reached(mut self) -> Reason {
        let Some(token) = self.credential else { return future::pending().await };
        loop {
            let exhausted =
                self.netmap.borrow_and_update().credentials.iter().any(|credential| credential.token == token && credential.exhausted);
            if exhausted {
                tracing::info!("这条连接用的凭据的流量上限用完了，断开它");
                return Reason::LimitReached;
            }
            if self.netmap.changed().await.is_err() {
                return future::pending().await;
            }
        }
    }
}

/// 一条到对端节点的流要用到的东西。
struct Outbound<'a> {
    peer: NodeId,
    request: &'a Open,
    meter: IngressMeter<'a>,
    /// 对端是出口时，发起这一侧看到的计数记在这里。
    exit: Option<&'a ExitTally>,
    limit: Limit,
}

/// 乐观回复：先告诉客户端连上了，读到它的首批数据，和 OPEN 一起发出去，不等对端的 REPLY。
async fn without_waiting<C: Connector + Datagrams, E>(shared: &Shared<C, E>, mut client: Client, front: Front, flow: Outbound<'_>) {
    let Outbound { peer, request, meter, exit, limit } = flow;
    if let Err(error) = client.stream.write_all(&front.established()).await {
        return tracing::debug!(%error, "客户端在应答写出去之前断开了");
    }
    let buffered = std::mem::take(&mut client.buffer);
    let first_data = match first_data::read(&mut client.stream, buffered, FIRST_BYTE_WAIT).await {
        Ok(first_data) => first_data,
        Err(error) => return tracing::debug!(%error, "客户端在发出数据之前断开了"),
    };
    // 首批数据随 OPEN 一起发出，不经过搬运时的关卡，在这里补记。
    meter.record(true, first_data.len());
    let route = RouteAtStart::now(&*shared.routes, peer);
    match i3t_flow::open(&*shared.connector, peer, request, &first_data).await {
        Ok(pending) => {
            let summary = pending.relay_metered(client.stream, limit.reached(), &meter).await;
            if let (Some(exit), End::Refused(reply)) = (exit, &summary.end) {
                exit.refused_with(reply.status);
            }
            log_summary(peer, request, &summary, &route.ended(&*shared.routes), &shared.netmap.borrow());
        }
        Err(error) => {
            if let Some(exit) = exit {
                exit.unreachable();
            }
            tracing::info!(%peer, ?error, "到对端节点建流失败，重置了客户端的连接");
            client.reset();
        }
    }
}

/// 不用乐观回复：等对端说连上了，才告诉客户端；对端拒绝时客户端拿到的是具体的错误码。
///
/// 转发明文 HTTP 请求（`plain`）时首批数据是改写好的请求，连上之后没有"连上了"要告诉客户端，目标的应答就是给它的回答；
/// 之后只搬这个请求剩下的请求体。
async fn after_reply<C: Connector + Datagrams, E>(
    shared: &Shared<C, E>,
    mut client: Client,
    front: Front,
    flow: Outbound<'_>,
    plain: Option<Plain>,
) {
    let Outbound { peer, request, meter, exit, limit } = flow;
    // 客户端这时还没有收到应答，通常什么都没有发；万一握手时多读进来了一点，就当作首批数据。
    let (first_data, mut body) = match plain {
        Some(Plain { first, body }) => (first, Some(body)),
        None => (std::mem::take(&mut client.buffer), None),
    };
    meter.record(true, first_data.len());
    let route = RouteAtStart::now(&*shared.routes, peer);
    let opened = match i3t_flow::open(&*shared.connector, peer, request, &first_data).await {
        Ok(pending) => pending.established().await,
        Err(error) => {
            if let Some(exit) = exit {
                exit.unreachable();
            }
            Err(error)
        }
    };
    let flow = match opened {
        Ok(flow) => flow,
        Err(FlowError::Refused(reply)) => {
            if let Some(exit) = exit {
                exit.refused_with(reply.status);
            }
            // 说明是对端写的，按 Debug 打印，里面的控制字符会被转义。
            tracing::info!(%peer, kind = request.target.kind(), status = ?reply.status, detail = ?reply.detail, "对端节点拒绝了建流请求");
            return client.fail(front, Failure::from(reply.status)).await;
        }
        Err(error) => {
            tracing::info!(%peer, ?error, "到对端节点建流失败");
            return client.fail(front, Failure::ExitUnavailable).await;
        }
    };
    let summary = match &mut body {
        Some(body) => flow.relay_framed(client.stream, limit.reached(), &meter, body).await,
        None => {
            if let Err(error) = client.stream.write_all(&front.established()).await {
                tracing::debug!(%error, "客户端在应答写出去之前断开了");
                return flow.abort(Reason::ClientReset);
            }
            flow.relay_metered(client.stream, limit.reached(), &meter).await
        }
    };
    log_summary(peer, request, &summary, &route.ended(&*shared.routes), &shared.netmap.borrow());
}

/// 就地出站要去的地方。
struct Destination<'a> {
    host: &'a Host,
    port: u16,
    policy: EgressPolicy,
    /// 转发的是一个明文 HTTP 请求。
    plain: Option<Plain>,
}

/// 就地出站：本机直接连目标，连上了才告诉客户端。
async fn locally<C: Datagrams, E: LocalEgress>(
    shared: &Shared<C, E>,
    mut client: Client,
    front: Front,
    destination: Destination<'_>,
    meter: IngressMeter<'_>,
    limit: Limit,
) {
    let Destination { host, port, policy, plain } = destination;
    let mut target = match shared.egress.connect(host, port, policy).await {
        Ok(target) => target,
        Err(reply) => {
            tracing::info!(?policy, status = ?reply.status, detail = %reply.detail, "就地出站没有连上目标");
            return client.fail(front, Failure::from(reply.status)).await;
        }
    };
    let (first_data, mut body) = match plain {
        Some(Plain { first, body }) => (first, Some(body)),
        // 握手时多读进来的字节是客户端发给目标的。
        None => (std::mem::take(&mut client.buffer), None),
    };
    let prepared = async {
        if body.is_none() {
            client.stream.write_all(&front.established()).await?;
        }
        target.write_all(&first_data).await
    };
    if let Err(error) = prepared.await {
        return tracing::debug!(%error, "就地出站的连接在开始搬运之前断开了");
    }
    meter.record(true, first_data.len());
    let framing = body.as_mut().map(|body| body as &mut (dyn Framing + Send));
    match egress::relay(client.stream, target, &meter, limit.reached(), framing).await {
        Ok(copied) => tracing::info!(sent = copied.from_client, received = copied.from_target, "就地出站的连接结束"),
        Err(error) => tracing::info!(%error, "就地出站的连接异常结束，两头都已重置"),
    }
}

/// 处理端口转发监听上的一条连接：它整个变成到目标节点的一条流。
///
/// 端口转发没有地方告诉客户端错误码，所以不等对端的 REPLY：失败一律表现为连接被重置。
pub(crate) async fn forward<C: Connector + Datagrams, E>(
    shared: &Shared<C, E>,
    netmap: &Netmap,
    forward: &Forward,
    mut stream: TcpStream,
    from: SocketAddr,
    connection: &Connection,
) {
    if shared.refuse_while_draining("端口转发的连接") {
        return reset(stream);
    }
    if !netmap.peer(forward.target).is_some_and(|peer| peer.outbound) {
        tracing::info!(target = %forward.target, "重置了一条端口转发的连接：目标节点不在 netmap 里，或者访问规则不允许本节点访问它");
        return reset(stream);
    }
    let first_data = match first_data::read(&mut stream, Vec::new(), FIRST_BYTE_WAIT).await {
        Ok(first_data) => first_data,
        Err(error) => return tracing::debug!(%error, "客户端在发出数据之前断开了"),
    };
    let meter = IngressMeter { listen: &connection.traffic, credential: None, exit: None };
    meter.record(true, first_data.len());
    let origin = Origin { credential: None, client: origin_address(from) };
    let class = forward.class.or(netmap.network.class).unwrap_or_default();
    let request = Open { target: Target::Forward { id: forward.id }, class, origin };
    let route = RouteAtStart::now(&*shared.routes, forward.target);
    match i3t_flow::open(&*shared.connector, forward.target, &request, &first_data).await {
        Ok(pending) => {
            let summary = pending.relay_metered(stream, future::pending(), &meter).await;
            if let Some(outcome) = TargetCause::of_summary(&summary) {
                shared.forward_health.record(forward.id, outcome);
            }
            log_summary(forward.target, &request, &summary, &route.ended(&*shared.routes), &shared.netmap.borrow());
        }
        Err(error) => {
            tracing::info!(target = %forward.target, ?error, "到目标节点建流失败，重置了端口转发的连接");
            shared.forward_health.record(forward.id, Err(TargetCause::NodeUnreachable));
            reset(stream);
        }
    }
}

/// 写进 OPEN 的客户端地址。本机的程序连过来时不带：回环地址对另一头没有意义。
pub(crate) fn origin_address(from: SocketAddr) -> Option<SocketAddr> {
    let ip = from.ip().to_canonical();
    (!ip.is_loopback()).then(|| SocketAddr::new(ip, from.port()))
}

/// 每条流结束时的一行摘要（白皮书 §13）。不记目标地址，只记它是哪一类；记这对节点的连接开始和结束时走的路（relay 写名字，
/// 从 `netmap` 里找）、中间换没换过。不采样。
pub(crate) fn log_summary(peer: NodeId, request: &Open, summary: &Summary, route: &RouteSpan, netmap: &Netmap) {
    tracing::info!(
        %peer,
        kind = request.target.kind(),
        class = ?request.class,
        sent = summary.from_tcp.bytes,
        received = summary.from_flow.bytes,
        first_byte_ms = summary.from_flow.first_byte.map(|elapsed| elapsed.as_millis()),
        end = ?summary.end,
        route_start = %route.start_label(netmap),
        route_end = %route.end_label(netmap),
        route_changed = route.changed,
        "流结束"
    );
}

#[cfg(test)]
mod tests {
    use ring::digest;

    use super::*;
    use crate::message::Password;

    fn credential(token: &str) -> Credential {
        let hash = digest::digest(&digest::SHA256, token.as_bytes());
        Credential {
            name: "gw".into(),
            token: TokenHash::from_bytes(hash.as_ref().try_into().unwrap()),
            max_concurrent: None,
            id: 1,
            exhausted: false,
        }
    }

    fn auth(password: &str) -> ProxyAuth {
        ProxyAuth { username: "nas-01".into(), password: Password::new(password.as_bytes()) }
    }

    #[test]
    fn a_listener_that_does_not_require_a_credential_ignores_the_password() {
        let credentials = [credential("token")];
        assert!(matches!(authorize(&credentials, false, None), Ok(None)));
        assert!(matches!(authorize(&credentials, false, Some(&auth("anything"))), Ok(None)));
    }

    #[test]
    fn a_required_credential_must_be_present_and_correct() {
        let credentials = [credential("token")];
        assert!(matches!(authorize(&credentials, true, Some(&auth("token"))), Ok(Some(found)) if found.name == "gw"));
        assert!(authorize(&credentials, true, Some(&auth("wrong"))).is_err());
        assert!(authorize(&credentials, true, Some(&auth(""))).is_err());
        assert!(authorize(&credentials, true, None).is_err());
        assert!(authorize(&[], true, Some(&auth("token"))).is_err());
    }

    #[test]
    fn non_loopback_listeners_always_require_a_credential() {
        let listener = |bind: &str, require_credential| Listener {
            name: "l".into(),
            bind: bind.parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            allow_from: Vec::new(),
            require_credential,
            default_action: None,
            fixed_exit: None,
            class: None,
            optimistic: None,
        };
        assert!(!requires_credential(&listener("127.0.0.1:1080", false)));
        assert!(requires_credential(&listener("127.0.0.1:1080", true)));
        assert!(requires_credential(&listener("0.0.0.0:1080", false)));
        assert!(requires_credential(&listener("192.168.1.5:1080", false)));
        assert!(!requires_credential(&listener("[::1]:1080", false)));
    }

    #[test]
    fn loopback_clients_are_not_reported_as_the_origin() {
        assert_eq!(origin_address("127.0.0.1:5000".parse().unwrap()), None);
        assert_eq!(origin_address("[::1]:5000".parse().unwrap()), None);
        assert_eq!(origin_address("[::ffff:127.0.0.1]:5000".parse().unwrap()), None);
        assert_eq!(origin_address("192.168.1.9:5000".parse().unwrap()), Some("192.168.1.9:5000".parse().unwrap()));
        assert_eq!(origin_address("[::ffff:192.168.1.9]:5000".parse().unwrap()), Some("192.168.1.9:5000".parse().unwrap()));
    }
}
