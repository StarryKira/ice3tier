//! 控制面的代码的英文写法。

use i3t_model::admin::diag::{CellState, ItemState, MatrixState, TestState};
use i3t_model::admin::{
    ApiError, CredentialState, ErrorCode, ExitHealth, ForwardStatus, LaneBlock, PackageState, PathSource, ReachCause, ReleaseCause,
    SkipCause, TokenState, UpdateWait,
};
use i3t_model::control::diag::{ChoiceCode, HopSource, PathPick, TestCode, TransferEnd};
use i3t_model::control::{BindCause, DirectKind, LegTransport, ListenState, RejectCause, RelayState, Route, RouteCause, RouteHint};
use i3t_model::update::{FailureKind, SelfUpdateBlocker, UpdateCause, UpdateStatus};

use super::{Codes, Names, cost_ms, relays};

pub(super) static CODES: Codes = Codes {
    route,
    route_cause,
    route_hint,
    direct_kind,
    leg,
    relay_state,
    listen_state,
    bind_cause,
    update_status,
    update_cause,
    blocker,
    update_wait,
    release_cause,
    package_state,
    skip_cause,
    reach_cause,
    reject_cause,
    api_error,
    lane_block,
    forward_status,
    token_state,
    credential_state,
    exit_health,
    path_source,
    test_code,
    test_state,
    item_state,
    matrix_state,
    cell_state,
    choice,
    hop_source,
    path_pick,
    transfer_end,
};

fn route(route: &Route, names: Names<'_>) -> String {
    match route {
        Route::None => "unreachable".to_owned(),
        Route::Direct { addr } => format!("direct {addr}"),
        Route::Relay { relays: path } => format!("via {}", relays(path, names)),
    }
}

fn route_cause(cause: &RouteCause, names: Names<'_>) -> String {
    let cost = |cost_us: Option<u64>| cost_us.map(|cost| format!(", cost {} ms", cost_ms(cost))).unwrap_or_default();
    match cause {
        RouteCause::NoRouteYet => "No route yet".to_owned(),
        RouteCause::TestPinned => "Pinned by an active test".to_owned(),
        RouteCause::TestEnded => "Active test ended, back to the chosen route".to_owned(),
        RouteCause::DirectVerified => "Direct path verified".to_owned(),
        RouteCause::DirectLost { hint } => format!("{}, back to relay", route_hint(*hint)),
        RouteCause::RelayReady => "Relay session ready".to_owned(),
        RouteCause::LegChanged { transport: Some(transport) } => format!("Relay session moved to {}", leg(*transport)),
        RouteCause::LegChanged { transport: None } => "Moved to this node's own relay".to_owned(),
        RouteCause::RelayChanged { hint, previous } => format!("{}, was via {}", route_hint(*hint), relays(previous, names)),
        RouteCause::Unreachable { hint } => route_hint(*hint).to_owned(),
        RouteCause::PeerRemoved => "Peer no longer in this node's configuration".to_owned(),
        RouteCause::Lan => "Same LAN, direct first".to_owned(),
        RouteCause::LowestCost { candidates, cost_us } => format!("Lowest cost of {candidates} candidates{}", cost(*cost_us)),
        RouteCause::Failover { hint, candidates, cost_us } => {
            format!("{}, switched at once to the lowest cost of {candidates} candidates{}", route_hint(*hint), cost(*cost_us))
        }
        RouteCause::Cheaper { previous, previous_cost_us, cost_us, windows } => format!(
            "Cheaper than {} for {windows} windows: {} ms, was {} ms",
            route(previous, names),
            cost_ms(*cost_us),
            cost_ms(*previous_cost_us)
        ),
        RouteCause::Unknown => "Unknown reason".to_owned(),
    }
}

