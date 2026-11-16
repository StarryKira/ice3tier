//! 把管理 API 的应答打印成给人看的样子。`--json` 时不经过这里。

use i3t_model::admin::{
    AccessCheck, AccessGraph, BackboneView, CreatedCredential, CreatedEnrollToken, Credential, Direction, EnrollTokenInfo, ExitView,
    ForcedUpdates, Forward, LaneBlock, LaneEffect, ListenStatus, Listener, LogEntry, Network, Node, NodeAccess, NodeDetail, NodeListeners,
    NodeRef, NodeUpdate, Overview, PairLaneSide, PairView, PathLeg, PathSource, PathView, RenamedTag, RouteMatrix, RouteTable, Rule,
    Status, Timestamp, UpdateOverview, UpdateWait,
};
use i3t_model::control::{DirectKind, LaneCounts, LaneReport, LegTransport, ListenState, PathSegment, PeerLanes, Route};
use i3t_model::netmap::DefaultAction;
use i3t_model::update::{FailureKind, UpdateStatus, UpdateTiming};
use i3t_proto::NodeId;

use crate::table::{NONE, Table, label, list, optional, yes_no};

pub fn status(status: &Status) {
    println!("controller {}", status.version);
    println!("  公钥        {}", status.controller_key);
    println!("  节点连接到  {}", status.public_addr);
    println!("  网络        {}", status.networks);
    println!("  节点        {}（在线 {}）", status.nodes, status.online);
    if let Some(newer) = &status.newer_version {
        println!("  有新版本    {newer}。controller 不会自己更新，请手动换上新的程序");
    }
}

pub fn overview(overview: &Overview) {
    println!("节点 {}（在线 {}）  relay {}  出口 {}", overview.nodes, overview.online, overview.relays, overview.exits);
    println!(
        "规则 {}  监听器 {}  端口转发 {}  凭据 {}  待用的注册令牌 {}",
        overview.rules, overview.listeners, overview.forwards, overview.credentials, overview.pending_tokens
    );
    println!("在线：{}", list(overview.online_nodes.iter().map(node_name)));
    if overview.unreachable_pairs.is_empty() {
        return;
    }
    println!("\n互相到不了的节点对（直连不通，也没有 relay 可走）：");
    for pair in &overview.unreachable_pairs {
        println!("  {} <-> {}", node_name(&pair.a), node_name(&pair.b));
    }
    if overview.relays == 0 {
        println!("这个网络没有 relay。在一台有公网地址的节点上执行 i3t set can-relay on，再用 i3tctl relay enable 启用它。");
    }
}

pub fn log(entries: &[LogEntry]) {
    let mut table = Table::new(&["ID", "TIME", "NETWORK", "ACTION", "DETAIL"]);
    for entry in entries {
        table.row(vec![entry.id.to_string(), time(entry.at), optional(entry.network), entry.action.clone(), entry.detail.clone()]);
    }
    table.print();
}

pub fn renamed_tag(renamed: &RenamedTag) {
    println!(
        "标签 {} 改名为 {}：{} 个节点，{} 条规则，{} 个注册令牌",
        renamed.from, renamed.to, renamed.nodes, renamed.rules, renamed.tokens
    );
}

/// 字节数写成好读的样子（按 1000 进位，和 `--quota`、`--max-bytes` 的写法一样）。
fn bytes(count: u64) -> String {
    const UNITS: [(u64, &str); 4] = [(1_000_000_000_000, "T"), (1_000_000_000, "G"), (1_000_000, "M"), (1_000, "K")];
    match UNITS.iter().find(|(scale, _)| count >= *scale) {
        Some((scale, unit)) => format!("{}.{}{unit}", count / scale, count % scale * 10 / scale),
        None => count.to_string(),
    }
}

fn listen_cells(status: Option<&ListenStatus>) -> (String, String, String) {
    let Some(status) = status else { return ("-".to_owned(), "-".to_owned(), "-".to_owned()) };
    let state = match &status.state {
        ListenState::Listening { local } => format!("listening {local}"),
        ListenState::PortInUse { detail } => format!("port_in_use（{detail}）"),
        ListenState::Failed { detail } => format!("failed（{detail}）"),
        ListenState::Virtual => "virtual".to_owned(),
        ListenState::Unsupported { detail } => format!("unsupported（{detail}）"),
        ListenState::Unknown => "unknown".to_owned(),
    };
    let traffic = format!("{}/{}", bytes(status.traffic.bytes_up), bytes(status.traffic.bytes_down));
    (state, status.traffic.connections.to_string(), traffic)
}

pub fn node_listeners(view: &NodeListeners) {
    if !view.online {
        println!("节点 {} 不在线，没有监听的状态", view.node.name);
        return;
    }
    let mut table = Table::new(&["ID", "NAME", "BIND", "PROTOCOL", "CONNECTIONS", "UP/DOWN", "STATE"]);
    for listener in &view.listeners {
        let (state, connections, traffic) = listen_cells(Some(&listener.status));
        let id = if listener.builtin { "本机".to_owned() } else { optional(listener.id) };
        table.row(vec![id, listener.name.clone(), listener.bind.to_string(), label(&listener.protocol), connections, traffic, state]);
    }
    for forward in &view.forwards {
        let (state, connections, traffic) = listen_cells(Some(&forward.status));
        let name = format!("forward-{}", forward.id);
        table.row(vec![forward.id.to_string(), name, forward.bind.to_string(), label(&forward.protocol), connections, traffic, state]);
    }
    table.print();
}

