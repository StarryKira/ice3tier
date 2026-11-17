//! 控制面的代码（`i3t-model` 里的原因、状态、错误码）在两种语言里的写法。节点程序和管理命令行都显示它们，所以放在一起。
//!
//! 每一样是一个函数，两种语言各写一份（[`zh`]、[`en`]），里面对枚举的 `match` 是穷尽的：模型里多了一个值，两边都编译不过，
//! 直到补上它的写法。说到节点的地方由调用方给一个把编号写成名字的函数（[`Names`]）。

mod en;
mod zh;

use i3t_model::admin::diag::{CellState, ItemState, MatrixState, TestState};
use i3t_model::admin::{
    ApiError, CredentialState, ExitHealth, ForwardStatus, LaneBlock, PackageState, PathSource, ReachCause, ReleaseCause, SkipCause,
    TokenState, UpdateWait,
};
use i3t_model::control::diag::{ChoiceCode, HopSource, PathPick, TestCode, TransferEnd};
use i3t_model::control::{
    BindCause, DirectKind, InterceptCause, InterceptState, LegTransport, ListenState, NameService, RejectCause, RelayState, Route,
    RouteCause, RouteHint,
};
use i3t_model::update::{SelfUpdateBlocker, UpdateCause, UpdateStatus};
use i3t_proto::NodeId;

use crate::Lang;

/// 把节点编号写成名字：知道名字的写名字，不知道的写 `#编号`。
pub type Names<'a> = &'a dyn Fn(NodeId) -> String;

/// 控制面的代码在一种语言里的写法。
#[derive(Debug)]
pub struct Codes {
    /// 一条路：`直连 203.0.113.7:20330`、`经 hk-1`、`不可达`。
    pub route: fn(&Route, Names<'_>) -> String,
    /// 为什么走现在这条路、为什么换到它。
    pub route_cause: fn(&RouteCause, Names<'_>) -> String,
    pub route_hint: fn(RouteHint) -> &'static str,
    pub direct_kind: fn(DirectKind) -> &'static str,
    pub leg: fn(LegTransport) -> &'static str,
    pub relay_state: fn(RelayState) -> &'static str,
    /// 一个监听的状态，一短句：`监听在 127.0.0.1:1080`、`端口被占用`。
    pub listen_state: fn(&ListenState) -> String,
    pub bind_cause: fn(BindCause) -> &'static str,
    /// 节点上的更新进行到哪一步，一短句。
    pub update_status: fn(&UpdateStatus) -> String,
    pub update_cause: fn(&UpdateCause) -> String,
    pub blocker: fn(SelfUpdateBlocker) -> &'static str,
    pub update_wait: fn(UpdateWait) -> &'static str,
    pub release_cause: fn(&ReleaseCause) -> String,
    pub package_state: fn(PackageState) -> &'static str,
    pub skip_cause: fn(&SkipCause) -> String,
    pub reach_cause: fn(ReachCause) -> &'static str,
    /// controller 为什么不接受这个节点（`i3t join` 被拒绝时）。
    pub reject_cause: fn(RejectCause) -> &'static str,
    /// 管理 API 的错误。中文直接用 controller 的原话；别的语言按错误码和 `what`、`field` 写。
    pub api_error: fn(&ApiError) -> String,
    pub lane_block: fn(&LaneBlock, Names<'_>) -> String,
    pub forward_status: fn(ForwardStatus) -> &'static str,
    pub token_state: fn(TokenState) -> &'static str,
    pub credential_state: fn(CredentialState) -> &'static str,
    pub exit_health: fn(ExitHealth) -> &'static str,
    pub path_source: fn(PathSource) -> &'static str,
    pub test_code: fn(TestCode) -> &'static str,
    pub test_state: fn(TestState) -> &'static str,
    pub item_state: fn(ItemState) -> &'static str,
    pub matrix_state: fn(MatrixState) -> &'static str,
    pub cell_state: fn(CellState) -> &'static str,
    pub choice: fn(ChoiceCode) -> &'static str,
    pub hop_source: fn(HopSource) -> &'static str,
    pub path_pick: fn(PathPick) -> &'static str,
    pub transfer_end: fn(TransferEnd) -> &'static str,
    /// 透明访问的状态，一短句：开着（拦截什么、名字怎么解析）、没开、开不起来（为什么）。
    pub intercept_state: fn(&InterceptState) -> String,
    pub intercept_cause: fn(&InterceptCause) -> String,
    pub name_service: fn(&NameService) -> String,
}

impl Lang {
    /// 控制面的代码在这种语言里的写法。
    #[must_use]
    pub const fn codes(self) -> &'static Codes {
        match self {
            Self::ZhCn => &zh::CODES,
            Self::English => &en::CODES,
        }
    }
}

/// 一组 relay 写成名字，用箭头连起来：`hk-1 → la-1`。
fn relays(relays: &[NodeId], names: Names<'_>) -> String {
    relays.iter().map(|relay| names(*relay)).collect::<Vec<_>>().join(" → ")
}

/// 代价的微秒数写成毫秒，取整。
fn cost_ms(micros: u64) -> u64 {
    (micros + 500) / 1000
}

#[cfg(test)]
mod tests {
    use i3t_model::admin::ErrorCode;
    use i3t_model::update::{FailureKind, Version};

    use super::*;
    use crate::cli::has_chinese;

    fn name(id: NodeId) -> String {
        format!("n{id}")
    }

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).expect("测试里的编号不是 0")
    }

