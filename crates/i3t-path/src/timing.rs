//! 路径层用到的时间参数，集中在一处，改的时候看得到彼此的关系。

use std::time::Duration;

/// 往返延迟的平滑估计，算法和 TCP 的 SRTT 一样：新样本占八分之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rtt(Duration);

impl Rtt {
    pub(crate) const fn new(sample: Duration) -> Self {
        Self(sample)
    }

    pub(crate) fn update(&mut self, sample: Duration) {
        self.0 = (self.0 * 7 + sample) / 8;
    }

    pub(crate) const fn get(self) -> Duration {
        self.0
    }

    /// 四舍五入到毫秒；不到 1 毫秒的报 1，免得和"没有测到"混淆。
    pub(crate) fn millis(self) -> u32 {
        u32::try_from((self.0.as_micros() + 500) / 1000).unwrap_or(u32::MAX).max(1)
    }

    /// 等一个探测的回答等多久算超时：延迟的 4 倍，夹在 [`PROBE_TIMEOUT_MIN`] 和 [`PROBE_TIMEOUT_MAX`] 之间。
    pub(crate) fn probe_timeout(self) -> Duration {
        (self.0 * 4).clamp(PROBE_TIMEOUT_MIN, PROBE_TIMEOUT_MAX)
    }
}

pub(crate) const PROBE_TIMEOUT_MIN: Duration = Duration::from_millis(300);
pub(crate) const PROBE_TIMEOUT_MAX: Duration = Duration::from_millis(1500);

/// 控制任务多久把所有的定时器过一遍。各种超时的精度就是这么多。
pub(crate) const TICK: Duration = Duration::from_millis(50);

/// 在用的直连地址、挂靠的 relay 多久探测一次（白皮书 §7.3）。
pub(crate) const PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// 探测连续几次没有回答就判为不可用。
pub(crate) const MAX_TIMEOUTS: u8 = 3;
/// 有包发出去、却这么久没有从这条路上收到任何东西时，不等探测间隔，立刻探测。
pub(crate) const SILENCE: Duration = Duration::from_secs(1);
/// 判断"发了却没有回音"时，多久之内发的包算数。
pub(crate) const UNANSWERED_WINDOW: Duration = Duration::from_secs(6);

/// 一个对端多久没有包要发就算不活跃：停止探测它的直连，为它临时建的中转会话可以回收。
/// 连接层每 15 秒保活一次，所以有连接的对端始终是活跃的。
pub(crate) const PEER_ACTIVE: Duration = Duration::from_secs(45);

/// 没有路或者路可疑时留着的最后几个包，最多放这么久。再旧的重放出去只会让对面算出离谱的延迟。
pub(crate) const STASH_MAX_AGE: Duration = Duration::from_secs(20);

/// 打洞失败之后隔多久再试（白皮书 §5.3）。
pub(crate) const PUNCH_BACKOFF: [Duration; 8] = [
    Duration::from_secs(1),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(8),
    Duration::from_secs(16),
];
/// 上面那一轮都不成之后：连接还在走 relay 并且有流量时，每隔这么久再试一次。
pub(crate) const PUNCH_IDLE_RETRY: Duration = Duration::from_secs(60);
/// 一次打洞尝试里，从开始算起在这些时刻各向所有候选地址发一轮探测。
/// 头一轮多半被对面的 NAT 挡掉（它那边还没有往外发过），后面两轮才是真正能通的。
pub(crate) const PUNCH_VOLLEYS: [Duration; 3] = [Duration::ZERO, Duration::from_millis(300), Duration::from_millis(600)];
/// 一次打洞尝试从开始到判为失败的时间。
pub(crate) const PUNCH_ATTEMPT: Duration = Duration::from_millis(1500);

/// 中转会话握手的每一步等多久重发，一共发几次。
pub(crate) const HANDSHAKE_RETRY: Duration = Duration::from_millis(500);
pub(crate) const HANDSHAKE_TRIES: u8 = 4;
/// 走在 TCP 里的握手，每一步等回答等多久。TCP 里的帧不会丢、只会迟到，所以不重发，只是等得久一些：
/// 丢包的链路上一次重传就是一两秒，按 UDP 的节奏（半秒一次、两秒放弃）会在链路只是慢的时候白白放弃。
/// 两步加起来要比 relay 给握手留的时间（10 秒）短。
///
/// 保活没有回答时不这样等，照样尽快拆掉重连。真实链路上量过（晚高峰，丢包两成）：卡住的 TCP 三分之二
/// 等 20 秒也不恢复，而新拨一条通常不到 1 秒就握上手；能自己恢复的那三分之一，恢复得也不比重连快。
pub(crate) const TCP_HANDSHAKE_STEP: Duration = Duration::from_secs(4);
/// 到 relay 的 TCP 上，发出去的数据这么久没有被确认，内核就放弃这条连接（`TCP_USER_TIMEOUT`，和重传上限一起设，
/// 见 `i3t_underlay::tcp_recovery`）。
///
/// 上面这一层自己判断这条 TCP 的死活：会话建立之后保活连续 3 次没有回答（最多 4.5 秒左右）就拆掉重连，握手时每一步等
/// [`TCP_HANDSHAKE_STEP`]。所以这里取 10 秒：比上面那一层能等的都长，TCP 不会先把它判死；上面那一层换掉它之后，
/// 关掉的那个 socket 在内核里也最多再留这么久。
pub(crate) const TCP_LEG_GIVE_UP: Duration = Duration::from_secs(10);
/// UDP 和 TCP 同时试的时候，UDP 这么久还没握上手就先用 TCP。
pub(crate) const UDP_HEAD_START: Duration = Duration::from_secs(1);
/// 走着 TCP 的节点多久再试一次 UDP。
pub(crate) const UDP_RETRY: Duration = Duration::from_secs(30);
/// 连不上的 relay 隔多久重试：从 1 秒起每次加倍，到这个数为止。
/// 试一次只是一个 TCP 连接加几个 UDP 包，而一个 relay 都连不上的节点和别人是断开的，所以不让它等太久。
pub(crate) const RELAY_RETRY_MAX: Duration = Duration::from_secs(10);
/// 没有挂靠的 relay 多久测一次延迟（白皮书 §7.3）。
pub(crate) const RELAY_SURVEY: Duration = Duration::from_secs(60);
/// 测量结果的有效期，过期作废（白皮书 §7.3）。
pub(crate) const ESTIMATE_TTL: Duration = Duration::from_secs(120);

