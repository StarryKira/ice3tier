//! 节点的状态打印成给人看的样子：`i3t status`（本机的配置，和正在运行的节点现在的情况）与 `i3t routes`。

use i3t_agent::config::Config;
use i3t_agent::state::StateDir;
use i3t_agent::status::{Link, LiveListener, LiveStatus};
use i3t_lang::table::{Fields, Table};
use i3t_lang::{Lang, NONE, optional};
use i3t_model::control::{BindCause, LaneReport};
use i3t_model::update::UpdateStatus;
use i3t_proto::NodeId;

use crate::texts::Ui;

fn context() -> (&'static Ui, Lang) {
    let lang = i3t_lang::current();
    (&crate::texts::texts_in(lang).ui, lang)
}

/// 本机的配置和加入的网络：节点没有在运行时也有。
pub fn local(state: &StateDir) -> anyhow::Result<()> {
    let (t, lang) = context();
    let config = Config::load(&state.config_path())?;
    let mut fields = Fields::new(0);
    fields.add(t.state_dir, state.path().display().to_string());
    fields.add(t.port, config.port.to_string());
    let declared = [(config.declare.exit, t.exit), (config.declare.relay, "relay"), (config.declare.forward, t.forward_target)];
    fields.add(t.declared, lang.list(declared.into_iter().filter_map(|(on, name)| on.then_some(name))));
    let (Some(controller), Some(netmap)) = (state.controller()?, state.netmap()?) else {
        fields.print();
        println!("{}", t.not_joined);
        return Ok(());
    };
    fields.add("controller", controller.address);
    fields.add(t.network, lang.paren(&netmap.network.name, &netmap.network.cidr.to_string()));
    fields.add(t.node, netmap.me.name.clone());
    fields.add(t.virtual_ip, netmap.me.virtual_ip.to_string());
    fields.add("netmap", (t.netmap)(netmap.version, netmap.peers.len(), netmap.relays.len()));
    fields.print();
    Ok(())
}

pub fn status(live: &LiveStatus) {
    let (t, lang) = context();
    println!("\n{}", (t.running)(&live.version, &lang.duration(live.uptime_secs)));
    let link = match live.link {
        Link::Static => t.link_static,
        Link::Online => t.link_online,
        Link::Offline => t.link_offline,
        Link::Revoked => t.link_revoked,
        Link::Rejected => t.link_rejected,
        Link::Unknown => t.unknown,
    };
    let mut fields = Fields::new(0);
    fields.add("controller", link.to_owned());
    fields.add("netmap", (t.netmap_version)(live.netmap_version));
    fields.add(t.flows, live.flows.to_string());
    fields.add(t.endpoints, lang.list(&live.endpoints));
    if live.mapping_varies == Some(true) {
        fields.add(t.nat, t.mapping_varies.to_owned());
    }
    fields.add(t.attached, lang.list(live.attached.iter().map(|attachment| format!("#{} {} ms", attachment.relay, attachment.rtt_ms))));
    if let Some(relay) = &live.relay {
        fields.add("relay", (t.relay_counts)(relay.sessions, relay.forwarded_packets, &lang.bytes(relay.forwarded_bytes), relay.dropped));
    }
    fields.add(t.update, update(live));
    if live.update.draining {
        fields.add(t.state, t.draining.to_owned());
    }
    if let Some(quota) = live.exit.as_ref().and_then(|exit| exit.quota) {
        fields.add(t.quota, format!("{} / {}", lang.bytes(quota.used_bytes), lang.bytes(quota.limit_bytes)));
    }
    if let Some(performance) = &live.performance {
        let mut summary = (t.profile_running)(performance.profile.spelling(), performance.threads);
        if performance.restart_pending {
            summary = lang.paren(&summary, t.restart_pending);
        }
        fields.add(t.profile, summary);
    }
    fields.print();

    println!("\n{}", t.listeners);
    let mut table = Table::new(&t.listeners_header).indented(2);
    for listener in &live.listeners {
        let traffic = format!("↑ {}  ↓ {}", lang.bytes(listener.bytes_up), lang.bytes(listener.bytes_down));
        table.row(vec![listener.name.clone(), listener.bind.to_string(), listener.active.to_string(), traffic, listen_state(listener)]);
    }
    print_table(&table);

    if let Some(lanes) = &live.lanes {
        print_lanes(live, lanes);
    }

    println!("\n{}", t.routes);
    route_table(live);
}

fn print_table(table: &Table) {
    let (t, _) = context();
    if table.is_empty() { println!("  {}", t.none) } else { table.print() }
}

/// 一个监听的状态，一短句。节点报了原因的代码时按代码写，否则用系统的原话。
fn listen_state(listener: &LiveListener) -> String {
    let (t, lang) = context();
    let codes = lang.codes();
    let detail = || listener.detail.clone().unwrap_or_default();
    match listener.state.as_str() {
        "listening" => listener.local.map_or_else(|| t.listening.to_owned(), |local| (t.listening_on)(&local.to_string())),
        "virtual" => t.on_virtual_ip.to_owned(),
        "port_in_use" => (codes.bind_cause)(BindCause::AddressInUse).to_owned(),
        "failed" | "unsupported" => {
            listener.cause.map_or_else(|| lang.labelled(t.not_bound, &detail()), |cause| (codes.bind_cause)(cause).to_owned())
        }
        _ => t.unknown.to_owned(),
    }
}

