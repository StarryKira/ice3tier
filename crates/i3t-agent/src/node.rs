//! 把各层装成一个运行着的节点：`i3t run` 做的事。
//!
//! ```text
//! controller 的长连接 ──▶ netmap ──▶ 按本机配置调整 ──▶ 数据面、入口、出口
//!        ▲                                                   │
//!        └──────────── 上报、信令 ◀───────────────────────────┘
//! ```

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use i3t_access::Access;
use i3t_conn::{Endpoint, EndpointConfig};
use i3t_exit::{Exit, Os, Outbound};
use i3t_model::Netmap;
use i3t_model::control::Report;
use i3t_model::netmap::{Listener, ListenerProtocol};
use tokio::sync::watch;

use crate::config::Config;
use crate::control::{ControlLink, LinkSetup, LinkState, Timing};
use crate::join::hello;
use crate::state::StateDir;

/// netmap 从哪里来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// 正常的用法：先 `i3t join` 过，之后由 controller 下发。
    Controller,
    /// 静态模式，给测试用：netmap 来自一个 JSON 文件，不连 controller。
    Static(PathBuf),
}

/// 运行节点，直到 `shutdown` 完成，或者 controller 吊销了这个节点。
///
/// # Errors
///
/// 配置文件有错、还没有加入网络、端口绑不上，或者运行中被 controller 吊销、拒绝。
pub async fn run(state: StateDir, source: Source, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    let config = Config::load(&state.config_path())?;
    let (key, initial) = match &source {
        Source::Controller => {
            let key = state.key()?.context(NOT_JOINED)?;
            (key, state.netmap()?.context(NOT_JOINED)?)
        }
        Source::Static(path) => {
            let text = std::fs::read(path).with_context(|| format!("读 netmap 文件 {}", path.display()))?;
            let netmap: Netmap = serde_json::from_slice(&text).with_context(|| format!("解析 netmap 文件 {}", path.display()))?;
            (state.key_or_generate()?, netmap)
        }
    };
    let key = Arc::new(key);
    let initial = Arc::new(initial);
    tracing::info!(node = %initial.me.name, virtual_ip = %initial.me.virtual_ip, network = %initial.network.name, key = %key.public(), "节点启动");

    let controller = match source {
        Source::Controller => Some(state.controller()?.context(NOT_JOINED)?),
        Source::Static(_) => None,
    };
    let addr_servers = match &controller {
        // 解析不了时只是少一个问自己地址的地方，不耽误启动。
        Some(controller) => tokio::net::lookup_host(&controller.address).await.map(Iterator::collect).unwrap_or_default(),
        None => Vec::new(),
    };

    // 各层看到的是按本机配置调整过的 netmap。
    let (local_tx, local_rx) = watch::channel(Arc::new(localize(&initial, &config)));
    let endpoint = Arc::new(
        Endpoint::bind(EndpointConfig {
            key: Arc::clone(&key),
            bind: SocketAddr::from(([0, 0, 0, 0], config.port)),
            netmap: local_rx.clone(),
            addr_servers,
        })
        .with_context(|| format!("绑定端口 {}", config.port))?,
    );
    let outbound = Outbound::new(local_rx.clone(), Os);
    let exit = Arc::new(Exit::new(local_rx.clone(), outbound.clone()));
    let access = Arc::new(Access::new(Arc::clone(&endpoint), outbound, local_rx));

    let serve_exit = tokio::spawn({
        let (endpoint, exit) = (Arc::clone(&endpoint), Arc::clone(&exit));
        async move { i3t_flow::serve(&*endpoint, exit).await }
    });
    let serve_access = tokio::spawn({
        let access = Arc::clone(&access);
        async move { access.run().await }
    });

    let outcome = match controller {
        Some(controller) => {
            let (_hello_tx, hello_rx) = watch::channel(hello(&config));
            let report = {
                let (endpoint, exit, access) = (Arc::clone(&endpoint), Arc::clone(&exit), Arc::clone(&access));
                Box::new(move || report(&endpoint, &exit, &access))
            };
            let link = ControlLink::spawn(LinkSetup {
                controller,
                key,
                initial: Arc::clone(&initial),
                hello: hello_rx,
                report,
                state,
                timing: Timing::default(),
            });
            managed(link, &endpoint, &local_tx, &config, shutdown).await
        }
        None => {
            shutdown.await;
            Ok(())
        }
    };

    tracing::info!("节点停止");
    serve_access.abort();
    serve_exit.abort();
    endpoint.close().await;
    outcome
}

const NOT_JOINED: &str = "这台机器还没有加入网络。在 WebUI 的“添加节点”里生成注册令牌，然后执行：echo <令牌> | i3t join";

/// 由 controller 管理时的主循环：把新的 netmap 交给各层，转发信令，直到要求停止或者被吊销。
async fn managed(
    mut link: ControlLink,
    endpoint: &Arc<Endpoint>,
    local: &watch::Sender<Arc<Netmap>>,
    config: &Config,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let signals = link.signal_sender();
    endpoint.set_signal_hook(move |to, signal| {
        // 信令是尽力而为的：队列满了就丢，路径层自己会重试。
        if signals.try_send((to, signal.clone())).is_err() {
            tracing::debug!(%to, "经 controller 的信令队列满了，丢掉一条");
        }
    });
    let mut netmaps = link.netmap();
    let mut states = link.state();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            Ok(()) = netmaps.changed() => {
                let netmap = Arc::clone(&netmaps.borrow_and_update());
                local.send_replace(Arc::new(localize(&netmap, config)));
            }
            Some((from, signal)) = link.next_signal() => endpoint.deliver_signal(from, signal),
            Ok(()) = states.changed() => match *states.borrow_and_update() {
                LinkState::Revoked => bail!("controller 吊销了这个节点"),
                LinkState::Rejected => bail!("controller 不认这个节点了（它可能已经被删除）。要重新加入网络：echo <令牌> | i3t join"),
                LinkState::Online => tracing::info!("已连上 controller"),
                LinkState::Offline => tracing::warn!("和 controller 的连接断了，按手上的配置继续运行"),
            },
        }
    }
}

