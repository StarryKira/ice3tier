//! 把管理 API 的应答打印成给人看的样子。`--json` 时不经过这里。
//!
//! 文字都从文字目录里取（[`crate::texts`]），控制面的代码（原因、状态）按 `i3t_lang::codes` 写，数和时间按 [`Lang`] 的方法写。

use i3t_lang::codes::Codes;
use i3t_lang::table::{Fields, Table};
use i3t_lang::{Lang, NONE, optional};
use i3t_model::admin::{
    AccessCheck, AccessGraph, BackboneView, CreatedCredential, CreatedEnrollToken, Credential, DeletedGroup, Direction, EnrollTokenInfo,
    ExitView, ForcedUpdates, Forward, Group, GroupRef, LaneEffect, ListenStatus, Listener, LogEntry, Network, Node, NodeAccess, NodeDetail,
    NodeListeners, NodeRef, NodeUpdate, Overview, PairLaneSide, PairView, PathLeg, PathSource, PathView, RelayEnabled, RouteMatrix,
    RouteTable, Rule, Status, UpdateOverview,
};
use i3t_model::control::{DirectKind, LaneCounts, LaneReport, LegTransport, PathSegment, PeerLanes, PerformanceReport, Route};
use i3t_model::netmap::{DefaultAction, Profile};
use i3t_model::update::UpdateTiming;
use i3t_proto::NodeId;

use crate::texts::{Ui, texts};

mod diag;

pub use diag::{matrix, test, tests};

/// 这一次要用的文字、语言和代码的写法。
fn context() -> (&'static Ui, Lang, &'static Codes) {
    let lang = i3t_lang::current();
    (&crate::texts::texts_in(lang).ui, lang, lang.codes())
}

/// 枚举在 JSON 里的写法（`direct_first` 之类）。命令行上要写的值（选路模式、协议、服务等级）照这个显示，和参数的写法一致。
pub fn label<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => NONE.to_owned(),
    }
}

fn yes_no(value: bool) -> String {
    let t = &texts().ui;
    if value { t.yes } else { t.no }.to_owned()
}

fn online(online: bool) -> &'static str {
    let t = &texts().ui;
    if online { t.online } else { t.offline }
}

pub fn status(status: &Status) {
    let (t, _, _) = context();
    println!("controller {}", status.version);
    let mut fields = Fields::new(2);
    fields.add(t.key, status.controller_key.to_string());
    fields.add(t.public_addr, status.public_addr.clone());
    fields.add(t.networks, status.networks.to_string());
    fields.add(t.nodes, (t.of_online)(u64::from(status.nodes), u64::from(status.online)));
    if let Some(newer) = &status.newer_version {
        fields.add(t.newer_version, newer.to_string());
    }
    fields.print();
}

pub fn overview(overview: &Overview) {
    let (t, lang, _) = context();
    let mut fields = Fields::new(0);
    fields.add(t.nodes, (t.of_online)(u64::from(overview.nodes), u64::from(overview.online)));
    fields.add(t.relays, overview.relays.to_string());
    fields.add(t.exits, overview.exits.to_string());
    fields.add(t.rules, overview.rules.to_string());
    fields.add(t.listeners, overview.listeners.to_string());
    fields.add(t.forwards, overview.forwards.to_string());
    fields.add(t.credentials, overview.credentials.to_string());
    fields.add(t.pending_tokens, overview.pending_tokens.to_string());
    fields.add(t.flows, overview.flows.to_string());
    fields.add(t.traffic, format!("↑ {}  ↓ {}", lang.bytes(overview.tx_bytes), lang.bytes(overview.rx_bytes)));
    fields.add(t.online, lang.list(overview.online_nodes.iter().map(node_name)));
    fields.print();
    if overview.unreachable_pairs.is_empty() {
        return;
    }
    println!("\n{}", t.unreachable_pairs);
    for pair in &overview.unreachable_pairs {
        println!("  {} ↔ {}", node_name(&pair.a), node_name(&pair.b));
    }
    if overview.relays == 0 {
        println!("{}", t.no_relay_hint);
    }
}

pub fn log(entries: &[LogEntry]) {
    let (t, lang, _) = context();
    let mut table = Table::new(&t.log_header);
    for entry in entries {
        let said = (texts().log)(entry).unwrap_or_else(|| entry.detail.clone());
        table.row(vec![entry.id.to_string(), lang.time(entry.at), optional(entry.network), entry.action.clone(), said]);
    }
    print_table(&table, t.none_found);
}

/// 一张表；没有行时说一声"没有"，不打印一个光秃秃的表头。
fn print_table(table: &Table, empty: &str) {
    if table.is_empty() { println!("{empty}") } else { table.print() }
}

pub fn groups(groups: &[Group]) {
    let (t, lang, _) = context();
    let mut table = Table::new(&t.groups_header);
    for group in groups {
        let name = if group.builtin { lang.paren(&group.name, t.builtin) } else { group.name.clone() };
        table.row(vec![group.id.to_string(), name, group.nodes.len().to_string(), group.rules.len().to_string()]);
    }
    print_table(&table, t.no_groups);
}

