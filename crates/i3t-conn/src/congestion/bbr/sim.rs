//! 测试用的网络模拟：一条有带宽、延迟、缓冲和随机丢包的链路，上面跑一条或几条连接，时钟是模拟的。
//!
//! 发送方照着 QUIC 库的行为写：在途量不到窗口才发；发送节奏是令牌桶，速率取"窗口 ÷ 平滑往返时间 × 1.25"；
//! 比已确认的包早 3 个序号、或者早 9/8 个往返时间的包判为丢失；长时间没有确认时发一个探测包。
//! 接收方每收到一个包就确认，确认在回程上只有延迟。丢掉的包不重传：这里只关心拥塞控制放出去多少、送到多少。

// clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助代码。
#![allow(clippy::unwrap_used)]

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::time::{Duration, Instant};

use super::{Bbr, State};

pub(super) const MTU: u64 = 1200;
/// 和 QUIC 库相同：拿到第一个样本之前假定的往返时间。
const INITIAL_RTT: Duration = Duration::from_millis(333);
/// 统计按这个粒度分桶。
const BUCKET: Duration = Duration::from_millis(100);

/// 模拟器眼里的拥塞控制器。库的接口里有一个参数的类型在库外建不出来，所以测试不经过库的接口。
pub(super) trait Cc {
    fn sent(&mut self, now: Instant, bytes: u64);
    fn acked(&mut self, now: Instant, sent: Instant, bytes: u64);
    fn end_acks(&mut self, now: Instant, in_flight: u64, app_limited: bool);
    fn lost(&mut self, now: Instant, sent: Instant, bytes: u64);
    fn window(&self) -> u64;
    fn as_bbr(&self) -> Option<&Bbr> {
        None
    }
}

impl Cc for Bbr {
    fn sent(&mut self, now: Instant, bytes: u64) {
        Self::sent(self, now, bytes);
    }

    fn acked(&mut self, now: Instant, sent: Instant, bytes: u64) {
        Self::acked(self, now, sent, bytes);
    }

    fn end_acks(&mut self, now: Instant, in_flight: u64, app_limited: bool) {
        Self::end_acks(self, now, in_flight, app_limited);
    }

    fn lost(&mut self, now: Instant, sent: Instant, bytes: u64) {
        Self::lost(self, now, sent, false, bytes);
    }

    fn window(&self) -> u64 {
        self.window
    }

    fn as_bbr(&self) -> Option<&Bbr> {
        Some(self)
    }
}

/// 把每次丢包都当作拥塞的对照组：慢启动，之后每个往返时间加一个包，丢包减半。
pub(super) struct Reno {
    window: u64,
    ssthresh: u64,
    recovery_start: Option<Instant>,
}

impl Reno {
    pub(super) fn new() -> Self {
        Self { window: 10 * MTU, ssthresh: u64::MAX, recovery_start: None }
    }
}

impl Cc for Reno {
    fn sent(&mut self, _now: Instant, _bytes: u64) {}

    fn acked(&mut self, _now: Instant, sent: Instant, bytes: u64) {
        if self.recovery_start.is_some_and(|start| sent <= start) {
            return;
        }
        if self.window < self.ssthresh {
            self.window += bytes;
        } else {
            self.window += MTU * bytes / self.window;
        }
    }

    fn end_acks(&mut self, _now: Instant, _in_flight: u64, _app_limited: bool) {}

    fn lost(&mut self, now: Instant, sent: Instant, _bytes: u64) {
        if self.recovery_start.is_some_and(|start| sent <= start) {
            return;
        }
        self.recovery_start = Some(now);
        self.window = (self.window / 2).max(2 * MTU);
        self.ssthresh = self.window;
    }

    fn window(&self) -> u64 {
        self.window
    }
}

/// 链路的参数。只有去程限速、排队、丢包；回程只有延迟。
#[derive(Debug, Clone, Copy)]
pub(super) struct Link {
    /// 字节每秒。0 表示不限速。
    pub(super) rate: u64,
    /// 单程延迟，两个方向相同。
    pub(super) delay: Duration,
    /// 等带宽的队列最多装多少字节，满了丢新来的。
    pub(super) buffer: u64,
    /// 随机丢包率，百万分之几。在进队列之前丢。
    pub(super) loss_ppm: u64,
    /// 丢包成片：一次连发的那一批包（最多 10 个）要丢就一起丢。内核把一批包当成一个大包交给限速器时就是这样。
    pub(super) bursty: bool,
    /// 接收方攒到这个时间的整数倍才把确认发出来。零表示每个包立即确认。
    pub(super) ack_every: Duration,
}

