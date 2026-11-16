//! 主动测试（白皮书 §7.6）在节点协议里的样子：controller 给两端的指令，和两端各自的上报。
//!
//! ```text
//! controller → 两端   Test { 编号, 对端, 发起方还是接受方, 测试项, 指定的路径, 时间窗 }
//! 发起方 → 对端       测试探测（经直连或者指定的路径）、测量流
//! 两端 → controller   TestReport { 编号, 自己这一侧看到的 }
//! controller → 两端   TestEnd { 编号 }                     做完了或者被取消了：关掉时间窗
//! ```
//!
//! 管理 API 里的结果也用这里的几个结构（延迟、每一段、吞吐），所以数都带着单位写在名字里：微秒、千分之几、字节、毫秒。

use std::fmt;

use i3t_proto::NodeId;
use serde::{Deserialize, Serialize};

use super::{DirectKind, LegTransport, PathCandidate, Route};
use crate::netmap::RoutingMode;

/// 两端在一个测试里的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestRole {
    /// 发探测、开测量流、量结果的一方（管理 API 里的 `a`）。
    Initiator,
    /// 回答探测、提供测量服务的一方（`b`）。
    Responder,
}

/// 一个测试要做哪几项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TestItems {
    /// 直连能力：现在就试一次直连。
    pub direct: bool,
    /// 直连质量：在直连上发探测包列。
    pub direct_quality: bool,
    /// relay 质量：经 relay 的路径上发探测包列。
    pub relay_quality: bool,
    /// 两种选路模式各会选哪条路。
    pub route: bool,
    /// 在测质量的路径上另外量吞吐（要上面两项之一）。
    pub throughput: bool,
}

/// controller 给两端的测试指令。两端拿到的是同一个编号、同样的测试项和路径，只是角色和对端不同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestOrder {
    pub test: u64,
    pub peer: NodeId,
    pub role: TestRole,
    pub items: TestItems,
    /// 管理员指定的 relay 路径，按发起方到接受方的顺序。没有指定时 relay 质量测发起方选的默认两条。
    #[serde(default)]
    pub paths: Vec<Vec<NodeId>>,
    /// 时间窗多长，秒。节点按自己的上限截断。
    pub window_secs: u32,
}

/// 一端的上报。发起方做完了才报一次；接受方做完自己那一侧的直连尝试和选路之后先报一次，测试结束时（`TestEnd`）
/// 或者时间窗到了再报一次最后的。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TestReport {
    pub test: u64,
    #[serde(default)]
    pub role: Option<TestRole>,
    /// 这是这一端的最后一次上报。
    #[serde(default)]
    pub last: bool,
    /// 节点拒绝了整个测试时的原因。
    #[serde(default)]
    pub refused: Option<TestProblem>,
    /// 这一端看到的直连能力。
    #[serde(default)]
    pub direct: Option<DirectSide>,
    /// 量过的路径，按量的顺序。只有发起方有。
    #[serde(default)]
    pub paths: Vec<PathMeasure>,
    /// 这一端发往对端的方向上，两种模式各会选哪条路。
    #[serde(default)]
    pub decision: Option<Decision>,
    /// 接受方：被要求提供吞吐测量、却没有提供的原因（本机关掉了吞吐测试等）。
    #[serde(default)]
    pub throughput_refused: Option<TestProblem>,
}

/// 测试里一样事做不成的原因：一个码，说的是哪个节点，和一小段给人看的话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestProblem {
    pub code: TestCode,
    /// 是哪个节点的原因；说不上是哪个节点时是 `null`。
    #[serde(default)]
    pub node: Option<NodeId>,
    #[serde(default)]
    pub message: String,
}

impl TestProblem {
    #[must_use]
    pub fn new(code: TestCode, node: Option<NodeId>) -> Self {
        Self { code, node, message: code.text().to_owned() }
    }
}

