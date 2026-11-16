//! 节点一侧到各个 relay 的联系：UDP 和 TCP 同时试、优先 UDP、掉线重连，以及挂靠哪几个 relay 的选择。
//!
//! 一个 [`RelayLink`] 管到一个 relay 的全部事情。它不直接碰 socket：要发的东西交给 [`Wire`]，
//! 所以整套"先试谁、什么时候换"的规则可以脱离网络来测。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use i3t_crypto::{NodeKey, Signature};
use i3t_model::netmap::Relay;
use i3t_proto::session::{Accepted, Auth, Frame, Init, Nonce, Ping, RefusalReason};
use i3t_proto::{NodeId, PublicKey};

use crate::attach::{Event, Send, Session, Step};
use crate::estimate::{Estimate, Quality};
use crate::timing::{ESTIMATE_TTL, PROBE_INTERVAL, RELAY_RETRY_MAX, RELAY_SURVEY, Rtt, SETTLE, UDP_HEAD_START, UDP_RETRY};

/// 挂靠几个 relay（白皮书 §7.2 的 K）。
pub(crate) const ATTACH_COUNT: usize = 2;
/// 挂靠的 relay 连不上超过这么久，就可以被别的顶替。
const FAILING_AFTER: Duration = Duration::from_secs(10);
/// TCP 多久连不上就算这次失败。
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 本节点是谁，用来填帧和签名。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Identity<'a> {
    pub(crate) me: NodeId,
    pub(crate) key: &'a NodeKey,
}

/// [`RelayLink`] 往外发东西的出口。
pub(crate) trait Wire {
    /// 向 relay 的 UDP 地址发一帧。
    fn udp(&mut self, to: SocketAddr, frame: &[u8]);
    /// 往到这个 relay 的 TCP 里发一帧。
    fn tcp(&mut self, frame: &[u8]);
    /// 开始连 relay 的 TCP。连上、断开、收到帧都会回到 [`RelayLink`] 对应的方法上。
    fn tcp_connect(&mut self, addrs: &[SocketAddr]);
    /// 关掉到这个 relay 的 TCP。
    fn tcp_close(&mut self);
}

/// relay 没有照办，原因出在某条路上而不是会话本身。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Complaint {
    /// 这个对端没有挂在它上面。
    PeerUnattached(NodeId),
    /// 多级中转的路径走不下去：它和这个 relay 之间没有骨干连接；是它自己的编号时，表示路径比网络允许的长。
    RouteUnavailable(NodeId),
}

/// 现在经这个 relay 收发包走的是哪一种传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Leg {
    Udp(SocketAddr),
    Tcp,
}

#[derive(Debug)]
struct TcpLeg {
    session: Session,
    connected: bool,
    started: Instant,
}

/// 到一个 relay 的联系。
#[derive(Debug)]
pub(crate) struct RelayLink {
    id: NodeId,
    key: PublicKey,
    udp_addrs: Vec<SocketAddr>,
    tcp_addrs: Vec<SocketAddr>,
    /// 要不要和它保持会话：它是挂靠的 relay，或者有活跃的对端要经它到达。
    wanted: bool,
    wanted_since: Option<Instant>,
    udp: Session,
    /// 这一次用 `udp_addrs` 里的第几个。握手失败就换下一个。
    udp_choice: usize,
    tcp: Option<TcpLeg>,
    /// TCP 那条腿上一个会话留下的测量值。断了重连还是同一段链路，新的会话接着用：不这样的话，
    /// 一条隔一会儿就断的链路每次重连之后都像新的一样好，延迟优先的选路会一次次被它骗回去。
    tcp_measured: Estimate,
    /// 最近一次是因为 UDP 握不上手才用 TCP 连上的。这样的会话断了重连时只连 TCP，不再让 UDP 先试一秒：
    /// 多半还是握不上，白等。连上之后照常定期试 UDP，通了就换回去。
    fell_back: bool,
    /// 会话断没断过，见 [`is_settled`](Self::is_settled)。
    steadiness: Steadiness,
    /// 这一轮连接尝试（UDP 和 TCP 同时）是什么时候开始的。
    attempt: Option<Instant>,
    retry_at: Instant,
    failures: u32,
    /// 走着 TCP 的时候，下一次试 UDP 的时刻。
    udp_retry_at: Instant,
    /// relay 看到的本节点 UDP 地址。
    observed: Option<(SocketAddr, Instant)>,
    /// 不保持会话的 relay 靠一个 INIT / CHALLENGE 的往返测延迟：relay 那边不留任何状态。
    survey: Option<(Nonce, Instant)>,
    surveyed: Option<(Duration, Instant)>,
    next_survey: Instant,
    /// 会话保活的间隔，见 [`set_probe_interval`](Self::set_probe_interval)。
    probe_interval: Duration,
    /// 要不要给这段链路发探测包列。
    training: bool,
}

impl RelayLink {
    pub(crate) fn new(relay: &Relay, now: Instant) -> Self {
        Self {
            id: relay.id,
            key: relay.key,
            udp_addrs: relay.udp.clone(),
            tcp_addrs: relay.tcp.clone(),
            wanted: false,
            wanted_since: None,
            udp: Session::new(),
            udp_choice: 0,
            tcp: None,
            tcp_measured: Estimate::new(),
            fell_back: false,
            steadiness: Steadiness::Unbroken,
            attempt: None,
            retry_at: now,
            failures: 0,
            udp_retry_at: now,
            observed: None,
            survey: None,
            surveyed: None,
            next_survey: now,
            probe_interval: PROBE_INTERVAL,
            training: false,
        }
    }

    /// UDP 上的会话多久保活一次。本节点是 relay 时，到别的 relay 的会话就是骨干上的连接，探测得更勤（白皮书 §7.3）。
    ///
    /// TCP 上的会话不跟着变：骨干只走 UDP，TCP 上探测得再勤也测不出骨干要的东西，
    /// 反倒让一条丢包的 TCP 更容易被判为断开（一个探测卡在重传里，后面的全跟着超时）。
    pub(crate) fn set_probe_interval(&mut self, interval: Duration) {
        self.probe_interval = interval;
        self.udp.set_interval(interval);
    }

    /// 这段链路是不是在正在评估的候选路径上：是的话定期发探测包列来估丢包。
    pub(crate) fn set_training(&mut self, training: bool) {
        self.training = training;
        self.udp.set_training(training);
        if let Some(tcp) = &mut self.tcp {
            tcp.session.set_training(training);
        }
    }

    /// netmap 更新了这个 relay 的信息。
    pub(crate) fn update(&mut self, relay: &Relay) {
        self.key = relay.key;
        if self.udp_addrs != relay.udp || self.tcp_addrs != relay.tcp {
            self.udp_addrs.clone_from(&relay.udp);
            self.tcp_addrs.clone_from(&relay.tcp);
            self.udp_choice = 0;
        }
    }

    pub(crate) fn udp_addrs(&self) -> &[SocketAddr] {
        &self.udp_addrs
    }

    pub(crate) const fn is_wanted(&self) -> bool {
        self.wanted
    }