const fn route_hint(hint: RouteHint) -> &'static str {
    match hint {
        RouteHint::DirectTimeouts => "Direct probes timed out",
        RouteHint::DirectLoss => "Direct path loss above 20%",
        RouteHint::DirectStale => "Direct address expired",
        RouteHint::DirectUnavailable => "Direct path unavailable",
        RouteHint::RelayDraining => "Relay going offline",
        RouteHint::RelayOverloaded => "Relay overloaded",
        RouteHint::RelayPathBroken => "Multi-relay path broken",
        RouteHint::PeerNotAttached => "Peer not attached to that relay",
        RouteHint::PathProbeSilent => "Multi-relay path probes unanswered",
        RouteHint::PreferredRelayReady => "Peer's preferred relay ready",
        RouteHint::Shortcut => "Shorter relay session available",
        RouteHint::RelayUnavailable => "Previous relay unavailable",
        RouteHint::RelayLost => "Relay session lost",
        RouteHint::RouteFailed => "Previous route failed",
        RouteHint::NoPath => "No usable route",
        RouteHint::Unknown => "Unknown reason",
    }
}

const fn direct_kind(kind: DirectKind) -> &'static str {
    match kind {
        DirectKind::Lan => "LAN",
        DirectKind::Ipv6 => "IPv6",
        DirectKind::Reachable => "reachable",
        DirectKind::Punched => "hole punched",
        DirectKind::Unknown => "unknown",
    }
}

const fn leg(transport: LegTransport) -> &'static str {
    match transport {
        LegTransport::Udp => "UDP",
        LegTransport::Tcp => "TCP",
        LegTransport::Unknown => "unknown",
    }
}

const fn relay_state(state: RelayState) -> &'static str {
    match state {
        RelayState::Serving => "serving",
        RelayState::Overloaded => "overloaded",
        RelayState::Draining => "going offline",
        RelayState::Unknown => "unknown",
    }
}

fn listen_state(state: &ListenState) -> String {
    let failed = |detail: &str, cause: Option<BindCause>| match cause {
        Some(cause) => bind_cause(cause).to_owned(),
        None => format!("Not bound: {detail}"),
    };
    match state {
        ListenState::Listening { local } => format!("Listening on {local}"),
        ListenState::PortInUse { .. } => bind_cause(BindCause::AddressInUse).to_owned(),
        ListenState::Failed { detail, cause } | ListenState::Unsupported { detail, cause } => failed(detail, *cause),
        ListenState::Virtual => "On the virtual IP".to_owned(),
        ListenState::Unknown => "Unknown".to_owned(),
    }
}

const fn bind_cause(cause: BindCause) -> &'static str {
    match cause {
        BindCause::AddressInUse => "Port in use",
        BindCause::PermissionDenied => "Permission denied",
        BindCause::AddressNotAvailable => "Address not on this machine",
        BindCause::VirtualPortZero => "Port required on the virtual IP",
        BindCause::Other => "Not bound",
        BindCause::Unknown => "Unknown reason",
    }
}

fn update_status(status: &UpdateStatus) -> String {
    match status {
        UpdateStatus::Idle => "Not updating".to_owned(),
        UpdateStatus::Downloading { version, received, total } => format!("Downloading {version} ({received}/{total} bytes)"),
        UpdateStatus::Verifying { version } => format!("Verifying {version}"),
        UpdateStatus::SelfCheck { version } => format!("Self-checking {version}"),
        UpdateStatus::WaitingForIdle { version } => format!("{version} ready, waiting for idle"),
        UpdateStatus::Draining { version } => format!("{version} ready, draining"),
        UpdateStatus::Installing { version } => format!("Installing {version}"),
        UpdateStatus::Restarting { version } => format!("Restarting into {version}"),
        UpdateStatus::Confirmed { version } => format!("Updated to {version}"),
        UpdateStatus::Failed { version, kind, cause, .. } => {
            let version = version.as_ref().map_or_else(|| "the new version".to_owned(), ToString::to_string);
            let why = cause.as_ref().map_or_else(|| "reason not reported".to_owned(), update_cause);
            let kind = match kind {
                FailureKind::VersionRejected => "won't retry this version",
                FailureKind::NotInstalled => "will retry",
            };
            format!("Update to {version} failed: {why} ({kind})")
        }
        UpdateStatus::Unknown => "Unknown".to_owned(),
    }
}

