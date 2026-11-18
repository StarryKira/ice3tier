//! 节点一侧到各个 relay 的联系：UDP 和 TCP 同时试、优先 UDP、掉线重连，以及挂靠哪几个 relay 的选择。
//!
//! 一个 [`RelayLink`] 管到一个 relay 的全部事情。它不直接碰 socket：要发的东西交给 [`Wire`]，
//! 所以整套"先试谁、什么时候换"的规则可以脱离网络来测。
//!
//! 一个 relay 可以有好几个地址（controller 找出来的，白皮书 §7.2），不一定每个都连得上。UDP 的握手把 INIT 同时发给每个地址，
//! 谁先回答 CHALLENGE 就用谁，之后的握手和保活都只走它；会话断了，下一次又是每个地址一起试。所以连不上的地址不多花时间。
//! 回答了 CHALLENGE、握手却走不完的地址（地址和别的 relay 撞了，回答的是另一台机器）这一轮里不再试，剩下的地址马上接着试。
//! TCP 一个接一个地错开一点时间去连（见 [`crate::tcp`]），上一次握手成功的地址排在最前，失败的排在最后。
//!
//! # 骨干
//!
//! 本节点自己是 relay 时，到别的 relay 的联系也是骨干上的一段：两边互相都要有会话，并且在同一种传输上（[`crate::backbone`]）。
//! UDP 通的时候和节点一样走 UDP（这时不像节点那样同时去连 TCP）。下面几种情况下这一段改走一条 TCP，两个会话都在上面：
//!
//! - 本节点到它的 UDP 握不上手；
//! - 本节点到它的 UDP 会话通着，它却 [`ONE_SIDED_GRACE`] 都没有在本节点的 relay 上建会话：它连不上本节点；
//! - 它拨进来了一条（本节点的 relay 接住，告诉路径层），本节点又没有通着的 UDP 会话，或者它的会话已经搬到了这条 TCP 上。
//!
//! 谁拨号：编号小的一方先拨；它连不上的话（对方连不进去），另一方等 [`DIAL_DEFER`] 没有等到它拨进来就自己拨。
//! 第二种情况下本节点知道对方连不上自己，马上拨。两边都拨通了的话，编号小的一方留着自己拨的那条，另一方改用它，放下自己的。
//!
//! 改走 TCP 之后，这条 TCP 断了也照样用 TCP 重连（像节点的 TCP 退路一样），不再另外试 UDP：一边换回 UDP、另一边还在 TCP 上的那一阵，
//! 两个会话不在同一处，骨干上转不过去。只有对方经 UDP 在本节点上建了会话（它那边在 UDP 上、而且连得上本节点）时才回到 UDP 再试。

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use i3t_crypto::{NodeKey, Signature};
use i3t_model::control::TryOutcome;
use i3t_model::netmap::Relay;
use i3t_proto::session::{Accepted, Auth, Echo, Frame, Init, Nonce, Ping, RefusalReason, Refused};
use i3t_proto::{NodeId, PublicKey};

use crate::SessionLeg;
use crate::attach::{Event, Send, Session, Step};
use crate::estimate::{Estimate, Quality};
use crate::timing::{
    DIAL_DEFER, ESTIMATE_TTL, OFFER_RETRY, ONE_SIDED_GRACE, PROBE_INTERVAL, RELAY_RETRY_MAX, RELAY_SURVEY, Rtt, SETTLE, UDP_HEAD_START,
    UDP_RETRY,
};

/// 挂靠几个 relay（白皮书 §7.2 的 K）。
pub(crate) const ATTACH_COUNT: usize = 2;
/// 挂靠的 relay 连不上超过这么久，就可以被别的顶替。
const FAILING_AFTER: Duration = Duration::from_secs(10);
/// TCP 多久连不上就算这次失败。
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 不保持会话的 relay 每分钟问一次延迟（INIT / CHALLENGE），这么久没有回答就算这一次没问到。
const SURVEY_SILENCE: Duration = Duration::from_secs(3);

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
    /// 改用这个 relay 拨进来、本节点的 relay 接住的那条 TCP（骨干，见模块文档）。之后往里写、从里收和自己拨的一样。
    /// 返回它的对端地址和 relay 给它的编号；没有这样一条时是 `None`。
    fn tcp_adopt(&mut self) -> Option<(SocketAddr, u64)>;
    /// 关掉到这个 relay 的 TCP。借来的那条只是不再用，它归本节点的 relay。
    fn tcp_close(&mut self);
}

/// 本节点是 relay 时，选路一方告诉 [`RelayLink`] 的骨干上的情况（见模块文档）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Backbone {
    /// 它在本节点的 relay 上的会话绑定在哪里。
    pub(crate) theirs: Option<SessionLeg>,
    /// 它拨进来、等着本节点用的那条 TCP 在 relay 那边的编号。
    pub(crate) offered: Option<u64>,
}

/// 骨干上的这一段走一条 TCP（见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TcpLink {
    since: Instant,
    /// 本节点知道它连不上本节点：不等它拨进来，本节点拨。
    must_dial: bool,
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
    /// 连上的是哪个地址。
    addr: Option<SocketAddr>,
    /// 这条 TCP 上的会话建起来过。
    established: bool,
    /// 这是别的 relay 拨进来、借来用的那一条，值是 relay 给它的编号（骨干）。
    adopted: Option<u64>,
}

/// 到一个 relay 的联系。
#[derive(Debug)]
pub(crate) struct RelayLink {
    id: NodeId,
    key: PublicKey,
    udp_addrs: Vec<SocketAddr>,
    tcp_addrs: Vec<SocketAddr>,
    /// 本节点自己的地址。relay 的地址里有它们的话不去试：发出去的包会回到本节点自己这里。
    own: Vec<SocketAddr>,
    /// 要不要和它保持会话：它是挂靠的 relay，或者有活跃的对端要经它到达。
    wanted: bool,
    wanted_since: Option<Instant>,
    udp: Session,
    /// UDP 的握手和会话用的地址：回答了这一次握手的 CHALLENGE 的那个。还没有谁回答时是 `None`，INIT 发给每个地址。
    udp_target: Option<SocketAddr>,
    /// UDP 的会话建起来过（之后再断就是"会话没了"，不是"握手失败"）。
    udp_established: bool,
    /// 这一轮里回答了 CHALLENGE、握手却没有走完的地址。这一轮剩下的时间里不再试。
    udp_shunned: Vec<SocketAddr>,
    /// `udp_target` 变过，收包时按地址认 relay 的表要重建（见 [`take_target_changed`](Self::take_target_changed)）。
    target_changed: bool,
    tcp: Option<TcpLeg>,
    /// 最近一次 TCP 上握手成功的地址，下一次先连它；握手失败的地址下一次排最后。
    tcp_good: Option<SocketAddr>,
    tcp_bad: Option<SocketAddr>,
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
    /// 最近一次试的结果和时刻（见 [`last_try`](Self::last_try)）。
    tried: Option<(TryOutcome, Instant)>,
    /// 它最近看到的本节点的 IP：UDP 和 TCP 上的会话应答里都有。TCP 上的那个不能当直连的候选端点，但出口地址就是它。
    seen: Option<(IpAddr, Instant)>,
    /// 本节点是 relay 时最近一次 [`poll`](Self::poll) 交进来的骨干上的情况；不是时是 `None`。
    backbone: Option<Backbone>,
    /// 骨干上的这一段改走了 TCP。
    tcp_link: Option<TcpLink>,
    /// 到它的 UDP 会话通着、它在本节点的 relay 上却没有 UDP 的会话：从这个时刻起。
    one_sided_since: Option<Instant>,
    /// 借来用、却没有用成的那条（握手没走完、会话断了），和那个时刻：[`OFFER_RETRY`] 之内不再借它。
    spoiled_offer: Option<(u64, Instant)>,
}

impl RelayLink {
    pub(crate) fn new(relay: &Relay, now: Instant) -> Self {
        Self {
            id: relay.id,
            key: relay.key,
            udp_addrs: relay.udp.clone(),
            tcp_addrs: relay.tcp.clone(),
            own: Vec::new(),
            wanted: false,
            wanted_since: None,
            udp: Session::new(),
            udp_target: None,
            udp_established: false,
            udp_shunned: Vec::new(),
            target_changed: false,
            tcp: None,
            tcp_good: None,
            tcp_bad: None,
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
            tried: None,
            seen: None,
            backbone: None,
            tcp_link: None,
            one_sided_since: None,
            spoiled_offer: None,
        }
    }