pub fn exits(exits: &[ExitView]) {
    for exit in exits {
        let settings = &exit.settings;
        println!(
            "出口 {}（#{}，{}）  健康 {}",
            exit.node.name,
            exit.node.id,
            if exit.online { "在线" } else { "离线" },
            label(&exit.health)
        );
        println!("  最近 5 分钟   {} 个请求，出口自己的失败 {} 个", exit.recent_requests, exit.recent_failures);
        println!(
            "  设置          端口 {}，局域网 {}，带宽 {}，地址族 {}，建连预算 {}",
            settings.ports.as_ref().map_or_else(|| "不限".to_owned(), list),
            yes_no(settings.lan),
            settings.bandwidth_mbps.map_or_else(|| "不限".to_owned(), |mbps| format!("{mbps} Mbps")),
            label(&settings.family),
            settings.connects_per_minute.map_or_else(|| "不限".to_owned(), |count| format!("每分钟 {count} 次"))
        );
        if let Some(usage) = &exit.usage {
            println!(
                "  它自己报的    {} 个请求，目标的问题 {}，被拒绝 {}，出口自己的问题 {}；发往目标 {}，从目标收到 {}{}",
                usage.total,
                usage.failed_by_target,
                usage.refused,
                usage.failed_by_exit,
                bytes(usage.bytes_to_target),
                bytes(usage.bytes_from_target),
                if usage.draining { "；正在排空" } else { "" }
            );
            if let Some(quota) = usage.quota {
                println!(
                    "  配额          本账期已用 {} / {}，{} 到 {}",
                    bytes(quota.used_bytes),
                    bytes(quota.limit_bytes),
                    time(quota.period_start),
                    time(quota.period_end)
                );
            }
        }
        for seen in &exit.observed {
            let counts = &seen.counts;
            println!(
                "  {} 看到的     {} 条流，发出 {}，收到 {}，目标的问题 {}，被拒绝 {}，出口的问题 {}，连不上出口 {}",
                seen.by.name,
                counts.flows,
                bytes(counts.bytes_up),
                bytes(counts.bytes_down),
                counts.failed_by_target,
                counts.refused,
                counts.failed_by_exit,
                counts.unreachable
            );
        }
    }
    if exits.is_empty() {
        println!("这个网络里没有启用了的出口");
    }
}

pub fn networks(networks: &[Network]) {
    let mut table = Table::new(&["ID", "NAME", "CIDR", "NODES", "ROUTING", "REACH", "MAX-HOPS"]);
    for network in networks {
        table.row(vec![
            network.id.to_string(),
            network.name.clone(),
            network.cidr.to_string(),
            network.nodes.to_string(),
            label(&network.routing),
            label(&network.reach),
            network.max_relay_hops.to_string(),
        ]);
    }
    table.print();
}

pub fn network(network: &Network) {
    println!("网络 {}（#{}）", network.name, network.id);
    println!("  域名后缀      {}", network.domain);
    println!("  地址段        {}", network.cidr);
    println!("  节点          {}", network.nodes);
    println!("  选路模式      {}", label(&network.routing));
    println!("  路径限制      {}", label(&network.reach));
    println!("  最大中转级数  {}", network.max_relay_hops);
}

pub fn nodes(nodes: &[Node]) {
    let mut table = Table::new(&["ID", "NAME", "VIRTUAL-IP", "STATUS", "ROLES", "VERSION", "PLATFORM", "TAGS"]);
    for node in nodes {
        table.row(vec![
            node.id.to_string(),
            node.name.clone(),
            node.virtual_ip.to_string(),
            if node.online { "online".to_owned() } else { "offline".to_owned() },
            roles(node),
            node.version.clone(),
            node.platform.clone(),
            list(&node.tags),
        ]);
    }
    table.print();
}

fn roles(node: &Node) -> String {
    list([node.relay.is_some().then_some("relay"), node.exit.is_some().then_some("exit")].into_iter().flatten())
}

/// 改动节点之后的回显：只说现在是什么样。
pub fn node_changed(node: &Node) {
    println!("节点 #{} {}（{}，{}）", node.id, node.name, node.virtual_ip, node.fqdn);
    println!("  标签  {}", list(&node.tags));
    println!("  能力  {}", roles(node));
}

