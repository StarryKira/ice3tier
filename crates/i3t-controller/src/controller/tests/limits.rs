//! 限额、用量和状态：凭据的上限和用量、出口的用量和健康、监听的状态、自动挑端口的端口转发、服务等级、
//! 注册令牌的记录、操作记录里提到的东西、"最近在线"。

use i3t_model::admin::{
    CreateCredential, CreateEnrollToken, CredentialState, ExitHealth, ForwardInput, ListenStatus, ListenerInput, LogRef, RefKind,
    TokenState, UpdateCredential, UpdateNetwork,
};
use i3t_model::control::{
    BindCause, CredentialUsage, Declared, ExitObservation, ExitReport, ForwardReport, ListenState, ListenTraffic, ListenerReport,
    QuotaUsage, Report, TargetCause, TargetFailureReport, ToController,
};
use i3t_model::netmap::{self, ExitRole, ForwardProtocol, ListenerProtocol};
use i3t_proto::flow::Class;

use super::super::testing::{CAN_DO_ALL, Lab};
use crate::store::LogFilter;

fn listener(node: i3t_proto::NodeId, name: &str, bind: &str, require_credential: bool) -> ListenerInput {
    ListenerInput {
        node,
        config: netmap::Listener {
            name: name.to_owned(),
            bind: bind.parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            allow_from: Vec::new(),
            require_credential,
            default_action: None,
            fixed_exit: None,
            class: None,
            optimistic: None,
        },
    }
}

fn forward(listen_node: i3t_proto::NodeId, target_node: i3t_proto::NodeId, bind: &str) -> ForwardInput {
    ForwardInput {
        name: String::new(),
        protocol: ForwardProtocol::Tcp,
        listen_node,
        bind: bind.parse().unwrap(),
        target_node,
        target_addr: "127.0.0.1:5432".parse().unwrap(),
        allow_from: Vec::new(),
        enabled: true,
        class: None,
        proxy_protocol: None,
    }
}

fn report(run: u64) -> Report {
    Report { run, ..Report::default() }
}

#[test]
fn joining_alone_does_not_count_as_having_been_online() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let joined = lab.join(network.id, &[]);
    let node = lab.controller.node(network.id, joined.id).unwrap().node;
    assert!(node.online, "注册的那条连接还在");
    assert_eq!(node.last_seen_at, None, "只是 i3t join 拿了配置，节点还没有真的跑起来");
    lab.controller.disconnect(joined.session);
    assert_eq!(lab.controller.node(network.id, joined.id).unwrap().node.last_seen_at, None);

    let running = lab.connect(joined.key, Declared::default()).unwrap();
    assert!(lab.controller.node(network.id, joined.id).unwrap().node.last_seen_at.is_some(), "带着 Hello 连上来才算");
    lab.controller.disconnect(running.session);
    let node = lab.controller.node(network.id, joined.id).unwrap().node;
    assert!(!node.online && node.last_seen_at.is_some(), "离线之后留着最近在线的时间");
}