fn update_cause(cause: &UpdateCause) -> String {
    match cause {
        UpdateCause::BadSignature => "Manifest signature invalid".to_owned(),
        UpdateCause::BadManifest => "Manifest unreadable".to_owned(),
        UpdateCause::CannotSelfUpdate { blocker: why } => blocker(*why).to_owned(),
        UpdateCause::NotNewer { offered, running } => format!("{offered} isn't newer than {running}"),
        UpdateCause::Prerelease { offered, .. } => format!("{offered} is a pre-release"),
        UpdateCause::FailedBefore { version } => format!("{version} failed here before"),
        UpdateCause::NoPackage { version, platform } => format!("{version} has no {platform} package"),
        UpdateCause::RestartPending { installed } => format!("{installed} installed, restart pending"),
        UpdateCause::Unavailable { detail } => format!("Package unavailable: {detail}"),
        UpdateCause::Corrupt { expected_size, actual_size } => {
            format!("Package doesn't match the manifest ({actual_size} bytes, expected {expected_size})")
        }
        UpdateCause::SelfCheck { exit_code: Some(code), output } => format!("Self-check failed (exit code {code}): {output}"),
        UpdateCause::SelfCheck { exit_code: None, output } => format!("Self-check failed: {output}"),
        UpdateCause::Cancelled => "No moment to switch programs".to_owned(),
        UpdateCause::Io { path, error } => format!("Couldn't read or write {path}: {error}"),
        UpdateCause::StateUnreadable { path } => format!("Update state file {path} unreadable"),
        UpdateCause::NotConfirmed { minutes } => format!("New program didn't reach the controller in {minutes} min, rolled back"),
        UpdateCause::StartsUnconfirmed { starts } => format!("New program didn't reach the controller in {starts} starts, rolled back"),
        UpdateCause::Unknown => "Unknown reason".to_owned(),
    }
}

const fn blocker(blocker: SelfUpdateBlocker) -> &'static str {
    match blocker {
        SelfUpdateBlocker::Container => "Runs in a container; update the image",
        SelfUpdateBlocker::PackageManager => "Installed by a package manager; update with it",
        SelfUpdateBlocker::ReadOnlyDir => "Program directory is read-only",
    }
}

const fn update_wait(wait: UpdateWait) -> &'static str {
    match wait {
        UpdateWait::Package => "Waiting for the package",
        UpdateWait::RelayTurn => "Waiting for the previous relay",
        UpdateWait::Unknown => "Waiting",
    }
}

fn release_cause(cause: &ReleaseCause) -> String {
    match cause {
        ReleaseCause::InvalidSource => "Release source malformed".to_owned(),
        ReleaseCause::FetchFailed { url, error } => format!("Couldn't fetch {url}: {error}"),
        ReleaseCause::Io { path, error } => format!("Couldn't read or write {path}: {error}"),
        ReleaseCause::BadSignatureFile => "Signature file isn't a signature".to_owned(),
        ReleaseCause::BadSignature => "Manifest signature invalid".to_owned(),
        ReleaseCause::BadManifest => "Manifest unreadable".to_owned(),
        ReleaseCause::Prerelease { version } => format!("Latest {version} is a pre-release; skipped"),
        ReleaseCause::BadFileName { file } => format!("Bad file name: {file}"),
        ReleaseCause::Mismatch { file, expected_size, actual_size } => {
            format!("{file} doesn't match the manifest ({actual_size} bytes, expected {expected_size})")
        }
        ReleaseCause::Internal => "Controller internal error".to_owned(),
        ReleaseCause::Unknown => "Unknown reason".to_owned(),
    }
}

const fn package_state(state: PackageState) -> &'static str {
    match state {
        PackageState::NotNeeded => "not needed",
        PackageState::Fetching => "downloading",
        PackageState::Cached => "downloaded",
        PackageState::Failed => "failed",
        PackageState::Unknown => "unknown",
    }
}