    /// UDP 上的会话多久保活一次。本节点是 relay 时，到别的 relay 的会话就是骨干上的连接，探测得更勤（白皮书 §7.3）。
    ///
    /// TCP 上的会话不跟着变，骨干改走 TCP 时也一样：TCP 不丢帧，探测得再勤也只多测出几个延迟样本，
    /// 反倒让一条丢包的 TCP 更容易被判为断开（一个探测卡在重传里，后面的全跟着超时）。所以走 TCP 的骨干每 5 秒测一次。
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

    /// netmap 更新了这个 relay 的信息。正在用的地址不在新的列表里也接着用：它还通着；会话断了之后才只试新的。
    pub(crate) fn update(&mut self, relay: &Relay) {
        self.key = relay.key;
        if self.udp_addrs != relay.udp || self.tcp_addrs != relay.tcp {
            self.udp_addrs.clone_from(&relay.udp);
            self.tcp_addrs.clone_from(&relay.tcp);
            self.udp_shunned.retain(|addr| relay.udp.contains(addr));
        }
    }

    /// 本节点自己的地址变了。
    pub(crate) fn set_own(&mut self, own: &[SocketAddr]) {
        own.clone_into(&mut self.own);
    }

    /// 要按来源地址认出这个 relay 的那些地址：netmap 里的，加上正在用的那个（它可能已经不在 netmap 里了）。
    pub(crate) fn dispatch_addrs(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.udp_addrs.iter().copied().chain(self.udp_target.filter(|target| !self.udp_addrs.contains(target)))
    }

    /// UDP 的握手和会话现在用的地址；还没有哪个地址回答时是 `None`。
    pub(crate) const fn udp_target(&self) -> Option<SocketAddr> {
        self.udp_target
    }

    /// `udp_target` 自上次问过之后变过没有。
    pub(crate) fn take_target_changed(&mut self) -> bool {
        std::mem::take(&mut self.target_changed)
    }

    fn set_target(&mut self, target: Option<SocketAddr>) {
        if self.udp_target != target {
            self.udp_target = target;
            self.target_changed = true;
        }
    }

    /// 这一轮还可以试的 UDP 地址。
    fn udp_candidates(&self) -> Vec<SocketAddr> {
        self.udp_addrs.iter().copied().filter(|addr| !self.own.contains(addr) && !self.udp_shunned.contains(addr)).collect()
    }

    fn has_udp(&self) -> bool {
        self.udp_addrs.iter().any(|addr| !self.own.contains(addr) && !self.udp_shunned.contains(addr))
    }

