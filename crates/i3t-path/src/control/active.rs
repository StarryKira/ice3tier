//! 控制任务里主动测试（白皮书 §7.6）的部分：时间窗、回答和发出测试的探测、暂时固定路、直连尝试、两种模式的选择。
//! 规矩见 [`crate::diag`]。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use i3t_crypto::Signature;
use i3t_proto::NodeId;
use i3t_proto::probe::{TestId, TestPing, TestPong};
use tokio::sync::oneshot;

use super::route::{RouteKey, View};
use super::{Attempt, Control, PeerState};
use crate::diag::{
    Admission, ChoiceReason, DirectAttempt, DirectFailure, HopReport, HopSource, MAX_PIN, ModeChoice, Pin, RouteDecision, Side, TestPath,
    TestSpec, TrainError, TrainReport, TrainRun, TrainSpec, Window,
};
use crate::relays::Leg;
use crate::report::{CandidateReport, RelayLeg};
use crate::routing::{Candidate, PathId};
use crate::table::{Hop, send_frame_via};

impl Control {
    /// 选路时看到的此刻的状态。`entries` 是 [`Control::entries`] 算出来的；只用来把路径落实成收发做法的时候可以是空的。
    fn view<'a>(&'a self, entries: &'a [crate::routing::Entry], now: Instant) -> View<'a> {
        View {
            shared: &self.shared,
            relays: &self.relays,
            legs: &self.legs,
            matrix: &self.matrix,
            broken: &self.broken,
            entries,
            allow_direct: self.allow_direct,
            allow_relay: self.allow_relay,
            max_hops: self.max_hops,
            now,
        }
    }

    pub(super) fn open_test(&mut self, spec: &TestSpec, now: Instant) {
        if !self.peers.contains_key(&spec.peer) {
            tracing::info!(test = spec.test, peer = %spec.peer, "不打开主动测试：对端不在 netmap 里");
            return;
        }
        let window = Window::new(spec, now);
        tracing::info!(test = spec.test, peer = %spec.peer, window_secs = window.until.saturating_duration_since(now).as_secs(), relays = ?spec.relays, "打开主动测试的时间窗");
        self.windows.insert(spec.test, window);
        self.sync_open_tests();
    }

    pub(super) fn close_test(&mut self, test: TestId, why: &'static str) {
        if self.windows.remove(&test).is_none() {
            return;
        }
        tracing::info!(test, why, "关闭主动测试的时间窗");
        self.sync_open_tests();
        // 测试没了，它的探测包列也就到此为止：已经发出去的照样算，没发的不发了。
        let now = Instant::now();
        let (ended, running): (Vec<TrainRun>, Vec<TrainRun>) =
            std::mem::take(&mut self.trains).into_iter().partition(|train| train.spec.test == test);
        self.trains = running;
        for mut train in ended {
            if let Some(reply) = train.reply.take() {
                let _waiting = reply.send(Ok(train.report(now)));
            }
        }
    }

    fn sync_open_tests(&self) {
        let open = self.windows.iter().map(|(test, window)| (*test, window.peer)).collect();
        *self.shared.open_tests.write().expect("open tests lock poisoned") = open;
    }