/// 测试里的原因码。界面按它显示，`message` 只是给没有翻译的地方兜底。以后可能增加，不认识的读成 `unknown`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestCode {
    /// 节点的版本太旧，不懂主动测试。
    NotSupported,
    /// 节点不在线。
    NodeOffline,
    /// 访问规则没有把这两个节点连通：它们拿不到对方的地址和公钥。
    NotConnected,
    /// 节点的路径限制是只经 relay：不参加直连的测试，不向对端暴露地址。
    RelayOnly,
    /// 节点的路径限制是只用直连：不参加经 relay 的测试。
    DirectOnly,
    /// 节点在本地关掉了吞吐测试。
    ThroughputOff,
    /// 网络里没有能用的 relay 路径。
    NoRelay,
    /// 两个节点之间没有可用的直连，直连质量测不了。
    NoDirect,
    /// 手上没有对端的候选地址。
    NoCandidates,
    /// 节点的 UDP 不通。
    UdpBlocked,
    /// 节点映射出来的端口随对端变化（对称型 NAT），打不了洞。
    MappingVaries,
    /// 两个节点都是只拨出的。
    DialOutOnly,
    /// 试过了，没有在时限之内成功。
    Timeout,
    /// 这条路径此刻走不了：到它的第一个 relay 没有会话，或者路径上有 relay 不转发。
    Unreachable,
    /// 对端拒绝了测量流。
    Refused,
    /// 节点上同时进行的测试太多，或者和这个对端的上一次测试刚刚做过。
    Busy,
    /// 同一对节点的上一次测试刚做过，要隔一会儿。
    TooSoon,
    /// 测试被取消了。
    Cancelled,
    /// 节点没有在时限之内上报结果。
    NoReport,
    /// 测量流中途断了。
    StreamFailed,
    /// 指定的路径不能用：经过的节点不是 relay、太长、或者有重复。
    InvalidPath,
    /// 节点内部出错。
    Internal,
    #[serde(other)]
    Unknown,
}

impl TestCode {
    /// 一小段给人看的说明。
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::NotSupported => "节点版本太旧，不支持主动测试",
            Self::NodeOffline => "节点不在线",
            Self::NotConnected => "访问规则没有连通这两个节点",
            Self::RelayOnly => "节点只经 relay，不参加直连测试",
            Self::DirectOnly => "节点只用直连，不参加 relay 测试",
            Self::ThroughputOff => "节点关掉了吞吐测试",
            Self::NoRelay => "没有可用的 relay 路径",
            Self::NoDirect => "没有可用的直连",
            Self::NoCandidates => "没有对端的候选地址",
            Self::UdpBlocked => "UDP 不通",
            Self::MappingVaries => "端口映射随对端变化，打不了洞",
            Self::DialOutOnly => "两个节点都只拨出",
            Self::Timeout => "超时",
            Self::Unreachable => "这条路径现在走不通",
            Self::Refused => "对端拒绝了测量",
            Self::Busy => "节点正忙",
            Self::TooSoon => "刚测过，稍后再试",
            Self::Cancelled => "已取消",
            Self::NoReport => "节点没有按时报告结果",
            Self::StreamFailed => "测量中途断开",
            Self::InvalidPath => "指定的路径不能用",
            Self::Internal => "节点内部出错",
            Self::Unknown => "未知原因",
        }
    }
}

impl fmt::Display for TestCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// 一端看到的直连能力。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DirectSide {
    /// 这一端到对端有了验证过的直连。
    pub ok: bool,
    /// 开始时就已经有直连（这时不算耗时）。
    #[serde(default)]
    pub already: bool,
    /// 直连是怎么建立的。
    #[serde(default)]
    pub kind: Option<DirectKind>,
    /// 从开始尝试到验证通过，毫秒。
    #[serde(default)]
    pub elapsed_ms: Option<u32>,
    /// 没有成功时这一端看到的原因。
    #[serde(default)]
    pub failure: Option<TestCode>,
    /// 这一端的 UDP 通不通；不知道是 `null`。
    #[serde(default)]
    pub udp: Option<bool>,
    /// 这一端映射出来的端口是不是随对端变化；不知道是 `null`。
    #[serde(default)]
    pub mapping_varies: Option<bool>,
    /// 尝试时手上有几个对端的候选地址。
    #[serde(default)]
    pub candidates: u32,
}

/// 量过的一条路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMeasure {
    /// 直连时是对端的地址，经 relay 时是按顺序经过的 relay。
    pub route: Route,
    /// 为什么量这一条。
    #[serde(default)]
    pub picked_by: Vec<PathPick>,
    /// 量不了时的原因。
    #[serde(default)]
    pub failure: Option<TestCode>,
    #[serde(default)]
    pub latency: Option<Latency>,
    /// 按白皮书 §7.4 的公式、用这次量到的延迟、抖动和丢包算的代价，微秒。
    #[serde(default)]
    pub cost_us: Option<u64>,
    /// 各段的测量值，按经过的顺序。
    #[serde(default)]
    pub hops: Vec<Hop>,
    /// 经 relay 时，发起方到第一个 relay 的那一段走的是 UDP 还是 TCP。
    #[serde(default)]
    pub relay_leg: Option<LegTransport>,
    #[serde(default)]
    pub throughput: Option<Throughput>,
}