fn skip_cause(cause: &SkipCause) -> String {
    match cause {
        SkipCause::Offline => "Offline".to_owned(),
        SkipCause::TooOld => "Too old to update itself".to_owned(),
        SkipCause::CannotSelfUpdate { blocker: Some(why) } => blocker(*why).to_owned(),
        SkipCause::CannotSelfUpdate { blocker: None } => "Can't update itself".to_owned(),
        SkipCause::NoRelease => "No release found yet".to_owned(),
        SkipCause::ControllerTooOld { min_controller, .. } => format!("Update the controller to {min_controller} first"),
        SkipCause::UnreadableVersion { version } => format!("Unreadable version: {version}"),
        SkipCause::UpToDate { running, .. } => format!("Up to date ({running})"),
        SkipCause::NoPackage { platform, .. } => format!("No {platform} package"),
        SkipCause::Unknown => "Unknown reason".to_owned(),
    }
}

const fn reach_cause(cause: ReachCause) -> &'static str {
    match cause {
        ReachCause::Refused => "Connection refused",
        ReachCause::Timeout => "Timed out",
        ReachCause::Unreachable => "Network unreachable",
        ReachCause::Local => "Controller-side error",
        ReachCause::Other => "Couldn't connect",
        ReachCause::Unknown => "Unknown reason",
    }
}

const fn reject_cause(cause: RejectCause) -> &'static str {
    match cause {
        RejectCause::BadToken => "Invalid enrollment token: unknown, used or expired",
        RejectCause::UnknownKey => "This key isn't registered; the node may have been deleted. Join again",
        RejectCause::KeyAlreadyEnrolled => "This key is already registered",
        RejectCause::PredecessorOnline => "The node to replace is still online",
        RejectCause::AddressUnavailable => "The token's preset virtual IP isn't available",
        RejectCause::RangeExhausted => "The address range is full",
        RejectCause::UnexpectedFirstMessage => "Unexpected first message",
        RejectCause::Unknown => "Unknown reason",
    }
}