pub fn node_detail(detail: &NodeDetail) {
    let node = &detail.node;
    println!("节点 {}（#{}）", node.name, node.id);
    println!("  全名          {}", node.fqdn);
    println!("  虚拟 IP       {}", node.virtual_ip);
    println!("  公钥          {}", node.key);
    println!("  标签          {}", list(&node.tags));
    let seen = if node.online {
        "在线".to_owned()
    } else {
        format!("离线，最近在线 {}", node.last_seen_at.map_or_else(|| "从未".to_owned(), time))
    };
    println!("  状态          {seen}");
    println!("  版本与平台    {} / {}", node.version, node.platform);
    println!("  主机名        {}", if node.hostname.is_empty() { NONE } else { &node.hostname });
    println!("  更新          {}", update_summary(&node.update));
    let declared =
        [node.declared.relay.then_some("relay"), node.declared.exit.then_some("exit"), node.declared.forward.then_some("forward")];
    println!("  节点声明可作  {}", list(declared.into_iter().flatten()));
    match &node.relay {
        Some(relay) => println!(
            "  relay         已启用：udp {}，tcp {}，区域 {}，带宽上限 {}",
            list(&relay.udp),
            list(&relay.tcp),
            if relay.region.is_empty() { NONE } else { &relay.region },
            optional(relay.bandwidth_mbps)
        ),
        None => println!("  relay         未启用"),
    }
    match &node.exit {
        Some(exit) => println!(
            "  出口          已启用：端口 {}，局域网 {}，带宽上限 {}，上游代理 {}",
            exit.ports.as_ref().map_or_else(|| "any".to_owned(), list),
            yes_no(exit.lan),
            optional(exit.bandwidth_mbps),
            optional(exit.upstream_socks5)
        ),
        None => println!("  出口          未启用"),
    }
    println!("  只拨出        {}", yes_no(node.dial_out_only));
    println!("  选路模式      {}", node.routing.as_ref().map_or_else(|| "用网络的".to_owned(), label));
    println!("  路径限制      {}", node.reach.as_ref().map_or_else(|| "用网络的".to_owned(), label));
    println!("  固定的 relay  {}", list(&node.pinned_relays));
    println!("  netmap        第 {} 版，已发 {}，已确认 {}", node.netmap_version, node.sent_version, node.acked_version);
    println!("  候选端点      {}", list(&detail.endpoints));
    println!("  看到的地址    {}", optional(detail.observed_endpoint));
    println!("  挂靠的 relay  {}", list(node.attached.iter().map(|attachment| format!("#{}({}ms)", attachment.relay, attachment.rtt_ms))));
    if detail.mapping_varies == Some(true) {
        println!("  注意          这台机器所在的网络映射端口随对端变化，基本打不了洞");
    }
    println!("  流与流量      {} 条流，发 {} 字节，收 {} 字节", detail.flows, detail.tx_bytes, detail.rx_bytes);
    if let Some(lanes) = &detail.lanes {
        lane_report(lanes);
    }
}

pub fn routes(table: &RouteTable) {
    if !table.online {
        println!("节点 {} 不在线，没有路由表", table.node.name);
        return;
    }
    let mut rows = Table::new(&["PEER", "VIRTUAL-IP", "VIA", "RTT-MS", "KIND", "DETAIL"]);
    for entry in &table.routes {
        let (via, detail) = route_cells(&entry.route);
        rows.row(vec![
            entry.peer.name.clone(),
            entry.peer.virtual_ip.to_string(),
            via.to_owned(),
            optional(entry.rtt_ms),
            kind_cell(&entry.route, entry.direct, entry.relay_leg),
            detail,
        ]);
    }
    rows.print();
}

/// 一条路的细节在表格里的一格：直连是怎么建立的，或者到第一个 relay 的那一段走 UDP 还是 TCP。节点没有说时是短横。
fn kind_cell(route: &Route, direct: Option<DirectKind>, relay_leg: Option<LegTransport>) -> String {
    match route {
        Route::Direct { .. } => optional(direct.map(|kind| label(&kind))),
        Route::Relay { .. } => optional(relay_leg.map(|leg| label(&leg))),
        Route::None => NONE.to_owned(),
    }
}

/// 一条路在表格里的两格：走法，和具体的地址或者经过的 relay。
fn route_cells(route: &Route) -> (&'static str, String) {
    match route {
        Route::None => ("none", NONE.to_owned()),
        Route::Direct { addr } => ("direct", addr.to_string()),
        Route::Relay { relays } => ("relay", list(relays.iter().map(|relay| format!("#{relay}")))),
    }
}

pub fn route_matrix(matrix: &RouteMatrix) {
    let name = |id| matrix.nodes.iter().find(|node| node.node.id == id).map_or_else(|| format!("#{id}"), |node| node.node.name.clone());
    let mut rows = Table::new(&["FROM", "TO", "VIA", "RTT-MS", "AGE-S", "STALE", "KIND", "DETAIL"]);
    for entry in &matrix.entries {
        let (via, detail) = route_cells(&entry.route);
        rows.row(vec![
            name(entry.from),
            name(entry.to),
            via.to_owned(),
            optional(entry.rtt_ms),
            entry.age_secs.to_string(),
            yes_no(entry.stale),
            kind_cell(&entry.route, entry.direct, entry.relay_leg),
            detail,
        ]);
    }
    rows.print();
    let silent = matrix.nodes.iter().filter(|node| !matrix.entries.iter().any(|entry| entry.from == node.node.id));
    let silent = list(silent.map(|node| node.node.name.as_str()));
    if silent != NONE {
        println!("\n没有上报过路由的节点：{silent}");
    }
}

