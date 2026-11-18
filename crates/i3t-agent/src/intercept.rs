//! 透明访问（白皮书 §9.5）在节点里的接法：按本机配置打开，把系统里的东西装上，跟着 netmap 改，节点停下时拆掉。
//!
//! 装什么、怎么认出自己的东西在 `i3t_intercept` 里；这里只管什么时候做：
//!
//! - 启动时：配置里打开了就装；没打开、但上次（崩溃、被杀掉）留下了东西，有权限的话把它拆掉。
//! - netmap 变了：能访问的对端变了只换内核里的那个集合；本节点的虚拟 IP、网络的地址段或者域名变了就整个重装。
//! - 停下时全部拆掉。进程被杀掉来不及拆的，下次启动时被认出来、换掉。
//! - 装着的时候每隔 5 秒再查一次网络的地址段和这台机器所在的真实网络重不重叠（后来才有的地址和路由：
//!   DHCP 换了地址、VPN 连上）。重叠了就当场全部拆掉，状态是 `failed`、`range_in_use`；因为重叠没装（或者拆掉）的，
//!   照样隔这么久查一次，重叠没了就再装上。查不清的那一次（`ip` 读不出来）什么也不改：装着的照旧装着，没装的照旧不装。
//!
//! 装不上不影响节点别的功能：上报里说清楚原因（[`InterceptState::Failed`]），代理和端口转发照常。
//! 系统命令是阻塞的，放在阻塞线程里执行：低占用档位的运行时只有一个工作线程，不能让它等。
//!
//! 透明访问只在本机打开：它改的是这台机器的网络配置，controller 没有开关。

use std::sync::Arc;

use i3t_exit::{Os, Outbound};
use i3t_model::Netmap;
use i3t_model::control::InterceptState;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::runtime::NodeAccess;

#[cfg(target_os = "linux")]
pub use platform::names_of;