/// 为什么量这一条路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPick {
    /// 直连质量。
    Direct,
    /// 默认模式会经的 relay（不算直连时）。
    DirectFirst,
    /// 延迟优先模式会选的经 relay 的路径。
    LowestCost,
    /// 管理员指定的。
    Admin,
    #[serde(other)]
    Unknown,
}

/// 一列探测量到的延迟和丢包。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Latency {
    /// 发了几个探测，有几个得到了回答。
    pub probes: u32,
    pub answered: u32,
    /// 往返延迟（减掉了对端处理的时间），微秒。一个回答都没有时是 `null`。
    pub min_us: Option<u32>,
    pub avg_us: Option<u32>,
    pub p95_us: Option<u32>,
    /// 相邻两个往返延迟之差的绝对值的平均，微秒。
    pub jitter_us: Option<u32>,
    /// 往返的丢包，千分之几。
    pub loss_permille: u16,
    /// 去程（发起方到对端）和回程的丢包，千分之几：按对端说它收到了几个分出来；分不出来时是 `null`。
    pub loss_up_permille: Option<u16>,
    pub loss_down_permille: Option<u16>,
    /// 回答里有几个是沿同一条路径倒着回来的。不是全部时，回程走的是对端自己的路。
    pub same_path: u32,
    /// 两个探测之间隔多少毫秒，整列用了多少毫秒。
    pub interval_ms: u32,
    pub duration_ms: u32,
}

/// 路径上的一段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub from: NodeId,
    pub to: NodeId,
    /// 往返延迟，微秒；不知道时是 `null`。
    #[serde(default)]
    pub rtt_us: Option<u32>,
    #[serde(default)]
    pub jitter_us: Option<u32>,
    /// 丢包，千分之几；不知道时是 `null`。
    #[serde(default)]
    pub loss_permille: Option<u16>,
    pub source: HopSource,
}

/// 一段的测量值从哪里来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HopSource {
    /// 这一段起点的节点自己测的（到 relay 的会话，或者直连）。
    Measured,
    /// relay 之间的骨干矩阵。
    Backbone,
    /// 对端上报的它到 relay 的延迟（只有延迟）。
    Reported,
    /// 没有数据。
    #[serde(other)]
    Unknown,
}

/// 一条路径上的吞吐。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Throughput {
    /// 发起方到对端，和对端到发起方。没有量（或者量不了）的方向是 `null`。
    #[serde(default)]
    pub up: Option<Transfer>,
    #[serde(default)]
    pub down: Option<Transfer>,
    /// 量不了时的原因（对端拒绝、关掉了吞吐测试、流中途断了等）。
    #[serde(default)]
    pub failure: Option<TestCode>,
    /// 用的传输。现在总是 `"quic"`：测量流走这一对节点之间共用的连接，不经独占 TCP。
    #[serde(default)]
    pub transport: String,
    /// 连接用的拥塞控制：`"bbr"` 或 `"cubic"`。
    #[serde(default)]
    pub congestion: String,
    /// 每个方向的上限：字节数和毫秒，先到为止。
    #[serde(default)]
    pub limit_bytes: u64,
    #[serde(default)]
    pub limit_ms: u32,
    /// 发起方到对端的那条连接在整个测量期间都固定在这条路径上。
    #[serde(default)]
    pub pinned: bool,
}

/// 一个方向的吞吐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub bytes: u64,
    /// 从第一个字节到最后一个字节，毫秒。
    pub ms: u32,
    /// 平均每秒多少比特；用时不到 1 毫秒时是 `null`。
    #[serde(default)]
    pub bits_per_second: Option<u64>,
    /// 为什么停的：对方结束了、到了字节上限、到了时长上限。
    pub stopped_by: TransferEnd,
}

/// 一个方向的搬运为什么停。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferEnd {
    Finished,
    Bytes,
    Time,
    #[serde(other)]
    Unknown,
}

/// 一端发往对端的方向上，两种模式各会选哪条路。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// 这一端现在用的模式；比这个程序新的节点报了不认识的模式时是 `null`。
    #[serde(default)]
    pub mode: Option<RoutingMode>,
    /// 这一端现在走的路。
    #[serde(default)]
    pub current: Option<Route>,
    pub direct_first: Choice,
    pub lowest_cost: Choice,
    /// 考虑过的候选路径和代价，按代价从低到高。
    #[serde(default)]
    pub candidates: Vec<PathCandidate>,
}

/// 一种模式的选择。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// 选的路；没有能走的路时是 `null`。
    #[serde(default)]
    pub route: Option<Route>,
    pub reason: ChoiceCode,
    /// 这条路的代价，微秒；没有测量值时是 `null`。
    #[serde(default)]
    pub cost_us: Option<u64>,
}

