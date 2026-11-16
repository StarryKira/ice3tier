//! 到一个对端的直连：候选地址、验证、打洞的重试、在用地址的保活。见 docs/architecture.md §5.3。
//!
//! 这里是一台不做 I/O 的状态机：外面告诉它时间和收到了什么，它回答该发什么、直连现在可不可用。
//! 签名和验签都在外面做，交进来的 PING / PONG 都是已经验过签名的。
//!
//! 一个地址要先验证才能用：向它发 PING，收到带着同一个随机数的 PONG。验证过的地址每 5 秒再探测一次，
//! 连续超时或者丢得太多就判为不可用。没有可用地址时按固定的节奏重试：每次尝试向所有候选地址发几轮 PING，
//! 并且发信令请对端同时向本端发，这就是打洞。

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use i3t_model::control::Signal;
use i3t_proto::session::Nonce;
use i3t_underlay::ifaddr;

use crate::timing::{MAX_TIMEOUTS, PROBE_INTERVAL, PUNCH_ATTEMPT, PUNCH_BACKOFF, PUNCH_IDLE_RETRY, PUNCH_VOLLEYS, Rtt, SILENCE};

/// 一个对端最多记多少个候选地址。对端上报的、信令里带来的、探测里看到的都算。
const MAX_CANDIDATES: usize = 16;
/// 一条信令里最多取多少个端点，多出来的不要。
const MAX_SIGNALLED: usize = 8;
/// 不是对端上报的候选地址多久没有再出现就忘掉；验证过的地址多久没有新的证明就不再认它发来的包。
const CANDIDATE_TTL: Duration = Duration::from_secs(120);
/// 发出去的 PING 等多久还没有回答就不再等。比一次打洞尝试长，晚到的 PONG 照样算数。
const PENDING_TTL: Duration = Duration::from_secs(5);
const MAX_PENDING: usize = 64;
/// 丢包率按最近这么多次探测算。
const LOSS_WINDOW: usize = 10;
/// 窗口满了之后，丢的超过这么多个（20%）就判为不可用。
const MAX_LOSSES: usize = LOSS_WINDOW / 5;
/// 同一个地址上，对端的 PING 引出的反向验证最多这么频繁。重放一个旧的 PING 没法让本端发得更多。
const REVERSE_PROBE_INTERVAL: Duration = Duration::from_secs(1);
/// 两条信令通道（controller、连接里）会把同一个打洞请求各送来一份，这么短的时间里的第二份不再处理。
const REQUEST_DEDUP: Duration = Duration::from_millis(300);
/// 对端不活跃时，验证过的地址多久没有新的证明就丢掉。
const STALE: Duration = Duration::from_secs(30);

/// 外面决定的、这个对端此刻适用的限制。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Policy<'a> {
    /// 现在需要到这个对端的直连：它是活跃的，并且没有禁用直连。
    pub(crate) wanted: bool,
    /// 可以由本端主动去探测它的候选地址。对端是只拨出的节点时不行：它不理陌生的探测，只能等它来找本端。
    pub(crate) initiate: bool,
    /// 可以和它互发打洞信令。任何一方只拨出时不行。
    pub(crate) signal: bool,
    /// 有包发给它，却有一阵没有从直连上收到它的包了。
    pub(crate) silent: bool,
    /// 本节点自己的候选端点，放进信令里告诉对端。
    pub(crate) local: &'a [SocketAddr],
}

/// 要外面去做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// 向 `to` 发一个带这个随机数的 PING。
    Ping { to: SocketAddr, nonce: Nonce },
    /// 把这条信令送给对端。
    Signal(Signal),
}

/// 直连的可用状态变了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// `addr` 验证通过，从现在起可以往它发连接的包。也可能是从一个已验证的地址换到了另一个。
    Up {
        addr: SocketAddr,
    },
    Down {
        reason: DownReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownReason {
    /// 探测连续超时。
    Timeouts,
    /// 最近的探测丢了超过 20%。
    Loss,
    /// 对端不活跃了，地址也很久没有验证过。
    Stale,
}

impl DownReason {
    pub(crate) const fn describe(self) -> &'static str {
        match self {
            Self::Timeouts => "直连探测连续超时",
            Self::Loss => "直连探测丢失超过 20%",
            Self::Stale => "对端不活跃，直连地址过期",
        }
    }
}

/// [`DirectPath::poll`] 的结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) actions: Vec<Action>,
    pub(crate) change: Option<Change>,
}

#[derive(Debug)]
struct Candidate {
    addr: SocketAddr,
    /// 在对端上报的端点里（netmap）。
    reported: bool,
    /// 最近一次从任何来源得知这个地址。
    refreshed: Instant,
    /// 最近一次从这个地址收到对端签过名的探测帧。
    proven: Option<Instant>,
}