    /// 要不要和这个 relay 保持会话。不要了就立刻放手：停止保活、关掉 TCP，relay 那边的会话会自己过期。
    pub(crate) fn set_wanted(&mut self, wanted: bool, now: Instant, wire: &mut dyn Wire) {
        if self.wanted == wanted {
            return;
        }
        self.wanted = wanted;
        self.wanted_since = wanted.then_some(now);
        if wanted {
            self.retry_at = now;
            self.failures = 0;
        } else {
            self.udp.stop();
            if self.retire_tcp().is_some() {
                wire.tcp_close();
            }
            self.attempt = None;
            self.steadiness = Steadiness::Unbroken;
        }
    }

    /// 放下 TCP 那条腿，把它测到的东西留下。
    fn retire_tcp(&mut self) -> Option<TcpLeg> {
        let tcp = self.tcp.take()?;
        self.tcp_measured = tcp.session.measured().clone();
        Some(tcp)
    }

    /// 到它的会话信得过：建立之后没有断过，或者断过、重新连上之后又保持了 [`SETTLE`] 那么久。
    ///
    /// 一条时好时坏的链路，会话断了重连之后头几秒看起来总是好的。等它保持够久再把流量换过去，
    /// 免得流量跟着它一断一连地来回换。
    pub(crate) fn is_settled(&self, now: Instant) -> bool {
        self.leg().is_some()
            && match self.steadiness {
                Steadiness::Unbroken => true,
                Steadiness::Down => false,
                Steadiness::Recovered(at) => now.saturating_duration_since(at) >= SETTLE,
            }
    }

    /// 建立好的会话没了。
    fn session_lost(&mut self) {
        self.steadiness = Steadiness::Down;
    }

    /// 会话建立了。从 TCP 换回 UDP 时会话没有断过，不算重新连上。
    fn session_up(&mut self, now: Instant) {
        if self.steadiness == Steadiness::Down {
            self.steadiness = Steadiness::Recovered(now);
        }
    }

    /// 现在能经它收发包的传输。UDP 和 TCP 都通时用 UDP。
    pub(crate) fn leg(&self) -> Option<Leg> {
        if self.udp.is_up() {
            return self.udp_addr().map(Leg::Udp);
        }
        self.tcp.as_ref().filter(|tcp| tcp.session.is_up()).map(|_| Leg::Tcp)
    }

    /// 到它的延迟：有会话时是会话上测的。
    pub(crate) fn rtt(&self) -> Option<Rtt> {
        match self.leg()? {
            Leg::Udp(_) => self.udp.rtt(),
            Leg::Tcp => self.tcp.as_ref().and_then(|tcp| tcp.session.rtt()),
        }
    }

    /// 现在在用的那个会话上测到的延迟、抖动和丢包。没有会话，或者估计过期了，是 `None`。
    pub(crate) fn quality(&self, now: Instant) -> Option<Quality> {
        match self.leg()? {
            Leg::Udp(_) => self.udp.quality(now),
            Leg::Tcp => self.tcp.as_ref().and_then(|tcp| tcp.session.quality(now)),
        }
    }

    /// 选挂靠对象时用的延迟估计：有会话的用会话上测的，没有的用最近一次单独测的；都没有或者过期了就是 `None`。
    pub(crate) fn estimate(&self, now: Instant) -> Option<Duration> {
        let surveyed = self.surveyed.filter(|(_, at)| now.saturating_duration_since(*at) < ESTIMATE_TTL).map(|(rtt, _)| rtt);
        self.rtt().map(Rtt::get).or(surveyed)
    }

    /// 要它却一直连不上。
    pub(crate) fn is_failing(&self, now: Instant) -> bool {
        self.leg().is_none() && self.wanted_since.is_some_and(|since| now.saturating_duration_since(since) > FAILING_AFTER)
    }

    /// 在用的会话可疑：有保活没有得到回答。
    pub(crate) fn in_doubt(&self) -> bool {
        match self.leg() {
            Some(Leg::Udp(_)) => self.udp.in_doubt(),
            Some(Leg::Tcp) => self.tcp.as_ref().is_some_and(|tcp| tcp.session.in_doubt()),
            None => false,
        }
    }

    /// relay 看到的本节点 UDP 地址，只有 UDP 会话上得到的才算。
    pub(crate) fn observed(&self, now: Instant) -> Option<SocketAddr> {
        self.observed.filter(|(_, at)| now.saturating_duration_since(*at) < ESTIMATE_TTL).map(|(addr, _)| addr)
    }

    fn udp_addr(&self) -> Option<SocketAddr> {
        self.udp_addrs.get(self.udp_choice % self.udp_addrs.len().max(1)).copied()
    }

    /// 推进定时器。`urgent`：有包正经它发出去，却有一阵没有从它收到任何东西了。
    /// `survey`：不保持会话时要不要定期测一下延迟。
    pub(crate) fn poll(&mut self, now: Instant, who: Identity<'_>, urgent: bool, survey: bool, wire: &mut dyn Wire) {
        if !self.wanted {
            if survey {
                self.survey(now, who, wire);
            }
            return;
        }
        let step = self.udp.poll(now, urgent);
        self.apply_udp(step, now, who, wire);
        self.poll_tcp(now, who, urgent, wire);

        if self.udp.is_up() {
            if self.retire_tcp().is_some() {
                tracing::info!(relay = %self.id, "到 relay 的 UDP 通了，不再用 TCP");
                wire.tcp_close();
            }
            return;
        }
        if self.tcp.as_ref().is_some_and(|tcp| tcp.session.is_up()) {
            // 走着 TCP：定期再试 UDP，通了就换回去。
            if self.udp.is_idle() && now >= self.udp_retry_at && self.udp_addr().is_some() {
                self.udp_retry_at = now + UDP_RETRY;
                self.start_udp(now, who, wire);
            }
            return;
        }
        match self.attempt {
            None if now >= self.retry_at => self.begin_attempt(now, who, wire),
            None => {}
            Some(started) => {
                // UDP 先走一步；它这么久还没握上手，就让等在 cookie 那一步的 TCP 继续。
                // 上一轮已经试出 UDP 握不上手的话，这一轮不再让它先走。
                let head_start = if self.failures == 0 { UDP_HEAD_START } else { Duration::ZERO };
                let head_start_over = now.saturating_duration_since(started) >= head_start || self.udp.is_idle();
                if head_start_over
                    && let Some(tcp) = &mut self.tcp
                    && let Some(send) = tcp.session.release(now)
                {
                    if self.fell_back {
                        tracing::info!(relay = %self.id, "到 relay 的 UDP 上一次握不上手，这一次直接用 TCP");
                    } else {
                        tracing::info!(relay = %self.id, "到 relay 的 UDP 握不上手，改用 TCP");
                    }
                    self.send_tcp(send, who, wire);
                }
                if self.udp.is_idle() && self.tcp.is_none() {
                    self.attempt_failed(now);
                }
            }
        }
    }