pub fn path(view: &PathView) {
    let mut rows = Table::new(&["FROM", "TO", "VIA", "RTT-MS", "SOURCE", "AGE-S", "KIND", "DETAIL"]);
    for leg in [&view.forward, &view.reverse] {
        let (via, detail) = leg.route.as_ref().map_or((NONE, NONE.to_owned()), route_cells);
        let kind = leg.route.as_ref().map_or_else(|| NONE.to_owned(), |route| kind_cell(route, leg.direct, leg.relay_leg));
        rows.row(vec![
            leg.from.name.clone(),
            leg.to.name.clone(),
            via.to_owned(),
            optional(leg.rtt_ms),
            label(&leg.source),
            optional(leg.age_secs),
            kind,
            detail,
        ]);
    }
    rows.print();
    for leg in [&view.forward, &view.reverse] {
        if let Some(note) = path_note(leg) {
            println!("{} → {}：{note}", leg.from.name, leg.to.name);
        }
    }
    for leg in [&view.forward, &view.reverse] {
        path_candidates(view, leg);
    }
}

/// 发送方考虑过的候选路径：每条一行，现在走的那条打星号。节点没有说的时候不打印。
fn path_candidates(view: &PathView, leg: &PathLeg) {
    let Some(candidates) = &leg.candidates else { return };
    let mode = optional(leg.mode.as_ref().map(label));
    println!("\n{} → {} 的候选路径（选路模式 {mode}）：", leg.from.name, leg.to.name);
    if let Some(reason) = leg.reason.as_deref().filter(|reason| !reason.is_empty()) {
        println!("  为什么是现在这条：{reason}");
    }
    if candidates.is_empty() {
        println!("  还没有任何测量值");
        return;
    }
    let name = |id: NodeId| {
        [&view.forward.from, &view.forward.to]
            .into_iter()
            .find(|node| node.id == id)
            .map_or_else(|| format!("#{id}"), |node| node.name.clone())
    };
    let mut rows = Table::new(&["", "VIA", "COST-MS", "LOAD%", "LEGS"]);
    for candidate in candidates {
        let (via, detail) = route_cells(&candidate.route);
        let legs = candidate.legs.iter().map(|segment| leg_text(segment, &name)).collect::<Vec<_>>().join("；");
        rows.row(vec![
            if candidate.current { "*" } else { "" }.to_owned(),
            format!("{via} {detail}"),
            millis(candidate.cost_us),
            candidate.load_percent.to_string(),
            legs,
        ]);
    }
    rows.print();
}

/// 一段的测量值：`laptop→#9 20.4ms ±0.5 丢 1.2%`。
fn leg_text(segment: &PathSegment, name: &impl Fn(NodeId) -> String) -> String {
    let loss = format!("{}.{}%", segment.loss_permille / 10, segment.loss_permille % 10);
    format!(
        "{}→{} {}ms ±{} 丢 {loss}",
        name(segment.from),
        name(segment.to),
        millis(u64::from(segment.rtt_us)),
        millis(u64::from(segment.jitter_us))
    )
}

/// 微秒写成带一位小数的毫秒。
fn millis(micros: u64) -> String {
    format!("{}.{}", micros / 1000, micros % 1000 / 100)
}

pub fn backbone(view: &BackboneView) {
    if view.relays.is_empty() {
        println!("这个网络里还没有 relay");
        return;
    }
    let name = |id: NodeId| {
        view.relays.iter().find(|relay| relay.relay.id == id).map_or_else(|| format!("#{id}"), |relay| relay.relay.name.clone())
    };
    let mut rows = Table::new(&["RELAY", "ONLINE", "STATE", "LOAD%", "AGE-MS", "FROM", "LINKS"]);
    for relay in &view.relays {
        let links = relay.links.iter().map(|link| match link.rtt_ms {
            Some(rtt) => {
                format!("{} {rtt}ms ±{} 丢 {}.{}%", name(link.to), link.jitter_ms, link.loss_permille / 10, link.loss_permille % 10)
            }
            None => format!("{} 不通", name(link.to)),
        });
        rows.row(vec![
            relay.relay.name.clone(),
            yes_no(relay.online),
            optional(relay.state.as_ref().map(label)),
            optional(relay.load_percent),
            optional(relay.age_ms),
            optional(relay.reported_by.map(|from| {
                view.answered.iter().find(|node| node.id == from).map_or_else(|| format!("#{from}"), |node| node.name.clone())
            })),
            list(links),
        ]);
    }
    rows.print();
    if view.answered.is_empty() {
        println!("\n没有问到任何节点：网络里没有在线的、版本够新的节点，所以每个 relay 的行都是空的");
    }
}