#[test]
fn credential_usage_from_ingress_reports_adds_up_and_an_exhausted_credential_is_flagged_in_the_netmap() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut gateway = lab.join(network.id, &[]);
    lab.controller.create_listener(network.id, listener(gateway.id, "public", "0.0.0.0:3128", true)).unwrap();
    let created = lab
        .controller
        .create_credential(network.id, &CreateCredential { name: "phone".to_owned(), max_concurrent: None, max_bytes: Some(10_000) })
        .unwrap();
    let id = created.credential.id;
    assert_eq!(created.credential.prefix, created.token[..6], "前缀就是令牌的开头");
    lab.settle(&mut [&mut gateway]);
    let delivered = gateway.netmap().credentials[0].clone();
    assert_eq!((delivered.id, delivered.exhausted), (id, false));

    let usage = |bytes, connections| CredentialUsage { id, bytes, connections, last_used_secs_ago: Some(0) };
    gateway.send(&lab.controller, ToController::Report(Box::new(Report { credentials: vec![usage(6_000, 2)], ..report(7) })));
    let shown = lab.controller.credentials(network.id).unwrap().remove(0);
    assert_eq!((shown.usage.bytes, shown.usage.connections, shown.state), (6_000, 2, CredentialState::Active));
    assert!(shown.usage.last_used_at.is_some());

    // 同一次运行里报的是累计数：只把新增的加上。
    gateway.send(&lab.controller, ToController::Report(Box::new(Report { credentials: vec![usage(11_000, 1)], ..report(7) })));
    lab.settle(&mut [&mut gateway]);
    let shown = lab.controller.credentials(network.id).unwrap().remove(0);
    assert_eq!((shown.usage.bytes, shown.state), (11_000, CredentialState::Exhausted));
    assert!(gateway.netmap().credentials[0].exhausted, "用完了：入口要据此拒绝新的连接、断开正在进行的");

    // 调高上限，不用重新签发令牌。
    let raised = UpdateCredential { max_bytes: Some(Some(20_000)), ..UpdateCredential::default() };
    assert_eq!(lab.controller.update_credential(network.id, id, &raised).unwrap().state, CredentialState::Active);
    lab.settle(&mut [&mut gateway]);
    assert!(!gateway.netmap().credentials[0].exhausted);
    assert_eq!(gateway.netmap().credentials[0].token, delivered.token, "令牌没有变");

    // 用量清零。
    let reset = UpdateCredential { reset_usage: true, ..UpdateCredential::default() };
    assert_eq!(lab.controller.update_credential(network.id, id, &reset).unwrap().usage.bytes, 0);
    assert!(
        lab.controller
            .update_credential(network.id, id, &UpdateCredential { max_bytes: Some(Some(0)), ..UpdateCredential::default() })
            .is_err()
    );

    // 吊销：记录还在，状态是吊销，入口不再拿到它。
    lab.controller.delete_credential(network.id, id).unwrap();
    lab.settle(&mut [&mut gateway]);
    let revoked = lab.controller.credentials(network.id).unwrap().remove(0);
    assert_eq!(revoked.state, CredentialState::Revoked);
    assert!(revoked.revoked_at.is_some());
    assert!(gateway.netmap().credentials.is_empty());
    let taken =
        lab.controller.create_credential(network.id, &CreateCredential { name: "phone".to_owned(), max_concurrent: None, max_bytes: None });
    assert!(taken.is_err(), "吊销了的还占着名字");
    lab.controller.delete_credential(network.id, id).unwrap();
    assert!(lab.controller.credentials(network.id).unwrap().is_empty(), "再删一次才彻底删掉");
}

#[test]
fn an_exit_keeps_its_own_counts_and_the_initiators_view_for_cross_checking_and_health_counts_only_its_own_failures() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let exit = lab.join_with(network.id, &[], CAN_DO_ALL);
    let laptop = lab.join(network.id, &[]);
    lab.controller.enable_exit(network.id, exit.id, ExitRole { quota_bytes: Some(1_000_000), ..ExitRole::default() }).unwrap();

    let view = lab.controller.exit(network.id, exit.id).unwrap();
    assert_eq!((view.health, view.usage.clone(), view.observed.len()), (ExitHealth::Unknown, None, 0));

    let quota = QuotaUsage { limit_bytes: 1_000_000, used_bytes: 400, period_start: 1, period_end: 2 };
    let counts =
        |total, failed_by_exit, refused| ExitReport { total, failed_by_exit, refused, quota: Some(quota), ..ExitReport::default() };
    exit.send(&lab.controller, ToController::Report(Box::new(Report { exit: Some(Box::new(counts(10, 0, 0))), ..report(1) })));
    // 被拒绝的和目标的问题都不算出口坏了，只有它自己的失败算。
    exit.send(&lab.controller, ToController::Report(Box::new(Report { exit: Some(Box::new(counts(40, 1, 25))), ..report(1) })));
    let observed = ExitObservation {
        exit: exit.id,
        flows: 31,
        bytes_up: 100,
        bytes_down: 300,
        failed_by_target: 2,
        refused: 25,
        failed_by_exit: 1,
        unreachable: 0,
    };
    laptop.send(&lab.controller, ToController::Report(Box::new(Report { exits: vec![observed.clone()], ..report(9) })));

    let views = lab.controller.exits(network.id).unwrap();
    assert_eq!(views.len(), 1, "只有启用了出口的节点");
    let view = &views[0];
    assert_eq!((view.recent_requests, view.recent_failures, view.health), (30, 1, ExitHealth::Ok));
    assert_eq!(view.usage.as_ref().unwrap().quota, Some(quota));
    assert_eq!(view.observed.len(), 1);
    assert_eq!((view.observed[0].by.id, view.observed[0].counts.clone()), (laptop.id, observed));

    exit.send(&lab.controller, ToController::Report(Box::new(Report { exit: Some(Box::new(counts(50, 5, 25))), ..report(1) })));
    assert_eq!(lab.controller.exit(network.id, exit.id).unwrap().health, ExitHealth::Degraded, "40 个请求里 5 个是出口自己的问题");
    assert!(lab.controller.exit(network.id, laptop.id).is_err(), "不是出口的节点没有出口的视图");
}