/// 为什么选这条路。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceCode {
    /// 同一个局域网，局域网直连无条件优先。
    Lan,
    /// 默认模式：直连可用就走直连。
    Direct,
    /// 对端自己就是 relay，两个节点之间有直接的中转会话。
    PeerIsRelay,
    /// 对端挂在这一端自己的 relay 上。
    OwnRelay,
    /// 对端挂靠的 relay。
    PeerRelay,
    /// 候选里代价最低的。
    LowestCost,
    /// 没有能走的路。
    NoPath,
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ToController, ToNode};

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    #[test]
    fn a_test_order_has_a_stable_json_shape() {
        let order = TestOrder {
            test: 12,
            peer: node(3),
            role: TestRole::Responder,
            items: TestItems { direct: true, relay_quality: true, throughput: true, ..TestItems::default() },
            paths: vec![vec![node(8), node(9)]],
            window_secs: 60,
        };
        let wire = serde_json::to_value(ToNode::Test(Box::new(order.clone()))).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "type": "test", "test": 12, "peer": 3, "role": "responder",
                "items": { "direct": true, "direct_quality": false, "relay_quality": true, "route": false, "throughput": true },
                "paths": [[8, 9]], "window_secs": 60
            })
        );
        assert_eq!(serde_json::from_value::<ToNode>(wire).unwrap(), ToNode::Test(Box::new(order)));
        assert_eq!(serde_json::to_value(ToNode::TestEnd { test: 12 }).unwrap(), serde_json::json!({ "type": "test_end", "test": 12 }));
    }

    #[test]
    fn a_test_report_reads_with_missing_fields_and_unknown_codes() {
        let report: ToController = serde_json::from_value(serde_json::json!({
            "type": "test_report", "test": 12,
            "refused": { "code": "something_new", "node": 3 },
            "direct": { "ok": false, "failure": "udp_blocked" }
        }))
        .unwrap();
        let ToController::TestReport(report) = report else { panic!("不是测试的上报") };
        assert_eq!(report.refused, Some(TestProblem { code: TestCode::Unknown, node: Some(node(3)), message: String::new() }));
        assert_eq!(report.direct.as_ref().and_then(|direct| direct.failure), Some(TestCode::UdpBlocked));
        assert!(report.paths.is_empty() && !report.last && report.role.is_none());
    }

    #[test]
    fn a_measured_path_has_numbers_with_units_in_their_names() {
        let path = PathMeasure {
            route: Route::Relay { relays: vec![node(9)] },
            picked_by: vec![PathPick::DirectFirst, PathPick::LowestCost],
            failure: None,
            latency: Some(Latency { probes: 100, answered: 98, min_us: Some(40_100), loss_permille: 20, ..Latency::default() }),
            cost_us: Some(55_000),
            hops: vec![Hop {
                from: node(1),
                to: node(9),
                rtt_us: Some(20_000),
                jitter_us: None,
                loss_permille: Some(0),
                source: HopSource::Measured,
            }],
            relay_leg: Some(LegTransport::Udp),
            throughput: Some(Throughput {
                up: Some(Transfer { bytes: 20_000_000, ms: 4100, bits_per_second: Some(39_024_390), stopped_by: TransferEnd::Bytes }),
                down: None,
                failure: Some(TestCode::ThroughputOff),
                transport: "quic".to_owned(),
                congestion: "bbr".to_owned(),
                limit_bytes: 20_000_000,
                limit_ms: 5000,
                pinned: true,
            }),
        };
        let wire = serde_json::to_value(&path).unwrap();
        assert_eq!(wire["picked_by"], serde_json::json!(["direct_first", "lowest_cost"]));
        assert_eq!(wire["latency"]["min_us"], 40_100);
        assert_eq!(wire["hops"][0]["source"], "measured");
        assert_eq!(wire["throughput"]["up"]["stopped_by"], "bytes");
        assert_eq!(wire["throughput"]["failure"], "throughput_off");
        assert_eq!(serde_json::from_value::<PathMeasure>(wire).unwrap(), path);
    }

    #[test]
    fn every_test_code_has_a_short_text() {
        assert_eq!(TestCode::UdpBlocked.to_string(), "UDP 不通");
        assert_eq!(TestProblem::new(TestCode::TooSoon, None).message, "刚测过，稍后再试");
        assert_eq!(serde_json::to_value(TestCode::ThroughputOff).unwrap(), "throughput_off");
    }
}
