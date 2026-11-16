//! 把各层装成一个运行着的节点：`i3t run` 做的事。
//!
//! ```text
//! controller 的长连接 ──▶ netmap ──▶ 按本机配置调整 ──▶ 数据面、入口、出口
//!        ▲    │                                              │
//!        │    └──▶ 更新的指示 ──▶ 更新库 ──▶ 换程序 ──▶ 以"请重启我"退出
//!        └──────────── 上报、信令、更新的进度 ◀──────────────┘
//! ```
//!
//! 更新（白皮书 §10.5）在这里的接法：
//!
//! 1. 启动时先问更新库上一次更新怎么样了：新程序没被确认过太多次或者太久，这时已经换回旧程序，本进程直接要求重启。
//! 2. 每次连上 controller 就向更新库确认一次：新程序连上了 controller 才算更新成功。
//! 3. controller 的指示交给 [`update`](crate::update)，包经长连接取，状态的每次变化马上上报。
//! 4. 更新进行到"排空中"时不再接新的流（见 [`drain`](crate::drain)）：入口和出口都明确拒绝新的请求，监听照常开着。
//! 5. 程序文件换好之后把最后的状态报上去，然后返回 [`Outcome::Restart`]。

use std::ffi::OsString;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use i3t_access::Access;
use i3t_conn::{Endpoint, EndpointConfig};
use i3t_exit::{Exit, Os, Outbound, QuotaState};
use i3t_model::Netmap;
use i3t_model::control::{Hello, SelfUpdate};
use i3t_model::netmap::{Listener, ListenerProtocol};
use i3t_model::update::UpdateStatus;
use i3t_update::{RolledBack, Startup, Updater, UpdaterConfig};
use tokio::sync::{Notify, watch};

use crate::config::Config;
use crate::control::{ControlLink, LinkSetup, LinkState, Timing};
use crate::drain::Drain;
use crate::flows;
use crate::join::hello;
use crate::local::LocalServers;
use crate::persist::{self, StatePorts};
use crate::program::Program;
use crate::runtime::{Parts, Runtime};
use crate::state::StateDir;
use crate::update::Driver;

/// 进程退出之前，等最后一次上报写出去最多等多久。controller 不在线的话没有什么可等的。
const FAREWELL_TIMEOUT: Duration = Duration::from_secs(2);

/// netmap 从哪里来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// 正常的用法：先 `i3t join` 过，之后由 controller 下发。
    Controller,
    /// 静态模式，给测试用：netmap 来自一个 JSON 文件，不连 controller，也不更新。
    Static(PathBuf),
}

/// 节点是怎么停下来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 被要求停止。
    Stopped,
    /// 程序文件换过了（更新，或者更新没成换回了旧的），要重新启动进程：
    /// 以 [`RESTART_EXIT_CODE`](i3t_update::RESTART_EXIT_CODE) 退出，由外面的看护程序拉起来。
    Restart,
}