/// 一个方向上值得多说一句的事。
fn path_note(leg: &PathLeg) -> Option<String> {
    match leg.source {
        PathSource::Query => leg.since.as_ref().map(|since| format!("{} 秒前换到这条路，原因：{}", since.secs_ago, since.reason)),
        PathSource::Report => Some(format!("没有问到 {}（不在线、版本太旧或者没有按时回答），这是它最近上报的", leg.from.name)),
        PathSource::None => Some(format!("没有数据：{} 没有上报过到 {} 的路，现在也问不到它", leg.from.name, leg.to.name)),
        PathSource::NotPeers => Some(format!("{} 说 {} 不在它的配置里：访问规则没有把这两个节点连通", leg.from.name, leg.to.name)),
        PathSource::Unknown => None,
    }
}

pub fn update_overview(overview: &UpdateOverview) {
    println!("自动更新    {}", if overview.auto_update { "开" } else { "关" });
    println!("发布源      {}{}", overview.source, if overview.source_is_default { "（默认）" } else { "" });
    println!("controller  {}", overview.controller_version);
    match &overview.release {
        Some(release) => {
            println!("最新正式版  {}（发布于 {}）", release.version, time(release.published));
            if release.controller_too_old {
                println!(
                    "  注意      这个版本的节点需要 controller 至少是 {}，现在不会下发。请先手动更新 controller",
                    release.min_controller
                );
            } else if release.newer_than_controller {
                println!("  提示      controller 自己也有新版本 {}，它不会自己更新，请手动换上新的程序", release.version);
            }
            let mut packages = Table::new(&["PLATFORM", "SIZE", "STATE", "ERROR"]);
            for package in &release.packages {
                packages.row(vec![
                    package.platform.clone(),
                    package.size.to_string(),
                    label(&package.state),
                    optional(package.error.as_deref()),
                ]);
            }
            packages.print();
        }
        None => println!("最新正式版  还没有查到"),
    }
    println!("上次检查    {}", overview.checked_at.map_or_else(|| "还没有查过".to_owned(), time));
    if let Some(error) = &overview.check_error {
        println!("检查失败    {error}");
    }
    let nodes = overview.nodes;
    println!(
        "节点        共 {}：正在更新 {}，可以更新 {}，要手动更新 {}，更新失败 {}，已是最新 {}",
        nodes.total, nodes.updating, nodes.outdated, nodes.manual, nodes.failed, nodes.up_to_date
    );
    for paused in &overview.paused {
        let node = paused.node.as_ref().map_or("一个已经删除的节点", node_name);
        println!("\n版本 {} 的自动更新已暂停：{node} 在 {} 更新失败（{}）", paused.version, time(paused.at), paused.reason);
        println!("看过原因之后，用 i3tctl update resume {} 继续。", paused.version);
    }
}

pub fn node_updates(nodes: &[Node]) {
    let mut table = Table::new(&["ID", "NAME", "STATUS", "VERSION", "AVAILABLE", "SELF-UPDATE", "UPDATE"]);
    for node in nodes {
        let ability = match node.update.self_update {
            None => "unknown".to_owned(),
            Some(ability) if ability.able => "yes".to_owned(),
            Some(ability) => ability.blocker.as_ref().map_or_else(|| "no".to_owned(), label),
        };
        table.row(vec![
            node.id.to_string(),
            node.name.clone(),
            if node.online { "online".to_owned() } else { "offline".to_owned() },
            node.version.clone(),
            optional(node.update.available.as_ref()),
            ability,
            update_summary(&node.update),
        ]);
    }
    table.print();
}

/// 强制更新一个节点之后的回显。
pub fn node_update(name: &str, update: &NodeUpdate) {
    println!("节点 {name}：{}", update_summary(update));
}

pub fn forced_updates(forced: &ForcedUpdates) {
    match &forced.version {
        Some(version) => println!("更新到 {version}："),
        None => println!("没有可以下发的版本："),
    }
    println!("  已下发指令  {}", list(forced.ordered.iter().map(node_name)));
    println!("  排队中      {}", list(forced.queued.iter().map(node_name)));
    for skipped in &forced.skipped {
        println!("  跳过        {}：{}", skipped.node.name, skipped.reason);
    }
}