    fn poll_tcp(&mut self, now: Instant, who: Identity<'_>, urgent: bool, wire: &mut dyn Wire) {
        let Some(tcp) = &mut self.tcp else { return };
        if !tcp.connected {
            if now.saturating_duration_since(tcp.started) >= TCP_CONNECT_TIMEOUT {
                tracing::debug!(relay = %self.id, "到 relay 的 TCP 连接超时");
                self.retire_tcp();
                wire.tcp_close();
            }
            return;
        }
        let step = tcp.session.poll(now, urgent);
        self.apply_tcp(step, now, who, wire);
    }

    fn begin_attempt(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        self.attempt = Some(now);
        let tcp_only = self.fell_back && !self.tcp_addrs.is_empty();
        if tcp_only {
            // 定期试 UDP 的握手要是正好进行到一半，放下它：relay 上一个节点只有一个会话，两边一起握手会互相顶掉。
            self.udp.stop();
        } else if self.udp_addr().is_some() {
            self.start_udp(now, who, wire);
        }
        if !self.tcp_addrs.is_empty() {
            let mut session = Session::resuming(self.tcp_measured.clone());
            session.set_training(self.training);
            self.tcp = Some(TcpLeg { session, connected: false, started: now });
            wire.tcp_connect(&self.tcp_addrs);
        }
        if self.udp_addr().is_none() && self.tcp_addrs.is_empty() {
            tracing::warn!(relay = %self.id, "netmap 里这个 relay 没有任何地址，连不了");
            self.attempt_failed(now);
        }
    }

    fn attempt_failed(&mut self, now: Instant) {
        self.attempt = None;
        self.fell_back = false;
        self.failures = self.failures.saturating_add(1);
        self.udp_choice = self.udp_choice.wrapping_add(1);
        let wait = retry_delay(self.failures);
        self.retry_at = now + wait;
        tracing::info!(relay = %self.id, failures = self.failures, "连不上 relay，{} 秒后再试", wait.as_secs());
    }

    /// 在用的会话没了：马上重连。同一个 relay 掉线时挂靠它的节点会同时重连，所以带一点随机的延迟错开。
    fn reconnect_soon(&mut self, now: Instant) {
        self.session_lost();
        self.attempt = None;
        self.failures = 0;
        self.retry_at = now + jitter(Duration::from_millis(300));
    }

    fn start_udp(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        let send = self.udp.start(now, false);
        self.send_udp(send, who, wire);
    }

    fn send_udp(&mut self, send: Send, who: Identity<'_>, wire: &mut dyn Wire) {
        if let Some(to) = self.udp_addr() {
            wire.udp(to, &self.encode(send, who));
        }
    }

    fn send_tcp(&mut self, send: Send, who: Identity<'_>, wire: &mut dyn Wire) {
        wire.tcp(&self.encode(send, who));
    }

    fn encode(&self, send: Send, who: Identity<'_>) -> Vec<u8> {
        match send {
            Send::Init { nonce } => Init { node: who.me, nonce }.to_bytes().to_vec(),
            Send::Auth { nonce, cookie } => {
                let message = Auth::signed_message(self.id, who.me, &nonce, &cookie);
                let signature = *who.key.sign(Auth::PURPOSE, &message).as_bytes();
                Auth { node: who.me, nonce, cookie, signature }.to_bytes().to_vec()
            }
            Send::Ping { nonce } => Ping { node: who.me, nonce }.to_bytes().to_vec(),
        }
    }

    fn apply_udp(&mut self, step: Step, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        if let Some(send) = step.send {
            self.send_udp(send, who, wire);
        }
        match step.event {
            Some(Event::Up { observed }) => {
                self.observed = Some((observed, now));
                self.attempt = None;
                self.failures = 0;
                self.fell_back = false;
                self.session_up(now);
                tracing::info!(relay = %self.id, %observed, "到 relay 的中转会话建立（UDP）");
            }
            Some(Event::Down { reason }) => {
                // 握手失败留给这一轮尝试去收尾；已经建好的会话掉了才需要马上重连。
                if self.attempt.is_none() && !self.tcp.as_ref().is_some_and(|tcp| tcp.session.is_up()) {
                    tracing::info!(relay = %self.id, reason, "到 relay 的中转会话断开（UDP）");
                    self.reconnect_soon(now);
                } else {
                    tracing::debug!(relay = %self.id, reason, "到 relay 的 UDP 握手失败");
                }
            }
            Some(Event::Held) | None => {}
        }
    }

    fn apply_tcp(&mut self, step: Step, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        if let Some(send) = step.send {
            self.send_tcp(send, who, wire);
        }
        match step.event {
            Some(Event::Up { .. }) => {
                self.attempt = None;
                self.failures = 0;
                self.fell_back = true;
                self.session_up(now);
                self.udp_retry_at = now + UDP_RETRY;
                tracing::info!(relay = %self.id, "到 relay 的中转会话建立（TCP）");
            }
            Some(Event::Down { reason }) => {
                tracing::info!(relay = %self.id, reason, "到 relay 的 TCP 中转不可用");
                self.retire_tcp();
                wire.tcp_close();
                // 握手失败时这一轮尝试还没结束（`attempt` 还在）；建好的会话掉了才算"断过"。
                if self.attempt.is_none() && self.leg().is_none() {
                    self.session_lost();
                }
            }
            Some(Event::Held) | None => {}
        }
    }

    /// relay 的 UDP 地址发来的会话帧。返回 relay 对某条路的抱怨（如果它说了的话）。
    pub(crate) fn on_udp_frame(&mut self, frame: &Frame, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) -> Option<Complaint> {
        match frame {
            Frame::Challenge(challenge) => {
                if let Some((_, sent)) = self.survey.take_if(|(nonce, _)| *nonce == challenge.nonce) {
                    self.surveyed = Some((now.saturating_duration_since(sent), now));
                    self.next_survey = now + RELAY_SURVEY;
                } else {
                    let step = self.udp.on_challenge(challenge, now);
                    self.apply_udp(step, now, who, wire);
                }
            }
            Frame::Ok(ok) if self.udp.expects_ok(ok) => {
                if self.ok_is_genuine(ok, who) {
                    let step = self.udp.on_ok(ok, now);
                    self.apply_udp(step, now, who, wire);
                }
            }
            Frame::Pong(pong) => {
                if let Some(observed) = self.udp.on_pong(pong, now) {
                    self.observed = Some((observed, now));
                }
            }
            Frame::Error(refused) => match refused.reason {
                RefusalReason::NoSession if refused.node == who.me => {
                    let step = self.udp.on_no_session();
                    self.apply_udp(step, now, who, wire);
                }
                RefusalReason::PeerUnattached => return Some(Complaint::PeerUnattached(refused.node)),
                RefusalReason::RouteUnavailable => return Some(Complaint::RouteUnavailable(refused.node)),
                RefusalReason::NoSession => {}
            },
            // 别的会话帧是节点发给 relay 的，不该从 relay 来。
            Frame::Ok(_) | Frame::Init(_) | Frame::Auth(_) | Frame::Ping(_) => {}
        }
        None
    }

    /// 到这个 relay 的 TCP 连上了：在里面开始握手，先停在 cookie 那一步。
    pub(crate) fn on_tcp_connected(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        let Some(tcp) = &mut self.tcp else { return };
        tcp.connected = true;
        let send = tcp.session.start(now, true);
        self.send_tcp(send, who, wire);
    }