/// 透明访问这一部分。丢弃它不会拆系统里的东西，要调用 [`Intercept::stop`]。
pub(crate) struct Intercept {
    state: watch::Receiver<InterceptState>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Intercept {
    /// 按 `enabled`（配置文件里的 `[intercept] enabled`）开始。装的过程在自己的任务里，状态从 [`state`](Self::state) 看。
    pub(crate) fn start(enabled: bool, netmap: watch::Receiver<Arc<Netmap>>, access: Arc<NodeAccess>, outbound: Outbound<Os>) -> Self {
        let (stop_tx, stop) = oneshot::channel();
        let (state, task) = platform::start(enabled, netmap, access, outbound, stop);
        Self { state, stop: Some(stop_tx), task }
    }

    /// 现在的状态。
    pub(crate) fn state(&self) -> watch::Receiver<InterceptState> {
        self.state.clone()
    }

    /// 拆掉装进系统里的东西，等拆完。
    pub(crate) async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            // 任务已经结束（没有打开）时没有人收，不要紧。
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take()
            && let Err(error) = task.await
        {
            tracing::error!(%error, "透明访问的任务没有正常结束，系统里可能留着它的规则，下次启动时会换掉");
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::sync::Arc;

    use i3t_exit::{Os, Outbound};
    use i3t_model::Netmap;
    use i3t_model::control::InterceptState;
    use tokio::sync::{oneshot, watch};
    use tokio::task::JoinHandle;

    use crate::runtime::NodeAccess;

    /// 这个平台上没有透明访问：状态定下来就不会再变，发送的一端不用留着。
    pub(super) fn start(
        enabled: bool,
        _netmap: watch::Receiver<Arc<Netmap>>,
        _access: Arc<NodeAccess>,
        _outbound: Outbound<Os>,
        _stop: oneshot::Receiver<()>,
    ) -> (watch::Receiver<InterceptState>, Option<JoinHandle<()>>) {
        let state = if enabled {
            tracing::warn!("配置文件里打开了透明访问，这个平台上没有（只有 Linux 有），不生效");
            InterceptState::Unsupported
        } else {
            InterceptState::Off
        };
        (watch::channel(state).1, None)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::BTreeSet;
    use std::io;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;

    use i3t_access::{Diversion, Diverted};
    use i3t_exit::{Os, Outbound};
    use i3t_intercept::machine::Shell;
    use i3t_intercept::names::Names;
    use i3t_intercept::setup::{self, Installed};
    use i3t_intercept::socket;
    use i3t_intercept::{Capability, CommandError, Plan, Program, ResolverOutcome, SetupError, Survey};
    use i3t_model::Netmap;
    use i3t_model::control::{InterceptCause, InterceptState, NameService};
    use tokio::net::UdpSocket;
    use tokio::sync::{oneshot, watch};
    use tokio::task::JoinHandle;

    use crate::runtime::NodeAccess;

    /// 透明访问装着（或者因为地址段重叠没装）的时候，隔多久再查一次重叠。每次是两个 `ip` 命令，几毫秒的 CPU；
    /// 机器后来进了一个重叠的网络时，最多这么久它就不再被切断。
    const RANGE_RECHECK: Duration = Duration::from_secs(5);

    pub(super) fn start(
        enabled: bool,
        netmap: watch::Receiver<Arc<Netmap>>,
        access: Arc<NodeAccess>,
        outbound: Outbound<Os>,
        stop: oneshot::Receiver<()>,
    ) -> (watch::Receiver<InterceptState>, Option<JoinHandle<()>>) {
        let (state, receiver) = watch::channel(InterceptState::Off);
        if !enabled {
            return (receiver, Some(tokio::spawn(clean_leftover())));
        }
        (receiver, Some(tokio::spawn(supervise(Parts { netmap, access, outbound }, state, stop))))
    }

    /// 透明访问用到的节点的部分。
    struct Parts {
        netmap: watch::Receiver<Arc<Netmap>>,
        access: Arc<NodeAccess>,
        outbound: Outbound<Os>,
    }

    /// 在阻塞线程里执行系统命令。
    async fn blocking<T: Send + 'static>(work: impl FnOnce(&mut Shell) -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(move || work(&mut Shell)).await.expect("执行系统命令的任务不会被取消，panic 了就让它传上来")
    }

    /// 没有打开透明访问时，拆掉上次留下的东西。没有权限或者没有那两个程序时什么都不做：那样的节点也装不了它。
    async fn clean_leftover() {
        if Capability::missing().contains(&Capability::NetAdmin) || [Program::Nft, Program::Ip].iter().any(|p| Shell::locate(*p).is_none())
        {
            return;
        }
        let survey = blocking(Survey::take).await;
        if !survey.found_any() {
            return;
        }
        match blocking(setup::remove).await {
            Ok(()) => tracing::info!("透明访问没有打开，拆掉了上次留下的规则和网卡"),
            Err(error) => tracing::warn!(%error, "透明访问没有打开，上次留下的规则拆不掉"),
        }
    }

    /// 装上、跟着 netmap 改、隔一阵再查地址段的重叠、收到停止就拆掉。
    async fn supervise(parts: Parts, state: watch::Sender<InterceptState>, mut stop: oneshot::Receiver<()>) {
        let mut netmap = parts.netmap.clone();
        let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + RANGE_RECHECK, RANGE_RECHECK);
        recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let current = Arc::clone(&netmap.borrow_and_update());
            let mut session = match Session::open(&parts, &current).await {
                Ok((session, on)) => {
                    tracing::info!(?on, "透明访问已打开");
                    state.send_replace(on);
                    Some(session)
                }
                Err(cause) => {
                    tracing::warn!(?cause, "透明访问打不开，系统里什么都没有留下；代理和端口转发照常可用");
                    state.send_replace(InterceptState::Failed { cause });
                    None
                }
            };
            let reinstall = loop {
                // 装着的要查会不会被切断；因为重叠没装的要查重叠还在不在。别的原因（权限、程序、别人占着的网卡）不会自己没了。
                let watching =
                    session.is_some() || matches!(*state.borrow(), InterceptState::Failed { cause: InterceptCause::RangeInUse { .. } });
                tokio::select! {
                    _ = &mut stop => break false,
                    changed = netmap.changed() => {
                        if changed.is_err() {
                            break false;
                        }
                        let next = Arc::clone(&netmap.borrow_and_update());
                        if let Some(session) = session.as_mut()
                            && !session.follow(&next).await
                        {
                            break true;
                        }
                    }
                    _ = recheck.tick(), if watching => {
                        let cidr = netmap.borrow().network.cidr;
                        match blocking(move |shell| setup::overlapping(shell, cidr)).await {
                            Ok(Some(with)) => {
                                if let Some(installed) = session.take() {
                                    tracing::warn!(
                                        %with,
                                        "网络的地址段和本机后来进的一个网络重叠了，透明访问当场拆掉，免得把这台机器从那个网络上切下来；重叠没了再装上"
                                    );
                                    installed.close().await;
                                }
                                let failed = InterceptState::Failed { cause: InterceptCause::RangeInUse { with } };
                                state.send_if_modified(|current| {
                                    if *current == failed {
                                        return false;
                                    }
                                    *current = failed;
                                    true
                                });
                            }
                            Ok(None) if session.is_none() => {
                                tracing::info!("和网络地址段重叠的网络没了，透明访问再装上");
                                break true;
                            }
                            Ok(None) => {}
                            Err(error) => tracing::debug!(%error, "查不清地址段和本机的网络重不重叠，这一次什么也不改"),
                        }
                    }
                }
            };
            if let Some(session) = session.take() {
                session.close().await;
            }
            if !reinstall {
                state.send_replace(InterceptState::Off);
                return;
            }
        }
    }