impl Link {
    /// 带宽 `mbit`、往返 `rtt_ms` 的链路，缓冲 100ms，不丢包。和端到端测试环境的设置相同。
    pub(super) fn new(mbit: u64, rtt_ms: u64) -> Self {
        let rate = mbit * 125_000;
        Self { rate, delay: Duration::from_micros(rtt_ms * 500), buffer: rate / 10, loss_ppm: 0, bursty: false, ack_every: Duration::ZERO }
    }

    pub(super) fn loss_percent(self, percent: u64) -> Self {
        Self { loss_ppm: percent * 10_000, ..self }
    }

    pub(super) fn bursty(self) -> Self {
        Self { bursty: true, ..self }
    }

    pub(super) fn acks_every(self, interval: Duration) -> Self {
        Self { ack_every: interval, ..self }
    }

    pub(super) fn buffer_packets(self, packets: u64) -> Self {
        Self { buffer: packets * MTU, ..self }
    }
}

/// 应用怎么给数据。
#[derive(Debug, Clone, Copy)]
pub(super) enum App {
    /// 一直有数据可发。
    Bulk,
    /// 每隔 `every` 来 `bytes` 字节。
    Chunks { bytes: u64, every: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Event {
    Relink,
    AppData { flow: usize },
    SetApp { flow: usize },
    Arrive { flow: usize, pn: u64 },
    Ack { flow: usize, pn: u64 },
    Wake { flow: usize },
}

#[derive(Debug, Clone, Copy)]
struct Packet {
    sent: Instant,
    size: u64,
}

/// 一个 100ms 的桶里的统计。
#[derive(Debug, Clone, Copy, Default)]
struct Bucket {
    delivered: u64,
    sent: u64,
    lost: u64,
    rtt_sum: Duration,
    rtt_count: u32,
}

struct Flow {
    cc: Box<dyn Cc>,
    app: App,
    /// 应用给了、还没有发出去的字节数。`App::Bulk` 时不用。
    backlog: u64,
    next_pn: u64,
    outstanding: BTreeMap<u64, Packet>,
    in_flight: u64,
    largest_acked: Option<u64>,
    srtt: Option<Duration>,
    latest_rtt: Duration,
    tokens: u64,
    pacer_window: u64,
    pacer_prev: Instant,
    app_limited: bool,
    next_wake: Option<Duration>,
    probe_deadline: Option<Duration>,
    buckets: Vec<Bucket>,
    states: Vec<(Duration, State)>,
}

impl Flow {
    fn rtt(&self) -> Duration {
        self.srtt.unwrap_or(INITIAL_RTT)
    }

    fn bucket(&mut self, at: Duration) -> &mut Bucket {
        let index = usize::try_from(at.as_nanos() / BUCKET.as_nanos()).unwrap();
        if self.buckets.len() <= index {
            self.buckets.resize(index + 1, Bucket::default());
        }
        &mut self.buckets[index]
    }

    fn has_data(&self) -> bool {
        matches!(self.app, App::Bulk) || self.backlog > 0
    }
}

pub(super) struct Sim {
    base: Instant,
    now: Duration,
    link: Link,
    /// 在排队等带宽的包各自什么时候发完、多大。
    queue: VecDeque<(Duration, u64)>,
    queued: u64,
    link_free_at: Duration,
    relinks: VecDeque<Link>,
    app_changes: VecDeque<App>,
    flows: Vec<Flow>,
    events: BinaryHeap<Reverse<(Duration, u64, Event)>>,
    sequence: u64,
    rng: u64,
}

impl Sim {
    pub(super) fn new(link: Link, seed: u64) -> Self {
        Self {
            base: Instant::now(),
            now: Duration::ZERO,
            link,
            queue: VecDeque::new(),
            queued: 0,
            link_free_at: Duration::ZERO,
            relinks: VecDeque::new(),
            app_changes: VecDeque::new(),
            flows: Vec::new(),
            events: BinaryHeap::new(),
            sequence: 0,
            rng: seed | 1,
        }
    }

    /// 加一条跑 BBR 的连接，`start` 时开始发。
    pub(super) fn add_bbr(&mut self, app: App, start: Duration) -> usize {
        let seed = 0x9e37_79b9_7f4a_7c15 ^ u64::try_from(self.flows.len()).unwrap();
        let mtu = u16::try_from(MTU).unwrap();
        self.add_flow(Box::new(Bbr::new(self.base + start, mtu, seed)), app, start)
    }

    pub(super) fn add_flow(&mut self, cc: Box<dyn Cc>, app: App, start: Duration) -> usize {
        let flow = self.flows.len();
        self.flows.push(Flow {
            cc,
            app,
            backlog: 0,
            next_pn: 0,
            outstanding: BTreeMap::new(),
            in_flight: 0,
            largest_acked: None,
            srtt: None,
            latest_rtt: INITIAL_RTT,
            tokens: 10 * MTU,
            pacer_window: 0,
            pacer_prev: self.base + start,
            app_limited: false,
            next_wake: None,
            probe_deadline: None,
            buckets: Vec::new(),
            states: Vec::new(),
        });
        self.push(start, Event::AppData { flow });
        flow
    }

    /// 到 `at` 时把链路换成 `link`。要按时间顺序调用。
    pub(super) fn relink_at(&mut self, at: Duration, link: Link) {
        self.relinks.push_back(link);
        self.push(at, Event::Relink);
    }

    /// 到 `at` 时把第 `flow` 条连接的应用行为换成 `app`。要按时间顺序调用。
    pub(super) fn set_app_at(&mut self, at: Duration, flow: usize, app: App) {
        self.app_changes.push_back(app);
        self.push(at, Event::SetApp { flow });
    }

    pub(super) fn run(&mut self, until: Duration) {
        while let Some(&Reverse((at, _, event))) = self.events.peek() {
            if at > until {
                break;
            }
            self.events.pop();
            self.now = at;
            match event {
                Event::Relink => self.link = self.relinks.pop_front().unwrap(),
                Event::SetApp { flow } => {
                    self.flows[flow].app = self.app_changes.pop_front().unwrap();
                    self.push(at, Event::AppData { flow });
                }
                Event::AppData { flow } => {
                    if let App::Chunks { bytes, every } = self.flows[flow].app {
                        self.flows[flow].backlog += bytes;
                        self.push(at + every, Event::AppData { flow });
                    }
                    self.pump(flow);
                }
                Event::Arrive { flow, pn } => {
                    let size = self.flows[flow].outstanding.get(&pn).map_or(MTU, |packet| packet.size);
                    self.flows[flow].bucket(at).delivered += size;
                    let step = self.link.ack_every.as_nanos();
                    let held = if step == 0 { 0 } else { (step - at.as_nanos() % step) % step };
                    self.push(at + Duration::from_nanos(u64::try_from(held).unwrap()) + self.link.delay, Event::Ack { flow, pn });
                }
                Event::Ack { flow, pn } => self.on_acks(flow, pn),
                Event::Wake { flow } => {
                    if self.flows[flow].next_wake == Some(at) {
                        self.flows[flow].next_wake = None;
                    }
                    self.detect_lost(flow);
                    self.probe_if_stalled(flow);
                    self.pump(flow);
                }
            }
        }
        self.now = until;
    }

    fn push(&mut self, at: Duration, event: Event) {
        self.sequence += 1;
        self.events.push(Reverse((at, self.sequence, event)));
    }

    fn wake(&mut self, flow: usize, at: Duration) {
        let at = at.max(self.now);
        if self.flows[flow].next_wake.is_none_or(|pending| at < pending) {
            self.flows[flow].next_wake = Some(at);
            self.push(at, Event::Wake { flow });
        }
    }

    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// 这一个（或者这一批）包是不是被随机丢掉了。
    fn is_dropped(&mut self) -> bool {
        self.random() % 1_000_000 < self.link.loss_ppm
    }

    /// 一个包进链路。返回它到达接收方的时刻；被丢掉时返回 `None`。
    fn transmit(&mut self, size: u64, batch_dropped: bool) -> Option<Duration> {
        if batch_dropped || (!self.link.bursty && self.is_dropped()) {
            return None;
        }
        if self.link.rate == 0 {
            return Some(self.now + self.link.delay);
        }
        while self.queue.front().is_some_and(|&(departs, _)| departs <= self.now) {
            let (_, departed) = self.queue.pop_front().unwrap();
            self.queued -= departed;
        }
        if self.queued + size > self.link.buffer.max(size) {
            return None;
        }
        let departs = self.link_free_at.max(self.now) + Duration::from_nanos(size * 1_000_000_000 / self.link.rate);
        self.link_free_at = departs;
        self.queue.push_back((departs, size));
        self.queued += size;
        Some(departs + self.link.delay)
    }

    fn send_packet(&mut self, flow: usize, size: u64, batch_dropped: bool) {
        let now = self.base + self.now;
        let at = self.now;
        let state = &mut self.flows[flow];
        let pn = state.next_pn;
        state.next_pn += 1;
        state.outstanding.insert(pn, Packet { sent: now, size });
        state.in_flight += size;
        state.bucket(at).sent += size;
        if let Some(arrival) = self.transmit(size, batch_dropped) {
            self.push(arrival, Event::Arrive { flow, pn });
        }
    }

    /// 发送方能发就发，照着 QUIC 库 `poll_transmit` 的检查顺序。
    fn pump(&mut self, flow: usize) {
        let now = self.base + self.now;
        loop {
            let state = &mut self.flows[flow];
            if !state.has_data() {
                state.app_limited = true;
                break;
            }
            let window = state.cc.window();
            if state.in_flight + MTU >= window {
                state.app_limited = false;
                break;
            }
            if let Some(delay) = pacing_delay(state, window, now) {
                state.app_limited = false;
                let at = self.now + delay;
                self.wake(flow, at);
                break;
            }
            // 一次最多连发 10 个包，相当于库的一次批量发送。
            let batch_dropped = self.link.bursty && self.is_dropped();
            let mut batch = 0;
            for _ in 0..10 {
                let state = &mut self.flows[flow];
                if !state.has_data() || state.in_flight + MTU >= window || state.tokens < MTU {
                    break;
                }
                let size = match state.app {
                    App::Bulk => MTU,
                    App::Chunks { .. } => state.backlog.min(MTU),
                };
                state.backlog = state.backlog.saturating_sub(size);
                state.tokens -= size;
                batch += size;
                self.send_packet(flow, size, batch_dropped);
            }
            self.flows[flow].cc.sent(now, batch);
            self.record_state(flow);
        }
        self.arm_probe(flow);
    }

    /// 处理同一时刻到达的一批确认。
    fn on_acks(&mut self, flow: usize, first: u64) {
        let mut acked = vec![first];
        while let Some(&Reverse((at, _, Event::Ack { flow: other, pn }))) = self.events.peek() {
            if at != self.now || other != flow {
                break;
            }
            self.events.pop();
            acked.push(pn);
        }
        acked.sort_unstable();

        let now = self.base + self.now;
        let at = self.now;
        let state = &mut self.flows[flow];
        let mut newest: Option<(u64, Instant)> = None;
        for pn in acked {
            let Some(packet) = state.outstanding.remove(&pn) else { continue };
            state.in_flight -= packet.size;
            state.cc.acked(now, packet.sent, packet.size);
            newest = Some((pn, packet.sent));
        }
        let Some((largest, largest_sent)) = newest else { return };
        state.cc.end_acks(now, state.in_flight, state.app_limited);
        if state.largest_acked.is_none_or(|previous| largest > previous) {
            state.largest_acked = Some(largest);
            state.latest_rtt = now.duration_since(largest_sent);
            state.srtt = Some(state.srtt.map_or(state.latest_rtt, |srtt| (7 * srtt + state.latest_rtt) / 8));
            let rtt = state.latest_rtt;
            let bucket = state.bucket(at);
            bucket.rtt_sum += rtt;
            bucket.rtt_count += 1;
        }
        self.record_state(flow);
        self.detect_lost(flow);
        self.pump(flow);
    }

    fn detect_lost(&mut self, flow: usize) {
        let now = self.base + self.now;
        let at = self.now;
        let state = &mut self.flows[flow];
        let Some(largest_acked) = state.largest_acked else { return };
        let loss_delay = (state.rtt().max(state.latest_rtt) * 9 / 8).max(Duration::from_millis(1));
        let mut lost = Vec::new();
        let mut next_timeout = None;
        for (&pn, packet) in state.outstanding.range(..largest_acked) {
            if largest_acked >= pn + 3 || now.duration_since(packet.sent) >= loss_delay {
                lost.push(pn);
            } else {
                let timeout = packet.sent + loss_delay;
                next_timeout = Some(next_timeout.map_or(timeout, |earliest: Instant| earliest.min(timeout)));
            }
        }
        if let Some(&last) = lost.last() {
            let last_sent = state.outstanding[&last].sent;
            let bytes: u64 = lost.iter().map(|pn| state.outstanding.remove(pn).unwrap().size).sum();
            state.in_flight -= bytes;
            state.bucket(at).lost += bytes;
            state.cc.lost(now, last_sent, bytes);
            self.record_state(flow);
        }
        if let Some(timeout) = next_timeout {
            let at = timeout.duration_since(self.base);
            self.wake(flow, at);
        }
    }

    fn arm_probe(&mut self, flow: usize) {
        let state = &mut self.flows[flow];
        if state.in_flight == 0 {
            state.probe_deadline = None;
            return;
        }
        let deadline = self.now + state.rtt() * 3 + Duration::from_millis(10);
        state.probe_deadline = Some(deadline);
        self.wake(flow, deadline);
    }

    /// 在途的包很久没有确认（比如这一窗的包全丢了）：不管窗口，发一个探测包，引出确认来触发丢包检测。
    fn probe_if_stalled(&mut self, flow: usize) {
        let state = &self.flows[flow];
        let Some(deadline) = state.probe_deadline else { return };
        if self.now < deadline {
            self.wake(flow, deadline);
            return;
        }
        if state.in_flight > 0 {
            let now = self.base + self.now;
            self.send_packet(flow, MTU, false);
            self.flows[flow].cc.sent(now, MTU);
        }
    }

    fn record_state(&mut self, flow: usize) {
        let at = self.now;
        let state = &mut self.flows[flow];
        let Some(current) = state.cc.as_bbr().map(|bbr| bbr.state) else { return };
        if state
            .states
            .last()
            .is_none_or(|&(_, last)| std::mem::discriminant(&last) != std::mem::discriminant(&current) || phase_changed(last, current))
        {
            state.states.push((at, current));
        }
    }

    pub(super) fn bbr(&self, flow: usize) -> &Bbr {
        self.flows[flow].cc.as_bbr().unwrap()
    }

    /// 第 `flow` 条连接经历过的状态，按时间顺序。同一个状态里计数的变化不算。
    pub(super) fn states(&self, flow: usize) -> &[(Duration, State)] {
        &self.flows[flow].states
    }

    fn buckets(&self, flow: usize, from: Duration, to: Duration) -> &[Bucket] {
        let index = |at: Duration| usize::try_from(at.as_nanos() / BUCKET.as_nanos()).unwrap();
        let buckets = &self.flows[flow].buckets;
        &buckets[index(from).min(buckets.len())..index(to).min(buckets.len())]
    }

    /// `from` 到 `to` 之间送达的速率，Mbit/s。
    pub(super) fn mbit(&self, flow: usize, from: Duration, to: Duration) -> f64 {
        let delivered: u64 = self.buckets(flow, from, to).iter().map(|bucket| bucket.delivered).sum();
        delivered as f64 * 8.0 / 1e6 / (to - from).as_secs_f64()
    }

    /// `from` 到 `to` 之间判为丢失的字节数占发出的比例。
    pub(super) fn loss_ratio(&self, flow: usize, from: Duration, to: Duration) -> f64 {
        let buckets = self.buckets(flow, from, to);
        let sent: u64 = buckets.iter().map(|bucket| bucket.sent).sum();
        let lost: u64 = buckets.iter().map(|bucket| bucket.lost).sum();
        if sent == 0 { 0.0 } else { lost as f64 / sent as f64 }
    }

    /// `from` 到 `to` 之间往返时间样本的平均值。
    pub(super) fn mean_rtt(&self, flow: usize, from: Duration, to: Duration) -> Duration {
        let buckets = self.buckets(flow, from, to);
        let count: u32 = buckets.iter().map(|bucket| bucket.rtt_count).sum();
        let sum: Duration = buckets.iter().map(|bucket| bucket.rtt_sum).sum();
        if count == 0 { Duration::ZERO } else { sum / count }
    }
}

fn phase_changed(last: State, current: State) -> bool {
    match (last, current) {
        (State::ProbeBw(last), State::ProbeBw(current)) => std::mem::discriminant(&last) != std::mem::discriminant(&current),
        _ => false,
    }
}

/// QUIC 库的发送节奏：令牌桶，容量是 2ms 的量（10～256 个包），空了之后等它重新攒满。
fn pacing_delay(flow: &mut Flow, window: u64, now: Instant) -> Option<Duration> {
    let rtt = flow.rtt();
    let capacity = u64::try_from(u128::from(window) * 2_000_000 / rtt.as_nanos().max(1)).unwrap().clamp(10 * MTU, 256 * MTU);
    if window != flow.pacer_window {
        flow.pacer_window = window;
        flow.tokens = flow.tokens.min(capacity);
    }
    if flow.tokens >= MTU {
        return None;
    }
    let elapsed = now.duration_since(flow.pacer_prev);
    let refill = u64::try_from(u128::from(window) * 5 * elapsed.as_nanos() / (4 * rtt.as_nanos().max(1))).unwrap_or(u64::MAX);
    flow.tokens = flow.tokens.saturating_add(refill).min(capacity);
    flow.pacer_prev = now;
    if flow.tokens >= MTU {
        return None;
    }
    let missing = u128::from(capacity - flow.tokens);
    let delay = rtt.as_nanos() * missing * 4 / (u128::from(window) * 5);
    Some(Duration::from_nanos(u64::try_from(delay).unwrap()).max(Duration::from_micros(1)))
}