/// 运行节点，直到 `shutdown` 完成、controller 吊销了这个节点，或者程序文件换过了需要重启。
///
/// # Errors
///
/// 配置文件有错、还没有加入网络、端口绑不上，或者运行中被 controller 吊销、拒绝。
pub async fn run(state: StateDir, source: Source, program: Program, shutdown: impl Future<Output = ()>) -> anyhow::Result<Outcome> {
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
    let controller = match source {
        Source::Controller => Some(state.controller()?.context(NOT_JOINED)?),
        Source::Static(_) => None,
    };

    // 更新的善后要在连 controller、绑端口之前做：上一次换上来的程序要是一直没被确认，这里就换回去并要求重启。
    let updater = controller.is_some().then(|| updater_for(&state, &program));
    let mut awaiting_confirmation = false;
    let updater = match updater.map(|updater| (updater.startup(), updater)) {
        Some((Ok(Startup::Clean), updater)) => Some(updater),
        Some((Ok(Startup::AwaitingConfirmation { version }), updater)) => {
            tracing::info!(%version, "这是刚换上来的新版本，连上 controller 才算更新成功");
            awaiting_confirmation = true;
            Some(updater)
        }
        Some((Ok(Startup::RolledBack(rolled)), _)) => {
            log_rollback(&rolled);
            return Ok(Outcome::Restart);
        }
        Some((Err(error), _)) => {
            // 更新的记录读不了。节点的本职是组网，不能因此起不来：这次运行不更新，并且如实告诉 controller。
            tracing::error!("更新的状态读不了，这次运行不接受更新：{:#}", anyhow::Error::from(error));
            None
        }
        None => None,
    };
    // 报给 controller、也显示在本机状态里的"能不能自己更新"。由 controller 管理、这次运行却不接受更新时，
    // 说"不能"而不给原因：原因不在程序文件上，是更新的记录读不了。
    let self_update = match (&updater, &controller) {
        (None, Some(_)) => SelfUpdate { able: false, blocker: None },
        (Some(_), _) | (None, None) => program.self_update(),
    };

    let key = Arc::new(key);
    let initial = Arc::new(initial);
    tracing::info!(
        node = %initial.me.name,
        virtual_ip = %initial.me.virtual_ip,
        network = %initial.network.name,
        key = %key.public(),
        version = %program.version,
        "节点启动"
    );
    let addr_servers = match &controller {
        // 解析不了时只是少一个问自己地址的地方，不耽误启动。
        Some(controller) => tokio::net::lookup_host(&controller.address).await.map(Iterator::collect).unwrap_or_default(),
        None => Vec::new(),
    };

    // 报给 controller 的更新状态。排空与否也看它。
    let (update_tx, update_rx) = watch::channel(UpdateStatus::Idle);
    let mut drain = Drain::new(update_rx.clone());

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
    // 出口不替别人连本节点自己对外的地址（反射地址、路由器映射出来的地址），这些地址数据面最清楚。
    let outbound = Outbound::new(local_rx.clone(), Os).with_own_endpoints({
        let endpoint = Arc::downgrade(&endpoint);
        move || endpoint.upgrade().map(|endpoint| endpoint.endpoints()).unwrap_or_default()
    });
    let quota = state.exit_quota().unwrap_or_else(|error| {
        // 读不出来只是这个账期的用量从零记起，不值得让节点起不来。
        tracing::warn!("上次存下的出口配额用量读不出来，从零开始记：{error:#}");
        None
    });
    let quota =
        quota.map_or_else(QuotaState::default, |record| QuotaState { period_start: record.period_start, used_bytes: record.used_bytes });
    let ports = Arc::new(StatePorts::open(state.clone()));
    let access = Arc::new(Access::new(Arc::clone(&endpoint), outbound.clone(), local_rx.clone()).with_port_book(ports));
    // 出口接力 UDP 端口转发时和入口共用一个开数据报上下文的：对端发回来的数据报只由入口的那一个分发。
    let exit = Arc::new(
        Exit::new(local_rx.clone(), outbound, Arc::clone(&endpoint)).with_quota_state(quota).with_opener(access.datagram_opener()),
    );

    let serve_exit = tokio::spawn({
        let (endpoint, exit) = (Arc::clone(&endpoint), Arc::clone(&exit));
        async move { i3t_flow::serve(&*endpoint, exit).await }
    });
    let save_quota = tokio::spawn(persist::save_quota(state.clone(), Arc::clone(&exit)));
    let state_for_quota = state.clone();
    let serve_access = tokio::spawn({
        let access = Arc::clone(&access);
        async move { access.run().await }
    });

    let (_hello_tx, hello_rx) = watch::channel(Hello { self_update: Some(self_update), ..hello(&config, &program) });
    let report_now = Arc::new(Notify::new());
    let (flows_tx, flows_rx) = watch::channel(0);

    // 长连接要调用运行时（取上报的内容、回答路由查询），运行时又要看长连接的状态。先建一个空的状态，连上之后再接过去。
    let (link_state_tx, link_state_rx) = watch::channel(LinkState::Offline);
    let runtime = Arc::new(Runtime::new(Parts {
        program,
        self_update,
        controller: controller.as_ref().map(|controller| controller.address.clone()),
        endpoint: Arc::clone(&endpoint),
        exit: Arc::clone(&exit),
        access: Arc::clone(&access),
        netmap: local_rx,
        link: controller.is_some().then_some(link_state_rx),
        update: update_rx,
        flows: flows_rx.clone(),
    }));
    let count_flows = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        flows::publish(move || runtime.sample_flows(), flows_tx)
    });
    let follow_routes = tokio::spawn(runtime.history.clone().follow(endpoint.route_changes()));
    let local_servers = LocalServers::start(&runtime, &state, config.metrics.listen.addr()).await;

    let outcome = match controller {
        Some(controller) => {
            let mut link = ControlLink::spawn(LinkSetup {
                controller,
                key,
                initial: Arc::clone(&initial),
                hello: hello_rx,
                report: Box::new({
                    let runtime = Arc::clone(&runtime);
                    move || runtime.report()
                }),
                route: Box::new({
                    let runtime = Arc::clone(&runtime);
                    move |peer| runtime.route_to(peer)
                }),
                backbone: Box::new({
                    let runtime = Arc::clone(&runtime);
                    move || runtime.backbone()
                }),
                report_now: Arc::clone(&report_now),
                state,
                timing: Timing::default(),
            });
            let watch_reachability = tokio::spawn(report_when_reachability_changes(Arc::clone(&endpoint), Arc::clone(&report_now)));
            let updates = updater.as_ref().and_then(|updater| {
                let commands = link.take_update_commands()?;
                let driver = Driver {
                    updater: Arc::clone(updater),
                    commands,
                    packages: link.packages(),
                    flows: flows_rx,
                    reported: update_tx,
                    report_now: Arc::clone(&report_now),
                };
                Some(driver.run())
            });
            let managed = Managed {
                link: &mut link,
                link_state: &link_state_tx,
                endpoint: &endpoint,
                exit: &exit,
                access: &access,
                local: &local_tx,
                config: &config,
                drain: &mut drain,
                updater: updater.as_deref(),
                awaiting_confirmation,
            };
            let outcome = managed.run(updates, shutdown).await;
            if matches!(outcome, Ok(Outcome::Restart)) {
                // 进程马上要退出：把"正在重启"（或者"换回了旧程序"）送到 controller，之后就没有机会了。
                let sent = link.report_and_wait(FAREWELL_TIMEOUT).await;
                tracing::info!(sent, "程序文件已经换过，节点退出，等看护程序把它重新拉起来");
            }
            watch_reachability.abort();
            outcome
        }
        None => {
            shutdown.await;
            Ok(Outcome::Stopped)
        }
    };

    tracing::info!("节点停止");
    drop(local_servers);
    save_quota.abort();
    persist::save_quota_now(&state_for_quota, &exit);
    count_flows.abort();
    follow_routes.abort();
    serve_access.abort();
    serve_exit.abort();
    endpoint.close().await;
    outcome
}