    /// 定时要做的：过了时间窗的测试关掉；带头的一方测试期间把对端算作活跃的（找路、探测直连、为它保持中转会话都照常进行）；
    /// 直连尝试有结果了就回答。
    pub(super) fn poll_tests(&mut self, now: Instant) {
        let expired: Vec<TestId> = self.windows.iter().filter(|(_, window)| !window.is_open(now)).map(|(test, _)| *test).collect();
        for test in expired {
            self.close_test(test, "时间窗到了");
        }
        let clock = &self.shared.clock;
        for window in self.windows.values().filter(|window| window.side == Side::Lead) {
            if let Some(peer) = self.peers.get(&window.peer) {
                peer.slot.last_demand.touch(clock);
            }
        }
        let attempts = std::mem::take(&mut self.attempts);
        for attempt in attempts {
            let Some(peer) = self.peers.get(&attempt.peer) else {
                let _waiting = attempt.reply.send(self.failed_attempt(None, DirectFailure::NotPeer));
                continue;
            };
            if let Some(established) = peer.direct.established_at().filter(|_| peer.direct.active().is_some()) {
                let elapsed = established.saturating_duration_since(attempt.started);
                tracing::info!(peer = %attempt.peer, elapsed_ms = elapsed.as_millis(), kind = ?peer.direct.kind(), "主动测试的直连尝试成功");
                let result = DirectAttempt {
                    ok: true,
                    already: false,
                    kind: peer.direct.kind(),
                    elapsed: Some(elapsed),
                    failure: None,
                    udp: self.udp_state(now),
                    mapping_varies: self.mapping_varies,
                    candidates: peer.direct.candidate_count(),
                };
                let _waiting = attempt.reply.send(result);
            } else if now >= attempt.deadline || attempt.reply.is_closed() {
                let failure = self.direct_failure(peer, now);
                tracing::info!(peer = %attempt.peer, ?failure, "主动测试的直连尝试没有成功");
                let _waiting = attempt.reply.send(self.failed_attempt(Some(peer), failure));
            } else {
                if attempt.side == Side::Lead {
                    peer.slot.last_demand.touch(clock);
                }
                self.attempts.push(attempt);
            }
        }
    }

    fn failed_attempt(&self, peer: Option<&PeerState>, failure: DirectFailure) -> DirectAttempt {
        DirectAttempt {
            ok: false,
            already: false,
            kind: None,
            elapsed: None,
            failure: Some(failure),
            udp: self.udp_state(Instant::now()),
            mapping_varies: self.mapping_varies,
            candidates: peer.map_or(0, |peer| peer.direct.candidate_count()),
        }
    }

    /// 本节点的 UDP 通不通：到某个 relay 走着 UDP、或者地址查询有回答就是通的；到 relay 都只能走 TCP 就是不通的；
    /// 两样都没有时不知道。
    fn udp_state(&self, now: Instant) -> Option<bool> {
        let legs: Vec<Leg> = self.relays.values().filter_map(|entry| entry.link.leg()).collect();
        if legs.iter().any(|leg| matches!(leg, Leg::Udp(_))) || self.addr_query.observations(now).next().is_some() {
            return Some(true);
        }
        (!legs.is_empty()).then_some(false)
    }

    /// 直连没有建起来，从本节点这一侧看最可能的原因。
    fn direct_failure(&self, peer: &PeerState, now: Instant) -> DirectFailure {
        if self.dial_out_only && peer.dial_out_only {
            DirectFailure::DialOutOnly
        } else if self.udp_state(now) == Some(false) {
            DirectFailure::UdpUnavailable
        } else if self.mapping_varies == Some(true) {
            DirectFailure::MappingVaries
        } else if peer.direct.candidate_count() == 0 {
            DirectFailure::NoCandidates
        } else {
            DirectFailure::Timeout
        }
    }

    /// 试一次直连，最多等 `limit`；已经有了就当场回答。带头时没有可用的直连就不等退避、从头尝试，并把对端当作活跃的；
    /// 跟随时只是等着看对端的尝试有没有让直连通起来（见 [`Side`]）。
    pub(super) fn attempt_direct(&mut self, id: NodeId, side: Side, limit: Duration, reply: oneshot::Sender<DirectAttempt>, now: Instant) {
        if !self.allow_direct {
            tracing::info!(peer = %id, "不做直连尝试：本节点的路径限制是只经 relay");
            let _waiting = reply.send(self.failed_attempt(None, DirectFailure::RelayOnly));
            return;
        }
        let Some(peer) = self.peers.get_mut(&id) else {
            let _waiting = reply.send(self.failed_attempt(None, DirectFailure::NotPeer));
            return;
        };
        if side == Side::Lead {
            peer.slot.last_demand.touch(&self.shared.clock);
        }
        if peer.direct.active().is_some() {
            let result = DirectAttempt {
                ok: true,
                already: true,
                kind: peer.direct.kind(),
                elapsed: None,
                failure: None,
                udp: None,
                mapping_varies: self.mapping_varies,
                candidates: peer.direct.candidate_count(),
            };
            let result = DirectAttempt { udp: self.udp_state(now), ..result };
            tracing::info!(peer = %id, kind = ?result.kind, "主动测试的直连尝试：已经有可用的直连");
            let _waiting = reply.send(result);
            return;
        }
        match side {
            Side::Lead => {
                tracing::info!(peer = %id, limit_ms = limit.as_millis(), "主动测试：现在就试一次直连，不等退避");
                peer.direct.retry_now();
            }
            Side::Follow => tracing::info!(peer = %id, limit_ms = limit.as_millis(), "主动测试：等对端来试直连"),
        }
        self.attempts.push(Attempt { peer: id, side, started: now, deadline: now + limit, reply });
    }