/// 一个分组：成员和用到它的规则。
pub fn group(group: &Group) {
    let (t, lang, _) = context();
    let title = format!("{} #{}", group.name, group.id);
    println!("{}", if group.builtin { lang.paren(&title, t.builtin) } else { title });
    let mut fields = Fields::new(2);
    fields.add(t.nodes, lang.list(group.nodes.iter().map(node_name)));
    fields.add(t.rules, lang.list(group.rules.iter().map(rule_text)));
    fields.print();
}

pub fn deleted_group(deleted: &DeletedGroup) {
    let (t, _, _) = context();
    println!("{}", (t.group_deleted)(&deleted.name, deleted.nodes.len(), deleted.rules.len()));
}

fn group_names(lang: Lang, groups: &[GroupRef]) -> String {
    lang.list(groups.iter().map(|group| group.name.as_str()))
}

fn listen_cells(status: Option<&ListenStatus>) -> [String; 3] {
    let (_, lang, codes) = context();
    let Some(status) = status else { return [NONE.to_owned(), NONE.to_owned(), NONE.to_owned()] };
    let traffic = format!("↑ {}  ↓ {}", lang.bytes(status.traffic.bytes_up), lang.bytes(status.traffic.bytes_down));
    [(codes.listen_state)(&status.state), status.traffic.connections.to_string(), traffic]
}

pub fn node_listeners(view: &NodeListeners) {
    let (t, _, _) = context();
    if !view.online {
        println!("{}", (t.node_offline)(&view.node.name));
        return;
    }
    let mut table = Table::new(&t.node_listeners_header);
    for listener in &view.listeners {
        let [state, connections, traffic] = listen_cells(Some(&listener.status));
        let id = if listener.builtin { t.builtin.to_owned() } else { optional(listener.id) };
        table.row(vec![id, listener.name.clone(), listener.bind.to_string(), label(&listener.protocol), connections, traffic, state]);
    }
    for forward in &view.forwards {
        let [state, connections, traffic] = listen_cells(Some(&forward.status));
        let name = (t.forward_name)(forward.id);
        table.row(vec![forward.id.to_string(), name, forward.bind.to_string(), label(&forward.protocol), connections, traffic, state]);
    }
    print_table(&table, t.none_found);
}

pub fn exits(exits: &[ExitView]) {
    let (t, lang, codes) = context();
    if exits.is_empty() {
        println!("{}", t.no_exits);
    }
    for (index, exit) in exits.iter().enumerate() {
        if index > 0 {
            println!();
        }
        let settings = &exit.settings;
        println!("{}", lang.paren(&exit.node.name, online(exit.online)));
        let unlimited = || t.unlimited.to_owned();
        let mut fields = Fields::new(2);
        fields.add(t.health, (codes.exit_health)(exit.health).to_owned());
        fields.add(t.recent, (t.recent_requests)(exit.recent_requests, exit.recent_failures));
        fields.add(t.ports, settings.ports.as_ref().map_or_else(unlimited, |ports| lang.list(ports)));
        fields.add(t.lan, yes_no(settings.lan));
        fields.add(t.bandwidth, settings.bandwidth_mbps.map_or_else(unlimited, |mbps| format!("{mbps} Mbit/s")));
        fields.add(t.family, label(&settings.family));
        fields.add(t.connects, settings.connects_per_minute.map_or_else(unlimited, |count| (t.per_minute)(count)));
        if let Some(usage) = &exit.usage {
            fields.add(t.requests, (t.exit_requests)(usage.total, usage.failed_by_target, usage.refused, usage.failed_by_exit));
            fields.add(t.traffic, format!("→ {}  ← {}", lang.bytes(usage.bytes_to_target), lang.bytes(usage.bytes_from_target)));
            if usage.draining {
                fields.add(t.state, t.draining.to_owned());
            }
            if let Some(quota) = usage.quota {
                let period = format!("{} – {}", lang.time(quota.period_start), lang.time(quota.period_end));
                fields.add(t.quota, format!("{} / {}  {period}", lang.bytes(quota.used_bytes), lang.bytes(quota.limit_bytes)));
            }
        }
        for seen in &exit.observed {
            let counts = &seen.counts;
            let traffic = format!("↑ {}  ↓ {}", lang.bytes(counts.bytes_up), lang.bytes(counts.bytes_down));
            let failures = (t.exit_seen)(counts.flows, counts.failed_by_target, counts.refused, counts.failed_by_exit, counts.unreachable);
            fields.add(&(t.seen_by)(&seen.by.name), format!("{failures}  {traffic}"));
        }
        fields.print();
    }
}

pub fn networks(networks: &[Network]) {
    let (t, _, _) = context();
    let mut table = Table::new(&t.networks_header);
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
    print_table(&table, t.no_networks);
}

pub fn network(network: &Network) {
    let (t, _, _) = context();
    println!("{} #{}", network.name, network.id);
    let mut fields = Fields::new(2);
    fields.add(t.domain, network.domain.clone());
    fields.add(t.cidr, network.cidr.to_string());
    fields.add(t.nodes, network.nodes.to_string());
    fields.add(t.routing, label(&network.routing));
    fields.add(t.reach, label(&network.reach));
    fields.add(t.max_relay_hops, network.max_relay_hops.to_string());
    fields.add(t.class, network.class.as_ref().map_or_else(|| "auto".to_owned(), label));
    fields.print();
}