    /// 到这个 relay 的 TCP 里收到的会话帧。返回值同 [`on_udp_frame`](Self::on_udp_frame)。
    pub(crate) fn on_tcp_frame(&mut self, frame: &Frame, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) -> Option<Complaint> {
        let genuine = matches!(frame, Frame::Ok(ok) if self.ok_is_genuine(ok, who));
        let tcp = self.tcp.as_mut()?;
        let step = match frame {
            Frame::Challenge(challenge) => tcp.session.on_challenge(challenge, now),
            Frame::Ok(ok) if genuine => tcp.session.on_ok(ok, now),
            Frame::Pong(pong) => {
                // TCP 上看到的地址是那条 TCP 的，不是 UDP 的候选端点，不记。
                tcp.session.on_pong(pong, now);
                Step::default()
            }
            Frame::Error(refused) => match refused.reason {
                RefusalReason::NoSession if refused.node == who.me => tcp.session.on_no_session(),
                RefusalReason::PeerUnattached => return Some(Complaint::PeerUnattached(refused.node)),
                RefusalReason::RouteUnavailable => return Some(Complaint::RouteUnavailable(refused.node)),
                RefusalReason::NoSession => Step::default(),
            },
            Frame::Ok(_) | Frame::Init(_) | Frame::Auth(_) | Frame::Ping(_) => Step::default(),
        };
        self.apply_tcp(step, now, who, wire);
        None
    }

    /// 到这个 relay 的 TCP 断了，或者没连上。
    pub(crate) fn on_tcp_closed(&mut self, now: Instant) {
        let Some(tcp) = self.retire_tcp() else { return };
        if tcp.session.is_up() {
            tracing::info!(relay = %self.id, "到 relay 的 TCP 断开，中转会话没有了");
            if !self.udp.is_up() {
                self.reconnect_soon(now);
            }
        } else {
            tracing::debug!(relay = %self.id, "到 relay 的 TCP 没有连上，或者握手时断开了");
        }
    }

    fn ok_is_genuine(&self, ok: &Accepted, who: Identity<'_>) -> bool {
        let message = Accepted::signed_message(self.id, who.me, &ok.nonce, ok.valid_for, ok.observed);
        let genuine = i3t_crypto::verify(&self.key, Accepted::PURPOSE, &message, &Signature::from_bytes(ok.signature)).is_ok();
        if !genuine {
            tracing::warn!(relay = %self.id, "relay 的握手应答签名不对，忽略");
        }
        genuine
    }

    fn survey(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        if now < self.next_survey {
            return;
        }
        let Some(to) = self.udp_addr() else { return };
        // 没有回答的话过一会儿就再问，早点知道它能不能用；有回答的话下一次在一个完整的间隔之后。
        self.next_survey = now + RELAY_SURVEY / 6;
        let nonce: Nonce = i3t_underlay::random_bytes();
        self.survey = Some((nonce, now));
        wire.udp(to, &Init { node: who.me, nonce }.to_bytes());
    }
}

/// 到一个 relay 的会话断没断过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Steadiness {
    /// 建立之后没有断过（或者还没有建立过）。
    Unbroken,
    /// 断了，还没有重新连上。
    Down,
    /// 断过，在这个时刻重新连上的。
    Recovered(Instant),
}

/// 连不上之后等多久再试：1 秒起每次加倍，封顶，再加上最多五分之一的随机量。
fn retry_delay(failures: u32) -> Duration {
    let base = Duration::from_secs(1).saturating_mul(1u32.checked_shl(failures.saturating_sub(1)).unwrap_or(u32::MAX)).min(RELAY_RETRY_MAX);
    base + jitter(base / 5)
}

/// 0 到 `max` 之间的一个随机时长。
pub(crate) fn jitter(max: Duration) -> Duration {
    let [byte]: [u8; 1] = i3t_underlay::random_bytes();
    max * u32::from(byte) / 255
}

/// 选挂靠对象时一个 relay 的情况。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Standing {
    pub(crate) id: NodeId,
    /// 到它的延迟；不知道时是 `None`。
    pub(crate) rtt: Option<Duration>,
    /// 正挂靠着它，却一直连不上。
    pub(crate) failing: bool,
}