    pub(super) fn pin(&mut self, id: NodeId, pin: Pin, now: Instant) {
        let Some(peer) = self.peers.get_mut(&id) else { return };
        let pin = Pin { until: pin.until.min(now + MAX_PIN), ..pin };
        tracing::info!(peer = %id, path = ?pin.path, secs = pin.until.saturating_duration_since(now).as_secs_f32(), "主动测试暂时固定到对端的路");
        peer.pin = Some(pin);
    }

    pub(super) fn unpin(&mut self, id: NodeId, pin: u64) {
        let Some(peer) = self.peers.get_mut(&id) else { return };
        if peer.pin.is_some_and(|current| current.id == pin) {
            tracing::info!(peer = %id, "主动测试解除固定的路");
            peer.pin = None;
        }
    }

    /// 发给 `id` 的测试探测走 `path` 时此刻的收发做法；走不了是 `None`。
    fn test_hop(&self, id: NodeId, path: TestPath, now: Instant) -> Option<Hop> {
        let peer = self.peers.get(&id)?;
        match path {
            TestPath::Direct => self.allow_direct.then(|| peer.direct.active()).flatten().map(|(addr, _)| Hop::Direct(addr)),
            TestPath::Relays(relays) => self.view(&[], now).resolve(id, peer, PathId::Relays(relays)).map(|(_, hop)| hop),
        }
    }

    /// 对端的测试探测：便宜的检查都过了才验签名，然后从同一条路径倒着回答。
    pub(super) fn on_test_ping(&mut self, from: SocketAddr, ping: &TestPing, at: Instant, relayed: bool, now: Instant) {
        if ping.dst != self.shared.me {
            return;
        }
        let path = TestPath::from_relays(ping.path);
        let Some(window) = self.windows.get(&ping.test) else { return };
        if let Err(why) = window.screen(ping.src, ping.seq, path, now) {
            tracing::debug!(test = ping.test, peer = %ping.src, seq = ping.seq, why, "不回答这个测试探测");
            return;
        }
        // 说是直连的探测必须是直连来的，反过来也一样；本节点不建立直连时不回答直连的探测，免得把地址告诉对端。
        if (path == TestPath::Direct) == relayed || (path == TestPath::Direct && !self.allow_direct) {
            tracing::debug!(test = ping.test, peer = %ping.src, relayed, "不回答这个测试探测：它来的路和它说的对不上，或者本节点不建立直连");
            return;
        }
        let Some(peer) = self.peers.get(&ping.src) else { return };
        let message = TestPing::signed_message(ping.src, ping.dst, ping.test, ping.seq, ping.path);
        if i3t_crypto::verify(&peer.key, TestPing::PURPOSE, &message, &Signature::from_bytes(ping.signature)).is_err() {
            tracing::debug!(test = ping.test, peer = %ping.src, "测试探测的签名不对，丢弃");
            return;
        }
        let hop = match path {
            TestPath::Direct => Some(Hop::Direct(from)),
            TestPath::Relays(_) => self.test_hop(ping.src, path.reversed(), now),
        };
        let Some(window) = self.windows.get_mut(&ping.test) else { return };
        let Admission::Answer { received } = window.admit(ping.seq, path) else { return };
        let held = Instant::now().saturating_duration_since(at);
        let mut pong = TestPong {
            src: self.shared.me,
            dst: ping.src,
            test: ping.test,
            seq: ping.seq,
            held_us: u32::try_from(held.as_micros()).unwrap_or(u32::MAX),
            received,
            same_path: hop.is_some(),
            signature: [0; 64],
        };
        pong.signature = *self.key.sign(TestPong::PURPOSE, &pong.signed_message()).as_bytes();
        let bytes = pong.to_bytes();
        match hop {
            Some(hop) => send_frame_via(&self.shared, ping.src, &hop, &bytes),
            // 倒过来的路径走不了：从本节点自己现在的路回答，回答里说明了。
            None => peer.slot.send_frame(&self.shared, &bytes),
        }
    }

