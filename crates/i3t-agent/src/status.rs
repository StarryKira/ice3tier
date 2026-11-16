//! 运行着的节点的状态，给本机的 `i3t status` 和 `i3t routes` 看。
//!
//! 这些类型就是本机状态接口应答的 JSON。问的一方和答的一方通常是同一个程序文件，但更新的那一小段时间里
//! 可能不是同一个版本，所以字段都有默认值，不认识的字段被忽略。

use std::net::{Ipv4Addr, SocketAddr};

use i3t_access::ListenState;
use i3t_model::control::{self, CredentialUsage, ExitReport, Route, RouteSince, SelfUpdate};
use i3t_model::netmap::Attachment;
use i3t_model::update::UpdateStatus;
use serde::{Deserialize, Serialize};

use crate::control::LinkState;
use crate::runtime::{Runtime, exit_report};

/// 一个运行着的节点现在的样子。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveStatus {
    /// 正在运行的程序的版本。
    pub version: String,
    pub uptime_secs: u64,
    pub name: String,
    pub virtual_ip: Option<Ipv4Addr>,
    pub network: String,
    pub netmap_version: u64,
    pub link: Link,
    /// controller 的地址；静态模式下没有。
    pub controller: Option<String>,
    /// 以本节点为起点或终点的活跃的流。更新等的就是它变成零。
    pub flows: usize,
    /// 本节点的 UDP 候选端点。
    pub endpoints: Vec<SocketAddr>,
    pub mapping_varies: Option<bool>,
    /// 挂靠的 relay 和到它们的延迟。
    pub attached: Vec<Attachment>,
    pub routes: Vec<LiveRoute>,
    pub listeners: Vec<LiveListener>,
    pub update: LiveUpdate,
    /// 本节点启用为 relay 时有值。
    pub relay: Option<LiveRelay>,
    /// 本节点替别人连目标（出口一侧）的计数，配额用到了哪里。
    pub exit: Option<ExitReport>,
    /// 入口上各个接入凭据这次运行以来的用量。
    pub credentials: Vec<CredentialUsage>,
}

/// 和 controller 的那条长连接。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    /// 静态模式：不连 controller。
    #[default]
    Static,
    Offline,
    Online,
    Revoked,
    Rejected,
    #[serde(other)]
    Unknown,
}

impl From<LinkState> for Link {
    fn from(state: LinkState) -> Self {
        match state {
            LinkState::Offline => Self::Offline,
            LinkState::Online => Self::Online,
            LinkState::Revoked => Self::Revoked,
            LinkState::Rejected => Self::Rejected,
        }
    }
}

/// 到一个对端现在怎么走。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRoute {
    pub peer: u32,
    pub name: String,
    pub virtual_ip: Option<Ipv4Addr>,
    pub route: Route,
    #[serde(default)]
    pub rtt_ms: Option<u32>,
    /// 这条路是多久之前换上的、为什么。这次启动以来没有换过时没有。
    #[serde(default)]
    pub since: Option<RouteSince>,
}

/// 一个代理监听器，或者一条端口转发的监听一侧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveListener {
    /// 监听器的名字；端口转发是 `forward-<编号>`。
    pub name: String,
    /// 配置里写的绑定地址。
    pub bind: SocketAddr,
    /// `listening`、`port_in_use`、`failed`、`virtual`、`unsupported`。
    pub state: String,
    /// `port_in_use`、`failed`、`unsupported` 时是原因；端口写 0 时是实际的端口。
    #[serde(default)]
    pub detail: Option<String>,
    /// 上面正在进行的连接数。
    #[serde(default)]
    pub active: usize,
    /// 这次运行以来客户端发进来的、发回给客户端的字节数。
    #[serde(default)]
    pub bytes_up: u64,
    #[serde(default)]
    pub bytes_down: u64,
}

/// 程序的更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveUpdate {
    pub status: UpdateStatus,
    /// 能不能自己更新，不能的话为什么。
    pub self_update: Option<SelfUpdate>,
    /// 正在排空：不接新的流，手上的结束之后重启。
    pub draining: bool,
}

/// relay 的计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveRelay {
    pub sessions: u64,
    pub forwarded_packets: u64,
    pub forwarded_bytes: u64,
    pub dropped: u64,
}