/// 按错误码写，`what` 和 `field` 补上细节；controller 的原话是中文的，不用。
fn api_error(error: &ApiError) -> String {
    let what = error.what.as_deref();
    let field = error.field.as_deref().map(|field| format!(" ({field})")).unwrap_or_default();
    let thing = |what: Option<&str>| match what {
        Some("network") => "network",
        Some("node") => "node",
        Some("token") => "enrollment token",
        Some("rule") => "rule",
        Some("credential") => "credential",
        Some("listener") => "listener",
        Some("forward") => "port forward",
        Some("pair") => "node pair setting",
        Some("paused_version") => "paused version",
        Some("exit") => "exit",
        Some("test") => "test",
        Some("matrix") => "test matrix",
        Some("endpoint") => "API endpoint",
        _ => "item",
    };
    match error.code {
        ErrorCode::Unauthorized => "Admin token missing or wrong".to_owned(),
        ErrorCode::NotFound => format!("No such {}{field}", thing(what)),
        ErrorCode::InvalidRequest => {
            let problem = match what {
                Some("too_small") => "too small",
                Some("zero" | "zero_port") => "can't be 0",
                Some("too_many") => "too many",
                Some("too_long") => "too long",
                Some("out_of_range") => "out of range",
                Some("empty") => "empty",
                Some("malformed") => "malformed",
                Some("all_digits") => "can't be all digits",
                Some("same" | "same_node") => "must differ",
                Some("builtin") => "built-in tag can't change",
                Some("required") => "required here",
                Some("missing") => "missing",
                Some("not_relay") => "not an enabled relay",
                Some("not_connectable") => "not a connectable address",
                Some("not_allowed") => "not allowed",
                Some("duplicate") => "repeated",
                Some("with_successor") => "can't be set together with a successor",
                Some("no_default") => "no free default range",
                Some("needs_quality") => "needs a quality test",
                Some("no_address") => "at least one UDP or TCP address required",
                Some("revoked") => "credential revoked",
                Some("method") => "method not allowed",
                Some(_) | None => "invalid",
            };
            format!("Invalid request{field}: {problem}")
        }
        ErrorCode::AddressOutOfRange => format!("Virtual IP outside the address range{field}"),
        ErrorCode::NameTaken => format!("Name already taken{field}"),
        ErrorCode::NameCoolingDown => format!("Name belongs to a recently deleted node{field}"),
        ErrorCode::AddressTaken => format!("Virtual IP already taken{field}"),
        ErrorCode::AddressCoolingDown => format!("Virtual IP belongs to a recently deleted node{field}"),
        ErrorCode::RangeOverlaps => "Address range overlaps another network".to_owned(),
        ErrorCode::RangeExhausted => "Address range full".to_owned(),
        ErrorCode::RangeNotSuperset => "New address range must contain the old one".to_owned(),
        ErrorCode::NetworkNotEmpty => "Network still has nodes".to_owned(),
        ErrorCode::NotDeclared => {
            let command = match what {
                Some("relay") => "i3t set can-relay on",
                Some("exit") => "i3t set can-exit on",
                Some("forward") => "i3t set can-forward on",
                _ => "i3t set can-… on",
            };
            format!("Node hasn't declared it. Run '{command}' on the node first.")
        }
        ErrorCode::PredecessorOnline => "Node to replace is still online".to_owned(),
        ErrorCode::Conflict => format!("Conflicts with an existing {}{field}", thing(what)),
        ErrorCode::NodeOffline => "Node offline".to_owned(),
        ErrorCode::CannotSelfUpdate => {
            let why = match what {
                Some("container") => blocker(SelfUpdateBlocker::Container),
                Some("package_manager") => blocker(SelfUpdateBlocker::PackageManager),
                Some("read_only_dir") => blocker(SelfUpdateBlocker::ReadOnlyDir),
                Some("too_old") => "Too old to update itself",
                Some(_) | None => "Can't update itself",
            };
            format!("Node can't update itself: {why}")
        }
        ErrorCode::NoUpdate => {
            let why = match what {
                Some("no_release") => "no release found yet",
                Some("controller_too_old") => "update the controller first",
                Some("unreadable_version") => "its version is unreadable",
                Some("up_to_date") => "already up to date",
                Some("no_package") => "no package for its platform",
                Some(_) | None => "nothing to update to",
            };
            format!("No update: {why}")
        }
        ErrorCode::TooSoon => "Too soon; try again shortly".to_owned(),
        ErrorCode::Busy => "Busy or already finished".to_owned(),
        ErrorCode::Internal => "Controller internal error; see its log".to_owned(),
        // 比这个程序新的 controller 才有的错误码：只能说出错了，原话是中文的。
        _ => "Request failed".to_owned(),
    }
}

fn lane_block(block: &LaneBlock, names: Names<'_>) -> String {
    match block {
        LaneBlock::NoRelay => "Neither node is a relay".to_owned(),
        LaneBlock::NoTcpAddress { node } => format!("{} has no TCP address", names(*node)),
        LaneBlock::NodeTooOld { node } => format!("{} is too old", names(*node)),
        LaneBlock::NotConnected => "No rule connects these nodes".to_owned(),
    }
}

const fn forward_status(status: ForwardStatus) -> &'static str {
    match status {
        ForwardStatus::TargetNotDeclared => "target not declared",
        ForwardStatus::Disabled => "disabled",
        ForwardStatus::AccessDenied => "no access",
        ForwardStatus::ListenerOffline => "listener offline",
        ForwardStatus::TargetOffline => "target offline",
        ForwardStatus::Active => "in effect",
    }
}

const fn token_state(state: TokenState) -> &'static str {
    match state {
        TokenState::Pending => "unused",
        TokenState::Used => "used",
        TokenState::Expired => "expired",
        TokenState::Unknown => "unknown",
    }
}

const fn credential_state(state: CredentialState) -> &'static str {
    match state {
        CredentialState::Active => "active",
        CredentialState::Exhausted => "limit reached",
        CredentialState::Revoked => "revoked",
        CredentialState::Unknown => "unknown",
    }
}

const fn exit_health(health: ExitHealth) -> &'static str {
    match health {
        ExitHealth::Ok => "ok",
        ExitHealth::Degraded => "degraded",
        ExitHealth::Unknown => "unknown",
    }
}