const NOT_JOINED: &str = "这台机器还没有加入网络。在 WebUI 的“添加节点”里生成注册令牌，然后执行：echo <令牌> | i3t join";

fn updater_for(state: &StateDir, program: &Program) -> Arc<Updater> {
    Arc::new(Updater::new(UpdaterConfig {
        program: program.path.clone(),
        state_dir: state.path().to_owned(),
        running: program.version.clone(),
        // 新程序的自检：`<新程序> --state-dir <状态目录> self-check`。
        self_check_args: vec![OsString::from("--state-dir"), state.path().into(), OsString::from("self-check")],
        blocker: program.blocker,
    }))
}

fn log_rollback(rolled: &RolledBack) {
    tracing::warn!(
        failed = %rolled.failed,
        back_to = %rolled.restart.version,
        reason = %rolled.reason,
        "新版本一直没有连上 controller，已经换回旧程序，需要重启"
    );
}

/// 由 controller 管理时的主循环要用的东西。
struct Managed<'a> {
    link: &'a mut ControlLink,
    /// 长连接的状态转到这里，给本机状态接口和指标看。
    link_state: &'a watch::Sender<LinkState>,
    endpoint: &'a Arc<Endpoint>,
    exit: &'a Exit,
    access: &'a Access<Endpoint, Outbound<Os>>,
    local: &'a watch::Sender<Arc<Netmap>>,
    config: &'a Config,
    drain: &'a mut Drain,
    updater: Option<&'a Updater>,
    /// 正在运行的是刚换上来、还没有被确认的新版本。
    awaiting_confirmation: bool,
}