pub fn nodes(nodes: &[Node]) {
    let (t, lang, _) = context();
    let mut table = Table::new(&t.nodes_header);
    for node in nodes {
        table.row(vec![
            node.id.to_string(),
            node.name.clone(),
            node.virtual_ip.to_string(),
            online(node.online).to_owned(),
            roles(node),
            node.version.clone(),
            node.platform.clone(),
            group_names(lang, &node.groups),
        ]);
    }
    print_table(&table, t.no_nodes);
}

fn roles(node: &Node) -> String {
    let (t, lang, _) = context();
    lang.list([node.relay.is_some().then_some("relay"), node.exit.is_some().then_some(t.exit)].into_iter().flatten())
}

/// 改动节点之后的回显：现在是什么样。
pub fn node_changed(node: &Node) {
    let (t, lang, _) = context();
    println!("{} #{}  {}  {}", node.name, node.id, node.virtual_ip, node.fqdn);
    let mut fields = Fields::new(2);
    fields.add(t.groups, group_names(lang, &node.groups));
    fields.add(t.roles, roles(node));
    fields.print();
}

/// 启用 relay 之后的回显，加上 controller 试连每个地址的结果。
pub fn relay_enabled(enabled: &RelayEnabled) {
    let (t, _, codes) = context();
    node_changed(&enabled.node);
    if enabled.reachability.is_empty() {
        return;
    }
    println!("\n{}", t.reachability);
    let mut table = Table::new(&t.reachability_header).indented(2);
    for check in &enabled.reachability {
        let result = match (check.reachable, check.cause) {
            (true, _) => t.reachable.to_owned(),
            (false, Some(cause)) => (codes.reach_cause)(cause).to_owned(),
            (false, None) => check.error.clone().unwrap_or_default(),
        };
        table.row(vec![label(&check.protocol), check.addr.to_string(), optional(check.rtt_ms), result]);
    }
    table.print();
    if enabled.reachability.iter().any(|check| !check.reachable) {
        println!("{}", t.reachability_note);
    }
}

pub fn node_detail(detail: &NodeDetail) {
    let (t, lang, _) = context();
    let node = &detail.node;
    println!("{} #{}", node.name, node.id);
    let mut fields = Fields::new(2);
    fields.add(t.fqdn, node.fqdn.clone());
    fields.add(t.virtual_ip, node.virtual_ip.to_string());
    fields.add(t.key, node.key.to_string());
    fields.add(t.groups, group_names(lang, &node.groups));
    let state = if node.online {
        t.online.to_owned()
    } else {
        lang.paren(t.offline, &lang.labelled(t.last_seen, &node.last_seen_at.map_or_else(|| t.never.to_owned(), |at| lang.time(at))))
    };
    fields.add(t.state, state);
    fields.add(t.version, format!("{} / {}", node.version, node.platform));
    fields.add(t.hostname, if node.hostname.is_empty() { NONE.to_owned() } else { node.hostname.clone() });
    fields.add(t.update, update_summary(&node.update));
    let declared =
        [node.declared.relay.then_some("relay"), node.declared.exit.then_some(t.exit), node.declared.forward.then_some(t.forward)];
    fields.add(t.declared, lang.list(declared.into_iter().flatten()));
    let relay = node.relay.as_ref().map_or_else(
        || t.disabled.to_owned(),
        |relay| {
            let region = if relay.region.is_empty() { NONE } else { &relay.region };
            (t.relay_settings)(&lang.list(&relay.udp), &lang.list(&relay.tcp), region, &bandwidth(relay.bandwidth_mbps))
        },
    );
    fields.add("relay", relay);
    let exit = node.exit.as_ref().map_or_else(
        || t.disabled.to_owned(),
        |exit| {
            let ports = exit.ports.as_ref().map_or_else(|| "any".to_owned(), |ports| lang.list(ports));
            (t.exit_settings)(&ports, &yes_no(exit.lan), &bandwidth(exit.bandwidth_mbps), &optional(exit.upstream_socks5))
        },
    );
    fields.add(t.exit, exit);
    fields.add(t.dial_out_only, yes_no(node.dial_out_only));
    fields.add(t.routing, node.routing.as_ref().map_or_else(|| t.from_network.to_owned(), label));
    fields.add(t.reach, node.reach.as_ref().map_or_else(|| t.from_network.to_owned(), label));
    fields.add(t.pinned, lang.list(node.pinned_relays.iter().map(|relay| format!("#{relay}"))));
    fields.add(t.profile, profile_summary(node.profile, detail.performance.as_ref()));
    fields.add("netmap", (t.netmap_versions)(node.netmap_version, node.sent_version, node.acked_version));
    fields.add(t.endpoints, lang.list(&detail.endpoints));
    fields.add(t.observed, optional(detail.observed_endpoint));
    fields.add(t.attached, lang.list(node.attached.iter().map(|attachment| format!("#{} {} ms", attachment.relay, attachment.rtt_ms))));
    if detail.mapping_varies == Some(true) {
        fields.add(t.nat, t.mapping_varies.to_owned());
    }
    fields.add(t.flows, detail.flows.to_string());
    fields.add(t.traffic, format!("↑ {}  ↓ {}", lang.bytes(detail.tx_bytes), lang.bytes(detail.rx_bytes)));
    if let Some(ago) = detail.reported_secs_ago {
        fields.add(t.reported, lang.ago(ago));
    }
    fields.print();
    if let Some(lanes) = &detail.lanes {
        lane_report(lanes);
    }
}