const fn path_source(source: PathSource) -> &'static str {
    match source {
        PathSource::Query => "asked",
        PathSource::Report => "reported",
        PathSource::None => "no data",
        PathSource::NotPeers => "not connected",
        PathSource::Unknown => "unknown",
    }
}

const fn test_code(code: TestCode) -> &'static str {
    match code {
        TestCode::NotSupported => "Too old for active tests",
        TestCode::NodeOffline => "Offline",
        TestCode::NotConnected => "No rule connects them",
        TestCode::RelayOnly => "Relay only; no direct tests",
        TestCode::DirectOnly => "Direct only; no relay tests",
        TestCode::ThroughputOff => "Throughput tests off",
        TestCode::NoRelay => "No relay path",
        TestCode::NoDirect => "No direct path",
        TestCode::NoCandidates => "No peer addresses",
        TestCode::UdpBlocked => "UDP blocked",
        TestCode::MappingVaries => "Port mapping varies by peer; no hole punching",
        TestCode::DialOutOnly => "Both nodes dial out only",
        TestCode::Timeout => "Timed out",
        TestCode::Unreachable => "Path unreachable",
        TestCode::Refused => "Peer refused the test",
        TestCode::Busy => "Node busy",
        TestCode::TooSoon => "Tested moments ago",
        TestCode::Cancelled => "Cancelled",
        TestCode::NoReport => "No report in time",
        TestCode::StreamFailed => "Test stream broke",
        TestCode::InvalidPath => "Path not usable",
        TestCode::Internal => "Internal error",
        TestCode::Unknown => "Unknown reason",
    }
}

const fn test_state(state: TestState) -> &'static str {
    match state {
        TestState::Running => "running",
        TestState::Done => "done",
        TestState::Failed => "failed",
        TestState::Cancelled => "cancelled",
        TestState::Unknown => "unknown",
    }
}

const fn item_state(state: ItemState) -> &'static str {
    match state {
        ItemState::Pending => "pending",
        ItemState::Done => "done",
        ItemState::Skipped => "skipped",
        ItemState::Failed => "failed",
        ItemState::Unknown => "unknown",
    }
}

const fn matrix_state(state: MatrixState) -> &'static str {
    match state {
        MatrixState::Running => "running",
        MatrixState::Done => "done",
        MatrixState::Cancelled => "cancelled",
        MatrixState::Unknown => "unknown",
    }
}

const fn cell_state(state: CellState) -> &'static str {
    match state {
        CellState::Pending => "pending",
        CellState::Running => "running",
        CellState::Done => "done",
        CellState::Skipped => "skipped",
        CellState::Failed => "failed",
        CellState::Cancelled => "cancelled",
        CellState::Unknown => "unknown",
    }
}

const fn choice(code: ChoiceCode) -> &'static str {
    match code {
        ChoiceCode::Lan => "LAN direct first",
        ChoiceCode::Direct => "direct available",
        ChoiceCode::PeerIsRelay => "peer is a relay",
        ChoiceCode::OwnRelay => "peer attached here",
        ChoiceCode::PeerRelay => "peer's relay",
        ChoiceCode::LowestCost => "lowest cost",
        ChoiceCode::NoPath => "no route",
        ChoiceCode::Unknown => "unknown",
    }
}

const fn hop_source(source: HopSource) -> &'static str {
    match source {
        HopSource::Measured => "measured",
        HopSource::Backbone => "backbone",
        HopSource::Reported => "reported",
        HopSource::Unknown => "unknown",
    }
}

const fn path_pick(pick: PathPick) -> &'static str {
    match pick {
        PathPick::Direct => "direct",
        PathPick::DirectFirst => "default",
        PathPick::LowestCost => "lowest latency",
        PathPick::Admin => "given",
        PathPick::Unknown => "unknown",
    }
}

const fn transfer_end(end: TransferEnd) -> &'static str {
    match end {
        TransferEnd::Finished => "finished",
        TransferEnd::Bytes => "byte limit",
        TransferEnd::Time => "time limit",
        TransferEnd::Unknown => "unknown",
    }
}