    /// 装好了的透明访问。
    struct Session {
        plan: Plan,
        domain: String,
        outbound: Outbound<Os>,
        tasks: Vec<JoinHandle<()>>,
    }

    /// 按 netmap 算出要装成什么样：本节点能访问的对端（访问规则允许的）被拦截。
    fn plan_of(netmap: &Netmap) -> Plan {
        let peers: BTreeSet<Ipv4Addr> = netmap.peers.iter().filter(|peer| peer.outbound).map(|peer| peer.virtual_ip).collect();
        Plan { cidr: netmap.network.cidr, own: netmap.me.virtual_ip, peers }
    }

    impl Session {
        /// 装上。失败时系统里什么都不留（`i3t_intercept::setup::install` 自己会撤回），返回原因。
        async fn open(parts: &Parts, netmap: &Netmap) -> Result<(Self, InterceptState), InterceptCause> {
            let missing = Capability::missing();
            if !missing.is_empty() {
                return Err(InterceptCause::NoPermission {
                    missing: missing.iter().map(|capability| capability.name().to_owned()).collect(),
                });
            }
            if let Some(program) = [Program::Nft, Program::Ip].into_iter().find(|program| Shell::locate(*program).is_none()) {
                return Err(InterceptCause::ProgramMissing { program: program.name().to_owned() });
            }
            let entry = SocketAddr::V4(socket::entry());
            // 先把入口开好再装规则：规则一装上，连接就会被改过来。
            let listener = socket::tcp_listener().map_err(|error| bind_cause(entry, &error))?;
            let diverted = socket::udp_divert_socket().map_err(|error| bind_cause(entry, &error))?;
            let plan = plan_of(netmap);
            let installed = blocking({
                let plan = plan.clone();
                move |shell| setup::install(shell, &plan)
            })
            .await
            .map_err(setup_cause)?;
            let Installed { replaced } = installed;
            if replaced {
                tracing::info!("系统里有上次留下的透明访问的规则（进程没有正常退出），已经整个换掉");
            }
            parts.outbound.set_presenting(true);

            let access = Arc::clone(&parts.access);
            let mut tasks = vec![tokio::spawn(async move {
                access.intercept_tcp(listener, |stream| socket::original_destination(stream).map(SocketAddr::V4)).await;
            })];
            let access = Arc::clone(&parts.access);
            tasks.push(tokio::spawn(async move { access.intercept_udp(Tproxy(diverted)).await }));
            let (names, responder) = start_names(parts.netmap.clone(), netmap.me.virtual_ip, netmap.network.domain.clone()).await;
            tasks.extend(responder);
            let session = Self { plan, domain: netmap.network.domain.clone(), outbound: parts.outbound.clone(), tasks };
            Ok((session, InterceptState::On { udp: true, source_address: true, names }))
        }