/// 管理员选的档位，加上节点报的实际情况（在线并且报过时）。
fn profile_summary(chosen: Option<Profile>, running: Option<&PerformanceReport>) -> String {
    let (t, lang, _) = context();
    let chosen = chosen.map_or(t.profile_local, Profile::spelling);
    let Some(running) = running else { return chosen.to_owned() };
    let mut notes = vec![(t.profile_running)(running.profile.spelling(), running.threads)];
    if running.restart_pending {
        notes.push(t.restart_pending.to_owned());
    }
    if !running.overrides.is_empty() {
        notes.push((t.local_overrides)(&lang.list(&running.overrides)));
    }
    lang.paren(chosen, &lang.list(notes))
}

fn bandwidth(mbps: Option<u32>) -> String {
    let (t, _, _) = context();
    mbps.map_or_else(|| t.unlimited.to_owned(), |mbps| format!("{mbps} Mbit/s"))
}

pub fn routes(table: &RouteTable) {
    let (t, lang, codes) = context();
    if !table.online {
        println!("{}", (t.node_offline)(&table.node.name));
        return;
    }
    let mut rows = Table::new(&t.routes_header);
    for entry in &table.routes {
        rows.row(vec![
            entry.peer.name.clone(),
            entry.peer.virtual_ip.to_string(),
            (codes.route)(&entry.route, &|id| format!("#{id}")),
            optional(entry.rtt_ms),
            kind_cell(&entry.route, entry.direct, entry.relay_leg),
            lang.ago(entry.since_secs),
        ]);
    }
    print_table(&rows, t.no_peers);
}

/// 一条路的细节：直连是怎么建立的，或者到第一个 relay 的那一段走 UDP 还是 TCP。节点没有说时是短横。
fn kind_cell(route: &Route, direct: Option<DirectKind>, relay_leg: Option<LegTransport>) -> String {
    let (_, _, codes) = context();
    match route {
        Route::Direct { .. } => optional(direct.map(codes.direct_kind)),
        Route::Relay { .. } => optional(relay_leg.map(codes.leg)),
        Route::None => NONE.to_owned(),
    }
}

pub fn route_matrix(matrix: &RouteMatrix) {
    let (t, lang, codes) = context();
    let name = |id| matrix.nodes.iter().find(|node| node.node.id == id).map_or_else(|| format!("#{id}"), |node| node.node.name.clone());
    let mut rows = Table::new(&t.matrix_header);
    for entry in &matrix.entries {
        rows.row(vec![
            name(entry.from),
            name(entry.to),
            (codes.route)(&entry.route, &name),
            optional(entry.rtt_ms),
            kind_cell(&entry.route, entry.direct, entry.relay_leg),
            if entry.stale { lang.paren(&lang.ago(entry.age_secs), t.stale) } else { lang.ago(entry.age_secs) },
        ]);
    }
    print_table(&rows, t.none_found);
    let silent: Vec<&str> = matrix
        .nodes
        .iter()
        .filter(|node| !matrix.entries.iter().any(|entry| entry.from == node.node.id))
        .map(|node| node.node.name.as_str())
        .collect();
    if !silent.is_empty() {
        println!("\n{}", lang.labelled(t.never_reported, &lang.list(silent)));
    }
}

pub fn path(view: &PathView) {
    let (t, lang, codes) = context();
    let name = |id: NodeId| {
        [&view.forward.from, &view.forward.to]
            .into_iter()
            .find(|node| node.id == id)
            .map_or_else(|| format!("#{id}"), |node| node.name.clone())
    };
    let mut rows = Table::new(&t.path_header);
    for leg in [&view.forward, &view.reverse] {
        let route = leg.route.as_ref().map_or_else(|| NONE.to_owned(), |route| (codes.route)(route, &name));
        let kind = leg.route.as_ref().map_or_else(|| NONE.to_owned(), |route| kind_cell(route, leg.direct, leg.relay_leg));
        rows.row(vec![
            format!("{} → {}", leg.from.name, leg.to.name),
            route,
            optional(leg.rtt_ms),
            kind,
            (codes.path_source)(leg.source).to_owned(),
            leg.age_secs.map_or_else(|| NONE.to_owned(), |age| lang.ago(age)),
        ]);
    }
    rows.print();
    for leg in [&view.forward, &view.reverse] {
        if let Some(note) = path_note(leg, &name) {
            println!("{} → {}  {note}", leg.from.name, leg.to.name);
        }
    }
    for leg in [&view.forward, &view.reverse] {
        path_candidates(leg, &name);
    }
}

