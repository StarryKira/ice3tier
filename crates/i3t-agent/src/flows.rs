//! 节点上"活跃的流"有多少：自动更新等的就是它变成零（白皮书 §10.5）。
//!
//! 算的是以这个节点为起点或终点的流：入口上正在进行的连接，加上出口正在处理的流和数据报上下文。
//! 走独占 TCP（lane）的流也在其中；正在承载流的 lane 至少和它们一样多（见 [`Sample::active`]），空闲的 lane 不算。
//! 替别人中转一般不算：relay 重启时挂靠它的节点会换到别的 relay 上，连接不断。
//! 例外是本节点是网络里**唯一的** relay：没有别的路可换，重启会断掉经它中转的连接，所以它正在替人转发时算作不空闲。
//! 这一条 controller 不用管，节点自己知道：等空闲的更新在唯一的 relay 上自然会等到没有人经它转发。
//!
//! 各层的计数都是普通的原子变量，没有变化通知，所以这里定时去读，把结果放进一个 `watch` 里给更新那一层等。

use std::time::{Duration, Instant};

use i3t_conn::RelayCounters;
use i3t_model::Netmap;
use tokio::sync::watch;

/// 多久读一次各层的计数。更新等到"流数是零"之后马上换程序，这个间隔就是那一刻判断可能滞后的时间。
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// 转发的包数这么久没有增加，才算没有在替人转发。经 relay 的连接每 15 秒至少有一个保活包，取两倍。
const FORWARDING_QUIET: Duration = Duration::from_secs(30);

/// 本节点是不是网络里唯一的 relay。
pub(crate) fn is_only_relay(netmap: &Netmap) -> bool {
    netmap.me.relay.is_some() && netmap.relays.iter().all(|relay| relay.id == netmap.me.id)
}

/// 盯着 relay 的转发计数，判断它现在是不是在替人转发。
#[derive(Debug, Default)]
pub(crate) struct RelayActivity {
    forwarded_packets: u64,
    last_forwarded: Option<Instant>,
}

impl RelayActivity {
    /// 记下最新的计数，返回现在算不算在转发。
    pub(crate) fn observe(&mut self, counters: &RelayCounters, now: Instant) -> bool {
        if counters.forwarded_packets != self.forwarded_packets {
            self.forwarded_packets = counters.forwarded_packets;
            self.last_forwarded = Some(now);
        }
        self.last_forwarded.is_some_and(|at| now.duration_since(at) < FORWARDING_QUIET)
    }
}

/// 某一刻读到的几个数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Sample {
    /// 入口上正在进行的连接：代理监听器和端口转发。
    pub(crate) ingress: usize,
    /// 出口正在处理的流和数据报上下文。
    pub(crate) exit: usize,
    /// 正在承载流的 lane。
    pub(crate) lanes: usize,
    /// 本节点是唯一的 relay，并且正在替人转发。
    pub(crate) sole_relay_forwarding: bool,
}

impl Sample {
    /// 更新要等它变成零的那个数。
    pub(crate) fn active(self) -> usize {
        // lane 上的流在入口或出口已经算过一次；lane 只是保证它没算到的那一刻（例如流刚交给 lane、还没进出口的计数）也不空闲。
        let streams = self.ingress.saturating_add(self.exit).max(self.lanes);
        streams.saturating_add(usize::from(self.sole_relay_forwarding))
    }
}

/// 定时调用 `sample`，把活跃的流数放进 `flows`。一直跑到 `flows` 没有人再看。
pub(crate) async fn publish(mut sample: impl FnMut() -> Sample, flows: watch::Sender<usize>) {
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    loop {
        tick.tick().await;
        let active = sample().active();
        flows.send_if_modified(|current| std::mem::replace(current, active) != active);
        if flows.is_closed() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::{Relay, RelayRole};
    use i3t_proto::{NodeId, PublicKey};

    use super::*;
    use crate::control::testing::netmap;

    fn relay(id: u32) -> Relay {
        serde_json::from_value(serde_json::json!({ "id": id, "key": PublicKey::from_bytes([7; 32]), "name": format!("r{id}") })).unwrap()
    }

    #[test]
    fn only_a_node_that_is_the_networks_single_relay_counts_as_the_only_relay() {
        let mut map = netmap(1);
        assert!(!is_only_relay(&map), "不是 relay");
        map.me.relay = Some(RelayRole::default());
        assert!(is_only_relay(&map), "relay 列表里还没有别人");
        map.relays = vec![relay(1)];
        assert!(is_only_relay(&map), "列表里只有它自己");
        map.relays.push(relay(2));
        assert!(!is_only_relay(&map), "还有别的 relay 可以换");
        assert_eq!(map.me.id, NodeId::new(1).unwrap());
    }

    #[test]
    fn a_relay_counts_as_forwarding_until_its_counter_has_been_still_for_a_while() {
        let mut activity = RelayActivity::default();
        let start = Instant::now();
        let counters = |forwarded_packets| {
            let mut counters = RelayCounters::default();
            counters.forwarded_packets = forwarded_packets;
            counters
        };
        assert!(!activity.observe(&counters(0), start), "从来没有转发过");
        assert!(activity.observe(&counters(10), start + Duration::from_secs(1)));
        assert!(activity.observe(&counters(10), start + Duration::from_secs(20)), "保活包的间隔之内不算停了");
        assert!(!activity.observe(&counters(10), start + Duration::from_secs(32)));
        assert!(activity.observe(&counters(11), start + Duration::from_secs(40)), "又有包经过");
    }

    #[test]
    fn relaying_only_adds_to_the_count_for_a_sole_relay() {
        assert_eq!(Sample { ingress: 2, exit: 3, lanes: 0, sole_relay_forwarding: false }.active(), 5);
        assert_eq!(Sample { ingress: 0, exit: 0, lanes: 0, sole_relay_forwarding: true }.active(), 1);
        // lane 上的流已经在入口或出口算过，不重复算；它们都还没算到时 lane 也让节点不空闲。
        assert_eq!(Sample { ingress: 2, exit: 0, lanes: 2, sole_relay_forwarding: false }.active(), 2);
        assert_eq!(Sample { ingress: 0, exit: 0, lanes: 1, sole_relay_forwarding: false }.active(), 1);
        assert_eq!(Sample::default().active(), 0);
    }

    #[tokio::test]
    async fn the_count_is_published_as_it_changes() {
        let (tx, mut rx) = watch::channel(0);
        let samples = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(2));
        let source = std::sync::Arc::clone(&samples);
        let task =
            tokio::spawn(publish(move || Sample { ingress: source.load(std::sync::atomic::Ordering::Relaxed), ..Sample::default() }, tx));
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|flows| *flows == 2)).await.unwrap().unwrap();
        samples.store(0, std::sync::atomic::Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|flows| *flows == 0)).await.unwrap().unwrap();
        drop(rx);
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("没有人看了就该停").unwrap();
    }
}