        /// 跟上新的 netmap。要整个重装时返回 `false`。
        async fn follow(&mut self, netmap: &Netmap) -> bool {
            let next = plan_of(netmap);
            if next.own != self.plan.own || next.cidr != self.plan.cidr || !netmap.network.domain.eq_ignore_ascii_case(&self.domain) {
                tracing::info!("本节点的虚拟 IP、网络的地址段或者域名变了，透明访问整个重装");
                return false;
            }
            if next.peers == self.plan.peers {
                return true;
            }
            let updated = blocking({
                let next = next.clone();
                move |shell| setup::update_peers(shell, &next)
            })
            .await;
            match updated {
                Ok(()) => {
                    tracing::info!(peers = next.peers.len(), "能访问的对端变了，换掉了内核里被拦截的地址");
                    self.plan = next;
                    true
                }
                Err(error) => {
                    tracing::warn!(%error, "换不了内核里被拦截的地址（表可能被别人删掉了），整个重装");
                    false
                }
            }
        }

        /// 拆掉。
        async fn close(self) {
            for task in &self.tasks {
                task.abort();
            }
            self.outbound.set_presenting(false);
            match blocking(setup::remove).await {
                Ok(()) => tracing::info!("透明访问已关闭，系统里它装的东西都拆掉了"),
                Err(error) => tracing::warn!(%error, "透明访问的规则没有全部拆掉，下次启动时会换掉"),
            }
        }
    }

    /// 内核用 TPROXY 交过来的 UDP 报文。
    struct Tproxy(UdpSocket);

    impl Diversion for Tproxy {
        async fn recv(&self, buf: &mut [u8]) -> io::Result<Diverted> {
            let diverted = socket::recv_diverted(&self.0, buf).await?;
            Ok(Diverted { len: diverted.len, from: SocketAddr::V4(diverted.from), to: SocketAddr::V4(diverted.to) })
        }

        fn reply_socket(&self, local: SocketAddr, client: SocketAddr) -> io::Result<UdpSocket> {
            match (local, client) {
                (SocketAddr::V4(local), SocketAddr::V4(client)) => socket::udp_reply_socket(local, client),
                _ => Err(io::Error::other("透明访问只有 IPv4")),
            }
        }
    }