/// 一个方向上值得多说一句的事：为什么换到这条路，或者数据从哪里来。
fn path_note(leg: &PathLeg, name: &dyn Fn(NodeId) -> String) -> Option<String> {
    let (t, lang, codes) = context();
    match leg.source {
        PathSource::Query => leg.since.as_ref().map(|since| {
            let why = since.cause.as_ref().map_or_else(|| since.reason.clone(), |cause| (codes.route_cause)(cause, name));
            lang.labelled(&(t.switched_ago)(&lang.ago(since.secs_ago)), &why)
        }),
        PathSource::Report => Some((t.path_from_report)(&leg.from.name)),
        PathSource::None => Some(t.path_no_data.to_owned()),
        PathSource::NotPeers => Some(t.path_not_peers.to_owned()),
        PathSource::Unknown => None,
    }
}

/// 发送方考虑过的候选路径：每条一行，现在走的那条打星号。节点没有说的时候不打印。
fn path_candidates(leg: &PathLeg, name: &dyn Fn(NodeId) -> String) {
    let (t, lang, codes) = context();
    let Some(candidates) = &leg.candidates else { return };
    let mode = optional(leg.mode.as_ref().map(label));
    println!("\n{}", (t.candidates_of)(&leg.from.name, &leg.to.name, &mode));
    let why =
        leg.cause.as_ref().map(|cause| (codes.route_cause)(cause, name)).or_else(|| leg.reason.clone().filter(|reason| !reason.is_empty()));
    if let Some(why) = why {
        println!("  {}", lang.labelled(t.why_this_route, &why));
    }
    if candidates.is_empty() {
        println!("  {}", t.no_measurements);
        return;
    }
    let mut rows = Table::new(&t.candidates_header).indented(2);
    for candidate in candidates {
        let legs = candidate.legs.iter().map(|segment| leg_text(segment, name)).collect::<Vec<_>>().join("; ");
        rows.row(vec![
            if candidate.current { "*" } else { "" }.to_owned(),
            (codes.route)(&candidate.route, name),
            lang.millis(candidate.cost_us),
            candidate.load_percent.to_string(),
            legs,
        ]);
    }
    rows.print();
}

/// 一段的测量值：`laptop→hk-1 20.4 ms ±0.5 1.2%`。
fn leg_text(segment: &PathSegment, name: &dyn Fn(NodeId) -> String) -> String {
    let (t, lang, _) = context();
    (t.segment)(
        &name(segment.from),
        &name(segment.to),
        &lang.millis(u64::from(segment.rtt_us)),
        &lang.millis(u64::from(segment.jitter_us)),
        &lang.percent(segment.loss_permille),
    )
}

pub fn backbone(view: &BackboneView) {
    let (t, lang, codes) = context();
    if view.relays.is_empty() {
        println!("{}", t.no_relays);
        return;
    }
    let name = |id: NodeId| {
        view.relays.iter().find(|relay| relay.relay.id == id).map_or_else(|| format!("#{id}"), |relay| relay.relay.name.clone())
    };
    let mut rows = Table::new(&t.backbone_header);
    for relay in &view.relays {
        let links = relay.links.iter().map(|link| match link.rtt_ms {
            Some(rtt) => (t.backbone_link)(&name(link.to), rtt, link.jitter_ms, &lang.percent(link.loss_permille)),
            None => (t.backbone_down)(&name(link.to)),
        });
        rows.row(vec![
            relay.relay.name.clone(),
            online(relay.online).to_owned(),
            optional(relay.state.map(codes.relay_state)),
            optional(relay.load_percent),
            optional(relay.age_ms),
            optional(relay.reported_by.map(|from| {
                view.answered.iter().find(|node| node.id == from).map_or_else(|| format!("#{from}"), |node| node.name.clone())
            })),
            lang.list(links),
        ]);
    }
    rows.print();
    if view.answered.is_empty() {
        println!("\n{}", t.backbone_nobody);
    }
}

pub fn update_overview(overview: &UpdateOverview) {
    let (t, lang, codes) = context();
    let mut fields = Fields::new(0);
    fields.add(t.auto_update, if overview.auto_update { t.on } else { t.off }.to_owned());
    let source = if overview.source_is_default { lang.paren(&overview.source, t.default) } else { overview.source.clone() };
    fields.add(t.source, source);
    fields.add("controller", overview.controller_version.to_string());
    match &overview.release {
        Some(release) => fields.add(t.latest, lang.paren(&release.version.to_string(), &lang.time(release.published))),
        None => fields.add(t.latest, t.not_found_yet.to_owned()),
    }
    fields.add(t.checked, overview.checked_at.map_or_else(|| t.never.to_owned(), |at| lang.time(at)));
    if let Some(error) = &overview.check_error {
        let why = overview.check_cause.as_ref().map_or_else(|| error.clone(), codes.release_cause);
        fields.add(t.check_failed, why);
    }
    let nodes = overview.nodes;
    fields.add(t.nodes, (t.update_counts)(nodes.total, nodes.updating, nodes.outdated, nodes.manual, nodes.failed, nodes.up_to_date));
    fields.print();
    if let Some(release) = &overview.release {
        if release.controller_too_old {
            println!("{}", (t.controller_too_old)(&release.min_controller.to_string()));
        } else if release.newer_than_controller {
            println!("{}", (t.controller_newer)(&release.version.to_string()));
        }
        println!();
        let mut packages = Table::new(&t.packages_header).indented(2);
        for package in &release.packages {
            let error = package.cause.as_ref().map(codes.release_cause).or_else(|| package.error.clone());
            packages.row(vec![
                package.platform.clone(),
                lang.bytes(package.size),
                (codes.package_state)(package.state).to_owned(),
                optional(error),
            ]);
        }
        packages.print();
    }
    for paused in &overview.paused {
        let node = paused.node.as_ref().map_or(t.deleted_node, node_name);
        let why = paused.cause.as_ref().map_or_else(|| paused.reason.clone(), codes.update_cause);
        println!("\n{}", (t.paused)(&paused.version.to_string(), node, &lang.time(paused.at), &why));
        println!("{}", (t.resume_hint)(&paused.version.to_string()));
    }
}