/// 一个节点的更新情况，写成一句话。
fn update_summary(update: &NodeUpdate) -> String {
    let timing = |timing: UpdateTiming| match timing {
        UpdateTiming::Idle => "空闲时",
        UpdateTiming::Now => "立刻",
        UpdateTiming::AfterDrain => "排空后",
    };
    let progress = match &update.status {
        UpdateStatus::Idle | UpdateStatus::Unknown => None,
        UpdateStatus::Downloading { version, received, total } => Some(format!("正在取 {version} 的包（{received}/{total} 字节）")),
        UpdateStatus::Verifying { version } => Some(format!("正在核对 {version} 的包")),
        UpdateStatus::SelfCheck { version } => Some(format!("正在运行 {version} 的自检")),
        UpdateStatus::WaitingForIdle { version } => Some(format!("{version} 已经准备好，在等节点空闲")),
        UpdateStatus::Draining { version } => Some(format!("{version} 已经准备好，节点不再接新的流，在等手上的流结束")),
        UpdateStatus::Installing { version } => Some(format!("正在换上 {version}")),
        UpdateStatus::Restarting { version } => Some(format!("已经换上 {version}，正在重启")),
        UpdateStatus::Confirmed { version } => Some(format!("已经更新到 {version}")),
        UpdateStatus::Failed { version, kind, reason } => {
            let blame = match kind {
                FailureKind::VersionRejected => "这个版本在这台机器上不能用",
                FailureKind::NotInstalled => "没有装成",
            };
            Some(format!("更新到 {} 失败（{blame}）：{reason}", optional(version.as_ref())))
        }
    };
    let order = update.ordered.as_ref().map(|order| format!("已下发指令：{}更新到 {}", timing(order.timing), order.version));
    let waiting = update.waiting.map(|waiting| match waiting {
        UpdateWait::Package => "在等 controller 把它的平台的包取回来".to_owned(),
        UpdateWait::RelayTurn => "在等前一个 relay 更新完重新上线".to_owned(),
        UpdateWait::Unknown => "在等待".to_owned(),
    });
    let parts: Vec<String> = [progress, order, waiting].into_iter().flatten().collect();
    match (&update.available, parts.is_empty()) {
        (_, false) => parts.join("；"),
        (Some(version), true) => format!("有新版本 {version}"),
        (None, true) => "没有新版本".to_owned(),
    }
}

pub fn node_access(access: &NodeAccess) {
    let mut table = Table::new(&["DIRECTION", "NODE", "VIRTUAL-IP", "RULES"]);
    let edges =
        access.can_reach.iter().map(|edge| ("can-reach", edge)).chain(access.reachable_by.iter().map(|edge| ("reachable-by", edge)));
    for (direction, edge) in edges {
        table.row(vec![
            direction.to_owned(),
            edge.node.name.clone(),
            edge.node.virtual_ip.to_string(),
            list(edge.rules.iter().map(|rule| format!("#{rule}"))),
        ]);
    }
    table.print();
    if !access.acknowledged {
        println!("节点 {} 还没有确认当前版本的配置，它还在按旧的规则判定", access.node.name);
    }
}

pub fn tokens(tokens: &[EnrollTokenInfo]) {
    let mut table = Table::new(&["ID", "PREFIX", "STATE", "EXPIRES", "USED-BY", "VIRTUAL-IP", "SUCCESSOR-OF", "TAGS"]);
    for token in tokens {
        table.row(vec![
            token.id.to_string(),
            if token.prefix.is_empty() { "-".to_owned() } else { format!("{}…", token.prefix) },
            label(&token.state),
            time(token.expires_at),
            optional(token.used_by.as_ref().map(|node| format!("{}（#{}）", node.name, node.id))),
            optional(token.virtual_ip),
            optional(token.successor_of.map(|node| format!("#{node}"))),
            list(&token.tags),
        ]);
    }
    table.print();
}

pub fn created_token(created: &CreatedEnrollToken) {
    println!("注册令牌 #{}，{} 过期。令牌文本只显示这一次：\n", created.info.id, time(created.info.expires_at));
    println!("    {}\n", created.token);
    println!("在要加入的机器上执行 i3t join，把它从标准输入交进去。");
}

pub fn rules(rules: &[Rule]) {
    let mut table = Table::new(&["ID", "SRC", "", "DST", "ENABLED", "NAME"]);
    for rule in rules {
        table.row(rule_row(rule));
    }
    table.print();
}

pub fn rule(rule: &Rule) {
    let state = if rule.enabled { "启用" } else { "停用" };
    println!("规则 #{}：{} {} {}（{state}）", rule.id, rule.src, arrow(rule.direction), rule.dst);
}

fn rule_row(rule: &Rule) -> Vec<String> {
    vec![rule.id.to_string(), rule.src.clone(), arrow(rule.direction).to_owned(), rule.dst.clone(), yes_no(rule.enabled), rule.name.clone()]
}

const fn arrow(direction: Direction) -> &'static str {
    match direction {
        Direction::Both => "<->",
        Direction::OneWay => "->",
    }
}

pub fn credentials(credentials: &[Credential]) {
    let mut table =
        Table::new(&["ID", "PREFIX", "STATE", "MAX-CONCURRENT", "MAX-BYTES", "USED", "CONNECTIONS", "LAST-USED", "CREATED", "NAME"]);
    for credential in credentials {
        table.row(vec![
            credential.id.to_string(),
            credential.prefix.clone(),
            label(&credential.state),
            optional(credential.max_concurrent),
            optional(credential.max_bytes.map(bytes)),
            bytes(credential.usage.bytes),
            credential.usage.connections.to_string(),
            optional(credential.usage.last_used_at.map(time)),
            time(credential.created_at),
            credential.name.clone(),
        ]);
    }
    table.print();
}

pub fn credential(credential: &Credential) {
    println!(
        "凭据 #{} {}（{}）：并发上限 {}，流量上限 {}，已用 {}",
        credential.id,
        credential.name,
        label(&credential.state),
        optional(credential.max_concurrent),
        optional(credential.max_bytes.map(bytes)),
        bytes(credential.usage.bytes)
    );
}