    /// 每一样都写一遍，英文里不能有汉字，中文里要有（说明真的翻译了，不是照抄的代码）。
    fn samples(codes: &Codes) -> Vec<String> {
        let version = |text: &str| text.parse::<Version>().expect("合法的版本号");
        let names: Names<'_> = &name;
        let mut out = vec![
            (codes.route)(&Route::Relay { relays: vec![node(3), node(4)] }, names),
            (codes.route)(&Route::None, names),
            (codes.listen_state)(&ListenState::Failed { detail: "x".into(), cause: Some(BindCause::PermissionDenied) }),
            (codes.listen_state)(&ListenState::Listening { local: "127.0.0.1:1080".parse().expect("地址") }),
            (codes.update_status)(&UpdateStatus::Failed {
                version: Some(version("0.2.0")),
                kind: FailureKind::VersionRejected,
                reason: "原话".into(),
                cause: Some(UpdateCause::SelfCheck { exit_code: Some(1), output: "boom".into() }),
            }),
            (codes.update_status)(&UpdateStatus::WaitingForIdle { version: version("0.2.0") }),
            (codes.release_cause)(&ReleaseCause::Prerelease { version: version("0.3.0-rc.1") }),
            (codes.skip_cause)(&SkipCause::UpToDate { running: version("0.2.0"), latest: version("0.2.0") }),
            (codes.lane_block)(&LaneBlock::NodeTooOld { node: node(2) }, names),
            (codes.api_error)(&ApiError { code: ErrorCode::NotFound, message: "没有".into(), what: Some("node".into()), field: None }),
            (codes.api_error)(&ApiError {
                code: ErrorCode::NotFound,
                message: "没有分组 #9".into(),
                what: Some("group".into()),
                field: Some("groups[1]".into()),
            }),
        ];
        let intercept = [
            InterceptState::Off,
            InterceptState::On { udp: true, source_address: true, names: NameService::Resolved },
            InterceptState::On { udp: false, source_address: true, names: NameService::NoResolver },
            InterceptState::Failed { cause: InterceptCause::NoPermission { missing: vec!["CAP_NET_ADMIN".into()] } },
            InterceptState::Failed { cause: InterceptCause::ProgramMissing { program: "nft".into() } },
            InterceptState::Failed { cause: InterceptCause::CommandFailed { program: "nft".into(), detail: "x".into() } },
            InterceptState::Failed { cause: InterceptCause::PortInUse { address: "127.0.0.1:1".parse().expect("地址") } },
            InterceptState::Failed {
                cause: InterceptCause::BindFailed { address: "127.0.0.1:1".parse().expect("地址"), detail: "x".into() },
            },
            InterceptState::Failed { cause: InterceptCause::TableInUse { table: 2033 } },
            InterceptState::Failed { cause: InterceptCause::LinkInUse { link: "i3t0".into() } },
            InterceptState::Failed { cause: InterceptCause::RangeInUse { with: "192.168.1.5/24 dev eth0".into() } },
            InterceptState::Failed { cause: InterceptCause::Unknown },
            InterceptState::Unsupported,
            InterceptState::Unknown,
        ];
        out.extend(intercept.iter().map(codes.intercept_state));
        out.push((codes.name_service)(&NameService::Failed {
            cause: InterceptCause::PortInUse { address: "10.66.0.1:53".parse().expect("地址") },
        }));
        let causes = [
            RouteCause::NoRouteYet,
            RouteCause::TestPinned,
            RouteCause::TestEnded,
            RouteCause::DirectVerified,
            RouteCause::DirectLost { hint: RouteHint::DirectTimeouts },
            RouteCause::RelayReady,
            RouteCause::LegChanged { transport: Some(LegTransport::Tcp) },
            RouteCause::RelayChanged { hint: RouteHint::Shortcut, previous: vec![node(5)] },
            RouteCause::Unreachable { hint: RouteHint::RelayLost },
            RouteCause::PeerRemoved,
            RouteCause::Lan,
            RouteCause::LowestCost { candidates: 3, cost_us: Some(12_400) },
            RouteCause::Failover { hint: RouteHint::RelayDraining, candidates: 2, cost_us: None },
            RouteCause::Cheaper { previous: Route::None, previous_cost_us: 20_000, cost_us: 12_000, windows: 3 },
        ];
        out.extend(causes.iter().map(|cause| (codes.route_cause)(cause, names)));
        out.extend(
            [UpdateCause::BadSignature, UpdateCause::Cancelled, UpdateCause::NotConfirmed { minutes: 5 }]
                .iter()
                .map(|cause| (codes.update_cause)(cause)),
        );
        out.extend([TestCode::UdpBlocked, TestCode::Timeout].map(|code| (codes.test_code)(code).to_owned()));
        out.push((codes.reach_cause)(ReachCause::Refused).to_owned());
        out
    }