pub fn node_updates(nodes: &[Node]) {
    let (t, _, codes) = context();
    let mut table = Table::new(&t.node_updates_header);
    for node in nodes {
        let ability = match node.update.self_update {
            None => t.too_old.to_owned(),
            Some(ability) if ability.able => t.yes.to_owned(),
            Some(ability) => ability.blocker.map_or_else(|| t.no.to_owned(), |blocker| (codes.blocker)(blocker).to_owned()),
        };
        table.row(vec![
            node.id.to_string(),
            node.name.clone(),
            online(node.online).to_owned(),
            node.version.clone(),
            optional(node.update.available.as_ref()),
            ability,
            update_summary(&node.update),
        ]);
    }
    print_table(&table, t.no_nodes);
}

/// 强制更新一个节点之后的回显。
pub fn node_update(name: &str, update: &NodeUpdate) {
    let (_, lang, _) = context();
    println!("{}", lang.labelled(name, &update_summary(update)));
}

pub fn forced_updates(forced: &ForcedUpdates) {
    let (t, lang, codes) = context();
    match &forced.version {
        Some(version) => println!("{}", (t.updating_to)(&version.to_string())),
        None => println!("{}", t.nothing_to_update),
    }
    let mut fields = Fields::new(2);
    fields.add(t.ordered, lang.list(forced.ordered.iter().map(node_name)));
    fields.add(t.queued, lang.list(forced.queued.iter().map(node_name)));
    for skipped in &forced.skipped {
        let why = skipped.cause.as_ref().map_or_else(|| skipped.reason.clone(), codes.skip_cause);
        fields.add(&(t.skipped)(&skipped.node.name), why);
    }
    fields.print();
}

/// 一个节点的更新情况，一短句。
fn update_summary(update: &NodeUpdate) -> String {
    let (t, _, codes) = context();
    let progress = match &update.status {
        i3t_model::update::UpdateStatus::Idle | i3t_model::update::UpdateStatus::Unknown => None,
        status => Some((codes.update_status)(status)),
    };
    let timing = |timing: UpdateTiming| match timing {
        UpdateTiming::Idle => t.when_idle,
        UpdateTiming::Now => t.now,
        UpdateTiming::AfterDrain => t.after_drain,
    };
    let order = update.ordered.as_ref().map(|order| (t.ordered_to)(&order.version.to_string(), timing(order.timing)));
    let waiting = update.waiting.map(|waiting| (codes.update_wait)(waiting).to_owned());
    let parts: Vec<String> = [progress, order, waiting].into_iter().flatten().collect();
    match (&update.available, parts.is_empty()) {
        (_, false) => parts.join(t.clauses),
        (Some(version), true) => (t.available)(&version.to_string()),
        (None, true) => t.up_to_date.to_owned(),
    }
}

pub fn node_access(access: &NodeAccess) {
    let (t, lang, _) = context();
    let mut table = Table::new(&t.access_header);
    let edges =
        access.can_reach.iter().map(|edge| (t.can_reach, edge)).chain(access.reachable_by.iter().map(|edge| (t.reachable_by, edge)));
    for (direction, edge) in edges {
        table.row(vec![
            direction.to_owned(),
            edge.node.name.clone(),
            edge.node.virtual_ip.to_string(),
            lang.list(edge.rules.iter().map(|rule| format!("#{rule}"))),
        ]);
    }
    print_table(&table, t.none_found);
    if !access.acknowledged {
        println!("{}", (t.not_acknowledged)(&access.node.name));
    }
}

pub fn tokens(tokens: &[EnrollTokenInfo]) {
    let (t, lang, codes) = context();
    let mut table = Table::new(&t.tokens_header);
    for token in tokens {
        table.row(vec![
            token.id.to_string(),
            if token.prefix.is_empty() { NONE.to_owned() } else { format!("{}…", token.prefix) },
            (codes.token_state)(token.state).to_owned(),
            lang.time(token.expires_at),
            optional(token.used_by.as_ref().map(|node| node.name.clone())),
            optional(token.virtual_ip),
            optional(token.successor_of.map(|node| format!("#{node}"))),
            group_names(lang, &token.groups),
        ]);
    }
    print_table(&table, t.no_tokens);
}