pub fn created_credential(created: &CreatedCredential) {
    println!("凭据 #{} {}。令牌只显示这一次：\n", created.credential.id, created.credential.name);
    println!("    {}\n", created.token);
    println!("调用者把它填在代理的密码字段里；用户名写出口节点的虚拟 IP 或名字。");
}

pub fn listeners(listeners: &[Listener]) {
    let mut table = Table::new(&["ID", "NODE", "BIND", "PROTOCOL", "CREDENTIAL", "DEFAULT", "FIXED-EXIT", "ALLOW-FROM", "NAME"]);
    for listener in listeners {
        table.row(listener_row(listener));
    }
    table.print();
}

pub fn listener(listener: &Listener) {
    println!(
        "监听器 #{} {}：节点 #{} 上的 {}（{}）",
        listener.id,
        listener.config.name,
        listener.node,
        listener.config.bind,
        label(&listener.config.protocol)
    );
}

fn listener_row(listener: &Listener) -> Vec<String> {
    let config = &listener.config;
    let default_action = match config.effective_default_action() {
        DefaultAction::Local => "local".to_owned(),
        DefaultAction::Reject => "reject".to_owned(),
        DefaultAction::Exit { node } => format!("exit:#{node}"),
    };
    vec![
        listener.id.to_string(),
        format!("#{}", listener.node),
        config.bind.to_string(),
        label(&config.protocol),
        yes_no(config.require_credential),
        default_action,
        optional(config.fixed_exit.map(|node| format!("#{node}"))),
        list(&config.allow_from),
        config.name.clone(),
    ]
}

pub fn forwards(forwards: &[Forward]) {
    let mut table =
        Table::new(&["ID", "PROTOCOL", "LISTEN", "TARGET", "STATUS", "LISTEN-STATE", "CONNECTIONS", "UP/DOWN", "ALLOW-FROM", "NAME"]);
    for forward in forwards {
        let (state, connections, traffic) = listen_cells(forward.listen.as_ref());
        table.row(vec![
            forward.id.to_string(),
            label(&forward.protocol),
            format!("#{} {}", forward.listen_node, forward.bind),
            format!("#{} {}", forward.target_node, forward.target_addr),
            label(&forward.status),
            state,
            connections,
            traffic,
            list(&forward.allow_from),
            forward.name.clone(),
        ]);
    }
    table.print();
}

pub fn forward(forward: &Forward) {
    println!(
        "转发 #{}（{}）：节点 #{} 的 {} → 节点 #{} 的 {}，状态 {}",
        forward.id,
        label(&forward.protocol),
        forward.listen_node,
        forward.bind,
        forward.target_node,
        forward.target_addr,
        label(&forward.status)
    );
}

pub fn pairs(pairs: &[PairView]) {
    let mut table = Table::new(&["A", "B", "KEEP-ALIVE", "LANE", "EFFECTIVE", "LANES-IN-USE", "FALLBACKS"]);
    for pair in pairs {
        let setting = &pair.setting;
        let effect = pair.lane_effect.as_ref().map_or_else(|| NONE.to_owned(), lane_effect_cell);
        let in_use: u64 = pair.lanes.iter().map(|side| side.lanes.counts.in_use).sum();
        let fallbacks: u64 = pair.lanes.iter().map(|side| side.lanes.counts.fallbacks).sum();
        table.row(vec![
            format!("#{}", setting.a),
            format!("#{}", setting.b),
            yes_no(setting.keep_alive),
            setting.lane.to_string(),
            effect,
            in_use.to_string(),
            fallbacks.to_string(),
        ]);
    }
    table.print();
}

pub fn pair(pair: &PairView) {
    let setting = &pair.setting;
    println!("节点 #{} 和 #{}：连接常驻 {}，独占 TCP {}", setting.a, setting.b, yes_no(setting.keep_alive), setting.lane);
    if let Some(effect) = &pair.lane_effect {
        println!("  {}", effect.summary);
        for block in &effect.blocked {
            println!("  不起作用：{}", lane_block(block));
        }
    }
    for PairLaneSide { node, lanes } in &pair.lanes {
        println!("  #{node} 这边：{}", peer_lanes(lanes));
    }
}

fn lane_effect_cell(effect: &LaneEffect) -> String {
    if effect.effective {
        return "yes".to_owned();
    }
    let reasons = effect.blocked.iter().map(|block| match block {
        LaneBlock::NoRelay => "no-relay".to_owned(),
        LaneBlock::NoTcpAddress { node } => format!("no-tcp-address(#{node})"),
        LaneBlock::NodeTooOld { node } => format!("too-old(#{node})"),
        LaneBlock::NotConnected => "not-connected".to_owned(),
    });
    format!("no: {}", reasons.collect::<Vec<_>>().join(","))
}

fn lane_block(block: &LaneBlock) -> String {
    match block {
        LaneBlock::NoRelay => "两个节点都没有启用 relay，没有哪一边的 TCP 端口在听".to_owned(),
        LaneBlock::NoTcpAddress { node } => format!("#{node} 启用了 relay，但还没有已知的 TCP 地址"),
        LaneBlock::NodeTooOld { node } => format!("#{node} 的版本太旧，不懂独占 TCP"),
        LaneBlock::NotConnected => "访问规则没有连通这两个节点".to_owned(),
    }
}

