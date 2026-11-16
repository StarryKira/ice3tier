//! 节点一侧的中转会话：握手、保活、测延迟。帧的格式见 [`i3t_proto::session`]。
//!
//! 一台不做 I/O 的状态机，UDP 和 TCP 上各用一份：外面告诉它时间和 relay 发来了什么，它回答该发什么、会话还在不在。
//! 签名在外面做：它只说"该发 AUTH 了"，OK 的签名由外面验过才交进来。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use i3t_proto::session::{Accepted, Challenge, Cookie, Nonce, Pong};

use crate::timing::{HANDSHAKE_RETRY, HANDSHAKE_TRIES, MAX_TIMEOUTS, PROBE_INTERVAL, Rtt, SILENCE};

/// 要外面去发的帧。节点编号和签名由外面填。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Send {
    Init { nonce: Nonce },
    Auth { nonce: Nonce, cookie: Cookie },
    Ping { nonce: Nonce },
}

/// 会话的状态变了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// 握手成功。`observed` 是 relay 看到的本节点地址。
    Up { observed: SocketAddr },
    /// 会话没了，或者握手失败。
    Down { reason: &'static str },
    /// 收到了 CHALLENGE，但握手被要求停在这里（见 [`Session::start`] 的 `hold`）。
    Held,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Step {
    pub(crate) send: Option<Send>,
    pub(crate) event: Option<Event>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    AwaitChallenge {
        nonce: Nonce,
        sent: Instant,
        tries: u8,
        hold: bool,
    },
    /// 有 cookie 了，等外面说可以继续。
    Held {
        nonce: Nonce,
        cookie: Cookie,
    },
    AwaitOk {
        nonce: Nonce,
        cookie: Cookie,
        sent: Instant,
        tries: u8,
    },
    Up {
        last_ping: Instant,
        awaiting: Option<(Nonce, Instant)>,
        missed: u8,
    },
}

/// 到一个 relay、走一种传输（UDP 或 TCP）的中转会话。
#[derive(Debug)]
pub(crate) struct Session {
    state: State,
    rtt: Option<Rtt>,
}

impl Session {
    pub(crate) const fn new() -> Self {
        Self { state: State::Idle, rtt: None }
    }

    pub(crate) const fn is_up(&self) -> bool {
        matches!(self.state, State::Up { .. })
    }

    /// 没有在握手，也没有会话。
    pub(crate) const fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    /// 会话可疑：有保活没有得到回答。
    pub(crate) const fn in_doubt(&self) -> bool {
        matches!(self.state, State::Up { missed: 1.., .. })
    }

    pub(crate) const fn rtt(&self) -> Option<Rtt> {
        self.rtt
    }

    /// 开始握手，返回要发的 INIT。
    ///
    /// `hold` 为真时拿到 cookie 就停下，等 [`release`](Self::release)：TCP 和 UDP 同时试的时候，TCP 这边先走到这一步
    /// 证明连得上，但不发 AUTH。因为 relay 上一个节点只有一个会话，后握手的顶掉先握手的，TCP 抢先的话包就会走 TCP。
    pub(crate) fn start(&mut self, now: Instant, hold: bool) -> Send {
        let nonce: Nonce = i3t_underlay::random_bytes();
        self.state = State::AwaitChallenge { nonce, sent: now, tries: 1, hold };
        Send::Init { nonce }
    }

    /// 停在 cookie 那一步的握手可以继续了。返回要发的 AUTH；不在那一步时返回 `None`。
    pub(crate) fn release(&mut self, now: Instant) -> Option<Send> {
        let State::Held { nonce, cookie } = self.state else { return None };
        self.state = State::AwaitOk { nonce, cookie, sent: now, tries: 1 };
        Some(Send::Auth { nonce, cookie })
    }

    /// 放弃：不再握手，也不再保活。relay 那边的会话会自己过期。
    pub(crate) fn stop(&mut self) {
        self.state = State::Idle;
    }

    pub(crate) fn on_challenge(&mut self, challenge: &Challenge, now: Instant) -> Step {
        let State::AwaitChallenge { nonce, sent, tries, hold } = self.state else { return Step::default() };
        if challenge.nonce != nonce {
            return Step::default();
        }
        // 握手的第一个往返顺便当一个延迟样本。重发过的话分不清这是在回答哪一个 INIT，就不取样。
        if tries == 1 {
            self.sample(now.saturating_duration_since(sent));
        }
        if hold {
            self.state = State::Held { nonce, cookie: challenge.cookie };
            return Step { send: None, event: Some(Event::Held) };
        }
        self.state = State::AwaitOk { nonce, cookie: challenge.cookie, sent: now, tries: 1 };
        Step { send: Some(Send::Auth { nonce, cookie: challenge.cookie }), event: None }
    }

    /// 这个 OK 是不是在回答本会话发出去的 AUTH。是的话外面要先验它的签名，再交给 [`on_ok`](Self::on_ok)。
    pub(crate) fn expects_ok(&self, ok: &Accepted) -> bool {
        matches!(self.state, State::AwaitOk { nonce, .. } if nonce == ok.nonce)
    }

    /// 签名已经验过的 OK。
    pub(crate) fn on_ok(&mut self, ok: &Accepted, now: Instant) -> Step {
        let State::AwaitOk { nonce, sent, tries, .. } = self.state else { return Step::default() };
        if nonce != ok.nonce {
            return Step::default();
        }
        // 握手的第二个往返同样可以当延迟样本，重发过就不取。
        if tries == 1 {
            self.sample(now.saturating_duration_since(sent));
        }
        // 两步都重发过的话还不知道延迟：不等保活间隔，马上量一次。
        let last_ping = if self.rtt.is_some() { now } else { now.checked_sub(PROBE_INTERVAL).unwrap_or(now) };
        self.state = State::Up { last_ping, awaiting: None, missed: 0 };
        Step { send: None, event: Some(Event::Up { observed: ok.observed }) }
    }

    /// PONG。返回 relay 看到的本节点地址；随机数对不上时返回 `None`。
    pub(crate) fn on_pong(&mut self, pong: &Pong, now: Instant) -> Option<SocketAddr> {
        let State::Up { last_ping, awaiting: Some((nonce, sent)), .. } = self.state else { return None };
        if pong.nonce != nonce {
            return None;
        }
        self.sample(now.saturating_duration_since(sent));
        self.state = State::Up { last_ping, awaiting: None, missed: 0 };
        Some(pong.observed)
    }

    /// relay 说本节点在它那里没有会话。
    pub(crate) fn on_no_session(&mut self) -> Step {
        if !self.is_up() {
            return Step::default();
        }
        self.state = State::Idle;
        Step { send: None, event: Some(Event::Down { reason: "relay 说本节点没有会话" }) }
    }

    /// 推进定时器。`urgent`：有包正经这个 relay 发出去，却有一阵没有从它收到任何东西了。
    pub(crate) fn poll(&mut self, now: Instant, urgent: bool) -> Step {
        match self.state {
            State::Idle | State::Held { .. } => Step::default(),
            State::AwaitChallenge { nonce, sent, tries, hold } => match retry(sent, tries, now) {
                Retry::Wait => Step::default(),
                Retry::Again => {
                    self.state = State::AwaitChallenge { nonce, sent: now, tries: tries + 1, hold };
                    Step { send: Some(Send::Init { nonce }), event: None }
                }
                Retry::GiveUp => self.fail("握手没有回应"),
            },
            State::AwaitOk { nonce, cookie, sent, tries } => match retry(sent, tries, now) {
                Retry::Wait => Step::default(),
                Retry::Again => {
                    self.state = State::AwaitOk { nonce, cookie, sent: now, tries: tries + 1 };
                    Step { send: Some(Send::Auth { nonce, cookie }), event: None }
                }
                Retry::GiveUp => self.fail("握手的最后一步没有回应"),
            },
            State::Up { last_ping, awaiting, missed } => self.keep_alive(now, urgent, last_ping, awaiting, missed),
        }
    }

    fn keep_alive(&mut self, now: Instant, urgent: bool, last_ping: Instant, awaiting: Option<(Nonce, Instant)>, missed: u8) -> Step {
        let since_ping = now.saturating_duration_since(last_ping);
        let timeout = self.rtt.map_or(crate::timing::PROBE_TIMEOUT_MAX, Rtt::probe_timeout);
        let (due, missed) = match awaiting {
            Some((_, sent)) if now.saturating_duration_since(sent) < timeout => (false, missed),
            // 超时之后不等下一个保活间隔，立刻再问。
            Some(_) => (true, missed + 1),
            None => (since_ping >= PROBE_INTERVAL || (urgent && since_ping >= SILENCE), missed),
        };
        if missed >= MAX_TIMEOUTS {
            return self.fail("保活连续没有回应");
        }
        if !due {
            return Step::default();
        }
        let nonce: Nonce = i3t_underlay::random_bytes();
        self.state = State::Up { last_ping: now, awaiting: Some((nonce, now)), missed };
        Step { send: Some(Send::Ping { nonce }), event: None }
    }

    fn fail(&mut self, reason: &'static str) -> Step {
        self.state = State::Idle;
        Step { send: None, event: Some(Event::Down { reason }) }
    }

    fn sample(&mut self, sample: Duration) {
        match &mut self.rtt {
            Some(rtt) => rtt.update(sample),
            None => self.rtt = Some(Rtt::new(sample)),
        }
    }
}

enum Retry {
    Wait,
    Again,
    GiveUp,
}

fn retry(sent: Instant, tries: u8, now: Instant) -> Retry {
    if now.saturating_duration_since(sent) < HANDSHAKE_RETRY {
        Retry::Wait
    } else if tries < HANDSHAKE_TRIES {
        Retry::Again
    } else {
        Retry::GiveUp
    }
}

#[cfg(test)]
mod tests {
    use i3t_proto::session::{COOKIE_LEN, SIGNATURE_LEN};

    use super::*;

    const MS: Duration = Duration::from_millis(1);
    const COOKIE: Cookie = [7; COOKIE_LEN];

    fn observed() -> SocketAddr {
        "203.0.113.7:4000".parse().unwrap()
    }

    fn ok(nonce: Nonce) -> Accepted {
        Accepted { nonce, valid_for: 30, observed: observed(), signature: [0; SIGNATURE_LEN] }
    }

    /// 握手成功的会话，返回它和成功的时刻。
    fn established() -> (Session, Instant) {
        let start = Instant::now();
        let mut session = Session::new();
        let Send::Init { nonce } = session.start(start, false) else { panic!("握手从 INIT 开始") };
        session.on_challenge(&Challenge { nonce, cookie: COOKIE }, start + 20 * MS);
        let up = start + 40 * MS;
        assert_eq!(session.on_ok(&ok(nonce), up).event, Some(Event::Up { observed: observed() }));
        (session, up)
    }

    #[test]
    fn the_handshake_goes_init_challenge_auth_ok() {
        let start = Instant::now();
        let mut session = Session::new();
        assert!(session.is_idle());
        let Send::Init { nonce } = session.start(start, false) else { panic!("握手从 INIT 开始") };
        assert!(!session.is_idle() && !session.is_up());

        assert_eq!(
            session.on_challenge(&Challenge { nonce: [0; 8], cookie: COOKIE }, start + 10 * MS),
            Step::default(),
            "随机数不对的 CHALLENGE 不理"
        );
        let step = session.on_challenge(&Challenge { nonce, cookie: COOKIE }, start + 20 * MS);
        assert_eq!(step, Step { send: Some(Send::Auth { nonce, cookie: COOKIE }), event: None });
        assert_eq!(session.rtt().map(Rtt::get), Some(20 * MS), "握手的第一个往返就是一个延迟样本");

        assert!(!session.expects_ok(&ok([0; 8])));
        assert_eq!(session.on_ok(&ok([0; 8]), start + 30 * MS), Step::default());
        assert!(session.expects_ok(&ok(nonce)));
        assert_eq!(session.on_ok(&ok(nonce), start + 40 * MS), Step { send: None, event: Some(Event::Up { observed: observed() }) });
        assert!(session.is_up() && !session.in_doubt());
        assert_eq!(session.on_ok(&ok(nonce), start + 50 * MS), Step::default(), "同一个 OK 不算两次");
    }

    #[test]
    fn each_handshake_step_is_retried_and_then_given_up() {
        let start = Instant::now();
        let mut session = Session::new();
        let Send::Init { nonce } = session.start(start, false) else { panic!("握手从 INIT 开始") };
        assert_eq!(session.poll(start + HANDSHAKE_RETRY - MS, false), Step::default());
        let mut now = start;
        for _ in 1..HANDSHAKE_TRIES {
            now += HANDSHAKE_RETRY;
            assert_eq!(session.poll(now, false).send, Some(Send::Init { nonce }), "重发用同一个随机数，哪个 CHALLENGE 回来都对得上");
        }
        now += HANDSHAKE_RETRY;
        assert_eq!(session.poll(now, false).event, Some(Event::Down { reason: "握手没有回应" }));
        assert!(session.is_idle());

        // 第二步同样。
        let Send::Init { nonce } = session.start(now, false) else { panic!("握手从 INIT 开始") };
        session.on_challenge(&Challenge { nonce, cookie: COOKIE }, now);
        for _ in 1..HANDSHAKE_TRIES {
            now += HANDSHAKE_RETRY;
            assert_eq!(session.poll(now, false).send, Some(Send::Auth { nonce, cookie: COOKIE }));
        }
        now += HANDSHAKE_RETRY;
        assert!(matches!(session.poll(now, false).event, Some(Event::Down { .. })));
    }

    #[test]
    fn a_retransmitted_init_does_not_yield_a_misleading_rtt_sample() {
        let start = Instant::now();
        let mut session = Session::new();
        let Send::Init { nonce } = session.start(start, false) else { panic!("握手从 INIT 开始") };
        session.poll(start + HANDSHAKE_RETRY, false);
        // 这个 CHALLENGE 可能是回答第一个 INIT 的，也可能是第二个的，算不出往返时间。
        let step = session.on_challenge(&Challenge { nonce, cookie: COOKIE }, start + HANDSHAKE_RETRY + 30 * MS);
        assert!(step.send.is_some(), "握手照常继续");
        assert_eq!(session.rtt(), None);

        // 第二步也重发了：握手完成时还是不知道延迟，所以马上发一个保活去量。
        let resent = start + 2 * HANDSHAKE_RETRY + 30 * MS;
        assert!(matches!(session.poll(resent, false).send, Some(Send::Auth { .. })));
        assert!(session.on_ok(&ok(nonce), resent + 10 * MS).event.is_some());
        assert_eq!(session.rtt(), None);
        let Some(Send::Ping { nonce }) = session.poll(resent + 11 * MS, false).send else { panic!("应该马上量一次延迟") };
        session.on_pong(&Pong { nonce, observed: observed() }, resent + 31 * MS);
        assert_eq!(session.rtt().map(Rtt::get), Some(20 * MS));
    }

    #[test]
    fn a_held_handshake_stops_at_the_cookie_until_released() {
        let start = Instant::now();
        let mut session = Session::new();
        let Send::Init { nonce } = session.start(start, true) else { panic!("握手从 INIT 开始") };
        let step = session.on_challenge(&Challenge { nonce, cookie: COOKIE }, start + 20 * MS);
        assert_eq!(step, Step { send: None, event: Some(Event::Held) });
        assert!(!session.is_idle() && !session.is_up());
        assert_eq!(session.poll(start + Duration::from_secs(60), false), Step::default(), "停着的时候什么都不发，也不超时");

        assert_eq!(session.release(start + Duration::from_secs(61)), Some(Send::Auth { nonce, cookie: COOKIE }));
        assert_eq!(session.release(start + Duration::from_secs(61)), None);
        assert!(session.on_ok(&ok(nonce), start + Duration::from_secs(61) + 20 * MS).event.is_some());
    }

    #[test]
    fn the_session_is_pinged_every_five_seconds_and_pong_updates_rtt_and_observed_address() {
        let (mut session, up) = established();
        assert_eq!(session.poll(up + PROBE_INTERVAL - MS, false), Step::default());
        let Some(Send::Ping { nonce }) = session.poll(up + PROBE_INTERVAL, false).send else { panic!("该发保活了") };

        let moved: SocketAddr = "203.0.113.7:4001".parse().unwrap();
        assert_eq!(session.on_pong(&Pong { nonce: [0; 8], observed: moved }, up + PROBE_INTERVAL + 5 * MS), None, "随机数不对");
        assert_eq!(session.on_pong(&Pong { nonce, observed: moved }, up + PROBE_INTERVAL + 60 * MS), Some(moved));
        assert_eq!(session.on_pong(&Pong { nonce, observed: moved }, up + PROBE_INTERVAL + 70 * MS), None, "同一个 PONG 不算两次");
        assert_eq!(session.rtt().map(Rtt::get), Some(25 * MS), "握手的两个往返都是 20ms，再加上一个 60ms 的样本");
    }

    #[test]
    fn three_unanswered_pings_take_the_session_down() {
        let (mut session, up) = established();
        let timeout = session.rtt().unwrap().probe_timeout();
        let mut now = up + PROBE_INTERVAL;
        assert!(session.poll(now, false).send.is_some());
        for _ in 1..MAX_TIMEOUTS {
            now += timeout;
            let step = session.poll(now, false);
            assert!(matches!(step, Step { send: Some(Send::Ping { .. }), event: None }), "超时之后立刻再问：{step:?}");
            assert!(session.in_doubt());
        }
        now += timeout;
        assert_eq!(session.poll(now, false).event, Some(Event::Down { reason: "保活连续没有回应" }));
        assert!(session.is_idle());
    }

    #[test]
    fn an_answer_clears_the_missed_count() {
        let (mut session, up) = established();
        let timeout = session.rtt().unwrap().probe_timeout();
        session.poll(up + PROBE_INTERVAL, false);
        let Some(Send::Ping { nonce }) = session.poll(up + PROBE_INTERVAL + timeout, false).send else { panic!("该重发保活了") };
        assert!(session.in_doubt());
        assert!(session.on_pong(&Pong { nonce, observed: observed() }, up + PROBE_INTERVAL + timeout + 5 * MS).is_some());
        assert!(session.is_up() && !session.in_doubt());
    }

    #[test]
    fn silence_while_traffic_flows_brings_the_next_ping_forward() {
        let (mut session, up) = established();
        assert_eq!(session.poll(up + SILENCE - MS, true), Step::default());
        assert!(matches!(session.poll(up + SILENCE, true).send, Some(Send::Ping { .. })));
    }

    #[test]
    fn a_refusal_from_the_relay_ends_the_session_so_that_it_can_be_rebuilt() {
        let (mut session, _up) = established();
        assert_eq!(session.on_no_session().event, Some(Event::Down { reason: "relay 说本节点没有会话" }));
        assert!(session.is_idle());
        assert_eq!(session.on_no_session(), Step::default(), "没有会话时这种帧没有意义，伪造的也触发不了什么");
    }

    #[test]
    fn stopping_makes_the_session_forget_everything_but_its_rtt() {
        let (mut session, up) = established();
        session.stop();
        assert!(session.is_idle());
        assert_eq!(session.poll(up + Duration::from_secs(60), true), Step::default());
        assert!(session.rtt().is_some());
    }
}