pub fn created_token(created: &CreatedEnrollToken) {
    let (t, lang, _) = context();
    println!("{}\n", (t.token_created)(created.info.id, &lang.time(created.info.expires_at)));
    println!("    {}\n", created.token);
    println!("{}", t.token_use);
}

pub fn rules(rules: &[Rule]) {
    let (t, _, _) = context();
    let mut table = Table::new(&t.rules_header);
    for rule in rules {
        table.row(vec![
            rule.id.to_string(),
            rule.src.name.clone(),
            arrow(rule.direction).to_owned(),
            rule.dst.name.clone(),
            yes_no(rule.enabled),
            rule.name.clone(),
        ]);
    }
    print_table(&table, t.no_rules);
}

pub fn rule(rule: &Rule) {
    let (t, lang, _) = context();
    let state = if rule.enabled { t.enabled } else { t.disabled };
    println!("{}", lang.paren(&format!("{} {}", t.rule, rule_text(rule)), state));
}

/// `#2 laptops -> servers`。
fn rule_text(rule: &Rule) -> String {
    format!("#{} {} {} {}", rule.id, rule.src.name, arrow(rule.direction), rule.dst.name)
}

const fn arrow(direction: Direction) -> &'static str {
    match direction {
        Direction::Both => "<->",
        Direction::OneWay => "->",
    }
}

pub fn credentials(credentials: &[Credential]) {
    let (t, lang, codes) = context();
    let mut table = Table::new(&t.credentials_header);
    for credential in credentials {
        table.row(vec![
            credential.id.to_string(),
            credential.prefix.clone(),
            (codes.credential_state)(credential.state).to_owned(),
            optional(credential.max_concurrent),
            optional(credential.max_bytes.map(|bytes| lang.bytes(bytes))),
            lang.bytes(credential.usage.bytes),
            credential.usage.connections.to_string(),
            optional(credential.usage.last_used_at.map(|at| lang.time(at))),
            credential.name.clone(),
        ]);
    }
    print_table(&table, t.no_credentials);
}

pub fn credential(credential: &Credential) {
    let (t, lang, codes) = context();
    println!(
        "{}",
        lang.paren(&format!("{} #{} {}", t.credential, credential.id, credential.name), (codes.credential_state)(credential.state))
    );
    let mut fields = Fields::new(2);
    fields.add(t.connection_limit, credential.max_concurrent.map_or_else(|| t.unlimited.to_owned(), |count| count.to_string()));
    fields.add(t.traffic_limit, credential.max_bytes.map_or_else(|| t.unlimited.to_owned(), |bytes| lang.bytes(bytes)));
    fields.add(t.usage, lang.bytes(credential.usage.bytes));
    fields.print();
}

pub fn created_credential(created: &CreatedCredential) {
    let (t, _, _) = context();
    println!("{}\n", (t.credential_created)(created.credential.id, &created.credential.name));
    println!("    {}\n", created.token);
    println!("{}", t.credential_use);
}

pub fn listeners(listeners: &[Listener]) {
    let (t, _, _) = context();
    let mut table = Table::new(&t.listeners_header);
    for listener in listeners {
        table.row(listener_row(listener));
    }
    print_table(&table, t.no_listeners);
}

pub fn listener(listener: &Listener) {
    let (t, _, _) = context();
    println!(
        "{} #{} {}  #{} {} {}",
        t.listener,
        listener.id,
        listener.config.name,
        listener.node,
        listener.config.bind,
        label(&listener.config.protocol)
    );
}

fn listener_row(listener: &Listener) -> Vec<String> {
    let (_, lang, codes) = context();
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
        lang.list(&config.allow_from),
        listener.status.as_ref().map_or_else(|| NONE.to_owned(), |status| (codes.listen_state)(&status.state)),
        config.name.clone(),
    ]
}

pub fn forwards(forwards: &[Forward]) {
    let (t, lang, codes) = context();
    let mut table = Table::new(&t.forwards_header);
    for forward in forwards {
        let [state, connections, traffic] = listen_cells(forward.listen.as_ref());
        table.row(vec![
            forward.id.to_string(),
            label(&forward.protocol),
            format!("#{} {}", forward.listen_node, forward.bind),
            format!("#{} {}", forward.target_node, forward.target_addr),
            (codes.forward_status)(forward.status).to_owned(),
            state,
            connections,
            traffic,
            lang.list(&forward.allow_from),
            forward.name.clone(),
        ]);
    }
    print_table(&table, t.no_forwards);
}

pub fn forward(forward: &Forward) {
    let (t, lang, codes) = context();
    let route = format!("#{} {} → #{} {}", forward.listen_node, forward.bind, forward.target_node, forward.target_addr);
    println!(
        "{}",
        lang.paren(&format!("{} #{} {}  {route}", t.forward, forward.id, label(&forward.protocol)), (codes.forward_status)(forward.status))
    );
}

pub fn pairs(pairs: &[PairView]) {
    let (t, _, _) = context();
    let mut table = Table::new(&t.pairs_header);
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
    print_table(&table, t.no_pairs);
}