#[test]
fn listener_and_forward_status_reported_by_the_node_shows_up_including_its_builtin_listeners() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let gw = lab.join(network.id, &[]);
    let target = lab.join_with(network.id, &[], CAN_DO_ALL);
    lab.switch_on_forward_target(network.id, target.id);
    let configured = lab.controller.create_listener(network.id, listener(gw.id, "lan", "192.168.1.5:1080", true)).unwrap();
    let auto = lab.controller.create_forward(network.id, forward(gw.id, target.id, "127.0.0.1:0")).unwrap();
    let busy = lab.controller.create_forward(network.id, forward(gw.id, target.id, "127.0.0.1:8080")).unwrap();
    assert_eq!((configured.status.clone(), auto.listen.clone()), (None, None), "还没有报过");

    let traffic = ListenTraffic { connections: 1, bytes_up: 10, bytes_down: 20 };
    let local = "127.0.0.1:41000".parse().unwrap();
    let report = Report {
        listeners: vec![
            ListenerReport {
                name: "lan".to_owned(),
                bind: "192.168.1.5:1080".parse().unwrap(),
                protocol: ListenerProtocol::Socks5,
                builtin: false,
                state: ListenState::Listening { local: "192.168.1.5:1080".parse().unwrap() },
                traffic,
            },
            ListenerReport {
                name: "local-socks5".to_owned(),
                bind: "127.0.0.1:1080".parse().unwrap(),
                protocol: ListenerProtocol::Socks5,
                builtin: true,
                state: ListenState::PortInUse { detail: "Address already in use".to_owned(), cause: Some(BindCause::AddressInUse) },
                traffic: ListenTraffic::default(),
            },
        ],
        forwards: vec![
            ForwardReport {
                id: auto.id,
                bind: "127.0.0.1:0".parse().unwrap(),
                protocol: ForwardProtocol::Tcp,
                state: ListenState::Listening { local },
                traffic,
                target_failure: None,
            },
            ForwardReport {
                id: busy.id,
                bind: "127.0.0.1:8080".parse().unwrap(),
                protocol: ForwardProtocol::Tcp,
                state: ListenState::PortInUse { detail: "Address already in use".to_owned(), cause: Some(BindCause::AddressInUse) },
                traffic: ListenTraffic::default(),
                target_failure: Some(TargetFailureReport { cause: TargetCause::Timeout, secs_ago: 30 }),
            },
        ],
        ..report(3)
    };
    gw.send(&lab.controller, ToController::Report(Box::new(report)));

    let listeners = lab.controller.listeners(network.id, None).unwrap();
    assert_eq!(
        listeners[0].status,
        Some(ListenStatus { state: ListenState::Listening { local: "192.168.1.5:1080".parse().unwrap() }, traffic })
    );
    let forwards = lab.controller.forwards(network.id).unwrap();
    assert_eq!(
        forwards[0].listen.as_ref().map(|listen| listen.state.clone()),
        Some(ListenState::Listening { local }),
        "端口写 0 时报的是挑定的端口"
    );
    assert!(matches!(forwards[1].listen.as_ref().map(|listen| &listen.state), Some(ListenState::PortInUse { .. })));
    assert_eq!(forwards[0].target_failure, None);
    let failure = forwards[1].target_failure.expect("监听节点报了目标连不上");
    assert_eq!(failure.cause, TargetCause::Timeout);
    let now = lab.controller.lock().clock.now();
    assert!((now - 32..=now - 30).contains(&failure.at), "按 controller 的时钟换算成 30 秒之前：{} 对 {now}", failure.at);

    let on_node = lab.controller.node_listeners(network.id, gw.id).unwrap();
    assert!(on_node.online && on_node.reported_secs_ago.is_some());
    assert_eq!(on_node.listeners.len(), 2);
    assert_eq!((on_node.listeners[0].id, on_node.listeners[0].builtin), (Some(configured.id), false));
    assert_eq!((on_node.listeners[1].id, on_node.listeners[1].builtin, on_node.listeners[1].name.as_str()), (None, true, "local-socks5"));
    assert_eq!(on_node.forwards.len(), 2);

    lab.controller.disconnect(gw.session);
    assert_eq!(lab.controller.listeners(network.id, None).unwrap()[0].status, None, "节点不在线时没有状态");
}