impl LiveStatus {
    pub(crate) fn of(runtime: &Runtime) -> Self {
        let netmap = runtime.netmap.borrow().clone();
        let update = runtime.update.borrow().clone();
        let routes = runtime.endpoint.routes().into_iter().map(|entry| {
            let peer = netmap.peer(entry.peer);
            LiveRoute {
                peer: entry.peer.get(),
                name: peer.map_or_else(|| format!("#{}", entry.peer), |peer| peer.name.clone()),
                virtual_ip: peer.map(|peer| peer.virtual_ip),
                route: entry.route,
                rtt_ms: entry.rtt_ms,
                since: runtime.history.since(entry.peer),
            }
        });
        let access = runtime.access.status();
        let listeners = access.listeners.iter().map(|listener| LiveListener {
            bytes_up: listener.traffic.up(),
            bytes_down: listener.traffic.down(),
            ..listener_of(listener.name.clone(), listener.bind, &listener.state, listener.active.get())
        });
        let forwards = access.forwards.iter().map(|forward| LiveListener {
            bytes_up: forward.traffic.up(),
            bytes_down: forward.traffic.down(),
            ..listener_of(format!("forward-{}", forward.id), forward.bind, &forward.state, forward.active.get())
        });
        let exit = runtime.exit.stats();
        let counters = runtime.endpoint.relay_counters();
        Self {
            version: runtime.program.version.to_string(),
            uptime_secs: runtime.started.elapsed().as_secs(),
            name: netmap.me.name.clone(),
            virtual_ip: Some(netmap.me.virtual_ip),
            network: netmap.network.name.clone(),
            netmap_version: netmap.version,
            link: runtime.link.as_ref().map_or(Link::Static, |link| Link::from(*link.borrow())),
            controller: runtime.controller.clone(),
            flows: *runtime.flows.borrow(),
            endpoints: runtime.endpoint.endpoints(),
            mapping_varies: runtime.endpoint.mapping_varies(),
            attached: runtime.endpoint.attached(),
            routes: routes.collect(),
            listeners: listeners.chain(forwards).collect(),
            update: LiveUpdate { draining: exit.draining, status: update, self_update: Some(runtime.self_update) },
            relay: netmap.me.relay.is_some().then(|| LiveRelay {
                sessions: counters.sessions,
                forwarded_packets: counters.forwarded_packets,
                forwarded_bytes: counters.forwarded_bytes,
                dropped: counters.dropped_total(),
            }),
            exit: netmap.me.exit.is_some().then(|| exit_report(&exit)),
            credentials: runtime.credential_usage(),
        }
    }
}

fn listener_of(name: String, bind: SocketAddr, state: &ListenState, active: usize) -> LiveListener {
    let (state, detail) = match state {
        ListenState::Listening { local } => ("listening", (*local != bind).then(|| format!("实际绑定在 {local}"))),
        ListenState::PortInUse { reason } => ("port_in_use", Some(reason.clone())),
        ListenState::Failed { reason } => ("failed", Some(reason.clone())),
        ListenState::Virtual => ("virtual", None),
        ListenState::Unsupported { reason } => ("unsupported", Some(reason.clone())),
        // 入口模块以后可能有新的状态；这里认不出来的照实说认不出来。
        _ => ("unknown", None),
    };
    LiveListener { name, bind, state: state.to_owned(), detail, active, bytes_up: 0, bytes_down: 0 }
}

/// 入口模块报的监听状态写成上报给 controller 的样子。
pub(crate) fn listen_state(state: &ListenState) -> control::ListenState {
    match state {
        ListenState::Listening { local } => control::ListenState::Listening { local: *local },
        ListenState::PortInUse { reason } => control::ListenState::PortInUse { detail: reason.clone() },
        ListenState::Failed { reason } => control::ListenState::Failed { detail: reason.clone() },
        ListenState::Virtual => control::ListenState::Virtual,
        ListenState::Unsupported { reason } => control::ListenState::Unsupported { detail: reason.clone() },
        // 入口模块以后可能有新的状态；这里认不出来的照实说认不出来。
        _ => control::ListenState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_from_another_version_still_reads() {
        let status: LiveStatus = serde_json::from_value(serde_json::json!({
            "version": "9.0.0", "link": "something_new", "added_later": 1,
            "update": { "status": { "state": "also_new" }, "added_later": true },
            "routes": [{ "peer": 2, "name": "nas", "virtual_ip": "10.66.0.2", "route": { "via": "none" } }]
        }))
        .unwrap();
        assert_eq!((status.link, status.update.status.clone(), status.routes.len()), (Link::Unknown, UpdateStatus::Unknown, 1));
        assert_eq!(serde_json::from_str::<LiveStatus>("{}").unwrap(), LiveStatus::default());
        assert_eq!(serde_json::to_value(Link::Online).unwrap(), "online");
    }

    #[test]
    fn listener_states_are_spelt_out() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        let listening = listener_of("socks".into(), bind, &ListenState::Listening { local }, 2);
        assert_eq!((listening.state.as_str(), listening.active), ("listening", 2));
        assert!(listening.detail.unwrap().contains("41000"), "端口写 0 时说出实际的端口");
        assert_eq!(listener_of("socks".into(), local, &ListenState::Listening { local }, 0).detail, None);
        let in_use = listener_of("http".into(), bind, &ListenState::PortInUse { reason: "地址已经在用".into() }, 0);
        assert_eq!((in_use.state.as_str(), in_use.detail.as_deref()), ("port_in_use", Some("地址已经在用")));
        let failed = listener_of("http".into(), bind, &ListenState::Failed { reason: "没有这个地址".into() }, 0);
        assert_eq!((failed.state.as_str(), failed.detail.as_deref()), ("failed", Some("没有这个地址")));
        assert_eq!(listener_of("forward-3".into(), bind, &ListenState::Virtual, 0).state, "virtual");
    }

    #[test]
    fn listen_states_are_reported_to_the_controller_with_their_reason() {
        let local: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        assert_eq!(listen_state(&ListenState::Listening { local }), control::ListenState::Listening { local });
        assert_eq!(
            listen_state(&ListenState::PortInUse { reason: "in use".into() }),
            control::ListenState::PortInUse { detail: "in use".into() }
        );
        assert_eq!(listen_state(&ListenState::Virtual), control::ListenState::Virtual);
        assert_eq!(
            listen_state(&ListenState::Unsupported { reason: "UDP".into() }),
            control::ListenState::Unsupported { detail: "UDP".into() }
        );
    }
}