    /// 去连 TCP 的顺序：上一次握手成功的在最前，失败的在最后，别的照 netmap 里的顺序。
    fn tcp_candidates(&self) -> Vec<SocketAddr> {
        let mut addrs: Vec<SocketAddr> = self.tcp_addrs.iter().copied().filter(|addr| !self.own.contains(addr)).collect();
        addrs.sort_by_key(|addr| match Some(*addr) {
            good if good == self.tcp_good => 0,
            bad if bad == self.tcp_bad => 2,
            _ => 1,
        });
        addrs
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
            self.udp_established = false;
            self.set_target(None);
            if self.retire_tcp().is_some() {
                wire.tcp_close();
            }
            self.attempt = None;
            self.steadiness = Steadiness::Unbroken;
            self.tcp_link = None;
            self.one_sided_since = None;
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
            return self.udp_target.map(Leg::Udp);
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

    /// 最近一次试它的结果和时刻。现在有会话的就是现在、会话走的传输；不然是最近一次会话的尝试（UDP 和 TCP 一起试的）
    /// 或者每分钟一次的 UDP 询问得到了回答。只有 UDP 询问没有回答、还没用会话试过的，没有结果（见 [`survey_silent`](Self::survey_silent)）。
    pub(crate) fn last_try(&self, now: Instant) -> Option<(TryOutcome, Instant)> {
        match self.leg() {
            Some(Leg::Udp(_)) => Some((TryOutcome::Udp, now)),
            Some(Leg::Tcp) => Some((TryOutcome::Tcp, now)),
            None => self.tried,
        }
    }

    /// 它最近看到的本节点的 IP。有会话时随保活的应答刷新；会话没了之后照旧是最后看到的那个，过了估计的有效期就不算。
    pub(crate) fn seen_as(&self, now: Instant) -> Option<IpAddr> {
        self.seen.filter(|(_, at)| now.saturating_duration_since(*at) < ESTIMATE_TTL).map(|(ip, _)| ip)
    }

    /// 不保持会话时每分钟一次的 UDP 询问这一次没有回答：UDP 到不了它，TCP 还没试过。
    pub(crate) fn survey_silent(&self, now: Instant) -> bool {
        !self.wanted && self.survey.is_some_and(|(_, sent)| now.saturating_duration_since(sent) >= SURVEY_SILENCE)
    }

    /// 推进定时器。`urgent`：有包正经它发出去，却有一阵没有从它收到任何东西了。
    /// `survey`：不保持会话时要不要定期测一下延迟。`backbone`：本节点是 relay 时骨干上的情况（见模块文档），不是时是 `None`。
    pub(crate) fn poll(
        &mut self,
        now: Instant,
        who: Identity<'_>,
        urgent: bool,
        survey: bool,
        backbone: Option<Backbone>,
        wire: &mut dyn Wire,
    ) {
        if !self.wanted {
            if survey {
                self.survey(now, who, wire);
            }
            return;
        }
        if backbone.is_none() && self.tcp_link.take().is_some() {
            // 本节点不再是 relay：这一段不再是骨干，照节点的规矩来（UDP 先试，TCP 是退路）。
            self.one_sided_since = None;
        }
        self.backbone = backbone;
        if let Some(backbone) = backbone {
            self.review_backbone(now, who, backbone, wire);
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
            // 走着 TCP：定期再试 UDP，通了就换回去。骨干改走了 TCP 时不试（见模块文档）。
            if self.tcp_link.is_none() && self.udp.is_idle() && now >= self.udp_retry_at && self.has_udp() {
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

    /// 骨干上的这一段该不该、该怎样改走 TCP（见模块文档）。本节点是 relay 时每一轮都看。
    fn review_backbone(&mut self, now: Instant, who: Identity<'_>, backbone: Backbone, wire: &mut dyn Wire) {
        let offered =
            backbone.offered.filter(|offer| self.spoiled_offer.is_none_or(|(spoiled, at)| spoiled != *offer || now >= at + OFFER_RETRY));
        let using = self.tcp.as_ref().and_then(|tcp| tcp.adopted);
        // 本节点到它的 UDP 会话通着也用：它的会话已经搬到这条 TCP 上了（它那边 UDP 不通），这一段只能都在 TCP 上。
        let moved = |offer: u64| backbone.theirs == Some(SessionLeg::Accepted(offer));
        if let Some(offer) = offered.filter(|offer| using != Some(*offer) && (!self.udp.is_up() || moved(*offer))) {
            // 自己拨的那条已经通了、按编号又该本节点拨的话留着它：对方看到本节点的会话从它接的那条上来，会放下它自己拨的。
            let own_wins = who.me < self.id && self.tcp.as_ref().is_some_and(|tcp| tcp.adopted.is_none() && tcp.session.is_up());
            if !own_wins {
                self.adopt(offer, now, who, wire);
                return;
            }
        }
        if self.tcp_link.is_none() && !self.udp.is_up() && self.tcp.as_ref().is_some_and(|tcp| tcp.session.is_up()) {
            // 本节点成为 relay 之前就照节点的规矩退到了 TCP：这一段已经在 TCP 上，照骨干的规矩接着用，不再定期试 UDP。
            self.tcp_link = Some(TcpLink { since: now, must_dial: false });
        }
        if self.tcp_link.is_some() {
            if backbone.theirs == Some(SessionLeg::Udp) {
                // 它经 UDP 连得上本节点、它那边也还在 UDP 上：回到 UDP 再试。不通的话这一轮失败之后又会改走 TCP。
                tracing::info!(relay = %self.id, "这个 relay 经 UDP 在本节点上建了会话：骨干上的这一段回到 UDP 再试");
                self.tcp_link = None;
                if self.retire_tcp().is_some() {
                    wire.tcp_close();
                }
                self.attempt = None;
                self.retry_at = now;
            }
            return;
        }
        match (self.udp.is_up(), backbone.theirs) {
            (true, Some(SessionLeg::Udp)) | (false, _) => self.one_sided_since = None,
            (true, Some(SessionLeg::Accepted(_) | SessionLeg::Dialled(_)) | None) => {
                let since = *self.one_sided_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= ONE_SIDED_GRACE {
                    tracing::info!(relay = %self.id, "到这个 relay 的 UDP 会话通着，它却一直没有经 UDP 在本节点上建会话：它连不上本节点，骨干上的这一段改走本节点拨出去的 TCP");
                    self.enter_tcp_link(now, true);
                }
            }
        }
    }

    /// 骨干上的这一段改走 TCP：放下 UDP 的会话，马上按 TCP 的规矩重来。
    fn enter_tcp_link(&mut self, now: Instant, must_dial: bool) {
        self.tcp_link = Some(TcpLink { since: now, must_dial });
        self.one_sided_since = None;
        self.udp.stop();
        self.udp_established = false;
        self.set_target(None);
        self.attempt = None;
        self.retry_at = now;
    }

    /// 改用对方拨进来的第 `offer` 条 TCP，在上面建本节点的会话。自己拨的那条（有的话）放下。
    fn adopt(&mut self, offer: u64, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        if self.tcp_link.is_none() {
            self.enter_tcp_link(now, false);
        }
        if self.retire_tcp().is_some() {
            wire.tcp_close();
        }
        let Some((addr, accepted)) = wire.tcp_adopt() else { return };
        tracing::info!(relay = %self.id, %addr, conn = accepted, "这个 relay 拨进来了一条 TCP：骨干上的这一段两个会话都走它");
        let mut session = Session::over_tcp(self.tcp_measured.clone());
        session.set_training(self.training);
        let send = session.start(now, false);
        self.tcp = Some(TcpLeg { session, connected: true, started: now, addr: Some(addr), established: false, adopted: Some(offer) });
        self.attempt = Some(now);
        self.send_tcp(send, who, wire);
    }

    /// 骨干走 TCP 时本节点现在能不能自己拨：编号小的一方先拨，另一方等一会儿（见模块文档）。
    fn may_dial(&self, link: TcpLink, now: Instant, who: Identity<'_>) -> bool {
        link.must_dial || who.me < self.id || now.saturating_duration_since(link.since) >= DIAL_DEFER
    }

    fn begin_attempt(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        if self.backbone.is_some() {
            return self.begin_backbone_attempt(now, who, wire);
        }
        self.attempt = Some(now);
        let tcp_addrs = self.tcp_candidates();
        let tcp_only = self.fell_back && !tcp_addrs.is_empty();
        if tcp_only {
            // 定期试 UDP 的握手要是正好进行到一半，放下它：relay 上一个节点只有一个会话，两边一起握手会互相顶掉。
            self.udp.stop();
            self.set_target(None);
        } else if self.has_udp() {
            self.start_udp(now, who, wire);
        }
        if !tcp_addrs.is_empty() {
            let mut session = Session::over_tcp(self.tcp_measured.clone());
            session.set_training(self.training);
            self.tcp = Some(TcpLeg { session, connected: false, started: now, addr: None, established: false, adopted: None });
            wire.tcp_connect(&tcp_addrs);
        }
        if !self.has_udp() && tcp_addrs.is_empty() {
            tracing::warn!(relay = %self.id, "netmap 里这个 relay 没有任何地址，连不了");
            self.attempt_failed(now);
        }
    }

    /// 骨干上的一次尝试：还没改走 TCP 时只试 UDP；改走了 TCP 时用对方拨进来的那条，没有的话轮到本节点拨就拨。
    fn begin_backbone_attempt(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        let Some(link) = self.tcp_link else {
            if self.has_udp() {
                self.attempt = Some(now);
                self.start_udp(now, who, wire);
            } else {
                // netmap 里没有能试的 UDP 地址：直接走 TCP。
                self.enter_tcp_link(now, false);
            }
            return;
        };
        let tcp_addrs = self.tcp_candidates();
        if !self.may_dial(link, now, who) {
            // 等对方拨进来（它拨进来时 `review_backbone` 马上就用）；等不到的话到时候自己拨。
            self.retry_at = link.since + DIAL_DEFER;
            return;
        }
        if tcp_addrs.is_empty() {
            tracing::warn!(relay = %self.id, "这个 relay 的 UDP 不通，netmap 里又没有它的 TCP 地址：只能等它拨过来");
            self.attempt = Some(now);
            self.attempt_failed(now);
            return;
        }
        tracing::info!(relay = %self.id, "骨干上到这个 relay 的这一段走 TCP：本节点拨号");
        self.attempt = Some(now);
        let mut session = Session::over_tcp(self.tcp_measured.clone());
        session.set_training(self.training);
        self.tcp = Some(TcpLeg { session, connected: false, started: now, addr: None, established: false, adopted: None });
        wire.tcp_connect(&tcp_addrs);
    }

    fn attempt_failed(&mut self, now: Instant) {
        self.tried = Some((TryOutcome::Unreached, now));
        self.attempt = None;
        self.fell_back = false;
        self.failures = self.failures.saturating_add(1);
        self.udp_shunned.clear();
        if self.backbone.is_some() && self.tcp_link.is_none() {
            // 骨干上 UDP 握不上手：这一段改走 TCP，不等退避。
            tracing::info!(relay = %self.id, "到这个 relay 的 UDP 握不上手，骨干上的这一段改走 TCP");
            self.enter_tcp_link(now, false);
            return;
        }
        let wait = retry_delay(self.failures);
        self.retry_at = now + wait;
        tracing::info!(relay = %self.id, failures = self.failures, "连不上 relay，{} 秒后再试", wait.as_secs());
    }

    /// 在用的会话没了：马上重连。同一个 relay 掉线时挂靠它的节点会同时重连，所以带一点随机的延迟错开。
    fn reconnect_soon(&mut self, now: Instant) {
        self.session_lost();
        self.attempt = None;
        self.failures = 0;
        self.udp_shunned.clear();
        self.retry_at = now + jitter(Duration::from_millis(300));
    }

    /// 开始一次 UDP 握手：INIT 发给每个还可以试的地址。
    fn start_udp(&mut self, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        self.set_target(None);
        self.udp_established = false;
        let send = self.udp.start(now, false);
        self.send_udp(send, who, wire);
    }

    /// 握手还没有地址回答时 INIT（和它的重发）发给每个地址；之后只发给回答了的那个。
    fn send_udp(&mut self, send: Send, who: Identity<'_>, wire: &mut dyn Wire) {
        let frame = self.encode(send, who);
        match (send, self.udp_target) {
            (_, Some(to)) => wire.udp(to, &frame),
            (Send::Init { .. }, None) => {
                for to in self.udp_candidates() {
                    wire.udp(to, &frame);
                }
            }
            // AUTH 和 PING 只在有地址回答过之后才会发。
            (Send::Auth { .. } | Send::Ping { .. }, None) => {}
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
                self.seen = Some((observed.ip().to_canonical(), now));
                self.tried = Some((TryOutcome::Udp, now));
                self.attempt = None;
                self.failures = 0;
                self.fell_back = false;
                self.udp_established = true;
                self.udp_shunned.clear();
                self.session_up(now);
                let via = self.udp_target.map(|addr| addr.to_string()).unwrap_or_default();
                tracing::info!(relay = %self.id, %observed, %via, "到 relay 的中转会话建立（UDP）");
            }
            Some(Event::Down { reason }) => {
                let target = self.udp_target;
                let established = std::mem::take(&mut self.udp_established);
                self.set_target(None);
                if !established
                    && self.wanted
                    && let Some(addr) = target
                {
                    // 回答了 CHALLENGE，握手却没有走完：这一轮不再试它，剩下的地址马上接着试，不等这一轮结束。
                    self.udp_shunned.push(addr);
                    if self.has_udp() {
                        tracing::info!(relay = %self.id, %addr, reason, "relay 的这个地址握手没有走完，换别的地址试");
                        self.start_udp(now, who, wire);
                        return;
                    }
                }
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
            Some(Event::Up { observed }) => {
                self.seen = Some((observed.ip().to_canonical(), now));
                if !self.udp.is_up() {
                    self.tried = Some((TryOutcome::Tcp, now));
                }
                self.attempt = None;
                self.failures = 0;
                // 这一轮没有同时试 UDP 的话（见 `fell_back`），连上之后马上在后台试一次：UDP 要是恢复了，不用等半分钟。
                self.udp_retry_at = if self.fell_back { now } else { now + UDP_RETRY };
                self.fell_back = true;
                self.session_up(now);
                let via = self.tcp.as_mut().and_then(|tcp| {
                    tcp.established = true;
                    tcp.addr
                });
                self.tcp_good = via;
                if self.tcp_bad == via {
                    self.tcp_bad = None;
                }
                let via = via.map(|addr| addr.to_string()).unwrap_or_default();
                tracing::info!(relay = %self.id, %via, "到 relay 的中转会话建立（TCP）");
            }
            Some(Event::Down { reason }) => {
                tracing::info!(relay = %self.id, reason, "到 relay 的 TCP 中转不可用");
                match self.tcp {
                    // 借来的那条没用成：一阵之内不再借它，本节点自己拨（对方也许是不会在上面回答的旧版本）。
                    Some(TcpLeg { adopted: Some(offer), .. }) => self.spoiled_offer = Some((offer, now)),
                    // 连上了、握手却没有走完的地址，下一次排在最后：它多半是另一台机器。
                    Some(TcpLeg { addr: Some(addr), established: false, .. }) => self.tcp_bad = Some(addr),
                    Some(_) | None => {}
                }
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

    /// relay 的地址 `from` 发来的会话帧。错误帧走 [`on_udp_refusal`](Self::on_udp_refusal)。
    ///
    /// 同一个地址可能是好几个 relay 的（不同局域网里的内网地址会撞），所以收包的一方会把帧交给每一个：
    /// 这里只认在回答本节点发出去的随机数的帧，OK 还要验过签名。
    pub(crate) fn on_udp_frame(&mut self, from: SocketAddr, frame: &Frame, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        match frame {
            Frame::Challenge(challenge) => {
                if let Some((_, sent)) = self.survey.take_if(|(nonce, _)| *nonce == challenge.nonce) {
                    self.surveyed = Some((now.saturating_duration_since(sent), now));
                    self.tried = Some((TryOutcome::Udp, now));
                    self.next_survey = now + RELAY_SURVEY;
                } else if self.udp_target.is_none_or(|target| target == from) && self.udp_candidates().contains(&from) {
                    let step = self.udp.on_challenge(challenge, now);
                    if matches!(step.send, Some(Send::Auth { .. })) {
                        self.set_target(Some(from));
                    }
                    self.apply_udp(step, now, who, wire);
                }
            }
            Frame::Ok(ok) if self.udp_target == Some(from) && self.udp.expects_ok(ok) => {
                if self.ok_is_genuine(ok, who) {
                    let step = self.udp.on_ok(ok, now);
                    self.apply_udp(step, now, who, wire);
                }
            }
            Frame::Pong(_) if self.udp_target != Some(from) => {}
            Frame::Pong(pong) => {
                if let Some(observed) = self.udp.on_pong(pong, now) {
                    self.observed = Some((observed, now));
                    self.seen = Some((observed.ip().to_canonical(), now));
                }
            }
            // 错误帧要和它回显的字节一起看，见 [`on_udp_refusal`](Self::on_udp_refusal)。
            // 别的会话帧是节点发给 relay 的，不该从 relay 来。
            Frame::Error(_) | Frame::Ok(_) | Frame::Init(_) | Frame::Auth(_) | Frame::Ping(_) => {}
        }
    }

    /// relay 的 UDP 地址发来的错误帧，回显的是 `echo`。返回 relay 对某条路的抱怨（只有回显对得上会话时才算数）。
    pub(crate) fn on_udp_refusal(
        &mut self,
        refused: &Refused,
        echo: &Echo,
        now: Instant,
        who: Identity<'_>,
        wire: &mut dyn Wire,
    ) -> Option<Complaint> {
        let (step, complaint) = Self::refusal(&mut self.udp, self.id, refused, echo, now, who);
        self.apply_udp(step, now, who, wire);
        complaint
    }

    /// 一个会话收到的错误帧：说本节点没有会话的交给会话自己核对；说路走不通的，回显是这个会话握手的随机数才算数。
    fn refusal(
        session: &mut Session,
        relay: NodeId,
        refused: &Refused,
        echo: &Echo,
        now: Instant,
        who: Identity<'_>,
    ) -> (Step, Option<Complaint>) {
        let complaint = match refused.reason {
            RefusalReason::NoSession if refused.node == who.me => return (session.on_no_session(echo, now), None),
            RefusalReason::NoSession => return (Step::default(), None),
            RefusalReason::PeerUnattached => Complaint::PeerUnattached(refused.node),
            RefusalReason::RouteUnavailable => Complaint::RouteUnavailable(refused.node),
        };
        if !session.is_session_echo(echo) {
            tracing::debug!(%relay, reason = ?refused.reason, "relay 的错误帧回显的不是本节点的会话，不理");
            return (Step::default(), None);
        }
        (Step::default(), Some(complaint))
    }

    /// 到这个 relay 的 TCP 连上了（`addr` 是连上的那个地址）：在里面开始握手，先停在 cookie 那一步。
    pub(crate) fn on_tcp_connected(&mut self, addr: SocketAddr, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        let Some(tcp) = &mut self.tcp else { return };
        tcp.connected = true;
        tcp.addr = Some(addr);
        // 骨干改走了 TCP 的话 UDP 不会再来，不用停在 cookie 那一步等它。
        let send = tcp.session.start(now, self.tcp_link.is_none());
        self.send_tcp(send, who, wire);
    }

    /// 到这个 relay 的 TCP 里收到的会话帧。错误帧走 [`on_tcp_refusal`](Self::on_tcp_refusal)。
    pub(crate) fn on_tcp_frame(&mut self, frame: &Frame, now: Instant, who: Identity<'_>, wire: &mut dyn Wire) {
        let genuine = matches!(frame, Frame::Ok(ok) if self.ok_is_genuine(ok, who));
        let Some(tcp) = self.tcp.as_mut() else { return };
        let step = match frame {
            Frame::Challenge(challenge) => tcp.session.on_challenge(challenge, now),
            Frame::Ok(ok) if genuine => tcp.session.on_ok(ok, now),
            Frame::Pong(pong) => {
                // TCP 上看到的地址是那条 TCP 的，不是 UDP 的候选端点，不记成 `observed`；出口地址照样是它。
                if let Some(observed) = tcp.session.on_pong(pong, now) {
                    self.seen = Some((observed.ip().to_canonical(), now));
                }
                Step::default()
            }
            Frame::Error(_) | Frame::Ok(_) | Frame::Init(_) | Frame::Auth(_) | Frame::Ping(_) => Step::default(),
        };
        self.apply_tcp(step, now, who, wire);
    }

    /// 到这个 relay 的 TCP 里收到的错误帧。规则和 UDP 上的一样（[`on_udp_refusal`](Self::on_udp_refusal)）：
    /// TCP 上的帧路径外的人伪造不了，但一样的规则让两边的行为一致，代价只是"没有会话"时多一个往返。
    pub(crate) fn on_tcp_refusal(
        &mut self,
        refused: &Refused,
        echo: &Echo,
        now: Instant,
        who: Identity<'_>,
        wire: &mut dyn Wire,
    ) -> Option<Complaint> {
        let tcp = self.tcp.as_mut()?;
        let (step, complaint) = Self::refusal(&mut tcp.session, self.id, refused, echo, now, who);
        self.apply_tcp(step, now, who, wire);
        complaint
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
        let candidates = self.udp_candidates();
        if candidates.is_empty() {
            return;
        }
        // 没有回答的话过一会儿就再问，早点知道它能不能用；有回答的话下一次在一个完整的间隔之后。
        self.next_survey = now + RELAY_SURVEY / 6;
        let nonce: Nonce = i3t_underlay::random_bytes();
        self.survey = Some((nonce, now));
        // 每个地址都问，先回答的那个就是量到的延迟。
        let init = Init { node: who.me, nonce }.to_bytes();
        for to in candidates {
            wire.udp(to, &init);
        }
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
    use crate::timing::{DIAL_DEFER, ONE_SIDED_GRACE};

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
        /// 每次去连 TCP 时给的地址，按顺序。
        tcp_targets: Vec<Vec<SocketAddr>>,
        tcp_closes: usize,
        /// 对方拨进来、可以借来用的那条：对端地址和编号。借走之后清掉。
        offered: Option<(SocketAddr, u64)>,
        /// 借来用过的，按顺序。
        adopted: Vec<u64>,
    }

    impl Wire for Recorder {
        fn udp(&mut self, to: SocketAddr, frame: &[u8]) {
            self.udp.push((to, Frame::parse(frame).expect("发出去的应该是合法的会话帧")));
        }

        fn tcp(&mut self, frame: &[u8]) {
            self.tcp.push(Frame::parse(frame).expect("发出去的应该是合法的会话帧"));
        }

        fn tcp_connect(&mut self, addrs: &[SocketAddr]) {
            self.tcp_connects += 1;
            self.tcp_targets.push(addrs.to_vec());
        }

        fn tcp_adopt(&mut self) -> Option<(SocketAddr, u64)> {
            let offered = self.offered.take()?;
            self.adopted.push(offered.1);
            Some(offered)
        }

        fn tcp_close(&mut self) {
            self.tcp_closes += 1;
        }
    }

    struct Setup {
        me: NodeId,
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
            Self::as_node(ME, udp, tcp)
        }

        /// 本节点的编号是 `me`（relay 是 9）：骨干上谁先拨号按编号定。
        fn as_node(me: u32, udp: &[&str], tcp: &[&str]) -> Self {
            let relay_key = NodeKey::generate().unwrap();
            let relay = Relay {
                id: node(RELAY),
                key: relay_key.public(),
                name: "relay".to_owned(),
                udp: udp.iter().map(|text| addr(text)).collect(),
                tcp: tcp.iter().map(|text| addr(text)).collect(),
            };
            let start = Instant::now();
            let mut setup = Self {
                me: node(me),
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
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.poll(self.start + at, who, false, false, None, &mut self.wire);
        }

        /// relay 从本节点最近一次发往的那个地址回答。
        fn udp_frame(&mut self, frame: Frame, at: Duration) {
            let from = self.wire.udp.last().expect("relay 只回答发给它的东西").0;
            self.udp_frame_from(from, frame, at);
        }

        fn udp_frame_from(&mut self, from: SocketAddr, frame: Frame, at: Duration) {
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.on_udp_frame(from, &frame, self.start + at, who, &mut self.wire);
        }

        fn tcp_frame(&mut self, frame: Frame, at: Duration) {
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.on_tcp_frame(&frame, self.start + at, who, &mut self.wire);
        }

        fn udp_refusal(&mut self, reason: RefusalReason, node: NodeId, echo: Echo, at: Duration) -> Option<Complaint> {
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.on_udp_refusal(&Refused { reason, node }, &echo, self.start + at, who, &mut self.wire)
        }

        fn tcp_refusal(&mut self, reason: RefusalReason, node: NodeId, echo: Echo, at: Duration) -> Option<Complaint> {
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.on_tcp_refusal(&Refused { reason, node }, &echo, self.start + at, who, &mut self.wire)
        }

        /// TCP 连上了，连上的是最近一次去连的地址里的第一个。
        fn tcp_connected(&mut self, at: Duration) {
            let to = self.wire.tcp_targets.last().and_then(|addrs| addrs.first()).copied().expect("应该去连过 TCP");
            self.tcp_connected_to(to, at);
        }

        fn tcp_connected_to(&mut self, to: SocketAddr, at: Duration) {
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.on_tcp_connected(to, self.start + at, who, &mut self.wire);
        }

        /// relay 对一个 AUTH 的回答，签名是真的。
        fn ok_for(&self, auth: &Auth, observed: &str) -> Frame {
            let observed = addr(observed);
            let message = Accepted::signed_message(node(RELAY), self.me, &auth.nonce, 30, observed);
            let signature = *self.relay_key.sign(Accepted::PURPOSE, &message).as_bytes();
            Frame::Ok(Accepted { nonce: auth.nonce, valid_for: 30, observed, signature })
        }

        fn last_udp(&self) -> Frame {
            self.wire.udp.last().expect("应该发过 UDP 帧").1
        }

        fn last_tcp(&self) -> Frame {
            *self.wire.tcp.last().expect("应该发过 TCP 帧")
        }

        /// 把 UDP 握手走完。返回会话握手时的随机数。
        fn complete_udp(&mut self, at: Duration) -> Nonce {
            let Frame::Init(init) = self.last_udp() else { panic!("UDP 上最后发的应该是 INIT：{:?}", self.wire.udp) };
            self.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), at);
            let Frame::Auth(auth) = self.last_udp() else { panic!("收到 CHALLENGE 之后应该发 AUTH") };
            let ok = self.ok_for(&auth, "203.0.113.7:4000");
            self.udp_frame(ok, at);
            auth.nonce
        }

        /// 把 TCP 握手从 INIT 走到拿到 cookie。
        fn challenge_tcp(&mut self, at: Duration) {
            let Frame::Init(init) = self.last_tcp() else { panic!("TCP 上最后发的应该是 INIT：{:?}", self.wire.tcp) };
            self.tcp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [4; 16] }), at);
        }

        /// 返回会话握手时的随机数。
        fn complete_tcp(&mut self, at: Duration) -> Nonce {
            let Frame::Auth(auth) = self.last_tcp() else { panic!("TCP 上最后发的应该是 AUTH：{:?}", self.wire.tcp) };
            let ok = self.ok_for(&auth, "203.0.113.7:5555");
            self.tcp_frame(ok, at);
            auth.nonce
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

        /// 从 `from` 起 relay 在 UDP 上什么也不回答了。每 50ms 推进一次，返回会话被判为断开的时刻。
        fn stall_udp(&mut self, from: Duration) -> Duration {
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

        /// 本节点是 relay：带着骨干上的情况推进。
        fn poll_as_relay(&mut self, at: Duration, backbone: Backbone) {
            let who = Identity { me: self.me, key: &self.node_key };
            self.link.poll(self.start + at, who, false, false, Some(backbone), &mut self.wire);
        }

        /// 本节点是 relay：从 `from` 到 `to` 每 50ms 推进一次，relay 照常回答 UDP 和 TCP 上的保活。返回最后推进到的时刻。
        fn relay_until(&mut self, from: Duration, to: Duration, backbone: Backbone) -> Duration {
            let (mut at, mut udp_seen, mut tcp_seen) = (from, self.wire.udp.len(), self.wire.tcp.len());
            while at < to {
                at += 50 * MS;
                self.poll_as_relay(at, backbone);
                let udp_pings: Vec<(SocketAddr, Ping)> = self.wire.udp[udp_seen..]
                    .iter()
                    .filter_map(|(to, frame)| if let Frame::Ping(ping) = frame { Some((*to, *ping)) } else { None })
                    .collect();
                let tcp_pings: Vec<Ping> = self.wire.tcp[tcp_seen..]
                    .iter()
                    .filter_map(|frame| if let Frame::Ping(ping) = frame { Some(*ping) } else { None })
                    .collect();
                (udp_seen, tcp_seen) = (self.wire.udp.len(), self.wire.tcp.len());
                for (to, ping) in udp_pings {
                    self.udp_frame_from(to, Frame::Pong(Pong { nonce: ping.nonce, observed: addr("203.0.113.7:4000") }), at + 10 * MS);
                }
                for ping in tcp_pings {
                    self.tcp_frame(Frame::Pong(Pong { nonce: ping.nonce, observed: addr("203.0.113.7:5555") }), at + 10 * MS);
                }
            }
            at
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

        // UDP 并没有被放弃：TCP 连上之后马上在后台试一次，之后照常定期试，通了就换回去。
        setup.keep_tcp_alive(died + 70 * MS, died + 200 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
        assert_eq!(setup.udp_inits(), inits + 1, "TCP 一连上就试了 UDP");
        setup.complete_udp(died + 250 * MS);
        setup.poll(died + 300 * MS);
        assert!(matches!(setup.link.leg(), Some(Leg::Udp(_))), "UDP 恢复了的话立刻换回去");
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
    fn a_handshake_over_tcp_waits_for_a_slow_answer_instead_of_resending() {
        let mut setup = Setup::new(&[], &["198.51.100.9:20330"]);
        setup.poll(Duration::ZERO);
        setup.tcp_connected(10 * MS);
        // 回答 3 秒之后才到（TCP 在重传）。这期间不重发，也不放弃。
        for step in 1..=60u32 {
            setup.poll(10 * MS + 50 * MS * step);
        }
        assert!(matches!(setup.wire.tcp[..], [Frame::Init(_)]), "TCP 里的帧不会丢，重发只是多排一个：{:?}", setup.wire.tcp);
        assert_eq!(setup.wire.tcp_closes, 0);
        let up = setup.reconnect_tcp(Duration::from_secs(3) - 10 * MS);
        assert!(setup.link.is_settled(setup.start + up));

        // 一直没有回答的话，等够了时间就放弃这条连接，换一条再试。
        let mut silent = Setup::new(&[], &["198.51.100.9:20330"]);
        silent.poll(Duration::ZERO);
        silent.tcp_connected(10 * MS);
        let mut at = 10 * MS;
        while silent.wire.tcp_closes == 0 {
            at += 50 * MS;
            assert!(at < Duration::from_secs(10), "握手没有回答，该放弃了");
            silent.poll(at);
        }
        assert!(at >= crate::timing::TCP_HANDSHAKE_STEP, "放弃得太早：{at:?}");
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

        // relay 重启了，它说本节点没有会话。这个错误帧回答的是一个中转帧，回显的东西本节点核对不了：先发一个 PING 去问。
        setup.udp_refusal(RefusalReason::NoSession, node(ME), [0x41; 8], Duration::from_secs(1));
        assert!(setup.link.leg().is_some(), "一个核对不了的错误帧不拆会话");
        let Frame::Ping(ping) = setup.last_udp() else { panic!("应该马上发 PING 去问：{:?}", setup.wire.udp) };
        // relay 对这个 PING 的回答回显了它的随机数：会话确实没了，马上重新握手。
        let sent_before = setup.wire.udp.len();
        setup.udp_refusal(RefusalReason::NoSession, node(ME), ping.nonce, Duration::from_secs(1) + 20 * MS);
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
        let session = setup.complete_udp(10 * MS);
        let sent = setup.wire.udp.len();
        assert_eq!(setup.udp_refusal(RefusalReason::NoSession, node(77), session, 20 * MS), None);
        assert!(setup.link.leg().is_some() && setup.wire.udp.len() == sent, "说的是别的节点");
        let unattached = setup.udp_refusal(RefusalReason::PeerUnattached, node(5), session, 20 * MS);
        assert_eq!(unattached, Some(Complaint::PeerUnattached(node(5))), "对端没挂在这个 relay 上：交给选路去换");
        let unroutable = setup.udp_refusal(RefusalReason::RouteUnavailable, node(8), session, 20 * MS);
        assert_eq!(unroutable, Some(Complaint::RouteUnavailable(node(8))), "多级路径走不下去：也交给选路去换");
        assert!(setup.link.leg().is_some());
    }

    #[test]
    fn a_complaint_that_does_not_echo_the_session_is_ignored() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let tcp_session = setup.fall_back_to_tcp();
        let tcp_nonce = {
            let Some(Frame::Auth(auth)) = setup.wire.tcp.iter().rev().find(|frame| matches!(frame, Frame::Auth(_))).copied() else {
                panic!("TCP 上发过 AUTH")
            };
            auth.nonce
        };
        // 伪造来源地址的人冒充 relay：它不知道会话握手的随机数。
        for reason in [RefusalReason::PeerUnattached, RefusalReason::RouteUnavailable] {
            assert_eq!(setup.tcp_refusal(reason, node(5), [0; 8], tcp_session), None);
            assert_eq!(setup.udp_refusal(reason, node(5), tcp_nonce, tcp_session), None, "UDP 上没有会话，回显什么都不算");
        }
        assert_eq!(
            setup.tcp_refusal(RefusalReason::PeerUnattached, node(5), tcp_nonce, tcp_session),
            Some(Complaint::PeerUnattached(node(5)))
        );
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
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
        setup.link.poll(setup.start, who, false, true, None, &mut setup.wire);
        let Frame::Init(init) = setup.last_udp() else { panic!("测延迟用的是 INIT") };
        assert_eq!(setup.link.estimate(setup.start), None);

        setup.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [0; 16] }), 35 * MS);
        assert_eq!(setup.link.estimate(setup.start + 35 * MS), Some(35 * MS));
        assert_eq!(setup.wire.udp.len(), 1, "只量一个往返，不接着握手");
        assert_eq!(setup.link.leg(), None);
        assert_eq!(setup.link.estimate(setup.start + ESTIMATE_TTL + 36 * MS), None, "过期作废");

        // 一分钟之后再量。
        setup.link.poll(setup.start + 35 * MS + RELAY_SURVEY - MS, who, false, true, None, &mut setup.wire);
        assert_eq!(setup.wire.udp.len(), 1);
        setup.link.poll(setup.start + 35 * MS + RELAY_SURVEY, who, false, true, None, &mut setup.wire);
        assert_eq!(setup.wire.udp.len(), 2);

        // 没有回答的话十秒之后就再问。
        setup.link.poll(setup.start + 35 * MS + RELAY_SURVEY + RELAY_SURVEY / 6, who, false, true, None, &mut setup.wire);
        assert_eq!(setup.wire.udp.len(), 3);
    }

    #[test]
    fn a_try_is_remembered_as_udp_tcp_only_or_unreached_together_with_the_address_the_relay_saw() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        assert_eq!(setup.link.last_try(setup.start), None, "还没试过");
        setup.poll(Duration::ZERO);
        setup.complete_udp(10 * MS);
        let now = setup.start + 20 * MS;
        assert_eq!(setup.link.last_try(now), Some((TryOutcome::Udp, now)), "有会话时就是现在");
        assert_eq!(setup.link.seen_as(now), Some(ip("203.0.113.7")));

        // UDP 一直没有回音，经 TCP 连上：只有 TCP 连得上。TCP 上看到的地址照样是出口地址。
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let up = setup.start + setup.fall_back_to_tcp();
        assert_eq!(setup.link.last_try(up).map(|(outcome, _)| outcome), Some(TryOutcome::Tcp));
        assert_eq!(setup.link.seen_as(up), Some(ip("203.0.113.7")));

        // 两种都连不上：这一轮失败，记成连不上。
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        let mut at = Duration::ZERO;
        while setup.link.last_try(setup.start + at).is_none() {
            setup.poll(at);
            at += 100 * MS;
            assert!(at < Duration::from_secs(30), "该算这一轮失败了");
        }
        assert_eq!(setup.link.last_try(setup.start + at).map(|(outcome, _)| outcome), Some(TryOutcome::Unreached));
    }

    #[test]
    fn an_unanswered_survey_says_udp_does_not_get_through_and_an_answered_one_counts_as_reached() {
        let mut setup = Setup::new(&["198.51.100.9:20330"], &["198.51.100.9:20330"]);
        setup.link.set_wanted(false, setup.start, &mut setup.wire);
        let key = NodeKey::generate().unwrap();
        let who = Identity { me: node(ME), key: &key };
        setup.link.poll(setup.start, who, false, true, None, &mut setup.wire);
        assert!(!setup.link.survey_silent(setup.start + MS), "刚问");
        assert!(setup.link.survey_silent(setup.start + SURVEY_SILENCE), "问了没有回答");
        assert_eq!(setup.link.last_try(setup.start + SURVEY_SILENCE), None, "只试了 UDP，还说不上连不连得上");

        let Frame::Init(init) = setup.last_udp() else { panic!("询问用的是 INIT") };
        setup.udp_frame(Frame::Challenge(Challenge { nonce: init.nonce, cookie: [0; 16] }), 40 * MS);
        assert!(!setup.link.survey_silent(setup.start + SURVEY_SILENCE));
        assert_eq!(setup.link.last_try(setup.start + SECOND), Some((TryOutcome::Udp, setup.start + 40 * MS)));
    }

    const SECOND: Duration = Duration::from_secs(1);
    const DEAD: &str = "198.51.100.9:20330";
    const LAN: &str = "192.168.1.5:20330";
    const MAPPED: &str = "203.0.113.9:40330";

    /// 发出去的 INIT 都发给了哪些地址（按发出的顺序）。
    fn init_targets(wire: &Recorder, from: usize) -> Vec<SocketAddr> {
        wire.udp[from..].iter().filter(|(_, frame)| matches!(frame, Frame::Init(_))).map(|(to, _)| *to).collect()
    }

    #[test]
    fn every_address_gets_the_init_and_the_first_to_answer_carries_the_session() {
        let mut setup = Setup::new(&[DEAD, LAN, MAPPED], &[DEAD, LAN, MAPPED]);
        setup.poll(Duration::ZERO);
        assert_eq!(init_targets(&setup.wire, 0), [addr(DEAD), addr(LAN), addr(MAPPED)], "同时问每个地址，第一个连不上不耽误别的");
        let nonces: Vec<Nonce> =
            setup.wire.udp.iter().filter_map(|(_, frame)| if let Frame::Init(init) = frame { Some(init.nonce) } else { None }).collect();
        assert!(nonces.windows(2).all(|pair| pair[0] == pair[1]), "是同一次握手");

        // 局域网里的地址先回答：和只有这一个地址时一样快。
        setup.udp_frame_from(addr(LAN), Frame::Challenge(Challenge { nonce: nonces[0], cookie: [3; 16] }), 2 * MS);
        assert!(matches!(setup.wire.udp.last(), Some((to, Frame::Auth(_))) if *to == addr(LAN)), "AUTH 只发给回答了的地址");
        // 另一个地址后来也回答了（经路由器绕了一圈）：不理，不能让 relay 那边的会话换来换去。
        let sent = setup.wire.udp.len();
        setup.udp_frame_from(addr(MAPPED), Frame::Challenge(Challenge { nonce: nonces[0], cookie: [5; 16] }), 4 * MS);
        assert_eq!(setup.wire.udp.len(), sent);
        let Frame::Auth(auth) = setup.last_udp() else { panic!("应该发了 AUTH") };
        let ok = setup.ok_for(&auth, "192.168.1.20:20330");
        setup.udp_frame_from(addr(MAPPED), ok, 5 * MS);
        assert_eq!(setup.link.leg(), None, "OK 要从握手的那个地址来");
        setup.udp_frame_from(addr(LAN), ok, 5 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Udp(addr(LAN))));
        assert!(setup.link.take_target_changed());

        setup.poll(PROBE_INTERVAL + 10 * MS);
        assert!(matches!(setup.wire.udp.last(), Some((to, Frame::Ping(_))) if *to == addr(LAN)), "保活只走在用的地址");
        setup.poll(UDP_HEAD_START + 50 * MS);
        assert!(setup.wire.tcp.iter().all(|frame| !matches!(frame, Frame::Auth(_))), "UDP 通了，TCP 从来没有走到 AUTH");
    }

    #[test]
    fn when_the_address_in_use_stops_answering_every_address_is_tried_again() {
        let mut setup = Setup::new(&[DEAD, LAN, MAPPED], &[]);
        setup.poll(Duration::ZERO);
        let Frame::Init(init) = setup.last_udp() else { panic!() };
        setup.udp_frame_from(addr(LAN), Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), 2 * MS);
        let Frame::Auth(auth) = setup.last_udp() else { panic!() };
        let ok = setup.ok_for(&auth, "192.168.1.20:20330");
        setup.udp_frame_from(addr(LAN), ok, 3 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Udp(addr(LAN))));