#[test]
fn a_forward_may_leave_its_port_to_the_node_except_on_a_virtual_ip() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let listen = lab.join_offline(network.id, &[], Declared::default());
    let target = lab.join_offline(network.id, &[], CAN_DO_ALL);
    lab.switch_on_forward_target(network.id, target);
    let first = lab.controller.create_forward(network.id, forward(listen, target, "127.0.0.1:0")).unwrap();
    lab.controller.create_forward(network.id, forward(listen, target, "127.0.0.1:0")).expect("端口写 0 的各挑各的，不算冲突");
    assert_eq!(
        lab.netmap(listen).me.forwards.iter().find(|entry| entry.id == first.id).unwrap().bind.port(),
        0,
        "下发的就是 0，由节点去挑"
    );
    let vip = lab.controller.node(network.id, listen).unwrap().node.virtual_ip;
    let refused = lab.controller.create_forward(network.id, forward(listen, target, &format!("{vip}:0")));
    assert!(refused.is_err(), "虚拟 IP 上的转发不占端口，没有空闲的端口可挑");
}

#[test]
fn classes_and_the_optimistic_reply_set_on_the_network_listener_forward_and_exit_reach_the_netmaps() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut laptop = lab.join(network.id, &[]);
    let mut exit = lab.join_with(network.id, &[], CAN_DO_ALL);
    lab.controller.update_network(network.id, &UpdateNetwork { class: Some(Some(Class::Reliable)), ..UpdateNetwork::default() }).unwrap();
    lab.controller
        .enable_exit(network.id, exit.id, ExitRole { class: Some(Class::Bulk), optimistic: Some(false), ..ExitRole::default() })
        .unwrap();
    let mut socks = listener(laptop.id, "socks", "127.0.0.1:1080", false);
    socks.config.class = Some(Class::Interactive);
    socks.config.optimistic = Some(false);
    lab.controller.create_listener(network.id, socks).unwrap();
    lab.switch_on_forward_target(network.id, exit.id);
    let mut tagged = forward(laptop.id, exit.id, "127.0.0.1:9000");
    tagged.class = Some(Class::Interactive);
    let created = lab.controller.create_forward(network.id, tagged).unwrap();
    assert_eq!(created.class, Some(Class::Interactive));
    lab.settle(&mut [&mut laptop, &mut exit]);

    let netmap = laptop.netmap();
    assert_eq!(netmap.network.class, Some(Class::Reliable));
    assert_eq!((netmap.me.listeners[0].class, netmap.me.listeners[0].optimistic), (Some(Class::Interactive), Some(false)));
    assert_eq!(netmap.me.forwards[0].class, Some(Class::Interactive));
    assert_eq!(netmap.peer(exit.id).unwrap().exit.unwrap().class, Some(Class::Bulk), "入口按出口的默认值挑等级");
    assert_eq!(netmap.peer(exit.id).unwrap().exit.unwrap().optimistic, Some(false), "入口按出口的设置决定等不等应答");
    assert_eq!(lab.controller.network(network.id).unwrap().class, Some(Class::Reliable));
    lab.controller.update_network(network.id, &UpdateNetwork { class: Some(None), ..UpdateNetwork::default() }).unwrap();
    assert_eq!(lab.controller.network(network.id).unwrap().class, None, "写 null 改回 auto");
}

#[test]
fn a_tcp_forward_can_ask_its_target_node_for_a_proxy_protocol_header_and_a_udp_one_cannot() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut laptop = lab.join(network.id, &[]);
    let mut target = lab.join_with(network.id, &[], CAN_DO_ALL);
    lab.switch_on_forward_target(network.id, target.id);
    let mut input = forward(laptop.id, target.id, "127.0.0.1:9000");
    input.proxy_protocol = Some(netmap::ProxyProtocol::V2);
    let created = lab.controller.create_forward(network.id, input.clone()).unwrap();
    assert_eq!(created.proxy_protocol, Some(netmap::ProxyProtocol::V2));
    lab.settle(&mut [&mut laptop, &mut target]);
    assert_eq!(target.netmap().me.forward_targets[0].proxy_protocol, Some(netmap::ProxyProtocol::V2), "目标节点收到这一项");
    assert_eq!(lab.controller.forwards(network.id).unwrap()[0].proxy_protocol, Some(netmap::ProxyProtocol::V2), "存下来了");

    let udp = ForwardInput { protocol: ForwardProtocol::Udp, bind: "127.0.0.1:9001".parse().unwrap(), ..input };
    let refused = lab.controller.create_forward(network.id, udp).unwrap_err();
    assert_eq!((refused.what().as_deref(), refused.field()), (Some("tcp_only"), Some("proxy_protocol")));
}