    /// 本节点的一列探测得到了回答。
    pub(super) fn on_test_pong(&mut self, pong: &TestPong, at: Instant) {
        if pong.dst != self.shared.me {
            return;
        }
        let Some(train) = self.trains.iter().position(|train| train.spec.test == pong.test && train.spec.peer == pong.src) else { return };
        let Some(index) = self.trains[train].index_of(pong.seq) else { return };
        let Some(peer) = self.peers.get(&pong.src) else { return };
        if i3t_crypto::verify(&peer.key, TestPong::PURPOSE, &pong.signed_message(), &Signature::from_bytes(pong.signature)).is_err() {
            tracing::debug!(test = pong.test, peer = %pong.src, "测试探测的回答签名不对，丢弃");
            return;
        }
        let held = Duration::from_micros(u64::from(pong.held_us));
        self.trains[train].answered(index, at, held, pong.received, pong.same_path);
    }

    pub(super) fn start_train(&mut self, spec: TrainSpec, reply: oneshot::Sender<Result<TrainReport, TrainError>>, now: Instant) {
        let refused = if !self.peers.contains_key(&spec.peer) {
            Some(TrainError::NotPeer)
        } else if !self.windows.get(&spec.test).is_some_and(|window| window.is_open(now) && window.peer == spec.peer) {
            Some(TrainError::NoWindow)
        } else if spec.path == TestPath::Direct && !self.allow_direct {
            Some(TrainError::DirectDisabled)
        } else if spec.path != TestPath::Direct && !self.allow_relay {
            Some(TrainError::RelayDisabled)
        } else if self.test_hop(spec.peer, spec.path, now).is_none() {
            Some(TrainError::Unreachable)
        } else {
            None
        };
        if let Some(error) = refused {
            tracing::info!(test = spec.test, peer = %spec.peer, path = ?spec.path, %error, "不发这列测试探测");
            let _waiting = reply.send(Err(error));
            return;
        }
        let first_seq = self.windows.get_mut(&spec.test).map_or(0, |window| window.take_seqs(spec.count));
        let mut train = TrainRun::new(spec, first_seq, now, reply);
        train.relay_leg = match spec.path {
            TestPath::Direct => None,
            TestPath::Relays(relays) => self.relays.get(&relays.first()).and_then(|entry| entry.link.leg()).map(|leg| match leg {
                Leg::Udp(_) => RelayLeg::Udp,
                Leg::Tcp => RelayLeg::Tcp,
            }),
        };
        train.peer_addr = self
            .peers
            .get(&spec.peer)
            .and_then(|peer| peer.direct.active())
            .map(|(addr, _)| addr)
            .filter(|_| spec.path == TestPath::Direct);
        tracing::info!(test = spec.test, peer = %spec.peer, path = ?spec.path, count = train.spec.count, interval_ms = train.spec.interval.as_millis(), "开始发一列测试探测");
        self.trains.push(train);
        self.poll_trains(now);
    }