/// 把 controller 下发的 netmap 按本机的配置调整成各层实际使用的样子。
///
/// 做两件事：
///
/// - 加上配置文件里的本机代理入口；
/// - 去掉本机没有声明过的能力。controller 被攻破或者管理员密码被盗时，它也不能把一台没有声明的机器
///   变成出口、relay 或者端口转发的目标。
#[must_use]
pub fn localize(netmap: &Netmap, config: &Config) -> Netmap {
    let mut netmap = netmap.clone();
    let declared = config.declare;
    if !declared.exit && netmap.me.exit.take().is_some() {
        tracing::warn!("controller 把本节点启用成了出口，但本机没有声明可以作为出口，不生效。要声明请执行 i3t set can-exit on");
    }
    if !declared.relay && netmap.me.relay.take().is_some() {
        netmap.relay_clients.clear();
        tracing::warn!("controller 把本节点启用成了 relay，但本机没有声明可以作为 relay，不生效。要声明请执行 i3t set can-relay on");
    }
    if !declared.forward && !netmap.me.forward_targets.is_empty() {
        netmap.me.forward_targets.clear();
        tracing::warn!("controller 把本节点配成了端口转发的目标，但本机没有声明，不生效。要声明请执行 i3t set can-forward on");
    }

    let local = [
        ("local-socks5", config.proxy.socks5.addr(), ListenerProtocol::Socks5),
        ("local-http", config.proxy.http.addr(), ListenerProtocol::Http),
    ];
    for (name, bind, protocol) in local {
        let Some(bind) = bind else { continue };
        // controller 也可以给同一个地址配监听器，那时以 controller 的为准。
        if netmap.me.listeners.iter().any(|listener| listener.bind == bind) {
            continue;
        }
        netmap.me.listeners.push(Listener {
            name: name.to_owned(),
            bind,
            protocol,
            allow_from: Vec::new(),
            require_credential: false,
            default_action: None,
            fixed_exit: None,
        });
    }
    netmap
}

fn report(endpoint: &Endpoint, exit: &Exit<Os>, access: &Access<Endpoint, Outbound<Os>>) -> Report {
    let stats = exit.stats();
    let status = access.status();
    let ingress: usize = status
        .listeners
        .iter()
        .map(|listener| listener.active.get())
        .chain(status.forwards.iter().map(|forward| forward.active.get()))
        .sum();
    let flows = stats.active.saturating_add(u64::try_from(ingress).unwrap_or(u64::MAX));
    Report {
        endpoints: endpoint.endpoints(),
        mapping_varies: endpoint.mapping_varies(),
        attached: endpoint.attached(),
        flows: u32::try_from(flows).unwrap_or(u32::MAX),
        tx_bytes: stats.bytes_to_target,
        rx_bytes: stats.bytes_from_target,
        routes: endpoint.routes(),
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::{ExitRole, ForwardProtocol, ForwardTarget, RelayClient, RelayRole};
    use i3t_proto::{NodeId, PublicKey};

    use super::*;
    use crate::control::testing::netmap;

    fn enabled_everything() -> Netmap {
        let mut netmap = netmap(1);
        let peer = NodeId::new(2).unwrap();
        netmap.me.exit = Some(ExitRole::default());
        netmap.me.relay = Some(RelayRole::default());
        netmap.relay_clients = vec![RelayClient { id: peer, key: PublicKey::from_bytes([2; 32]) }];
        netmap.me.forward_targets =
            vec![ForwardTarget { id: 1, protocol: ForwardProtocol::Tcp, addr: "127.0.0.1:5432".parse().unwrap(), listener: peer }];
        netmap
    }

    #[test]
    fn capabilities_that_were_not_declared_locally_are_stripped() {
        let local = localize(&enabled_everything(), &Config::default());
        assert!(local.me.exit.is_none() && local.me.relay.is_none());
        assert!(local.relay_clients.is_empty() && local.me.forward_targets.is_empty());
    }

    #[test]
    fn declared_capabilities_are_kept() {
        let config = Config::parse("[declare]\nexit = true\nrelay = true\nforward = true\n").unwrap();
        let local = localize(&enabled_everything(), &config);
        assert!(local.me.exit.is_some() && local.me.relay.is_some());
        assert_eq!((local.relay_clients.len(), local.me.forward_targets.len()), (1, 1));
    }

    #[test]
    fn local_proxy_listeners_are_added_unless_switched_off_or_taken() {
        let names = |netmap: &Netmap| netmap.me.listeners.iter().map(|listener| listener.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&localize(&netmap(1), &Config::default())), ["local-socks5", "local-http"]);

        let only_http = Config::parse("[proxy]\nsocks5 = \"off\"\n").unwrap();
        assert_eq!(names(&localize(&netmap(1), &only_http)), ["local-http"]);

        let mut managed = netmap(1);
        managed.me.listeners.push(Listener {
            name: "from-controller".into(),
            bind: "127.0.0.1:1080".parse().unwrap(),
            protocol: ListenerProtocol::Socks5,
            allow_from: Vec::new(),
            require_credential: false,
            default_action: None,
            fixed_exit: None,
        });
        assert_eq!(names(&localize(&managed, &Config::default())), ["from-controller", "local-http"]);
    }
}