/// 决定挂靠哪几个 relay（白皮书 §7.2）。
///
/// - 管理员固定了的话就是固定的那几个。
/// - 否则保留现在挂靠的，不足 [`ATTACH_COUNT`] 个时补上延迟最低的；延迟还不知道的按 netmap 里的顺序排在后面，
///   这样刚启动时不用等测量就有 relay 可用。
/// - 外面有明显更好的（延迟低出 `max(10ms, 20%)`，或者现在挂靠的连不上）就换掉最差的一个。一次只换一个，
///   有迟滞，免得两个延迟相近的 relay 之间来回换。
pub(crate) fn choose_attached(current: &[NodeId], relays: &[Standing], pinned: &[NodeId]) -> Vec<NodeId> {
    let known = |id: &NodeId| relays.iter().any(|relay| relay.id == *id);
    if !pinned.is_empty() {
        return pinned.iter().copied().filter(known).collect();
    }
    // 延迟未知的排在已知的后面；`sort_by_key` 是稳定的，所以同样未知的保持 netmap 里的顺序。
    let rank = |relay: &Standing| (relay.failing, relay.rtt.is_none(), relay.rtt);
    let mut chosen: Vec<NodeId> = current.iter().copied().filter(known).take(ATTACH_COUNT).collect();
    let mut outside: Vec<Standing> = relays.iter().copied().filter(|relay| !chosen.contains(&relay.id)).collect();
    outside.sort_by_key(rank);
    let mut outside = outside.into_iter();
    while chosen.len() < ATTACH_COUNT {
        let Some(next) = outside.next() else { return chosen };
        chosen.push(next.id);
    }
    let standing = |id: &NodeId| relays.iter().find(|relay| relay.id == *id).copied();
    let worst = chosen.iter().filter_map(standing).max_by_key(rank);
    if let (Some(worst), Some(best)) = (worst, outside.next())
        && let Some(best_rtt) = best.rtt
        && !best.failing
    {
        let better = match worst.rtt {
            _ if worst.failing => true,
            Some(worst_rtt) => best_rtt + (worst_rtt / 5).max(Duration::from_millis(10)) < worst_rtt,
            // 现在挂靠的那个延迟还没测出来：先不动，等它有了数再比。
            None => false,
        };
        if better && let Some(slot) = chosen.iter_mut().find(|id| **id == worst.id) {
            *slot = best.id;
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use i3t_proto::session::{Challenge, Pong, Refused};

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// 记下 [`RelayLink`] 想发什么。
    #[derive(Debug, Default)]
    struct Recorder {
        udp: Vec<(SocketAddr, Frame)>,
        tcp: Vec<Frame>,
        tcp_connects: usize,
        tcp_closes: usize,
    }

    impl Wire for Recorder {
        fn udp(&mut self, to: SocketAddr, frame: &[u8]) {
            self.udp.push((to, Frame::parse(frame).expect("发出去的应该是合法的会话帧")));
        }

        fn tcp(&mut self, frame: &[u8]) {
            self.tcp.push(Frame::parse(frame).expect("发出去的应该是合法的会话帧"));
        }

        fn tcp_connect(&mut self, _addrs: &[SocketAddr]) {
            self.tcp_connects += 1;
        }

        fn tcp_close(&mut self) {
            self.tcp_closes += 1;
        }
    }

    struct Setup {
        link: RelayLink,
        wire: Recorder,
        node_key: NodeKey,
        relay_key: NodeKey,
        start: Instant,
    }

    const ME: u32 = 1;
    const RELAY: u32 = 9;

    impl Setup {
        fn new(udp: &[&str], tcp: &[&str]) -> Self {
            let relay_key = NodeKey::generate().unwrap();
            let relay = Relay {
                id: node(RELAY),
                key: relay_key.public(),
                name: "relay".to_owned(),
                udp: udp.iter().map(|text| addr(text)).collect(),
                tcp: tcp.iter().map(|text| addr(text)).collect(),
                region: String::new(),
            };
            let start = Instant::now();
            let mut setup = Self {
                link: RelayLink::new(&relay, start),
                wire: Recorder::default(),
                node_key: NodeKey::generate().unwrap(),
                relay_key,
                start,
            };
            setup.link.set_wanted(true, start, &mut setup.wire);
            setup
        }

        fn poll(&mut self, at: Duration) {
            let who = Identity { me: node(ME), key: &self.node_key };
            self.link.poll(self.start + at, who, false, false, &mut self.wire);
        }

        fn udp_frame(&mut self, frame: Frame, at: Duration) -> Option<Complaint> {
            let who = Identity { me: node(ME), key: &self.node_key };
            self.link.on_udp_frame(&frame, self.start + at, who, &mut self.wire)
        }

        fn tcp_frame(&mut self, frame: Frame, at: Duration) -> Option<Complaint> {
            let who = Identity { me: node(ME), key: &self.node_key };
            self.link.on_tcp_frame(&frame, self.start + at, who, &mut self.wire)
        }

        fn tcp_connected(&mut self, at: Duration) {
            let who = Identity { me: node(ME), key: &self.node_key };
            self.link.on_tcp_connected(self.start + at, who, &mut self.wire);
        }

        /// relay 对一个 AUTH 的回答，签名是真的。
        fn ok_for(&self, auth: &Auth, observed: &str) -> Frame {
            let observed = addr(observed);
            let message = Accepted::signed_message(node(RELAY), node(ME), &auth.nonce, 30, observed);
            let signature = *self.relay_key.sign(Accepted::PURPOSE, &message).as_bytes();
            Frame::Ok(Accepted { nonce: auth.nonce, valid_for: 30, observed, signature })
        }

        fn last_udp(&self) -> Frame {
            self.wire.udp.last().expect("应该发过 UDP 帧").1
        }

        fn last_tcp(&self) -> Frame {
            *self.wire.tcp.last().expect("应该发过 TCP 帧")
        }

        /// 把 UDP 握手走完。
        fn complete_udp(&mut self, at: Duration) {
            let Frame::Init(init) = self.last_udp() else { panic!("UDP 上最后发的应该是 INIT：{:?}", self.wire.udp) };
            self.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), at);
            let Frame::Auth(auth) = self.last_udp() else { panic!("收到 CHALLENGE 之后应该发 AUTH") };
            let ok = self.ok_for(&auth, "203.0.113.7:4000");
            self.udp_frame(ok, at);
        }

        /// 把 TCP 握手从 INIT 走到拿到 cookie。
        fn challenge_tcp(&mut self, at: Duration) {
            let Frame::Init(init) = self.last_tcp() else { panic!("TCP 上最后发的应该是 INIT：{:?}", self.wire.tcp) };
            self.tcp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [4; 16] }), at);
        }

        fn complete_tcp(&mut self, at: Duration) {
            let Frame::Auth(auth) = self.last_tcp() else { panic!("TCP 上最后发的应该是 AUTH：{:?}", self.wire.tcp) };
            let ok = self.ok_for(&auth, "203.0.113.7:5555");
            self.tcp_frame(ok, at);
        }

        /// UDP 一直没有回音，靠 TCP 把会话建起来。返回建好的时刻。
        fn fall_back_to_tcp(&mut self) -> Duration {
            self.poll(Duration::ZERO);
            self.tcp_connected(10 * MS);
            self.challenge_tcp(20 * MS);
            self.poll(UDP_HEAD_START);
            self.complete_tcp(UDP_HEAD_START + 20 * MS);
            assert_eq!(self.link.leg(), Some(Leg::Tcp));
            UDP_HEAD_START + 20 * MS
        }

        /// 从 `from` 到 `to` 每 50ms 推进一次，relay 照常回答 TCP 上的保活。
        fn keep_tcp_alive(&mut self, from: Duration, to: Duration) {
            let mut at = from;
            let mut seen = self.wire.tcp.len();
            while at < to {
                at += 50 * MS;
                self.poll(at);
                let pings: Vec<Ping> = self.wire.tcp[seen..]
                    .iter()
                    .filter_map(|frame| if let Frame::Ping(ping) = frame { Some(*ping) } else { None })
                    .collect();
                seen = self.wire.tcp.len();
                for ping in pings {
                    self.tcp_frame(Frame::Pong(Pong { nonce: ping.nonce, observed: addr("203.0.113.7:5555") }), at + 20 * MS);
                }
            }
        }

        /// 从 `from` 起 relay 在 TCP 上什么也不回答了（链路卡住了）。每 50ms 推进一次，返回会话被判为断开的时刻。
        fn stall_tcp(&mut self, from: Duration) -> Duration {
            let mut at = from;
            while self.link.leg().is_some() {
                at += 50 * MS;
                assert!(at < from + Duration::from_secs(30), "保活没有回答，会话早该断了");
                self.poll(at);
            }
            at
        }

        /// 重连的 TCP 连上之后把握手走完。返回建好的时刻。
        fn reconnect_tcp(&mut self, at: Duration) -> Duration {
            self.tcp_connected(at + 10 * MS);
            self.challenge_tcp(at + 20 * MS);
            self.poll(at + 50 * MS);
            self.complete_tcp(at + 70 * MS);
            assert_eq!(self.link.leg(), Some(Leg::Tcp));
            at + 70 * MS
        }

        fn udp_inits(&self) -> usize {
            self.wire.udp.iter().filter(|(_, frame)| matches!(frame, Frame::Init(_))).count()
        }
    }

    #[test]
    fn udp_and_tcp_are_tried_together_and_udp_wins_when_it_works() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        setup.poll(Duration::ZERO);
        assert!(matches!(setup.wire.udp[..], [(to, Frame::Init(init))] if to == addr("198.51.100.9:20330") && init.node == node(ME)));
        assert_eq!(setup.wire.tcp_connects, 1, "同时去连 TCP");

        // TCP 连上了，握手走到 cookie 就停下：AUTH 先不发。
        setup.tcp_connected(10 * MS);
        setup.challenge_tcp(20 * MS);
        assert!(matches!(setup.wire.tcp[..], [Frame::Init(_)]), "{:?}", setup.wire.tcp);

        setup.complete_udp(30 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Udp(addr("198.51.100.9:20330"))));
        assert_eq!(setup.link.observed(setup.start + 30 * MS), Some(addr("203.0.113.7:4000")));
        setup.poll(40 * MS);
        assert_eq!(setup.wire.tcp_closes, 1, "UDP 通了，TCP 不要了");
        assert!(matches!(setup.wire.tcp[..], [Frame::Init(_)]), "TCP 上始终没有发过 AUTH，relay 那边没有绑过它");
    }

    #[test]
    fn tcp_takes_over_when_udp_stays_silent_past_its_head_start() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        setup.poll(Duration::ZERO);
        setup.tcp_connected(10 * MS);
        setup.challenge_tcp(20 * MS);
        setup.poll(UDP_HEAD_START - MS);
        assert!(matches!(setup.wire.tcp[..], [Frame::Init(_)]), "还在给 UDP 留时间");

        setup.poll(UDP_HEAD_START);
        assert!(matches!(setup.last_tcp(), Frame::Auth(_)));
        setup.complete_tcp(UDP_HEAD_START + 20 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
        assert_eq!(setup.link.observed(setup.start + UDP_HEAD_START + 20 * MS), None, "TCP 上看到的地址不是 UDP 的候选端点");
        assert!(setup.link.rtt().is_some());
    }

    #[test]
    fn a_node_on_tcp_keeps_retrying_udp_and_switches_back() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        setup.poll(Duration::ZERO);
        setup.tcp_connected(10 * MS);
        setup.challenge_tcp(20 * MS);
        setup.poll(UDP_HEAD_START);
        setup.complete_tcp(UDP_HEAD_START + 20 * MS);

        // 头一次 UDP 握手耗尽重试之后，隔一段时间再来一次。停在第二次握手进行到一半的时候。
        // 这期间 relay 照常回答 TCP 上的保活。
        let mut at = UDP_HEAD_START;
        let mut answered = setup.wire.tcp.len();
        while at < UDP_RETRY + Duration::from_secs(2) {
            at += 100 * MS;
            setup.poll(at);
            let pings: Vec<Ping> = setup.wire.tcp[answered..]
                .iter()
                .filter_map(|frame| if let Frame::Ping(ping) = frame { Some(*ping) } else { None })
                .collect();
            answered = setup.wire.tcp.len();
            for ping in pings {
                setup.tcp_frame(Frame::Pong(Pong { nonce: ping.nonce, observed: addr("203.0.113.7:5555") }), at);
            }
        }
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
        let inits = setup.wire.udp.iter().filter(|(_, frame)| matches!(frame, Frame::Init(_))).count();
        assert!(inits > usize::from(crate::timing::HANDSHAKE_TRIES), "走着 TCP 的时候又试了 UDP：{inits} 个 INIT");
        assert_eq!(setup.wire.tcp_closes, 0);

        // 这一次 UDP 通了：换回 UDP，关掉 TCP。
        setup.complete_udp(at + 10 * MS);
        setup.poll(at + 20 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Udp(addr("198.51.100.9:20330"))));
        assert_eq!(setup.wire.tcp_closes, 1);
    }

    #[test]
    fn a_tcp_session_that_dies_is_rebuilt_over_tcp_alone_without_waiting_for_udp() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let up = setup.fall_back_to_tcp();
        let died = setup.stall_tcp(up);
        assert_eq!(setup.wire.tcp_connects, 2, "断开的那一刻就开始重连");
        let inits = setup.udp_inits();

        setup.tcp_connected(died + 10 * MS);
        setup.challenge_tcp(died + 20 * MS);
        setup.poll(died + 50 * MS);
        assert!(matches!(setup.last_tcp(), Frame::Auth(_)), "上一次就是 UDP 握不上手才用的 TCP，这一次不再等它一秒");
        setup.complete_tcp(died + 70 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
        assert_eq!(setup.udp_inits(), inits, "重连时没有同时去试 UDP");

        // UDP 并没有被放弃：连上之后照常定期试，通了就换回去。
        setup.keep_tcp_alive(died + 70 * MS, died + UDP_RETRY + Duration::from_secs(1));
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
        assert!(setup.udp_inits() > inits, "走着 TCP 的时候又试了 UDP");
    }

    #[test]
    fn a_reconnect_over_tcp_does_not_wait_for_a_udp_attempt_that_happens_to_be_under_way() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let up = setup.fall_back_to_tcp();
        // 走着 TCP 时定期试 UDP。TCP 在那之前不久卡住，于是会话断开的时候 UDP 的握手正进行到一半。
        let inits = setup.udp_inits();
        let stalled = up + UDP_RETRY - PROBE_INTERVAL + 100 * MS;
        setup.keep_tcp_alive(up, stalled);
        let died = setup.stall_tcp(stalled);
        let under_way = setup.udp_inits();
        assert!(under_way > inits && died < up + UDP_RETRY + Duration::from_secs(2), "UDP 的握手应该正在进行");

        let again = setup.reconnect_tcp(died);
        assert!(again < died + UDP_HEAD_START, "不等那次 UDP 握手");
        assert_eq!(setup.udp_inits(), under_way, "那次 UDP 握手放下了，不会和 TCP 的握手互相顶掉");
    }

    #[test]
    fn a_relay_that_cannot_be_reached_over_tcp_either_gets_both_transports_tried_again() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let up = setup.fall_back_to_tcp();
        let died = setup.stall_tcp(up);
        let inits = setup.udp_inits();
        // 重连的 TCP 一直连不上：这一轮算失败，下一轮 UDP 和 TCP 又一起试。
        let mut at = died;
        while setup.udp_inits() == inits {
            at += 100 * MS;
            assert!(at < died + Duration::from_secs(30), "该重新试 UDP 了");
            setup.poll(at);
        }
        assert!(setup.wire.tcp_connects >= 3);
    }

    #[test]
    fn what_a_tcp_leg_measured_is_kept_when_it_reconnects() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let up = setup.fall_back_to_tcp();
        // 好好的 40 秒（8 个保活都有回答），然后卡住：3 个保活没有回答，会话断开。
        let stalled = up + PROBE_INTERVAL * 8 + Duration::from_secs(1);
        setup.keep_tcp_alive(up, stalled);
        let died = setup.stall_tcp(stalled);
        let again = setup.reconnect_tcp(died);

        let quality = setup.link.quality(setup.start + again).expect("重连之后马上就有估计");
        assert_eq!(quality.lost, usize::from(crate::timing::MAX_TIMEOUTS), "断开之前没有回答的那几个保活还记着：{quality:?}");
        assert!(quality.probes >= 13, "断开之前的样本也还在：{quality:?}");
    }

    #[test]
    fn a_session_over_tcp_is_probed_at_the_ordinary_pace_even_towards_another_relay() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        // 本节点自己是 relay：到别的 relay 的 UDP 会话是骨干连接，2 秒探测一次。TCP 上的不跟着变。
        setup.link.set_probe_interval(crate::timing::BACKBONE_PROBE_INTERVAL);
        let up = setup.fall_back_to_tcp();
        let pings = |setup: &Setup| setup.wire.tcp.iter().filter(|frame| matches!(frame, Frame::Ping(_))).count();
        setup.keep_tcp_alive(up, up + PROBE_INTERVAL - 200 * MS);
        assert_eq!(pings(&setup), 0, "不到 5 秒不发保活");
        setup.keep_tcp_alive(up + PROBE_INTERVAL - 200 * MS, up + PROBE_INTERVAL + 200 * MS);
        assert_eq!(pings(&setup), 1);
    }

    #[test]
    fn a_session_that_never_broke_is_settled_at_once_and_one_that_reconnected_only_after_it_has_held() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        assert!(!setup.link.is_settled(setup.start), "还没有会话");
        let up = setup.fall_back_to_tcp();
        assert!(setup.link.is_settled(setup.start + up), "头一次连上，没有理由不信它");

        // 断了一次：重连上来之后要保持够久才算数。
        setup.keep_tcp_alive(up, up + Duration::from_secs(20));
        let died = setup.stall_tcp(up + Duration::from_secs(20));
        assert!(!setup.link.is_settled(setup.start + died));
        let again = setup.reconnect_tcp(died);
        assert!(!setup.link.is_settled(setup.start + again + SETTLE - MS));
        assert!(setup.link.is_settled(setup.start + again + SETTLE));
    }

    #[test]
    fn a_handshake_that_fails_before_any_session_existed_does_not_count_as_a_broken_session() {
        let mut setup = Setup::new(&[], &["198.51.100.9:20330"]);
        // 头一次 TCP 连上了，但握手的最后一步一直没有回应；下一轮才成功。
        setup.poll(Duration::ZERO);
        setup.tcp_connected(10 * MS);
        setup.challenge_tcp(20 * MS);
        let mut at = 20 * MS;
        while setup.wire.tcp_connects < 2 {
            at += 100 * MS;
            assert!(at < Duration::from_secs(30), "握手失败之后应该重试");
            setup.poll(at);
        }
        let up = setup.reconnect_tcp(at);
        assert!(setup.link.is_settled(setup.start + up), "从来没有建立过会话，谈不上断过");
    }

    #[test]
    fn switching_from_tcp_back_to_udp_does_not_restart_the_settling_time() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let up = setup.fall_back_to_tcp();
        let died = setup.stall_tcp(up);
        let again = setup.reconnect_tcp(died);
        // 重连之后走着 TCP，直到下一次试 UDP 的握手发出去；这一次 UDP 通了。
        let at = again + UDP_RETRY + 100 * MS;
        setup.keep_tcp_alive(again, at);
        setup.complete_udp(at + 60 * MS);
        setup.poll(at + 100 * MS);
        assert!(matches!(setup.link.leg(), Some(Leg::Udp(_))));
        assert!(setup.link.is_settled(setup.start + at + 100 * MS), "从重连上 TCP 算起已经保持够久了，换到 UDP 不重新算");
    }

    #[test]
    fn a_forged_ok_is_ignored() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &[]);
        setup.poll(Duration::ZERO);
        let Frame::Init(init) = setup.last_udp() else { panic!("先发 INIT") };
        setup.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), 10 * MS);
        let Frame::Auth(auth) = setup.last_udp() else { panic!("再发 AUTH") };

        let forged = Accepted { nonce: auth.nonce, valid_for: 30, observed: addr("203.0.113.7:4000"), signature: [0; 64] };
        setup.udp_frame(Frame::Ok(forged), 20 * MS);
        assert_eq!(setup.link.leg(), None);
        // 签名对，但签的是另一个观察到的地址。
        let Frame::Ok(genuine) = setup.ok_for(&auth, "203.0.113.7:4000") else { unreachable!("ok_for 返回的就是 OK") };
        let tampered = Accepted { observed: addr("203.0.113.99:4000"), ..genuine };
        setup.udp_frame(Frame::Ok(tampered), 20 * MS);
        assert_eq!(setup.link.leg(), None);
        setup.udp_frame(Frame::Ok(genuine), 20 * MS);
        assert!(setup.link.leg().is_some());
    }

    #[test]
    fn the_auth_frame_carries_a_signature_the_relay_can_verify() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &[]);
        setup.poll(Duration::ZERO);
        let Frame::Init(init) = setup.last_udp() else { panic!("先发 INIT") };
        setup.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), 10 * MS);
        let Frame::Auth(auth) = setup.last_udp() else { panic!("再发 AUTH") };
        let message = Auth::signed_message(node(RELAY), node(ME), &auth.nonce, &auth.cookie);
        i3t_crypto::verify(&setup.node_key.public(), Auth::PURPOSE, &message, &Signature::from_bytes(auth.signature)).unwrap();
        assert_eq!((auth.node, auth.cookie), (node(ME), [3; 16]));
    }

    #[test]
    fn a_lost_session_is_rebuilt_at_once_and_an_unreachable_relay_is_retried_with_backoff() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &[]);
        setup.poll(Duration::ZERO);
        setup.complete_udp(10 * MS);
        assert!(setup.link.leg().is_some());

        // relay 说没有会话（它重启了）：马上重新握手。
        let sent_before = setup.wire.udp.len();
        setup.udp_frame(Frame::Error(Refused { reason: RefusalReason::NoSession, node: node(ME) }), Duration::from_secs(1));
        assert_eq!(setup.link.leg(), None);
        setup.poll(Duration::from_secs(1) + 400 * MS);
        assert!(matches!(setup.wire.udp[sent_before..], [(_, Frame::Init(_))]), "{:?}", &setup.wire.udp[sent_before..]);

        // 这回一直没有回答：尝试失败，过一阵再试，间隔越来越长。
        let mut attempts = Vec::new();
        let mut at = Duration::from_secs(1) + 400 * MS;
        let mut seen = setup.wire.udp.len();
        let mut last_nonce = None;
        while at < Duration::from_secs(40) {
            at += 100 * MS;
            setup.poll(at);
            for (_, frame) in &setup.wire.udp[seen..] {
                if let Frame::Init(init) = frame
                    && last_nonce.replace(init.nonce) != Some(init.nonce)
                {
                    attempts.push(at);
                }
            }
            seen = setup.wire.udp.len();
        }
        assert!((4..=7).contains(&attempts.len()), "40 秒里大约试 5 次：{attempts:?}");
        let gaps: Vec<Duration> = attempts.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(gaps.windows(2).all(|pair| pair[1] > pair[0]), "间隔应该越来越长：{gaps:?}");
        assert!(setup.link.is_failing(setup.start + at));
    }

    #[test]
    fn a_refusal_meant_for_another_node_or_about_a_peer_does_not_touch_the_session() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &[]);
        setup.poll(Duration::ZERO);
        setup.complete_udp(10 * MS);
        assert_eq!(setup.udp_frame(Frame::Error(Refused { reason: RefusalReason::NoSession, node: node(77) }), 20 * MS), None);
        assert!(setup.link.leg().is_some(), "说的是别的节点");
        let unattached = setup.udp_frame(Frame::Error(Refused { reason: RefusalReason::PeerUnattached, node: node(5) }), 20 * MS);
        assert_eq!(unattached, Some(Complaint::PeerUnattached(node(5))), "对端没挂在这个 relay 上：交给选路去换");
        let unroutable = setup.udp_frame(Frame::Error(Refused { reason: RefusalReason::RouteUnavailable, node: node(8) }), 20 * MS);
        assert_eq!(unroutable, Some(Complaint::RouteUnavailable(node(8))), "多级路径走不下去：也交给选路去换");
        assert!(setup.link.leg().is_some());
    }

    #[test]
    fn pong_refreshes_the_observed_address() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &[]);
        setup.poll(Duration::ZERO);
        setup.complete_udp(10 * MS);
        setup.poll(crate::timing::PROBE_INTERVAL + 10 * MS);
        let Frame::Ping(ping) = setup.last_udp() else { panic!("该发保活了：{:?}", setup.wire.udp) };
        assert_eq!(ping.node, node(ME));
        let moved = addr("203.0.113.7:4999");
        setup.udp_frame(Frame::Pong(Pong { nonce: ping.nonce, observed: moved }), crate::timing::PROBE_INTERVAL + 30 * MS);
        assert_eq!(setup.link.observed(setup.start + crate::timing::PROBE_INTERVAL + 30 * MS), Some(moved));
    }

    #[test]
    fn letting_go_of_a_relay_stops_everything() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        setup.poll(Duration::ZERO);
        setup.tcp_connected(10 * MS);
        setup.link.set_wanted(false, setup.start + 20 * MS, &mut setup.wire);
        assert_eq!(setup.wire.tcp_closes, 1);
        assert_eq!(setup.link.leg(), None);
        let sent = setup.wire.udp.len();
        setup.poll(Duration::from_secs(30));
        assert_eq!(setup.wire.udp.len(), sent, "不要了的 relay 不再握手也不再保活");
    }

    #[test]
    fn an_unattached_relay_is_surveyed_with_a_stateless_round_trip() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &[]);
        setup.link.set_wanted(false, setup.start, &mut setup.wire);
        let who_key = NodeKey::generate().unwrap();
        let who = Identity { me: node(ME), key: &who_key };
        setup.link.poll(setup.start, who, false, true, &mut setup.wire);
        let Frame::Init(init) = setup.last_udp() else { panic!("测延迟用的是 INIT") };
        assert_eq!(setup.link.estimate(setup.start), None);

        setup.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [0; 16] }), 35 * MS);
        assert_eq!(setup.link.estimate(setup.start + 35 * MS), Some(35 * MS));
        assert_eq!(setup.wire.udp.len(), 1, "只量一个往返，不接着握手");
        assert_eq!(setup.link.leg(), None);
        assert_eq!(setup.link.estimate(setup.start + ESTIMATE_TTL + 36 * MS), None, "过期作废");

        // 一分钟之后再量。
        setup.link.poll(setup.start + 35 * MS + RELAY_SURVEY - MS, who, false, true, &mut setup.wire);
        assert_eq!(setup.wire.udp.len(), 1);
        setup.link.poll(setup.start + 35 * MS + RELAY_SURVEY, who, false, true, &mut setup.wire);
        assert_eq!(setup.wire.udp.len(), 2);

        // 没有回答的话十秒之后就再问。
        setup.link.poll(setup.start + 35 * MS + RELAY_SURVEY + RELAY_SURVEY / 6, who, false, true, &mut setup.wire);
        assert_eq!(setup.wire.udp.len(), 3);
    }

    fn standing(id: u32, rtt_ms: Option<u64>) -> Standing {
        Standing { id: node(id), rtt: rtt_ms.map(Duration::from_millis), failing: false }
    }

    #[test]
    fn pinned_relays_are_used_as_given() {
        let relays = [standing(1, Some(5)), standing(2, Some(50)), standing(3, Some(500))];
        assert_eq!(
            choose_attached(&[node(1)], &relays, &[node(3), node(2), node(9)]),
            [node(3), node(2)],
            "固定的照用，不在 netmap 里的去掉"
        );
    }

    #[test]
    fn the_two_lowest_latency_relays_are_attached() {
        let relays = [standing(1, Some(80)), standing(2, Some(20)), standing(3, Some(40)), standing(4, None)];
        assert_eq!(choose_attached(&[], &relays, &[]), [node(2), node(3)]);
        assert_eq!(choose_attached(&[], &relays[..1], &[]), [node(1)], "只有一个就挂一个");
        assert_eq!(choose_attached(&[], &[], &[]), []);
    }

    #[test]
    fn before_anything_is_measured_the_first_relays_in_the_netmap_are_used() {
        let relays = [standing(7, None), standing(3, None), standing(5, None)];
        assert_eq!(choose_attached(&[], &relays, &[]), [node(7), node(3)]);
    }

    #[test]
    fn a_clearly_better_relay_replaces_the_worst_attached_one_but_a_marginal_one_does_not() {
        let relays = [standing(1, Some(100)), standing(2, Some(30)), standing(3, Some(85)), standing(4, Some(20))];
        // 4 比 1 好得多：换掉 1。
        assert_eq!(choose_attached(&[node(1), node(2)], &relays, &[]), [node(4), node(2)]);
        // 3 只比 1 好 15%：不换。
        assert_eq!(choose_attached(&[node(1), node(2)], &relays[..3], &[]), [node(1), node(2)]);
        // 延迟很低时用 10ms 的门槛：12ms 和 8ms 之间不换。
        let close = [standing(1, Some(12)), standing(2, Some(5)), standing(3, Some(8))];
        assert_eq!(choose_attached(&[node(1), node(2)], &close, &[]), [node(1), node(2)]);
    }

    #[test]
    fn an_attached_relay_that_cannot_be_reached_gives_way_to_one_that_can() {
        let relays = [Standing { failing: true, ..standing(1, None) }, standing(2, Some(30)), standing(3, Some(300))];
        assert_eq!(choose_attached(&[node(1), node(2)], &relays, &[]), [node(3), node(2)]);
        // 外面那个没有量出延迟（多半也连不上）：不换。
        let unknown = [Standing { failing: true, ..standing(1, None) }, standing(2, Some(30)), standing(3, None)];
        assert_eq!(choose_attached(&[node(1), node(2)], &unknown, &[]), [node(1), node(2)]);
    }

    #[test]
    fn relays_removed_from_the_netmap_are_dropped_and_replaced() {
        let relays = [standing(2, Some(30)), standing(3, Some(300))];
        assert_eq!(choose_attached(&[node(1), node(2)], &relays, &[]), [node(2), node(3)]);
    }

    #[test]
    fn retry_delays_double_up_to_the_cap() {
        let delay = |failures| retry_delay(failures).as_secs_f64();
        assert!((1.0..=1.2).contains(&delay(1)));
        assert!((2.0..=2.4).contains(&delay(2)));
        assert!((16.0..=19.2).contains(&delay(5)));
        assert!((30.0..=36.0).contains(&delay(6)));
        assert!((30.0..=36.0).contains(&delay(60)));
        assert!(jitter(Duration::from_millis(300)) <= Duration::from_millis(300));
    }
}