    /// 发到点了的测试探测，回答等完了的列交出结果。
    pub(super) fn poll_trains(&mut self, now: Instant) {
        let mut trains = std::mem::take(&mut self.trains);
        for train in &mut trains {
            while let Some((index, _)) = train.next_due().filter(|(_, due)| *due <= now) {
                let seq = train.first_seq.saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
                let (test, peer, path) = (train.spec.test, train.spec.peer, train.spec.path);
                // 发的时候走不了（路刚断）也记成发过了：它算作丢失，这正是测试要量的。
                if let Some(hop) = self.test_hop(peer, path, now) {
                    let message = TestPing::signed_message(self.shared.me, peer, test, seq, path.relays());
                    let signature = *self.key.sign(TestPing::PURPOSE, &message).as_bytes();
                    let ping = TestPing { src: self.shared.me, dst: peer, test, seq, path: path.relays(), signature };
                    let bytes = ping.to_bytes();
                    let sent = Instant::now();
                    send_frame_via(&self.shared, peer, &hop, &bytes);
                    train.sent_at[index] = Some(sent);
                } else {
                    train.sent_at[index] = Some(now);
                }
            }
        }
        for mut train in trains {
            if train.is_finished(now) || train.reply.as_ref().is_none_or(oneshot::Sender::is_closed) {
                let report = train.report(now);
                let summary = report.summary();
                tracing::info!(
                    test = train.spec.test,
                    peer = %train.spec.peer,
                    path = ?train.spec.path,
                    sent = summary.sent,
                    answered = summary.answered,
                    avg_us = summary.avg_us,
                    "一列测试探测结束"
                );
                if let Some(reply) = train.reply.take() {
                    let _waiting = reply.send(Ok(report));
                }
            } else {
                self.trains.push(train);
            }
        }
    }

    /// 两种选路模式此刻各会选哪条路，以及 relay 质量默认要测的两条路径。对端不在 netmap 里时是 `None`。
    pub(super) fn decide(&self, id: NodeId, now: Instant) -> Option<RouteDecision> {
        let peer = self.peers.get(&id)?;
        let entries = self.entries(now);
        let view = self.view(&entries, now);
        let mut candidates = view.candidates(id, peer);
        candidates.retain(|candidate| view.resolve(id, peer, candidate.path).is_some());
        let cost_of = |path: PathId| candidates.iter().find(|candidate| candidate.path == path).map(|candidate| candidate.cost_ms);
        let me = self.shared.me;

        let direct_first = match view.direct_first(id, peer) {
            Some(path @ PathId::Direct(_)) => {
                let reason = if peer.direct.is_lan_active() { ChoiceReason::Lan } else { ChoiceReason::Direct };
                ModeChoice { route: route_of(path), reason, cost_ms: cost_of(path) }
            }
            Some(path @ PathId::Relays(relays)) => {
                let reason = match relays.first() {
                    first if first == id => ChoiceReason::PeerIsRelay,
                    first if first == me => ChoiceReason::OwnRelay,
                    _ => ChoiceReason::PeerRelay,
                };
                ModeChoice { route: route_of(path), reason, cost_ms: cost_of(path) }
            }
            None => ModeChoice { route: i3t_model::control::Route::None, reason: ChoiceReason::NoPath, cost_ms: None },
        };
        let lowest_cost = match candidates.iter().find(|candidate| candidate.lan).or_else(|| candidates.first()) {
            Some(candidate) => ModeChoice {
                route: route_of(candidate.path),
                reason: if candidate.lan { ChoiceReason::Lan } else { ChoiceReason::LowestCost },
                cost_ms: Some(candidate.cost_ms),
            },
            None => ModeChoice { route: i3t_model::control::Route::None, reason: ChoiceReason::NoPath, cost_ms: None },
        };
        // 不算直连时默认模式会经的 relay：同一套规则，只是假装直连不可用。
        let without_direct = View { allow_direct: false, ..self.view(&entries, now) };
        let default_relay = match without_direct.direct_first(id, peer) {
            Some(PathId::Relays(relays)) => Some(relays),
            Some(PathId::Direct(_)) | None => None,
        };
        let cheapest_relay = candidates.iter().find_map(|candidate| match candidate.path {
            PathId::Relays(relays) => Some(relays),
            PathId::Direct(_) => None,
        });
        Some(RouteDecision {
            mode: self.mode,
            current: peer.route.to_route(),
            direct_first,
            lowest_cost,
            default_relay,
            cheapest_relay,
            candidates: candidates.iter().map(|candidate| candidate_report(candidate, peer.route)).collect(),
        })
    }