        // 换了个局域网：原来的地址不再回答，保活连续没有回音，会话断开，马上重新问每个地址。
        let sent = setup.wire.udp.len();
        let died = setup.stall_udp(10 * MS);
        setup.poll(died + 400 * MS);
        assert_eq!(
            init_targets(&setup.wire, sent).iter().rev().take(3).rev().copied().collect::<Vec<_>>(),
            [addr(DEAD), addr(LAN), addr(MAPPED)]
        );
        let Frame::Init(init) = setup.last_udp() else { panic!() };
        setup.udp_frame_from(addr(MAPPED), Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), died + 420 * MS);
        let Frame::Auth(auth) = setup.last_udp() else { panic!() };
        let ok = setup.ok_for(&auth, "198.51.100.30:20330");
        setup.udp_frame_from(addr(MAPPED), ok, died + 440 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Udp(addr(MAPPED))), "这回用的是现在连得上的那个");
    }

    #[test]
    fn an_address_that_answers_but_cannot_finish_the_handshake_is_left_out_for_the_rest_of_the_round() {
        // 第一个地址上回答的是另一台机器（和别的 relay 撞了地址）：它给 cookie，却不认本节点的 AUTH。
        let mut setup = Setup::new(&[LAN, MAPPED], &[]);
        setup.poll(Duration::ZERO);
        let Frame::Init(init) = setup.last_udp() else { panic!() };
        setup.udp_frame_from(addr(LAN), Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), MS);
        let sent = setup.wire.udp.len();
        let mut at = MS;
        while setup.link.udp_target() == Some(addr(LAN)) {
            at += 50 * MS;
            assert!(at < Duration::from_secs(5), "握手走不完总该放弃");
            setup.poll(at);
        }
        assert_eq!(init_targets(&setup.wire, sent), [addr(MAPPED)], "马上接着试剩下的地址，不等这一轮结束");
        assert!(at < Duration::from_secs(3), "{at:?}");
        let Frame::Init(init) = setup.last_udp() else { panic!() };
        setup.udp_frame_from(addr(LAN), Frame::Challenge(Challenge { nonce: init.nonce, cookie: [3; 16] }), at + MS);
        assert!(!matches!(setup.last_udp(), Frame::Auth(_)), "这一轮里不再理它");
        setup.udp_frame_from(addr(MAPPED), Frame::Challenge(Challenge { nonce: init.nonce, cookie: [4; 16] }), at + 2 * MS);
        let Frame::Auth(auth) = setup.last_udp() else { panic!() };
        let ok = setup.ok_for(&auth, "198.51.100.30:20330");
        setup.udp_frame_from(addr(MAPPED), ok, at + 3 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Udp(addr(MAPPED))));
    }

    #[test]
    fn the_nodes_own_addresses_are_never_tried() {
        let mut setup = Setup::new(&[LAN, MAPPED], &[LAN, MAPPED]);
        setup.link.set_own(&[addr(LAN)]);
        setup.poll(Duration::ZERO);
        assert_eq!(init_targets(&setup.wire, 0), [addr(MAPPED)]);
        assert_eq!(setup.wire.tcp_targets, [vec![addr(MAPPED)]]);
    }

    #[test]
    fn a_relay_whose_udp_never_gets_through_works_over_tcp_on_the_same_addresses() {
        let mut setup = Setup::new(&[DEAD, MAPPED], &[DEAD, MAPPED]);
        setup.poll(Duration::ZERO);
        assert_eq!(setup.wire.tcp_targets, [vec![addr(DEAD), addr(MAPPED)]]);
        setup.tcp_connected_to(addr(MAPPED), 300 * MS);
        setup.challenge_tcp(310 * MS);
        setup.poll(UDP_HEAD_START);
        setup.complete_tcp(UDP_HEAD_START + 20 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp), "没有人说过这个 relay 只能用 TCP");

        // 下一次连 TCP 时，上一次握手成功的地址排在最前。
        let died = setup.stall_tcp(UDP_HEAD_START + 20 * MS);
        assert_eq!(setup.wire.tcp_targets.last(), Some(&vec![addr(MAPPED), addr(DEAD)]), "断开于 {died:?}");
    }

    #[test]
    fn a_tcp_address_that_connects_but_cannot_finish_the_handshake_goes_last_next_time() {
        let mut setup = Setup::new(&[], &[LAN, MAPPED]);
        setup.poll(Duration::ZERO);
        setup.tcp_connected_to(addr(LAN), 10 * MS);
        // 连上的是另一台机器：它回了 cookie，AUTH 却等不到回答。
        setup.challenge_tcp(20 * MS);
        let mut at = 20 * MS;
        while setup.wire.tcp_targets.len() < 2 {
            at += 100 * MS;
            assert!(at < Duration::from_secs(30), "握手走不完总该重连");
            setup.poll(at);
        }
        assert_eq!(setup.wire.tcp_targets[1], [addr(MAPPED), addr(LAN)]);
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

    const RELAY_ADDR: &str = "198.51.100.9:20330";

    /// 骨干：本节点是 relay，`theirs` 是对方在本节点的 relay 上的会话，`offered` 是它拨进来、可以借来用的那一条。
    fn backbone(theirs: Option<SessionLeg>, offered: Option<u64>) -> Backbone {
        Backbone { theirs, offered }
    }

    /// UDP 握手的四次重发都没有回音之后的时刻（再多一点余量）。
    const UDP_GIVEN_UP: Duration = Duration::from_millis(2500);

    #[test]
    fn between_relays_udp_is_tried_alone_and_when_it_fails_the_relay_with_the_lower_id_dials_tcp_at_once() {
        let mut setup = Setup::as_node(1, &[RELAY_ADDR], &[RELAY_ADDR]);
        setup.poll_as_relay(Duration::ZERO, backbone(None, None));
        assert!(matches!(setup.wire.udp[..], [(_, Frame::Init(_))]));
        assert_eq!(setup.wire.tcp_connects, 0, "两个 relay 之间不像节点那样同时去连 TCP：UDP 通的话两个会话都在 UDP 上");

        let at = setup.relay_until(Duration::ZERO, UDP_GIVEN_UP, backbone(None, None));
        assert_eq!(setup.wire.tcp_connects, 1, "UDP 握不上手，编号小的这一方马上拨");
        setup.tcp_connected(at + 10 * MS);
        setup.challenge_tcp(at + 20 * MS);
        assert!(matches!(setup.last_tcp(), Frame::Auth(_)), "不停在 cookie 那一步等 UDP：它不会再来了");
        setup.complete_tcp(at + 30 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));

        // 走着 TCP 的骨干不另外试 UDP：一边换回 UDP、另一边还在 TCP 上的那一阵，骨干上转不过去。
        let inits = setup.udp_inits();
        setup.relay_until(at + 30 * MS, at + UDP_RETRY + Duration::from_secs(5), backbone(Some(SessionLeg::Dialled(1)), None));
        assert_eq!((setup.link.leg(), setup.udp_inits(), setup.wire.tcp_closes), (Some(Leg::Tcp), inits, 0));
    }

    #[test]
    fn the_relay_with_the_higher_id_waits_for_the_other_to_dial_and_borrows_the_connection_it_opens() {
        let mut setup = Setup::as_node(20, &[RELAY_ADDR], &[RELAY_ADDR]);
        let at = setup.relay_until(Duration::ZERO, UDP_GIVEN_UP + DIAL_DEFER - Duration::from_secs(1), backbone(None, None));
        assert_eq!(setup.wire.tcp_connects, 0, "编号大的一方先等对方拨");

        // 对方拨进来了：本节点在同一条 TCP 上建自己的会话。
        setup.wire.offered = Some((addr("198.51.100.9:44000"), 77));
        setup.poll_as_relay(at + 10 * MS, backbone(Some(SessionLeg::Accepted(77)), Some(77)));
        assert_eq!(setup.wire.adopted, [77]);
        setup.challenge_tcp(at + 20 * MS);
        setup.complete_tcp(at + 30 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
        setup.relay_until(at + 30 * MS, at + DIAL_DEFER * 2, backbone(Some(SessionLeg::Accepted(77)), Some(77)));
        assert_eq!((setup.wire.tcp_connects, setup.wire.adopted.len()), (0, 1), "借来的那条一直用着，自己不拨");

        // 对方一直没有拨进来（它连不进本节点）：等够了自己拨。
        let mut alone = Setup::as_node(20, &[RELAY_ADDR], &[RELAY_ADDR]);
        let at = alone.relay_until(Duration::ZERO, UDP_GIVEN_UP + DIAL_DEFER, backbone(None, None));
        assert_eq!(alone.wire.tcp_connects, 1, "等到 {at:?} 还没有人拨进来，自己拨");
    }

    #[test]
    fn a_relay_that_cannot_reach_this_one_makes_it_carry_both_sessions_over_a_tcp_connection_it_dials() {
        // 本节点到它的 UDP 会话通着，它在本节点上也有 UDP 的会话：骨干就在 UDP 上，一直不动。
        let mut setup = Setup::as_node(20, &[RELAY_ADDR], &[RELAY_ADDR]);
        setup.poll_as_relay(Duration::ZERO, backbone(None, None));
        setup.complete_udp(10 * MS);
        let at = setup.relay_until(10 * MS, ONE_SIDED_GRACE * 2, backbone(Some(SessionLeg::Udp), None));
        assert_eq!((setup.link.leg(), setup.wire.tcp_connects), (Some(Leg::Udp(addr(RELAY_ADDR))), 0));

        // 它一直没在本节点上建会话：它连不上本节点。本节点放下 UDP，自己拨 TCP，不管编号。
        let start = setup.relay_until(at, at + ONE_SIDED_GRACE - Duration::from_secs(1), backbone(None, None));
        assert_eq!(setup.wire.tcp_connects, 0, "还在等它的会话");
        let at = setup.relay_until(start, start + Duration::from_secs(2), backbone(None, None));
        assert_eq!(setup.wire.tcp_connects, 1);
        assert_eq!(setup.link.leg(), None, "UDP 的会话放下了：两个会话要在同一条 TCP 上");
        setup.tcp_connected(at + 10 * MS);
        setup.challenge_tcp(at + 20 * MS);
        setup.complete_tcp(at + 30 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
    }

    #[test]
    fn a_relay_whose_session_moved_onto_the_connection_it_dialled_is_followed_there_at_once() {
        // 本节点到它的 UDP 通着，它那边的 UDP 却不通：它拨了 TCP，会话搬了上去。不等 `ONE_SIDED_GRACE`，马上跟过去。
        let mut setup = Setup::as_node(1, &[RELAY_ADDR], &[RELAY_ADDR]);
        setup.poll_as_relay(Duration::ZERO, backbone(None, None));
        setup.complete_udp(10 * MS);
        setup.wire.offered = Some((addr("198.51.100.9:44000"), 5));
        setup.poll_as_relay(100 * MS, backbone(Some(SessionLeg::Accepted(5)), Some(5)));
        assert_eq!(setup.wire.adopted, [5]);
        assert_eq!(setup.link.leg(), None, "UDP 的会话放下了，在借来的那条上握手");
        setup.challenge_tcp(120 * MS);
        setup.complete_tcp(130 * MS);
        assert_eq!(setup.link.leg(), Some(Leg::Tcp));
    }

    #[test]
    fn when_both_relays_dial_the_one_with_the_lower_id_keeps_its_own_connection() {
        let mut first = Setup::as_node(1, &[RELAY_ADDR], &[RELAY_ADDR]);
        let at = first.relay_until(Duration::ZERO, UDP_GIVEN_UP, backbone(None, None));
        first.tcp_connected(at);
        first.challenge_tcp(at + 10 * MS);
        first.complete_tcp(at + 20 * MS);
        first.wire.offered = Some((addr("198.51.100.9:44000"), 5));
        first.relay_until(at + 20 * MS, at + Duration::from_secs(2), backbone(Some(SessionLeg::Accepted(5)), Some(5)));
        assert_eq!((first.wire.adopted.len(), first.wire.tcp_closes, first.link.leg()), (0, 0, Some(Leg::Tcp)), "留着自己拨的");

        let mut second = Setup::as_node(20, &[RELAY_ADDR], &[RELAY_ADDR]);
        let at = second.relay_until(Duration::ZERO, UDP_GIVEN_UP + DIAL_DEFER, backbone(None, None));
        second.tcp_connected(at);
        second.challenge_tcp(at + 10 * MS);
        second.complete_tcp(at + 20 * MS);
        second.wire.offered = Some((addr("198.51.100.9:44000"), 5));
        second.poll_as_relay(at + 30 * MS, backbone(Some(SessionLeg::Accepted(5)), Some(5)));
        assert_eq!(second.wire.adopted, [5], "编号大的一方改用对方拨的那条");
        assert!(matches!(second.last_tcp(), Frame::Init(_)), "在借来的那条上从头握手");
    }

    #[test]
    fn a_relay_on_tcp_goes_back_to_udp_once_the_other_relay_has_a_udp_session_here() {
        let mut setup = Setup::as_node(1, &[RELAY_ADDR], &[RELAY_ADDR]);
        let at = setup.relay_until(Duration::ZERO, UDP_GIVEN_UP, backbone(None, None));
        setup.tcp_connected(at);
        setup.challenge_tcp(at + 10 * MS);
        setup.complete_tcp(at + 20 * MS);
        let inits = setup.udp_inits();
        setup.relay_until(at + 20 * MS, at + Duration::from_secs(1), backbone(Some(SessionLeg::Udp), None));
        assert_eq!(setup.wire.tcp_closes, 1, "它在 UDP 上、连得上本节点：回到 UDP 再试");
        assert!(setup.udp_inits() > inits);
    }

    #[test]
    fn a_borrowed_connection_that_yields_no_session_is_not_borrowed_again_for_a_while() {
        let mut setup = Setup::as_node(20, &[RELAY_ADDR], &[RELAY_ADDR]);
        let at = setup.relay_until(Duration::ZERO, UDP_GIVEN_UP, backbone(None, None));
        setup.wire.offered = Some((addr("198.51.100.9:44000"), 5));
        // 对方是不在这条 TCP 上回答的旧版本：本节点的握手等够了就放弃。
        let at = setup.relay_until(at, at + crate::timing::TCP_HANDSHAKE_STEP + Duration::from_secs(1), backbone(None, Some(5)));
        assert_eq!(setup.wire.adopted, [5]);
        setup.wire.offered = Some((addr("198.51.100.9:44000"), 5));
        let at = setup.relay_until(at, at + Duration::from_secs(5), backbone(None, Some(5)));
        assert_eq!(setup.wire.adopted, [5], "同一条不再借");
        setup.wire.offered = Some((addr("198.51.100.9:44001"), 6));
        setup.poll_as_relay(at + 10 * MS, backbone(None, Some(6)));
        assert_eq!(setup.wire.adopted, [5, 6], "新拨进来的那条照样借");
    }

    #[test]
    fn retry_delays_double_up_to_the_cap() {
        let delay = |failures| retry_delay(failures).as_secs_f64();
        assert!((1.0..=1.2).contains(&delay(1)));
        assert!((2.0..=2.4).contains(&delay(2)));
        assert!((8.0..=9.6).contains(&delay(4)));
        assert!((10.0..=12.0).contains(&delay(5)));
        assert!((10.0..=12.0).contains(&delay(60)));
        assert!(jitter(Duration::from_millis(300)) <= Duration::from_millis(300));
    }
}