#[test]
fn used_and_expired_tokens_stay_listed_with_the_node_that_used_them() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let created = lab.controller.create_token(network.id, &CreateEnrollToken::default()).unwrap();
    assert!(created.token.starts_with(&created.info.prefix) && created.info.prefix.len() == 18);
    let other = lab.controller.create_token(network.id, &CreateEnrollToken::default()).unwrap();
    assert_ne!(created.info.prefix, other.info.prefix, "前缀分得出是哪一个");
    let secret = created.token.parse::<i3t_model::control::EnrollToken>().unwrap().secret;
    let key = lab.fresh_key();
    let joined = lab.enroll(key, &secret, Declared::default()).unwrap();

    let pending = lab.controller.tokens(network.id, false).unwrap();
    assert_eq!(pending.iter().map(|token| token.id).collect::<Vec<_>>(), [other.info.id], "默认只列还能用的");
    let all = lab.controller.tokens(network.id, true).unwrap();
    let used = all.iter().find(|token| token.id == created.info.id).unwrap();
    assert_eq!((used.state, used.used_by.as_ref().map(|node| node.id)), (TokenState::Used, Some(joined.id)));
    assert!(used.used_at.is_some());
    assert_eq!(lab.controller.overview(network.id).unwrap().pending_tokens, 1);

    lab.controller.advance_clock(2 * 24 * 60 * 60);
    let all = lab.controller.tokens(network.id, true).unwrap();
    assert_eq!(all.iter().find(|token| token.id == other.info.id).unwrap().state, TokenState::Expired);
    assert!(lab.controller.tokens(network.id, false).unwrap().is_empty());
}

#[test]
fn log_entries_name_what_they_are_about() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let node = lab.join_offline(network.id, &[], Declared::default());
    let credential = lab
        .controller
        .create_credential(network.id, &CreateCredential { name: "c".to_owned(), max_concurrent: None, max_bytes: None })
        .unwrap();
    let about_node =
        lab.controller.oplog_filtered(&LogFilter { limit: 10, reference: Some(LogRef::node(node)), ..LogFilter::default() }).unwrap();
    assert_eq!(about_node.iter().map(|entry| entry.action.as_str()).collect::<Vec<_>>(), ["node.enroll"]);
    let about_credential = LogRef::new(RefKind::Credential, credential.credential.id);
    let entries =
        lab.controller.oplog_filtered(&LogFilter { limit: 10, reference: Some(about_credential), ..LogFilter::default() }).unwrap();
    assert_eq!(entries[0].action, "credential.create");
    let by_network = lab.controller.oplog_filtered(&LogFilter { limit: 100, network: Some(network.id), ..LogFilter::default() }).unwrap();
    assert!(by_network.iter().all(|entry| entry.network == Some(network.id)));
}

#[test]
fn the_overview_adds_up_flows_and_traffic_of_online_nodes() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let a = lab.join(network.id, &[]);
    let b = lab.join(network.id, &[]);
    a.send(&lab.controller, ToController::Report(Box::new(Report { flows: 2, tx_bytes: 100, rx_bytes: 1000, ..Report::default() })));
    b.send(&lab.controller, ToController::Report(Box::new(Report { flows: 3, tx_bytes: 50, rx_bytes: 5, ..Report::default() })));
    let overview = lab.controller.overview(network.id).unwrap();
    assert_eq!((overview.flows, overview.tx_bytes, overview.rx_bytes), (5, 150, 1005));
    lab.controller.disconnect(b.session);
    assert_eq!(lab.controller.overview(network.id).unwrap().flows, 2, "只算在线的");
}

#[test]
fn the_node_list_carries_the_relays_each_node_is_attached_to() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let relay = lab.join_with(network.id, &[], CAN_DO_ALL);
    let member = lab.join(network.id, &[]);
    let attached = vec![netmap::Attachment { relay: relay.id, rtt_ms: 12, shared: false }];
    member.send(&lab.controller, ToController::Report(Box::new(Report { attached: attached.clone(), ..Report::default() })));
    let nodes = lab.controller.nodes(network.id).unwrap();
    assert_eq!(nodes.iter().find(|node| node.id == member.id).unwrap().attached, attached);
    assert!(nodes.iter().filter(|node| node.attached.iter().any(|attachment| attachment.relay == relay.id)).count() == 1);
}