    #[test]
    fn english_has_no_chinese_and_chinese_is_translated() {
        for text in samples(Lang::English.codes()) {
            assert!(!has_chinese(&text), "英文里有中文：{text}");
            assert!(!text.is_empty());
        }
        let chinese = samples(Lang::ZhCn.codes());
        assert!(chinese.iter().filter(|text| has_chinese(text)).count() >= chinese.len() - 1, "{chinese:?}");
    }

    #[test]
    fn english_counts_read_right_for_one() {
        let codes = Lang::English.codes();
        let names: Names<'_> = &name;
        let route_causes = [
            RouteCause::LowestCost { candidates: 1, cost_us: Some(1_000) },
            RouteCause::Failover { hint: RouteHint::RelayDraining, candidates: 1, cost_us: Some(1_000) },
            RouteCause::Cheaper { previous: Route::None, previous_cost_us: 1_000, cost_us: 1_000, windows: 1 },
        ];
        let mut said: Vec<String> = route_causes.iter().map(|cause| (codes.route_cause)(cause, names)).collect();
        let version = || "0.2.0".parse::<Version>().expect("合法的版本号");
        said.push((codes.update_status)(&UpdateStatus::Downloading { version: version(), received: 1, total: 1 }));
        said.extend(
            [
                UpdateCause::Corrupt { expected_size: 1, actual_size: 1 },
                UpdateCause::NotConfirmed { minutes: 1 },
                UpdateCause::StartsUnconfirmed { starts: 1 },
            ]
            .iter()
            .map(|cause| (codes.update_cause)(cause)),
        );
        said.push((codes.release_cause)(&ReleaseCause::Mismatch { file: "i3t-linux-x86_64".into(), expected_size: 1, actual_size: 1 }));
        said.push((codes.intercept_state)(&InterceptState::Failed { cause: InterceptCause::TableInUse { table: 1 } }));
        for text in said {
            assert_eq!(crate::cli::plural_after_one(&text), None, "{text}");
        }
    }

    #[test]
    fn group_errors_say_it_is_a_group_in_english() {
        let error =
            |code, what: &str, field: &str| ApiError { code, message: "原话".into(), what: Some(what.into()), field: Some(field.into()) };
        let english = Lang::English.codes();
        let missing = (english.api_error)(&error(ErrorCode::NotFound, "group", "groups[1]"));
        assert!(missing.contains("group") && missing.contains("groups[1]"), "{missing}");
        assert!((english.api_error)(&error(ErrorCode::NameTaken, "group", "name")).starts_with("Group name"));
        assert!((english.api_error)(&error(ErrorCode::InvalidRequest, "builtin", "group")).contains("All"));
        assert_eq!((Lang::ZhCn.codes().api_error)(&error(ErrorCode::NotFound, "group", "groups[1]")), "原话", "中文用 controller 的原话");
    }

    #[test]
    fn values_named_by_a_cause_appear_in_its_sentence() {
        for lang in Lang::ALL {
            let codes = lang.codes();
            let names: Names<'_> = &name;
            let changed = (codes.route_cause)(&RouteCause::RelayChanged { hint: RouteHint::RelayDraining, previous: vec![node(5)] }, names);
            assert!(changed.contains("n5"), "{changed}");
            let cheaper = (codes.route_cause)(
                &RouteCause::Cheaper { previous: Route::None, previous_cost_us: 20_000, cost_us: 12_400, windows: 3 },
                names,
            );
            assert!(cheaper.contains("20") && cheaper.contains("12"), "{cheaper}");
            let check = (codes.update_cause)(&UpdateCause::SelfCheck { exit_code: Some(3), output: "boom".into() });
            assert!(check.contains('3') && check.contains("boom"), "{check}");
        }
    }
}
