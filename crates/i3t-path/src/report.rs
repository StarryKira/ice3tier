//! 给界面看的选路情况（白皮书 §7.7 的路由视图和路径诊断）：到一个对端考虑过哪些候选路径、各自测出来的代价、
//! 现在走的是哪一条、为什么。还有本节点手上的骨干矩阵。
//!
//! 这些都是某一时刻的快照，由控制任务定期刷新；[`crate::Paths::path_reports`] 等方法读的就是最近的一份。

use i3t_model::control::{Route, RouteCause};
use i3t_model::netmap::RoutingMode;
use i3t_proto::NodeId;
use i3t_proto::backbone::RelayState;
use serde::{Deserialize, Serialize};

/// 直连是怎么建立起来的。
///
/// 白皮书 §7.7 还列了"端口映射"这一种。对端的候选端点在 netmap 里不分来源，本节点看不出某个地址是不是它的路由器上
/// 映射出来的，所以这里归在 [`DirectKind::Reachable`] 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectKind {
    /// 同一个局域网：对端的信标是在本地网段上看到的，或者两边的公网地址相同而且对端的这个地址是私有地址。
    Lan,
    /// 对端的 IPv6 地址。
    Ipv6,
    /// 对端的这个地址本来就连得上（公网地址、端口转发、端口映射），没有用到打洞信令。
    Reachable,
    /// 和对端交换打洞信令之后打通的。
    Punched,
}

/// 到一个对端的直连现在在哪一步（白皮书 §7.7）。从路径层为直连的尝试和退避记着的状态里读出来，不为它另外探测，
/// 也不改什么时候去试：一对节点第一次用到时试，失败了按退避重试，主动测试立刻试（docs/internal/architecture.md §5.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectStatus {
    pub state: DirectState,
    /// `Verified` 时：直连是怎么建立起来的。
    #[serde(default)]
    pub kind: Option<DirectKind>,
    /// 最近一次失败的原因。通着的时候没有；正在试的时候是上一次的。
    #[serde(default)]
    pub failure: Option<DirectFailureCause>,
    /// 最近一次尝试是多少毫秒之前开始的；从来没有试过时是 `None`。
    #[serde(default)]
    pub attempted_ms: Option<u64>,
}

/// 直连的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectState {
    /// 还没有试过：这一对节点还没有用过（直连只在第一次用到时去试），或者不能由本端去试（只经 relay、对端只主动连接）。
    NotTried,
    /// 正在试：向对端的候选地址发探测，等它回答。
    Trying,
    /// 验证通过，可以用。
    Verified,
    /// 试过了，没有通；或者通过的直连后来断了。按退避再试。
    Failed,
}

/// 直连上一次为什么没有成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectFailureCause {
    /// 试满了一次的时限，对端的哪个候选地址都没有回答。
    NoAnswer,
    /// 手上没有对端的任何候选地址，没有地方可以试。
    NoCandidates,
    /// 用着的直连上探测连续超时。
    Timeouts,
    /// 用着的直连上探测丢了超过 20%。
    Loss,
    /// 这一对节点闲着，用着的直连很久没有验证过，放下了。
    Stale,
}

/// 经 relay 时，本节点到路径上第一个 relay 的那一段用的传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayLeg {
    Udp,
    /// UDP 用不了时，经 relay 的 TCP 监听。
    Tcp,
}

/// 路径上一段的测量值。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LegReport {
    pub from: NodeId,
    pub to: NodeId,
    /// 往返延迟，毫秒。
    pub rtt_ms: f64,
    /// 抖动，毫秒。
    pub jitter_ms: f64,
    /// 丢包率，0 到 1。目的节点到它挂靠的 relay 的那一段只知道延迟，抖动和丢包都是 0。
    pub loss: f64,
}

/// 一条考虑过的候选路径。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateReport {
    pub route: Route,
    /// 按白皮书 §7.4 算出来的代价，毫秒。默认模式不拿它做选择，但照样算出来给人看。
    pub cost_ms: f64,
    /// 各段的测量值，按经过的顺序。
    pub legs: Vec<LegReport>,
    /// 路径上负载最高的 relay 的带宽占用百分比。
    pub load_percent: u8,
    /// 现在走的就是这一条。
    pub current: bool,
    /// 经 relay 的路径：本节点现在到它的第一个 relay 走 UDP 还是 TCP。直连、第一个 relay 就是本节点自己、
    /// 或者此刻到它没有会话时是 `None`。
    #[serde(default)]
    pub relay_leg: Option<RelayLeg>,
}

/// 到一个对端的选路情况。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathReport {
    pub peer: NodeId,
    /// 本节点用的选路模式。
    pub mode: RoutingMode,
    /// 现在走的路。每个方向由发送方自己选，这是本节点发往对端的方向。
    pub route: Route,
    /// 现在走直连时，它是怎么建立起来的。
    pub direct: Option<DirectKind>,
    /// 现在经 relay 时，本节点到第一个 relay 的那一段走 UDP 还是 TCP。不经 relay、或者第一个 relay 就是本节点自己时是 `None`。
    #[serde(default)]
    pub relay_leg: Option<RelayLeg>,
    /// 为什么是现在这条路，给人看的。
    pub reason: String,
    /// `reason` 的代码和它要用到的值。
    #[serde(default)]
    pub cause: Option<RouteCause>,
    /// 上一次换路是多少毫秒之前；从来没有过路时是 `None`。
    pub since_switch_ms: Option<u64>,
    /// 直连现在在哪一步，不管现在走不走它。
    pub direct_status: DirectStatus,
    /// 本节点没在用的那种选路模式此刻会选的路：默认模式下是延迟优先会选的（代价最低，按选路器的差距从现在的路算），
    /// 延迟优先模式下是默认模式会选的（有验证过的直连就直连，否则照常挑 relay）。那种模式没有路可走时是 [`Route::None`]。
    /// 和现在的路一样、或者还没有算过（默认模式下第一个评估窗口之前）时是 `None`。
    #[serde(default)]
    pub alternative: Option<Alternative>,
    /// 考虑过的候选路径，按代价从低到高。没有任何测量值的路径不在里面。
    pub candidates: Vec<CandidateReport>,
}

/// 另一种选路模式会选的路。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alternative {
    pub route: Route,
    /// 这条路上的往返延迟，毫秒，和路由表里现在这条路的算法一样；没有测量值时是 `None`。
    pub rtt_ms: Option<u32>,
}

/// 骨干矩阵里的一条链路。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BackboneLink {
    pub to: NodeId,
    /// 往返延迟，毫秒；`None` 表示连不上。
    pub rtt_ms: Option<u16>,
    pub jitter_ms: u8,
    /// 丢包率，0 到 1。
    pub loss: f64,
}

/// 骨干矩阵的一行：一个 relay 的状态，和它到别的 relay 的链路。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackboneRow {
    pub relay: NodeId,
    pub state: RelayState,
    /// 带宽占用的百分比；没有设上限的 relay 是 0。
    pub load_percent: u8,
    /// 这一行是多少毫秒之前收到的。
    pub age_ms: u64,
    pub links: Vec<BackboneLink>,
}