    /// 开名字服务（本节点虚拟 IP 的 53 端口），再让 systemd-resolved 把网络的域名交给它。
    async fn start_names(netmap: watch::Receiver<Arc<Netmap>>, own: Ipv4Addr, domain: String) -> (NameService, Option<JoinHandle<()>>) {
        let address = SocketAddr::from((own, i3t_intercept::DNS_PORT));
        let socket = match UdpSocket::bind(address).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(%address, %error, "名字服务开不起来，按虚拟 IP 访问照常可用");
                return (NameService::Failed { cause: bind_cause(address, &error) }, None);
            }
        };
        let responder = tokio::spawn(answer_names(socket, netmap));
        let outcome = blocking(move |shell| setup::configure_resolver(shell, &domain, own)).await;
        let service = match outcome {
            ResolverOutcome::Configured => NameService::Resolved,
            ResolverOutcome::Absent => {
                tracing::info!("这台机器没有在跑 systemd-resolved：名字要写进 /etc/hosts（i3t hosts），按虚拟 IP 访问照常可用");
                NameService::NoResolver
            }
            ResolverOutcome::Failed(error) => {
                tracing::warn!(%error, "systemd-resolved 不接受设置，名字解析不了，按虚拟 IP 访问照常可用");
                NameService::Failed { cause: command_cause(error) }
            }
        };
        (service, Some(responder))
    }

    /// 回答名字的查询，用节点现在看到的网络：本节点能访问的对端和自己。
    async fn answer_names(socket: UdpSocket, netmap: watch::Receiver<Arc<Netmap>>) {
        let mut buf = vec![0; 1500];
        let mut cached: Option<(u64, Names)> = None;
        loop {
            let (len, from) = match socket.recv_from(&mut buf).await {
                Ok(received) => received,
                Err(error) => {
                    tracing::debug!(%error, "名字服务收报文出错");
                    continue;
                }
            };
            let current = Arc::clone(&netmap.borrow());
            if cached.as_ref().is_none_or(|(version, _)| *version != current.version) {
                cached = Some((current.version, names_of(&current)));
            }
            let Some((_, names)) = &cached else { continue };
            if let Some(answer) = names.answer(&buf[..len])
                && let Err(error) = socket.send_to(&answer, from).await
            {
                tracing::debug!(%error, %from, "名字服务的应答发不出去");
            }
        }
    }

    /// netmap 里的名字：本节点能访问的对端，加上自己。名字服务和 `i3t hosts` 答的都是它。
    #[must_use]
    pub fn names_of(netmap: &Netmap) -> Names {
        let peers = netmap.peers.iter().filter(|peer| peer.outbound).map(|peer| (peer.name.clone(), peer.virtual_ip));
        Names::new(&netmap.network.domain, peers.chain([(netmap.me.name.clone(), netmap.me.virtual_ip)]))
    }

    fn bind_cause(address: SocketAddr, error: &io::Error) -> InterceptCause {
        if error.kind() == io::ErrorKind::AddrInUse {
            InterceptCause::PortInUse { address }
        } else {
            InterceptCause::BindFailed { address, detail: error.to_string() }
        }
    }

    fn command_cause(error: CommandError) -> InterceptCause {
        match error {
            CommandError::Missing(program) => InterceptCause::ProgramMissing { program: program.name().to_owned() },
            CommandError::Failed { program, detail, .. } | CommandError::Io { program, detail } => {
                InterceptCause::CommandFailed { program: program.name().to_owned(), detail }
            }
        }
    }

    fn setup_cause(error: SetupError) -> InterceptCause {
        match error {
            SetupError::Command(error) => command_cause(error),
            SetupError::TableInUse { .. } => InterceptCause::TableInUse { table: i3t_intercept::ROUTE_TABLE },
            SetupError::LinkInUse => InterceptCause::LinkInUse { link: i3t_intercept::LINK.to_owned() },
            SetupError::RangeInUse { with } => InterceptCause::RangeInUse { with },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::control::testing::netmap;

        fn peer(raw: u8, name: &str, outbound: bool) -> i3t_model::netmap::Peer {
            serde_json::from_value(serde_json::json!({
                "id": raw, "key": i3t_proto::PublicKey::from_bytes([raw; 32]).to_string(), "name": name,
                "virtual_ip": format!("10.66.0.{raw}"), "inbound": true, "outbound": outbound
            }))
            .unwrap()
        }

        #[test]
        fn only_peers_this_node_may_access_are_intercepted_and_named() {
            let mut map = netmap(1);
            map.peers = vec![peer(5, "nas", true), peer(9, "hidden", false)];
            let plan = plan_of(&map);
            assert!(!plan.peers.contains(&Ipv4Addr::new(10, 66, 0, 9)), "访问规则不允许的对端不拦截：连它当场被拒绝");
            assert!(plan.peers.contains(&map.peers[0].virtual_ip));
            assert_eq!(plan.own, map.me.virtual_ip);
            let names = names_of(&map);
            assert_eq!(names.lookup(&format!("{}.{}", map.peers[0].name, map.network.domain)), Some(map.peers[0].virtual_ip));
            assert_eq!(names.lookup(&format!("hidden.{}", map.network.domain)), None);
            assert_eq!(names.lookup(&format!("{}.{}", map.me.name, map.network.domain)), Some(map.me.virtual_ip), "自己的名字也答");
        }

        #[test]
        fn setup_failures_become_codes_with_their_values() {
            assert_eq!(setup_cause(SetupError::LinkInUse), InterceptCause::LinkInUse { link: "i3t0".into() });
            assert_eq!(setup_cause(SetupError::TableInUse { routes: 2 }), InterceptCause::TableInUse { table: 2033 });
            let overlap = SetupError::RangeInUse { with: "192.168.1.5/24 dev eth0".into() };
            assert_eq!(setup_cause(overlap), InterceptCause::RangeInUse { with: "192.168.1.5/24 dev eth0".into() });
            let failed = CommandError::Failed { program: Program::Nft, status: Some(1), detail: "Error: Could not process rule".into() };
            assert_eq!(
                setup_cause(SetupError::Command(failed)),
                InterceptCause::CommandFailed { program: "nft".into(), detail: "Error: Could not process rule".into() }
            );
            let in_use = io::Error::from(io::ErrorKind::AddrInUse);
            let address: SocketAddr = "127.0.0.1:1".parse().unwrap();
            assert_eq!(bind_cause(address, &in_use), InterceptCause::PortInUse { address });
        }
    }
}