/// relay 和 relay 之间多久探测一次（白皮书 §7.3）。只管 UDP 上的会话，骨干走 TCP 时照 [`PROBE_INTERVAL`]（见 `crate::relays`）。
pub(crate) const BACKBONE_PROBE_INTERVAL: Duration = Duration::from_secs(2);
/// 骨干上：到对方的 UDP 会话通着、对方却这么久都没有经 UDP 在本节点上建会话，就当它连不上本节点，这一段改走本节点拨出去的 TCP。
/// 对方的 netmap 晚到几秒、它的握手重试几次都在这之内；比它短的话，对方重启的那一阵会被误判。
pub(crate) const ONE_SIDED_GRACE: Duration = Duration::from_secs(10);
/// 骨干走 TCP、按编号不该本节点先拨时，等对方拨进来最多等多久，之后自己拨（对方可能连不进本节点）。
/// 对方拨号要等它自己的 UDP 握不上手（两秒）再加上连 TCP、握手（连不上的地址每个最多四秒）。
pub(crate) const DIAL_DEFER: Duration = Duration::from_secs(10);
/// 借来的 TCP 上没有建成会话（对方不在上面回答，比如它是不懂这件事的旧版本）之后，多久之内不再借同一条。
pub(crate) const OFFER_RETRY: Duration = Duration::from_secs(60);
/// 给正在评估的候选路径发探测包列：多久一组，一组几个（白皮书 §7.3）。
pub(crate) const TRAIN_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const TRAIN_PROBES: u8 = 10;
/// 延迟优先模式多久比较一次候选路径的代价（白皮书 §7.5 的评估窗口）。
pub(crate) const EVAL_WINDOW: Duration = Duration::from_secs(5);
/// 新的路要连续几个评估窗口都更优才换过去。
pub(crate) const SWITCH_WINDOWS: u8 = 3;
/// 因为代价换过一次路之后至少停留多久。
pub(crate) const MIN_DWELL: Duration = Duration::from_secs(30);
/// 两条路来回换的时候停留期每次加倍，最长到这么久。
pub(crate) const MAX_DWELL: Duration = Duration::from_secs(480);
/// 到一个 relay 的会话断了重连之后要保持这么久，流量才会再换到经它的路上（原来的路断了的时候除外）。
pub(crate) const SETTLE: Duration = MIN_DWELL;
/// relay 多久把骨干矩阵发给它的会话一次。状态变了的时候不等这个间隔。
pub(crate) const ROW_INTERVAL: Duration = Duration::from_secs(5);
/// 延迟优先模式下，入口 relay 只在离本节点最近的这么多个里选（白皮书 §7.1），为的是限制探测量。
pub(crate) const ENTRY_RELAYS: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtt_moves_an_eighth_of_the_way_towards_each_sample() {
        let mut rtt = Rtt::new(Duration::from_millis(80));
        rtt.update(Duration::from_millis(160));
        assert_eq!(rtt.get(), Duration::from_millis(90));
        assert_eq!(rtt.millis(), 90);
        assert_eq!(Rtt::new(Duration::from_micros(40)).millis(), 1);
        assert_eq!(Rtt::new(Duration::from_micros(1500)).millis(), 2);
    }

    #[test]
    fn probe_timeout_follows_the_rtt_within_bounds() {
        assert_eq!(Rtt::new(Duration::from_millis(1)).probe_timeout(), PROBE_TIMEOUT_MIN);
        assert_eq!(Rtt::new(Duration::from_millis(200)).probe_timeout(), Duration::from_millis(800));
        assert_eq!(Rtt::new(Duration::from_secs(2)).probe_timeout(), PROBE_TIMEOUT_MAX);
    }

    #[test]
    fn the_punch_schedule_is_the_one_in_the_design() {
        let seconds: Vec<u64> = PUNCH_BACKOFF.iter().map(Duration::as_secs).collect();
        assert_eq!(seconds, [1, 1, 2, 4, 4, 8, 8, 16]);
        assert!(PUNCH_VOLLEYS.iter().all(|&at| at < PUNCH_ATTEMPT));
    }
}