#[derive(Debug)]
struct Pending {
    nonce: Nonce,
    addr: SocketAddr,
    sent: Instant,
}

#[derive(Debug)]
struct Active {
    addr: SocketAddr,
    rtt: Rtt,
    /// 最近一次得到回答。
    confirmed: Instant,
    last_probe: Instant,
    /// 在等回答的那个保活探测是什么时候发的。
    awaiting: Option<Instant>,
    timeouts: u8,
    /// 最近几次保活探测是不是丢了。
    history: VecDeque<bool>,
}

impl Active {
    fn record(&mut self, lost: bool) {
        if self.history.len() == LOSS_WINDOW {
            self.history.pop_front();
        }
        self.history.push_back(lost);
    }

    fn too_lossy(&self) -> bool {
        self.history.len() == LOSS_WINDOW && self.history.iter().filter(|&&lost| lost).count() > MAX_LOSSES
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Punch {
    /// 下一次 `poll` 就开始第一次尝试。
    Idle,
    /// 第 `round` 次重试（0 是头一次）正在进行，已经发了 `volleys` 轮探测。
    Attempt { round: usize, started: Instant, volleys: usize },
    /// 第 `round` 次尝试失败了，等到 `until` 再试。
    Backoff { round: usize, until: Instant },
    /// 一整轮都失败了。之后候选地址有变化就立刻重来，否则到 `retry_at` 再试一次。
    Exhausted { retry_at: Instant },
}

/// 到一个对端的直连状态。
#[derive(Debug)]
pub(crate) struct DirectPath {
    candidates: Vec<Candidate>,
    pending: VecDeque<Pending>,
    active: Option<Active>,
    punch: Punch,
    last_request: Option<Instant>,
}

impl DirectPath {
    pub(crate) const fn new() -> Self {
        Self { candidates: Vec::new(), pending: VecDeque::new(), active: None, punch: Punch::Idle, last_request: None }
    }

    /// 在用的直连地址和到它的延迟。
    pub(crate) fn active(&self) -> Option<(SocketAddr, Rtt)> {
        self.active.as_ref().map(|active| (active.addr, active.rtt))
    }

    /// 在用的地址是不是可疑：有保活探测没有得到回答。
    pub(crate) fn in_doubt(&self) -> bool {
        self.active.as_ref().is_some_and(|active| active.timeouts > 0)
    }

    /// 可以认作"这个对端发来的连接包"的来源地址：最近从它们收到过对端签了名的探测帧。
    ///
    /// 比"本端验证过的地址"宽一点：对端先验证完、先改走直连时，它的包会比本端的反向验证早到一步。
    /// 放进来的即使是别人重放的旧探测，也只是让一些垃圾包多走到连接层，在那里解密不了被丢掉。
    pub(crate) fn sources(&self, now: Instant) -> impl Iterator<Item = SocketAddr> + '_ {
        let fresh = move |candidate: &&Candidate| candidate.proven.is_some_and(|at| now.saturating_duration_since(at) < CANDIDATE_TTL);
        self.candidates.iter().filter(fresh).map(|candidate| candidate.addr)
    }

    /// netmap 里对端上报的端点。每次 netmap 更新都调用。
    pub(crate) fn set_reported(&mut self, endpoints: &[SocketAddr], now: Instant) {
        for candidate in &mut self.candidates {
            candidate.reported = endpoints.contains(&candidate.addr);
        }
        let mut appeared = false;
        for &addr in endpoints.iter().take(MAX_CANDIDATES) {
            appeared |= self.learn(addr, now);
            if let Some(candidate) = self.candidate_mut(addr) {
                candidate.reported = true;
            }
        }
        if appeared {
            self.candidates_changed();
        }
    }

    /// 本节点自己的候选端点变了（换了网络、拿到了新的反射地址）。
    pub(crate) fn local_changed(&mut self) {
        self.candidates_changed();
    }

    /// 对端发来打洞信令。返回要发的东西：对请求的回答，以及立刻向它的候选地址发的一轮探测。
    pub(crate) fn on_signal(&mut self, signal: &Signal, now: Instant, policy: &Policy<'_>) -> Vec<Action> {
        if !policy.signal {
            return Vec::new();
        }
        let (endpoints, is_request) = match signal {
            Signal::PunchRequest { endpoints } => (endpoints, true),
            Signal::PunchReply { endpoints } => (endpoints, false),
        };
        if is_request {
            if self.last_request.is_some_and(|last| now.saturating_duration_since(last) < REQUEST_DEDUP) {
                return Vec::new();
            }
            self.last_request = Some(now);
        }
        for &addr in endpoints.iter().take(MAX_SIGNALLED) {
            self.learn(addr, now);
        }
        let mut actions = Vec::new();
        if is_request {
            actions.push(Action::Signal(Signal::PunchReply { endpoints: policy.local.to_vec() }));
        }
        if self.active.is_none() {
            // 对端此刻正在向本端发探测。本端也立刻发，并把这一次尝试的节奏对齐到现在，后面两轮跟着来。
            let round = match self.punch {
                Punch::Idle => 0,
                Punch::Attempt { round, .. } | Punch::Backoff { round, .. } => round,
                Punch::Exhausted { .. } => PUNCH_BACKOFF.len(),
            };
            self.punch = Punch::Attempt { round, started: now, volleys: 1 };
            actions.extend(self.volley(now));
        }
        actions
    }

    /// 从 `from` 收到了对端的 PING（签名已经验过，PONG 由外面去回）。返回要不要向 `from` 发一个反方向的 PING。
    pub(crate) fn on_ping(&mut self, from: SocketAddr, now: Instant) -> Option<Action> {
        self.learn(from, now);
        self.prove(from, now);
        if self.active.as_ref().is_some_and(|active| active.addr == from) {
            return None;
        }
        let recently = |pending: &Pending| pending.addr == from && now.saturating_duration_since(pending.sent) < REVERSE_PROBE_INTERVAL;
        if self.pending.iter().any(recently) {
            return None;
        }
        Some(self.probe(from, now))
    }

    /// 是不是向 `from` 发过带这个随机数的 PING、还在等回答。验 PONG 的签名之前先问这个，不花冤枉的计算。
    pub(crate) fn expects_pong(&self, from: SocketAddr, nonce: &Nonce) -> bool {
        self.pending.iter().any(|pending| pending.nonce == *nonce && pending.addr == from)
    }

    /// 从 `from` 收到了对端的 PONG（签名已经验过）。随机数对不上发往 `from` 的任何一个 PING 时不算数。
    pub(crate) fn on_pong(&mut self, from: SocketAddr, nonce: &Nonce, now: Instant) -> Option<Change> {
        let index = self.pending.iter().position(|pending| pending.nonce == *nonce && pending.addr == from)?;
        let sample = now.saturating_duration_since(self.pending.remove(index)?.sent);
        self.prove(from, now);
        match &mut self.active {
            Some(active) if active.addr == from => {
                active.rtt.update(sample);
                active.confirmed = now;
                active.timeouts = 0;
                if active.awaiting.take().is_some() {
                    active.record(false);
                }
                None
            }
            // 局域网直连无条件优先（白皮书 §7.1）：已经在用公网地址时，局域网地址验证通过就换过去。别的情况先到先得。
            Some(active) if !(ifaddr::is_private(from.ip()) && !ifaddr::is_private(active.addr.ip())) => None,
            Some(_) | None => {
                self.active = Some(Active {
                    addr: from,
                    rtt: Rtt::new(sample),
                    confirmed: now,
                    last_probe: now,
                    awaiting: None,
                    timeouts: 0,
                    history: VecDeque::new(),
                });
                self.punch = Punch::Idle;
                Some(Change::Up { addr: from })
            }
        }
    }

    /// 推进定时器。隔一小段时间调用一次。
    pub(crate) fn poll(&mut self, now: Instant, policy: &Policy<'_>) -> Step {
        let mut step = Step::default();
        self.expire(now);
        if !policy.wanted {
            self.punch = Punch::Idle;
            if self.active.as_ref().is_some_and(|active| now.saturating_duration_since(active.confirmed) > STALE) {
                self.active = None;
                step.change = Some(Change::Down { reason: DownReason::Stale });
            }
            return step;
        }
        if self.active.is_some() {
            self.maintain(now, policy, &mut step);
        }
        if self.active.is_none() && policy.initiate {
            self.punch(now, policy, &mut step);
        }
        step
    }

    /// 给在用的地址保活，判断它还能不能用。
    fn maintain(&mut self, now: Instant, policy: &Policy<'_>, step: &mut Step) {
        let Some(active) = &mut self.active else { return };
        let since_probe = now.saturating_duration_since(active.last_probe);
        let due = match active.awaiting {
            Some(sent) if now.saturating_duration_since(sent) < active.rtt.probe_timeout() => false,
            Some(_) => {
                active.awaiting = None;
                active.timeouts += 1;
                active.record(true);
                let verdict = if active.timeouts >= MAX_TIMEOUTS {
                    Some(DownReason::Timeouts)
                } else if active.too_lossy() {
                    Some(DownReason::Loss)
                } else {
                    None
                };
                if let Some(reason) = verdict {
                    self.active = None;
                    // 路是断了的话马上从头重试；只是丢包多的话它多半还能验证通过，立刻重试只会来回切，所以隔一阵再说。
                    self.punch = match reason {
                        DownReason::Loss => Punch::Exhausted { retry_at: now + PUNCH_IDLE_RETRY },
                        DownReason::Timeouts | DownReason::Stale => Punch::Idle,
                    };
                    step.change = Some(Change::Down { reason });
                    return;
                }
                // 超时之后不等下一个探测间隔，立刻再问。
                true
            }
            None => since_probe >= PROBE_INTERVAL || (policy.silent && since_probe >= SILENCE),
        };
        if due {
            let addr = active.addr;
            active.awaiting = Some(now);
            active.last_probe = now;
            step.actions.push(self.probe(addr, now));
        }
    }

    /// 没有可用地址时按节奏重试。
    fn punch(&mut self, now: Instant, policy: &Policy<'_>, step: &mut Step) {
        self.punch = match self.punch {
            Punch::Idle => Punch::Attempt { round: 0, started: now, volleys: 0 },
            Punch::Backoff { round, until } if now >= until => Punch::Attempt { round: round + 1, started: now, volleys: 0 },
            Punch::Exhausted { retry_at } if now >= retry_at => Punch::Attempt { round: PUNCH_BACKOFF.len(), started: now, volleys: 0 },
            waiting @ (Punch::Attempt { .. } | Punch::Backoff { .. } | Punch::Exhausted { .. }) => waiting,
        };
        let Punch::Attempt { round, started, volleys } = self.punch else { return };
        let elapsed = now.saturating_duration_since(started);
        if PUNCH_VOLLEYS.get(volleys).is_some_and(|&at| elapsed >= at) {
            if volleys == 0 && policy.signal {
                step.actions.push(Action::Signal(Signal::PunchRequest { endpoints: policy.local.to_vec() }));
            }
            step.actions.extend(self.volley(now));
            self.punch = Punch::Attempt { round, started, volleys: volleys + 1 };
        }
        if elapsed >= PUNCH_ATTEMPT {
            self.punch = match PUNCH_BACKOFF.get(round) {
                Some(&wait) => Punch::Backoff { round, until: now + wait },
                None => Punch::Exhausted { retry_at: now + PUNCH_IDLE_RETRY },
            };
        }
    }

    /// 向每个候选地址发一个 PING。
    fn volley(&mut self, now: Instant) -> Vec<Action> {
        let targets: Vec<SocketAddr> = self.candidates.iter().map(|candidate| candidate.addr).collect();
        targets.into_iter().map(|addr| self.probe(addr, now)).collect()
    }

    fn probe(&mut self, to: SocketAddr, now: Instant) -> Action {
        let nonce: Nonce = i3t_underlay::random_bytes();
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back(Pending { nonce, addr: to, sent: now });
        Action::Ping { to, nonce }
    }

    /// 记下一个候选地址。返回它是不是新的。
    fn learn(&mut self, addr: SocketAddr, now: Instant) -> bool {
        if addr.port() == 0 || addr.ip().is_unspecified() {
            return false;
        }
        if let Some(candidate) = self.candidate_mut(addr) {
            candidate.refreshed = now;
            return false;
        }
        if self.candidates.len() >= MAX_CANDIDATES {
            // 满了就挤掉最久没有出现的一个；对端上报的和在用的不动。
            let active = self.active.as_ref().map(|active| active.addr);
            let evictable =
                self.candidates.iter().enumerate().filter(|(_, candidate)| !candidate.reported && Some(candidate.addr) != active);
            let Some((oldest, _)) = evictable.min_by_key(|(_, candidate)| candidate.refreshed) else { return false };
            self.candidates.swap_remove(oldest);
        }
        self.candidates.push(Candidate { addr, reported: false, refreshed: now, proven: None });
        true
    }

    fn prove(&mut self, addr: SocketAddr, now: Instant) {
        if let Some(candidate) = self.candidate_mut(addr) {
            candidate.proven = Some(now);
        }
    }

    fn candidate_mut(&mut self, addr: SocketAddr) -> Option<&mut Candidate> {
        self.candidates.iter_mut().find(|candidate| candidate.addr == addr)
    }

    /// 任何一方的候选端点变了：之前的失败记录清掉，立刻重新尝试（白皮书 §5.3）。
    fn candidates_changed(&mut self) {
        if self.active.is_none() {
            self.punch = Punch::Idle;
        }
    }

    fn expire(&mut self, now: Instant) {
        while self.pending.front().is_some_and(|pending| now.saturating_duration_since(pending.sent) > PENDING_TTL) {
            self.pending.pop_front();
        }
        let active = self.active.as_ref().map(|active| active.addr);
        self.candidates.retain(|candidate| {
            candidate.reported || Some(candidate.addr) == active || now.saturating_duration_since(candidate.refreshed) < CANDIDATE_TTL
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn policy(local: &[SocketAddr]) -> Policy<'_> {
        Policy { wanted: true, initiate: true, signal: true, silent: false, local }
    }

    fn pings(actions: &[Action]) -> Vec<(SocketAddr, Nonce)> {
        actions.iter().filter_map(|action| if let Action::Ping { to, nonce } = action { Some((*to, *nonce)) } else { None }).collect()
    }

    fn signals(actions: &[Action]) -> Vec<&Signal> {
        actions.iter().filter_map(|action| if let Action::Signal(signal) = action { Some(signal) } else { None }).collect()
    }

    /// 一条已经验证通过的直连，返回它和验证通过的时刻。
    fn established(peer: SocketAddr) -> (DirectPath, Instant) {
        let start = Instant::now();
        let mut path = DirectPath::new();
        path.set_reported(&[peer], start);
        let step = path.poll(start, &policy(&[]));
        let (to, nonce) = pings(&step.actions)[0];
        let up = start + 20 * MS;
        assert_eq!(path.on_pong(to, &nonce, up), Some(Change::Up { addr: peer }));
        (path, up)
    }

    #[test]
    fn a_candidate_becomes_usable_only_after_a_matching_pong() {
        let start = Instant::now();
        let peer = addr("203.0.113.7:20330");
        let local = [addr("198.51.100.1:20330")];
        let mut path = DirectPath::new();
        path.set_reported(&[peer], start);
        assert_eq!(path.active(), None);

        let step = path.poll(start, &policy(&local));
        assert_eq!(signals(&step.actions), [&Signal::PunchRequest { endpoints: local.to_vec() }], "尝试开始时请对端同时发");
        let [(to, nonce)] = pings(&step.actions)[..] else { panic!("应该向唯一的候选发一个 PING：{step:?}") };
        assert_eq!(to, peer);
        assert_eq!(path.active(), None, "发了 PING 还不算验证过");

        assert_eq!(path.on_pong(peer, &[0; 8], start + 10 * MS), None, "随机数不对");
        assert_eq!(path.on_pong(addr("203.0.113.8:20330"), &nonce, start + 10 * MS), None, "不是从被问的地址来的");
        assert_eq!(path.active(), None);

        assert_eq!(path.on_pong(peer, &nonce, start + 30 * MS), Some(Change::Up { addr: peer }));
        let (active, rtt) = path.active().unwrap();
        assert_eq!((active, rtt.get()), (peer, 30 * MS));
        assert_eq!(path.on_pong(peer, &nonce, start + 40 * MS), None, "同一个 PONG 不能用两次");
    }

    #[test]
    fn an_attempt_sends_three_volleys_and_then_backs_off_on_the_documented_schedule() {
        let start = Instant::now();
        let peer = addr("203.0.113.7:20330");
        let mut path = DirectPath::new();
        path.set_reported(&[peer], start);
        let policy = policy(&[]);

        // 每 10ms 推进一次，记下每个 PING 和每条信令发出的时刻。
        let mut ping_times = Vec::new();
        let mut request_times = Vec::new();
        for tick in 0..=6_000u32 {
            let now = start + 10 * MS * tick;
            let step = path.poll(now, &policy);
            assert_eq!(step.change, None);
            if !pings(&step.actions).is_empty() {
                ping_times.push(tick * 10);
            }
            if !signals(&step.actions).is_empty() {
                request_times.push(tick * 10);
            }
        }
        // 一次尝试 1.5 秒，然后等 1、1、2、4、4、8、8、16 秒。
        let expected: Vec<u32> = [0u32, 2500, 5000, 8500, 14000, 19500, 29000, 38500, 56000].to_vec();
        assert_eq!(request_times, expected, "每次尝试开始时发一条信令");
        let first_volleys: Vec<u32> = expected.iter().flat_map(|&at| [at, at + 300, at + 600]).collect();
        assert_eq!(ping_times, first_volleys, "每次尝试三轮探测");
    }

    #[test]
    fn after_the_schedule_is_exhausted_it_retries_once_a_minute_or_at_once_when_candidates_change() {
        let start = Instant::now();
        let peer = addr("203.0.113.7:20330");
        let mut path = DirectPath::new();
        path.set_reported(&[peer], start);
        let policy = policy(&[]);
        let exhausted_at = (0..).map(|tick| start + 100 * MS * tick).find(|&now| {
            path.poll(now, &policy);
            matches!(path.punch, Punch::Exhausted { .. })
        });
        let exhausted_at = exhausted_at.unwrap();

        assert!(path.poll(exhausted_at + Duration::from_secs(59), &policy).actions.is_empty());
        let retry = path.poll(exhausted_at + Duration::from_secs(60), &policy);
        assert_eq!(pings(&retry.actions).len(), 1, "一分钟之后再试一次");
        let failed = exhausted_at + Duration::from_secs(60) + PUNCH_ATTEMPT;
        path.poll(failed, &policy);
        assert!(matches!(path.punch, Punch::Exhausted { .. }), "再失败还是每分钟一次，不重走整个退避表");

        // 对端上报了新的端点：立刻重试，之前的失败记录清掉。
        let moved = addr("203.0.113.9:4000");
        path.set_reported(&[peer, moved], failed + 5 * MS);
        let step = path.poll(failed + 5 * MS, &policy);
        assert_eq!(pings(&step.actions).iter().map(|(to, _)| *to).collect::<Vec<_>>(), [peer, moved]);
        assert!(matches!(path.punch, Punch::Attempt { round: 0, .. }));

        // 本端的端点变了也一样。
        path.poll(failed + 5 * MS + PUNCH_ATTEMPT, &policy);
        assert!(matches!(path.punch, Punch::Backoff { round: 0, .. }));
        path.local_changed();
        assert_eq!(path.punch, Punch::Idle);
    }

    #[test]
    fn the_active_address_is_probed_every_five_seconds() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let policy = policy(&[]);
        assert!(path.poll(up + PROBE_INTERVAL - MS, &policy).actions.is_empty());
        let step = path.poll(up + PROBE_INTERVAL, &policy);
        let [(to, nonce)] = pings(&step.actions)[..] else { panic!("该发保活探测了：{step:?}") };
        assert_eq!(to, peer);
        assert!(signals(&step.actions).is_empty(), "保活不发信令");
        assert_eq!(path.on_pong(peer, &nonce, up + PROBE_INTERVAL + 10 * MS), None);
        assert!(!path.in_doubt());
        assert!(path.poll(up + PROBE_INTERVAL + 20 * MS, &policy).actions.is_empty());
    }

    #[test]
    fn three_consecutive_timeouts_take_the_path_down_and_restart_punching_at_once() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let policy = policy(&[]);
        let timeout = path.active().unwrap().1.probe_timeout();

        let mut now = up + PROBE_INTERVAL;
        assert_eq!(pings(&path.poll(now, &policy).actions).len(), 1);
        for expected_timeouts in 1..MAX_TIMEOUTS {
            now += timeout;
            let step = path.poll(now, &policy);
            assert_eq!(step.change, None);
            assert_eq!(pings(&step.actions).len(), 1, "超时之后立刻再问，不等 5 秒");
            assert!(path.in_doubt());
            assert_eq!(path.active.as_ref().unwrap().timeouts, expected_timeouts);
        }
        now += timeout;
        let step = path.poll(now, &policy);
        assert_eq!(step.change, Some(Change::Down { reason: DownReason::Timeouts }));
        assert_eq!(path.active(), None);
        assert_eq!(pings(&step.actions).len(), 1, "同一步里就开始重新验证");
        assert!(matches!(path.punch, Punch::Attempt { round: 0, .. }));
    }

    #[test]
    fn an_answer_in_time_clears_the_timeout_count() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let policy = policy(&[]);
        let timeout = path.active().unwrap().1.probe_timeout();
        let mut now = up + PROBE_INTERVAL;
        path.poll(now, &policy);
        now += timeout;
        let retry = path.poll(now, &policy);
        let (_, nonce) = pings(&retry.actions)[0];
        assert!(path.in_doubt());
        assert_eq!(path.on_pong(peer, &nonce, now + 5 * MS), None);
        assert!(!path.in_doubt());
        assert_eq!(path.active().map(|(addr, _)| addr), Some(peer));
    }