    /// 一条路径上各段的测量值：本节点到第一个 relay 是自己测的，relay 之间来自骨干矩阵，最后一个 relay 到对端是对端上报的。
    pub(super) fn hops(&self, id: NodeId, path: TestPath, now: Instant) -> Vec<HopReport> {
        let me = self.shared.me;
        let Some(peer) = self.peers.get(&id) else { return Vec::new() };
        let unknown = |from, to| HopReport { from, to, rtt_ms: None, jitter_ms: None, loss: None, source: HopSource::Unknown };
        let measured = |from, to, quality: Option<crate::estimate::Quality>| match quality {
            Some(quality) => {
                let segment = quality.segment();
                let loss = (quality.probes > 0).then_some(segment.loss);
                HopReport { from, to, rtt_ms: Some(segment.rtt_ms), jitter_ms: Some(segment.jitter_ms), loss, source: HopSource::Measured }
            }
            None => unknown(from, to),
        };
        let relays = match path {
            TestPath::Direct => return vec![measured(me, id, peer.direct.quality(now))],
            TestPath::Relays(relays) => relays,
        };
        let list = relays.as_slice();
        let mut hops = Vec::with_capacity(list.len() + 1);
        if let Some(&first) = list.first().filter(|first| **first != me) {
            hops.push(measured(me, first, self.relays.get(&first).and_then(|entry| entry.link.quality(now))));
        }
        for pair in list.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let link = self.matrix.links_from(from, now).find(|(next, _)| *next == to).map(|(_, quality)| quality);
            hops.push(match link {
                Some(quality) => HopReport {
                    from,
                    to,
                    rtt_ms: Some(f64::from(quality.rtt_ms)),
                    jitter_ms: Some(f64::from(quality.jitter_ms)),
                    loss: Some(quality.loss()),
                    source: HopSource::Backbone,
                },
                None => unknown(from, to),
            });
        }
        if let Some(&last) = list.last().filter(|last| **last != id) {
            let reported = peer.attached.iter().find(|attachment| attachment.relay == last);
            hops.push(match reported {
                Some(attachment) => HopReport {
                    from: last,
                    to: id,
                    rtt_ms: Some(f64::from(attachment.rtt_ms)),
                    jitter_ms: None,
                    loss: None,
                    source: HopSource::Reported,
                },
                None => unknown(last, id),
            });
        }
        hops
    }
}

fn route_of(path: PathId) -> i3t_model::control::Route {
    match path {
        PathId::Direct(addr) => i3t_model::control::Route::Direct { addr },
        PathId::Relays(relays) => i3t_model::control::Route::Relay { relays: relays.as_slice().to_vec() },
    }
}

fn candidate_report(candidate: &Candidate, current: RouteKey) -> CandidateReport {
    let current = match current {
        RouteKey::None => None,
        RouteKey::Direct(addr) => Some(PathId::Direct(addr)),
        RouteKey::Relay { path, .. } => Some(PathId::Relays(path)),
    };
    CandidateReport {
        route: route_of(candidate.path),
        cost_ms: candidate.cost_ms,
        legs: candidate
            .legs
            .iter()
            .map(|leg| crate::report::LegReport {
                from: leg.from,
                to: leg.to,
                rtt_ms: leg.segment.rtt_ms,
                jitter_ms: leg.segment.jitter_ms,
                loss: leg.segment.loss,
            })
            .collect(),
        load_percent: candidate.load_percent,
        current: Some(candidate.path) == current,
        relay_leg: None,
    }
}