/// 独占 TCP：每个打开了它的对端一行。
fn print_lanes(live: &LiveStatus, lanes: &LaneReport) {
    let (t, lang) = context();
    println!("\n{}", t.lanes);
    let name =
        |peer: u32| live.routes.iter().find(|route| route.peer == peer).map_or_else(|| format!("#{peer}"), |route| route.name.clone());
    let mut table = Table::new(&t.lanes_header).indented(2);
    for peer in &lanes.peers {
        let counts = &peer.counts;
        table.row(vec![
            name(peer.peer.get()),
            if peer.dials { t.dial } else { t.accept }.to_owned(),
            if peer.control { t.up } else { t.down }.to_owned(),
            format!("{}/{}/{}", peer.idle, peer.ready, peer.parked_at_peer),
            counts.in_use.to_string(),
            format!("{} ({})", counts.claims, counts.claims_waited),
            counts.fallbacks.to_string(),
            counts.dial_failures.to_string(),
            lang.bytes(counts.sealed_bytes + counts.passthrough_bytes + counts.spliced_bytes),
        ]);
    }
    print_table(&table);
    let failures = &lanes.handshake_failures;
    let failed = failures.deadline + failures.unknown_key + failures.tls + failures.protocol + failures.busy;
    if failed > 0 {
        println!(
            "  {}",
            (t.handshake_failures)(failed, failures.deadline, failures.unknown_key, failures.tls, failures.protocol, failures.busy)
        );
    }
}

pub fn routes(live: &LiveStatus) {
    let (t, _) = context();
    if live.routes.is_empty() {
        println!("{}", t.no_peers);
        return;
    }
    route_table(live);
}

fn route_table(live: &LiveStatus) {
    let (t, lang) = context();
    let codes = lang.codes();
    let name =
        |id: NodeId| live.routes.iter().find(|route| route.peer == id.get()).map_or_else(|| format!("#{id}"), |route| route.name.clone());
    let mut table = Table::new(&t.routes_header).indented(2);
    for entry in &live.routes {
        let since = entry.since.as_ref().map_or_else(
            || NONE.to_owned(),
            |since| {
                let why = since.cause.as_ref().map_or_else(|| since.reason.clone(), |cause| (codes.route_cause)(cause, &name));
                lang.labelled(&lang.ago(since.secs_ago), &why)
            },
        );
        table.row(vec![entry.name.clone(), optional(entry.virtual_ip), (codes.route)(&entry.route, &name), optional(entry.rtt_ms), since]);
    }
    print_table(&table);
}

/// 更新的情况，一短句；节点不能自己更新时说为什么。
fn update(live: &LiveStatus) -> String {
    let (t, lang) = context();
    let codes = lang.codes();
    let progress = match &live.update.status {
        UpdateStatus::Unknown => t.unknown.to_owned(),
        status => (codes.update_status)(status),
    };
    match live.update.self_update {
        Some(ability) if !ability.able => {
            let why = ability.blocker.map_or(t.update_record_unreadable, codes.blocker);
            lang.sentences(&format!("{progress}{}", t.period), &lang.labelled(t.cannot_self_update, why))
        }
        _ => progress,
    }
}

#[cfg(test)]
mod tests {
    use i3t_agent::status::LiveUpdate;
    use i3t_model::control::SelfUpdate;
    use i3t_model::update::SelfUpdateBlocker;

    use super::*;

    #[test]
    fn the_update_line_says_what_is_going_on_and_why_a_node_cannot_update_itself() {
        let (t, lang) = context();
        let codes = lang.codes();
        let with =
            |status, self_update| LiveStatus { update: LiveUpdate { status, self_update, draining: false }, ..LiveStatus::default() };
        assert_eq!(update(&with(UpdateStatus::Idle, Some(SelfUpdate::ABLE))), (codes.update_status)(&UpdateStatus::Idle));
        let blocked = update(&with(UpdateStatus::Idle, Some(SelfUpdate::from(Some(SelfUpdateBlocker::Container)))));
        assert!(blocked.contains(t.cannot_self_update) && blocked.contains((codes.blocker)(SelfUpdateBlocker::Container)), "{blocked}");
        let waiting = update(&with(UpdateStatus::WaitingForIdle { version: "0.2.0".parse().unwrap() }, None));
        assert!(waiting.contains("0.2.0"), "{waiting}");
    }

    #[test]
    fn listen_states_read_from_their_cause_when_the_node_gave_one() {
        let (t, lang) = context();
        let listener = |state: &str, cause, detail: &str| LiveListener {
            name: "socks".into(),
            bind: "127.0.0.1:1080".parse().unwrap(),
            state: state.into(),
            detail: Some(detail.into()),
            local: None,
            cause,
            active: 0,
            bytes_up: 0,
            bytes_down: 0,
        };
        let denied = listen_state(&listener("failed", Some(BindCause::PermissionDenied), "Permission denied (os error 13)"));
        assert_eq!(denied, (lang.codes().bind_cause)(BindCause::PermissionDenied));
        let older = listen_state(&listener("failed", None, "boom"));
        assert!(older.contains("boom") && older.contains(t.not_bound), "旧版本的节点只给原话：{older}");
    }
}