pub fn pair(pair: &PairView) {
    let (t, lang, codes) = context();
    let setting = &pair.setting;
    println!("#{} ↔ #{}", setting.a, setting.b);
    let mut fields = Fields::new(2);
    fields.add(t.keep_alive, yes_no(setting.keep_alive));
    fields.add(t.lanes, setting.lane.to_string());
    let names = |id: NodeId| format!("#{id}");
    if let Some(effect) = &pair.lane_effect {
        fields.add(t.lane_effect, lane_effect_cell(effect));
        if !effect.dialers.is_empty() {
            fields.add(t.dialers, lang.list(effect.dialers.iter().map(|id| format!("#{id}"))));
        }
        for block in &effect.blocked {
            fields.add(t.no_effect, (codes.lane_block)(block, &names));
        }
        for opening in &effect.opening {
            let path = if opening.udp_path { t.udp_path } else { t.no_udp_path };
            let lanes = if opening.lanes { t.new_streams_on_lanes } else { t.new_streams_shared };
            fields.add(&format!("#{}", opening.node), lang.paren(lanes, path));
        }
    }
    for PairLaneSide { node, lanes, reported_secs_ago } in &pair.lanes {
        let label = match reported_secs_ago {
            Some(ago) => format!("#{node} ({})", lang.ago(*ago)),
            None => format!("#{node}"),
        };
        fields.add(&label, peer_lanes(lanes));
    }
    fields.print();
}

fn lane_effect_cell(effect: &LaneEffect) -> String {
    let (t, lang, codes) = context();
    if effect.effective {
        return t.in_effect.to_owned();
    }
    let names = |id: NodeId| format!("#{id}");
    lang.paren(t.no_effect, &lang.list(effect.blocked.iter().map(|block| (codes.lane_block)(block, &names))))
}

/// 和一个对端之间的 lane，一行。
fn peer_lanes(lanes: &PeerLanes) -> String {
    let (t, _, _) = context();
    (t.peer_lanes)(lanes.dials, lanes.control, lanes.idle, lanes.ready, lanes.parked_at_peer, &lane_counts(&lanes.counts))
}

fn lane_counts(counts: &LaneCounts) -> String {
    let (t, lang, _) = context();
    (t.lane_counts)(
        counts,
        &format!("{} / {} / {}", lang.bytes(counts.sealed_bytes), lang.bytes(counts.passthrough_bytes), lang.bytes(counts.spliced_bytes)),
    )
}

fn lane_report(report: &LaneReport) {
    let (t, _, _) = context();
    let failures = &report.handshake_failures;
    println!("\n{}", t.lanes);
    let mut fields = Fields::new(2);
    fields.add(t.total, lane_counts(&report.total));
    fields.add(
        t.handshake_failures,
        (t.handshake_failure_counts)(failures.deadline, failures.unknown_key, failures.tls, failures.protocol, failures.busy),
    );
    for peer in &report.peers {
        fields.add(&format!("#{}", peer.peer), peer_lanes(peer));
    }
    fields.print();
}

pub fn access_graph(graph: &AccessGraph) {
    let (t, lang, _) = context();
    let mut groups = Table::new(&t.graph_groups_header);
    for group in &graph.groups {
        groups.row(vec![group.name.clone(), group.nodes.len().to_string(), lang.list(group.nodes.iter().map(node_name))]);
    }
    groups.print();
    println!();
    rules(&graph.rules);
    if !graph.unconnected.is_empty() {
        println!("\n{}", lang.labelled(t.unconnected, &lang.list(graph.unconnected.iter().map(node_name))));
    }
    if !graph.unacknowledged.is_empty() {
        println!("\n{}", lang.labelled(t.unacknowledged, &lang.list(graph.unacknowledged.iter().map(node_name))));
    }
}

pub fn access_check(check: &AccessCheck) {
    let (t, lang, _) = context();
    if check.allowed {
        let rules = lang.list(check.rules.iter().map(rule_text));
        println!("{}", lang.labelled(&(t.can_access)(&check.from.name, &check.to.name), &rules));
    } else {
        println!("{}", (t.cannot_access)(&check.from.name, &check.to.name));
    }
}

fn node_name(node: &NodeRef) -> &str {
    &node.name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_details_and_segments_read_in_the_current_language() {
        let (laptop, relay) = (NodeId::new(1).unwrap(), NodeId::new(9).unwrap());
        let segment = PathSegment { from: laptop, to: relay, rtt_us: 20_400, jitter_us: 500, loss_permille: 12 };
        let name = |id: NodeId| if id == laptop { "laptop".to_owned() } else { format!("#{id}") };
        let text = leg_text(&segment, &name);
        assert!(text.contains("laptop→#9") && text.contains("20.4") && text.contains("1.2%"), "{text}");
        let direct = Route::Direct { addr: "203.0.113.9:20330".parse().unwrap() };
        assert_ne!(kind_cell(&direct, Some(DirectKind::Punched), None), NONE);
        assert_eq!(kind_cell(&Route::Relay { relays: vec![relay] }, Some(DirectKind::Lan), Some(LegTransport::Tcp)), "TCP");
        assert_eq!(kind_cell(&direct, None, Some(LegTransport::Tcp)), NONE, "节点没有说的时候是短横");
        assert_eq!(label(&i3t_model::netmap::RoutingMode::LowestCost), "lowest_cost");
    }
}