impl Managed<'_> {
    /// 把新的 netmap 交给各层，转发信令，跟着更新走，直到要求停止、被吊销，或者需要重启。
    ///
    /// `updates` 是管更新的那个任务：它返回就是程序文件换好了。这次运行不接受更新时没有它。
    async fn run(
        self,
        updates: Option<impl Future<Output = i3t_update::RestartRequired>>,
        shutdown: impl Future<Output = ()>,
    ) -> anyhow::Result<Outcome> {
        let Self { link, link_state, endpoint, exit, access, local, config, drain, updater, mut awaiting_confirmation } = self;
        let signals = link.signal_sender();
        endpoint.set_signal_hook(move |to, signal| {
            // 信令是尽力而为的：队列满了就丢，路径层自己会重试。
            if signals.try_send((to, signal.clone())).is_err() {
                tracing::debug!(%to, "经 controller 的信令队列满了，丢掉一条");
            }
        });
        let mut netmaps = link.netmap();
        let mut states = link.state();
        let relocalize = |netmaps: &watch::Receiver<Arc<Netmap>>| {
            local.send_replace(Arc::new(localize(&netmaps.borrow(), config)));
        };
        let has_updates = updates.is_some();
        let updates = async {
            match updates {
                Some(updates) => updates.await,
                None => std::future::pending().await,
            }
        };
        // 新程序换上来之后限时要连上 controller，到时候还没连上就换回旧的。
        let deadline = async {
            match updater {
                Some(updater) => updater.enforce_confirmation_deadline().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(shutdown, updates, deadline);
        loop {
            tokio::select! {
                () = &mut shutdown => return Ok(Outcome::Stopped),
                Ok(()) = netmaps.changed() => {
                    netmaps.borrow_and_update();
                    relocalize(&netmaps);
                }
                draining = drain.changed() => {
                    if draining {
                        // 两种情况会走到这里：管理员选了"排空后更新"，或者任何一种更新走到了换程序的那一步。
                        tracing::info!("不再接新的流，直到更新之后重启：本机入口和别的节点开过来的新请求都明确拒绝，监听照常开着；手上的流不受影响");
                    } else {
                        tracing::info!("更新没有继续下去：照常接受新的流");
                    }
                    let detail = drain.detail();
                    exit.set_draining(detail.clone());
                    access.set_draining(detail);
                }
                Some((from, signal)) = link.next_signal() => endpoint.deliver_signal(from, signal),
                Ok(()) = states.changed() => {
                    let state = *states.borrow_and_update();
                    link_state.send_replace(state);
                    match state {
                        LinkState::Revoked => bail!("controller 吊销了这个节点"),
                        LinkState::Rejected => bail!("controller 不认这个节点了（它可能已经被删除）。要重新加入网络：echo <令牌> | i3t join"),
                        LinkState::Online => {
                            tracing::info!("已连上 controller");
                            if let Some(updater) = updater {
                                match updater.confirm() {
                                    Ok(Some(version)) => tracing::info!(%version, "更新成功：新版本连上了 controller"),
                                    Ok(None) => {}
                                    Err(error) => tracing::error!("没能记下更新已经成功：{:#}", anyhow::Error::from(error)),
                                }
                                awaiting_confirmation = false;
                            }
                        }
                        LinkState::Offline => tracing::warn!("和 controller 的连接断了，按手上的配置继续运行"),
                    }
                }
                restart = &mut updates, if has_updates => {
                    tracing::info!(version = %restart.version, "新程序已经换上去");
                    return Ok(Outcome::Restart);
                }
                rolled = &mut deadline, if awaiting_confirmation => {
                    awaiting_confirmation = false;
                    match rolled {
                        Ok(Some(rolled)) => {
                            log_rollback(&rolled);
                            return Ok(Outcome::Restart);
                        }
                        // 已经确认过了。
                        Ok(None) => {}
                        Err(error) => tracing::error!("新版本一直没有连上 controller，但换不回旧程序：{:#}", anyhow::Error::from(error)),
                    }
                }
            }
        }
    }
}

/// 别的节点怎么找到本节点（候选端点、挂靠的 relay）一有变化就马上上报，不等定时上报。
///
/// 延迟的小幅波动不算变化，否则每秒都在报。
async fn report_when_reachability_changes(endpoint: Arc<Endpoint>, report_now: Arc<Notify>) {
    let snapshot = |endpoint: &Endpoint| {
        let relays: Vec<_> = endpoint.attached().into_iter().map(|attachment| attachment.relay).collect();
        (endpoint.endpoints(), relays)
    };
    let mut last = snapshot(&endpoint);
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tick.tick().await;
        let now = snapshot(&endpoint);
        if now != last {
            last = now;
            report_now.notify_one();
        }
    }
}

/// 节点按本机配置文件自己开的回环代理入口的名字。上报时据此标出哪些监听器不是 controller 下发的。
pub(crate) const BUILTIN_LISTENERS: [&str; 2] = ["local-socks5", "local-http"];

/// 把 controller 下发的 netmap 按本机的配置调整成各层实际使用的样子。
///
/// 做两件事：
///
/// - 加上配置文件里的本机代理入口；
/// - 去掉本机没有声明过的能力。controller 被攻破或者管理员密码被盗时，它也不能把一台没有声明的机器
///   变成出口、relay 或者端口转发的目标。
///
/// 排空不在这里管：监听照常开着，由入口和出口模块明确拒绝新的请求（见 [`drain`](crate::drain)）。
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
        (BUILTIN_LISTENERS[0], config.proxy.socks5.addr(), ListenerProtocol::Socks5),
        (BUILTIN_LISTENERS[1], config.proxy.http.addr(), ListenerProtocol::Http),
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
            class: None,
            optimistic: None,
        });
    }
    netmap
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
            class: None,
            optimistic: None,
        });
        assert_eq!(names(&localize(&managed, &Config::default())), ["from-controller", "local-http"]);
    }
}