    #[test]
    fn losing_more_than_a_fifth_of_recent_probes_takes_the_path_down_and_waits_before_retrying() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let policy = policy(&[]);
        let mut now = up;
        let mut down = None;
        // 每三个探测丢一个：从不连续超时三次，但丢包率是三分之一。
        for round in 0..30 {
            now += PROBE_INTERVAL;
            let step = path.poll(now, &policy);
            let (_, nonce) = pings(&step.actions)[0];
            if round % 2 == 0 {
                path.on_pong(peer, &nonce, now + 5 * MS);
                continue;
            }
            now += path.active().unwrap().1.probe_timeout();
            let step = path.poll(now, &policy);
            if let Some(change) = step.change {
                down = Some(change);
                assert!(step.actions.is_empty(), "丢包多的路不立刻重试，免得来回切");
                break;
            }
            let (_, nonce) = pings(&step.actions)[0];
            path.on_pong(peer, &nonce, now + 5 * MS);
        }
        assert_eq!(down, Some(Change::Down { reason: DownReason::Loss }));
        assert!(path.poll(now + Duration::from_secs(59), &policy).actions.is_empty());
        assert!(!path.poll(now + PUNCH_IDLE_RETRY, &policy).actions.is_empty());
    }

    #[test]
    fn silence_on_a_busy_path_triggers_an_early_probe() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let silent = Policy { silent: true, ..policy(&[]) };
        assert!(path.poll(up + SILENCE - MS, &silent).actions.is_empty());
        assert_eq!(pings(&path.poll(up + SILENCE, &silent).actions).len(), 1);
    }

    #[test]
    fn a_ping_from_the_peer_triggers_one_reverse_probe_per_second_per_address() {
        let start = Instant::now();
        let from = addr("203.0.113.7:4000");
        let mut path = DirectPath::new();
        assert_eq!(path.sources(start).count(), 0);

        let Some(Action::Ping { to, nonce }) = path.on_ping(from, start) else { panic!("应该反过来验证这个地址") };
        assert_eq!(to, from);
        assert_eq!(path.sources(start).collect::<Vec<_>>(), [from], "收到过签了名的 PING 的地址，可以认它发来的连接包");
        assert_eq!(path.active(), None, "但还不能往它发");
        assert_eq!(path.on_ping(from, start + 500 * MS), None, "重放同一个 PING 不会让本端多发");
        assert!(path.on_ping(from, start + REVERSE_PROBE_INTERVAL).is_some());

        assert_eq!(path.on_pong(from, &nonce, start + 30 * MS), Some(Change::Up { addr: from }));
        assert_eq!(path.on_ping(from, start + 5 * REVERSE_PROBE_INTERVAL), None, "已经在用的地址有自己的保活");
    }

    #[test]
    fn a_punch_request_is_answered_and_both_sides_probe_at_once() {
        let start = Instant::now();
        let local = [addr("198.51.100.1:20330")];
        let theirs = vec![addr("203.0.113.7:4000"), addr("192.168.1.5:20330")];
        let mut path = DirectPath::new();
        let request = Signal::PunchRequest { endpoints: theirs.clone() };

        let actions = path.on_signal(&request, start, &policy(&local));
        assert_eq!(signals(&actions), [&Signal::PunchReply { endpoints: local.to_vec() }]);
        assert_eq!(pings(&actions).iter().map(|(to, _)| *to).collect::<Vec<_>>(), theirs);
        assert!(path.on_signal(&request, start + 50 * MS, &policy(&local)).is_empty(), "另一条通道送来的同一个请求");

        // 后面两轮探测跟着这次请求的时刻走，并且不再发自己的请求。
        let step = path.poll(start + 300 * MS, &policy(&local));
        assert_eq!(pings(&step.actions).len(), 2);
        assert!(signals(&step.actions).is_empty());

        // 回答只带来候选和一轮探测，不再回信令。
        let mut requester = DirectPath::new();
        let actions = requester.on_signal(&Signal::PunchReply { endpoints: theirs.clone() }, start, &policy(&local));
        assert!(signals(&actions).is_empty());
        assert_eq!(pings(&actions).len(), 2);
    }

    #[test]
    fn signals_are_ignored_when_punching_is_not_allowed() {
        let start = Instant::now();
        let mut path = DirectPath::new();
        let no_signal = Policy { signal: false, ..policy(&[]) };
        let request = Signal::PunchRequest { endpoints: vec![addr("203.0.113.7:4000")] };
        assert!(path.on_signal(&request, start, &no_signal).is_empty());
        assert!(path.candidates.is_empty());

        // 不许发信令时，尝试照常进行，只是不带信令。
        path.set_reported(&[addr("203.0.113.7:20330")], start);
        let step = path.poll(start, &no_signal);
        assert!(signals(&step.actions).is_empty());
        assert_eq!(pings(&step.actions).len(), 1);
    }

    #[test]
    fn nothing_is_initiated_towards_a_peer_that_only_dials_out() {
        let start = Instant::now();
        let mut path = DirectPath::new();
        path.set_reported(&[addr("203.0.113.7:20330")], start);
        let passive = Policy { initiate: false, signal: false, ..policy(&[]) };
        assert_eq!(path.poll(start, &passive), Step::default());
        // 它来找本端时照样反向验证，验证过的地址照样保活。
        let from = addr("203.0.113.7:5555");
        let Some(Action::Ping { nonce, .. }) = path.on_ping(from, start) else { panic!("应该反向验证") };
        assert_eq!(path.on_pong(from, &nonce, start + 10 * MS), Some(Change::Up { addr: from }));
        assert_eq!(pings(&path.poll(start + PROBE_INTERVAL + 10 * MS, &passive).actions).len(), 1);
    }

    #[test]
    fn an_inactive_peer_is_not_probed_and_its_address_goes_stale() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let idle = Policy { wanted: false, ..policy(&[]) };
        assert_eq!(path.poll(up + PROBE_INTERVAL, &idle), Step::default());
        assert!(path.active().is_some(), "刚不活跃时地址还留着，马上又有流量的话不用重新验证");
        let step = path.poll(up + STALE + MS, &idle);
        assert_eq!(step, Step { actions: Vec::new(), change: Some(Change::Down { reason: DownReason::Stale }) });

        // 重新活跃之后从头开始。
        let step = path.poll(up + STALE + 2 * MS, &policy(&[]));
        assert_eq!(pings(&step.actions).len(), 1);
    }

    #[test]
    fn an_address_that_was_idle_is_probed_as_soon_as_the_peer_is_wanted_again() {
        let peer = addr("203.0.113.7:20330");
        let (mut path, up) = established(peer);
        let idle = Policy { wanted: false, ..policy(&[]) };
        path.poll(up + 2 * PROBE_INTERVAL, &idle);
        let step = path.poll(up + 2 * PROBE_INTERVAL + MS, &policy(&[]));
        assert_eq!(pings(&step.actions).len(), 1, "隔了超过一个探测间隔，先确认它还在");
    }

    #[test]
    fn a_lan_address_replaces_a_public_one_but_not_the_other_way_round() {
        let start = Instant::now();
        let (public, lan) = (addr("203.0.113.7:4000"), addr("192.168.1.5:20330"));
        let mut path = DirectPath::new();
        path.set_reported(&[public, lan], start);
        let step = path.poll(start, &policy(&[]));
        let probes = pings(&step.actions);
        let nonce_of = |target: SocketAddr| probes.iter().find(|(to, _)| *to == target).unwrap().1;

        assert_eq!(path.on_pong(public, &nonce_of(public), start + 30 * MS), Some(Change::Up { addr: public }));
        assert_eq!(path.on_pong(lan, &nonce_of(lan), start + 40 * MS), Some(Change::Up { addr: lan }), "局域网地址优先");

        let mut path = DirectPath::new();
        path.set_reported(&[public, lan], start);
        let probes = pings(&path.poll(start, &policy(&[])).actions);
        let nonce_of = |target: SocketAddr| probes.iter().find(|(to, _)| *to == target).unwrap().1;
        assert_eq!(path.on_pong(lan, &nonce_of(lan), start + 30 * MS), Some(Change::Up { addr: lan }));
        assert_eq!(path.on_pong(public, &nonce_of(public), start + 40 * MS), None, "已经在用局域网地址就不换");
        assert_eq!(path.sources(start + 40 * MS).count(), 2, "两个地址来的包都认");
    }

    #[test]
    fn the_candidate_list_is_bounded_and_forgets_what_is_no_longer_seen() {
        let start = Instant::now();
        let mut path = DirectPath::new();
        let reported = addr("203.0.113.7:20330");
        path.set_reported(&[reported], start);
        for port in 1..=100u16 {
            path.on_ping(SocketAddr::from(([198, 51, 100, 1], port)), start + MS * u32::from(port));
        }
        assert_eq!(path.candidates.len(), MAX_CANDIDATES);
        assert!(path.candidates.iter().any(|candidate| candidate.addr == reported), "对端上报的不会被挤掉");
        assert!(path.pending.len() <= MAX_PENDING);

        path.poll(start + CANDIDATE_TTL + Duration::from_secs(1), &Policy { wanted: false, ..policy(&[]) });
        assert_eq!(path.candidates.iter().map(|candidate| candidate.addr).collect::<Vec<_>>(), [reported]);
        assert!(path.pending.is_empty());

        // 上报的端点换了，旧的也跟着过期。
        let later = start + 2 * CANDIDATE_TTL + Duration::from_secs(2);
        path.set_reported(&[], later);
        path.poll(later, &Policy { wanted: false, ..policy(&[]) });
        assert!(path.candidates.is_empty());
        assert!(!path.learn(addr("0.0.0.0:5"), later) && !path.learn(addr("203.0.113.7:0"), later), "没法发的地址不记");
    }
}