/// 和一个对端之间的 lane，一行。
fn peer_lanes(lanes: &PeerLanes) -> String {
    let control = if lanes.control { "控制 lane 在" } else { "控制 lane 不在" };
    format!(
        "{control}，{}拨对方；空闲 {}（对方拨来）/ {}（自己拨好）/ 停在对方 {}；{}",
        if lanes.dials { "" } else { "不" },
        lanes.idle,
        lanes.ready,
        lanes.parked_at_peer,
        lane_counts(&lanes.counts)
    )
}

fn lane_counts(counts: &LaneCounts) -> String {
    format!(
        "在用 {}，拨号中 {}；取用 {}（等过 {}，共 {}ms，没取到 {}），改走共用连接 {}，拨号失败 {}，拒绝 {}；\
         字节 加密 {} / 不加密 {} / 内核搬运 {}",
        counts.in_use,
        counts.dialling,
        counts.claims,
        counts.claims_waited,
        counts.claim_wait_ms,
        counts.claims_failed,
        counts.fallbacks,
        counts.dial_failures,
        counts.refused,
        counts.sealed_bytes,
        counts.passthrough_bytes,
        counts.spliced_bytes
    )
}

fn lane_report(report: &LaneReport) {
    let failures = &report.handshake_failures;
    println!("  独占 TCP      {}", lane_counts(&report.total));
    println!(
        "                拨进来没握手成功：超时 {}，不认识的公钥 {}，TLS {}，协议 {}，太多 {}",
        failures.deadline, failures.unknown_key, failures.tls, failures.protocol, failures.busy
    );
    for peer in &report.peers {
        println!("    #{:<10} {}", peer.peer, peer_lanes(peer));
    }
}

pub fn access_graph(graph: &AccessGraph) {
    let mut tags = Table::new(&["TAG", "MEMBERS", "NODES"]);
    for entry in &graph.tags {
        tags.row(vec![entry.tag.clone(), entry.nodes.len().to_string(), list(entry.nodes.iter().map(node_name))]);
    }
    tags.print();
    println!();
    rules(&graph.rules);
    if !graph.unconnected.is_empty() {
        println!("\n没有被任何规则连通的节点：{}", list(graph.unconnected.iter().map(node_name)));
    }
    if !graph.unacknowledged.is_empty() {
        println!("\n还没有确认当前配置的节点（还在按旧的规则判定）：{}", list(graph.unacknowledged.iter().map(node_name)));
    }
}

pub fn access_check(check: &AccessCheck) {
    if check.allowed {
        let rules = list(check.rules.iter().map(|rule| format!("#{} {} {} {}", rule.id, rule.src, arrow(rule.direction), rule.dst)));
        println!("{} 可以访问 {}，允许它的规则：{rules}", check.from.name, check.to.name);
    } else {
        println!("{} 不能访问 {}：没有规则连通这两个节点", check.from.name, check.to.name);
    }
}

fn node_name(node: &NodeRef) -> &str {
    &node.name
}

/// 把 Unix 秒写成 UTC 的日期和时间。
fn time(at: Timestamp) -> String {
    const SECS_PER_DAY: u64 = 86400;
    let (days, secs) = (at / SECS_PER_DAY, at % SECS_PER_DAY);
    // 公历的日期换算（Howard Hinnant 的 civil_from_days），以 400 年为一个周期，从 0000-03-01 算起。
    let shifted = days + 719_468;
    let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_measurements_are_shown_in_milliseconds_and_percent() {
        assert_eq!((millis(41_250), millis(999), millis(0)), ("41.2".to_owned(), "0.9".to_owned(), "0.0".to_owned()));
        let (laptop, relay) = (NodeId::new(1).unwrap(), NodeId::new(9).unwrap());
        let segment = PathSegment { from: laptop, to: relay, rtt_us: 20_400, jitter_us: 500, loss_permille: 12 };
        let name = |id: NodeId| if id == laptop { "laptop".to_owned() } else { format!("#{id}") };
        assert_eq!(leg_text(&segment, &name), "laptop→#9 20.4ms ±0.5 丢 1.2%");
        let direct = Route::Direct { addr: "203.0.113.9:20330".parse().unwrap() };
        assert_eq!(kind_cell(&direct, Some(DirectKind::Punched), None), "punched");
        assert_eq!(kind_cell(&Route::Relay { relays: vec![relay] }, Some(DirectKind::Lan), Some(LegTransport::Tcp)), "tcp");
        assert_eq!(kind_cell(&direct, None, Some(LegTransport::Tcp)), "-", "节点没有说的时候是短横");
    }

    #[test]
    fn timestamps_are_shown_as_utc_dates() {
        assert_eq!(time(0), "1970-01-01 00:00:00Z");
        assert_eq!(time(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(time(1_794_787_199), "2026-11-15 23:59:59Z");
        assert_eq!(time(4_102_444_800), "2100-01-01 00:00:00Z");
    }
}
