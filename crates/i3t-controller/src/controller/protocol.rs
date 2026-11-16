//! 节点协议的各个消息在 controller 这一侧的处理。
//!
//! 这里不碰 socket：连接由 [`crate::node_server`] 管，它把收到的消息交到这里，从发送队列里取要发的消息。
//! 节点是谁由连接的 TLS 握手确定，所以这里的入口拿到的公钥和节点编号都是认证过的。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use i3t_model::admin::LogRef;
use i3t_model::control::{Hello, MAX_PACKAGE_CHUNK, Report, Signal, ToController, ToNode};
use i3t_model::netmap::Attachment;
use i3t_model::update::{Platform, Version};
use i3t_proto::{NodeId, PublicKey};
use serde_json::json;
use tokio::sync::mpsc;

use super::usage::UsageReport;
use super::{Controller, Inner, Live, Seeds};
use crate::error::{Error, Rejection, error_chain};
use crate::netmap::MAX_ENDPOINTS;
use crate::store::NodeRecord;

/// 一个节点最多报这么多个挂靠的 relay。默认只挂靠 2 个。
const MAX_ATTACHMENTS: usize = 16;
/// 节点自报的版本号、平台最长留这么多个字符。
const MAX_HELLO_TEXT: usize = 64;
/// 节点自报的主机名最长留这么多个字符。域名最长就是这么长。
const MAX_HOSTNAME: usize = 253;
/// 到 relay 的延迟变化不超过这个数（毫秒）、也不超过原来的四分之一时，不算变化。
/// 延迟每次测出来都差几毫秒，不这样就每次上报都要给全部对端重发 netmap。
const RTT_NOISE_MS: u32 = 10;

/// 一条被接受的连接：哪个节点，第几条连接。之后这条连接上的消息都带着它交进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admitted {
    pub node: NodeId,
    pub(crate) conn: u64,
}

impl Controller {
    /// 已经注册的节点连上来报到。`key` 是它在 TLS 握手里证明过的公钥。
    ///
    /// 成功时节点已经登记为在线（之前的连接被顶替），当前的 netmap 已经在 `outbox` 里。
    ///
    /// # Errors
    ///
    /// 这把公钥没有注册。原因可以原样告诉节点。
    pub fn hello(&self, key: PublicKey, hello: &Hello, remote: IpAddr, outbox: mpsc::Sender<ToNode>) -> Result<Admitted, Rejection> {
        let mut inner = self.lock();
        let known = inner.store.node_by_key(&key).map_err(Error::from);
        let outcome = match known {
            Ok(Some(node)) => inner.admit(node, Joined { hello, remote, outbox, enrolling: false }),
            Ok(None) => {
                if let Some(suppressed) = self.refusal_log.allow() {
                    tracing::warn!(key = ?key, %remote, suppressed, "拒绝连接：这把公钥没有注册");
                }
                return Err(Rejection::UnknownKey);
            }
            Err(err) => Err(err),
        };
        self.wake_if_needed(&inner);
        outcome.map_err(|err| {
            tracing::error!(key = ?key, %remote, "处理节点报到时出错：{}", error_chain(&err));
            Rejection::Internal
        })
    }

    /// 处理一条已经被接受的连接上收到的消息。连接已经被顶替、或者节点已经被删掉时，消息被忽略。
    ///
    /// # Errors
    ///
    /// 读写存储失败。这条消息没有处理完，连接可以继续用。
    pub fn handle(&self, session: Admitted, remote: IpAddr, message: ToController) -> Result<(), Error> {
        let mut inner = self.lock();
        if inner.live.get(&session.node).is_none_or(|live| live.conn != session.conn) {
            return Ok(());
        }
        let outcome = match message {
            // 取包要读文件，不能拿着锁做。
            ToController::PackageRequest { request, version, platform, offset, len } => {
                drop(inner);
                self.package_request(session, request, &version, &platform, PackageRange { offset, len });
                return Ok(());
            }
            ToController::RouteAnswer { request, answer } => {
                inner.route_answered(session.node, request, answer);
                Ok(())
            }
            ToController::BackboneAnswer { request, rows } => {
                inner.backbone_answered(session.node, request, rows);
                Ok(())
            }
            ToController::TestReport(report) => inner.test_reported(session.node, *report),
            ToController::Report(report) => inner.report(session.node, report),
            ToController::Ack { version } => inner.ack(session.node, version),
            ToController::Signal { to, signal } => {
                inner.signal(session.node, to, signal);
                Ok(())
            }
            // 节点的本地声明变了的时候会在同一条连接上再发一次。
            ToController::Hello(hello) => inner.rehello(session.node, &hello, remote),
            ToController::Heartbeat => Ok(()),
            ToController::Enroll { .. } => {
                tracing::debug!(node = %session.node, "忽略已经报到的连接上的注册请求");
                Ok(())
            }
        };
        self.wake_if_needed(&inner);
        outcome
    }

    /// 一条连接结束了。它还是这个节点当前的连接时，节点记为离线。
    pub fn disconnect(&self, session: Admitted) {
        let mut inner = self.lock();
        if inner.live.get(&session.node).is_some_and(|live| live.conn == session.conn) {
            tracing::info!(node = %session.node, "节点离线");
            inner.drop_session(session.node);
            self.wake_if_needed(&inner);
        }
    }

    /// 现在在线的节点。
    #[must_use]
    pub fn online_nodes(&self) -> Vec<NodeId> {
        self.lock().live.keys().copied().collect()
    }

    /// 节点要更新包里的一段。包是公开的发布文件，所以只要是已经报到的节点就给，不看它有没有收到过更新指令。
    fn package_request(&self, session: Admitted, request: u64, version: &Version, platform: &Platform, range: PackageRange) {
        let is_current = |inner: &Inner| inner.live.get(&session.node).is_some_and(|live| live.conn == session.conn);
        let cached = {
            let inner = self.lock();
            if !is_current(&inner) {
                return;
            }
            inner.updates.cached_package(version, platform.as_str()).map(|(path, artifact)| (path.to_owned(), artifact.size))
        };
        let chunk = match cached {
            Some((path, size)) => range.read(&path, size),
            None => Err(format!("controller 上没有 {version} 的 {platform} 平台的包")),
        };
        let mut inner = self.lock();
        if !is_current(&inner) {
            return;
        }
        let reply = match chunk {
            Ok(data) => {
                inner.counters.package_bytes += u64::try_from(data.len()).unwrap_or(u64::MAX);
                ToNode::PackageChunk { request, data }
            }
            Err(reason) => {
                tracing::warn!(node = %session.node, %version, %platform, offset = range.offset, %reason, "给不了节点要的这一段更新包");
                ToNode::PackageUnavailable { request, reason }
            }
        };
        inner.send(session.node, reply);
    }
}

/// 节点要的那一段：从哪里起，最多多少字节。
#[derive(Debug, Clone, Copy)]
struct PackageRange {
    offset: u64,
    len: u32,
}

impl PackageRange {
    /// 从缓存里的包读出这一段。给出来的不会是空的，也不会超过 [`MAX_PACKAGE_CHUNK`]；读不出来时返回给节点看的原因。
    fn read(self, path: &Path, size: u64) -> Result<Vec<u8>, String> {
        let wanted = u64::from(self.len.min(MAX_PACKAGE_CHUNK)).min(size.saturating_sub(self.offset));
        if wanted == 0 {
            return Err(format!("要的位置 {} 不在包里（包有 {size} 字节），或者要的长度是 0", self.offset));
        }
        let read = || -> std::io::Result<Vec<u8>> {
            let mut file = File::open(path)?;
            file.seek(SeekFrom::Start(self.offset))?;
            let mut data = Vec::with_capacity(usize::try_from(wanted).unwrap_or(0));
            file.take(wanted).read_to_end(&mut data)?;
            Ok(data)
        };
        match read() {
            Ok(data) if !data.is_empty() => Ok(data),
            Ok(_) => Err("缓存里的包比清单上写的短".to_owned()),
            Err(error) => Err(format!("读不了缓存里的包：{error}")),
        }
    }
}

/// 一条刚连上来、要被接受的连接。
pub(super) struct Joined<'a> {
    pub(super) hello: &'a Hello,
    pub(super) remote: IpAddr,
    pub(super) outbox: mpsc::Sender<ToNode>,
    /// 这是 `i3t join` 注册时的那一条连接：节点只是拿了配置就走，还没有真的跑起来。
    pub(super) enrolling: bool,
}

impl Inner {
    /// 让一个节点上线：记下它的自我介绍，登记连接，发给它当前的 netmap。
    pub(super) fn admit(&mut self, mut node: NodeRecord, joined: Joined<'_>) -> Result<Admitted, Error> {
        let Joined { hello, remote, outbox, enrolling } = joined;
        let seeds = self.apply_hello(&mut node, hello, remote)?;
        let conn = self.next_conn();
        let live = Live {
            conn,
            network: node.network,
            outbox,
            mapping_varies: None,
            flows: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            protocol: hello.protocol,
            update: None,
            order: None,
            enrolling,
            reported_at: None,
            listeners: Vec::new(),
            forwards: Vec::new(),
            credential_connections: BTreeMap::new(),
            lanes: None,
            throughput_tests: hello.throughput_tests,
        };
        self.drop_route_queries(node.id);
        if self.live.insert(node.id, live).is_some() {
            // 旧连接的发送队列随着被替换掉而关闭，那条连接自己会结束。
            tracing::info!(node = %node.id, "同一个节点的新连接顶替了旧连接");
        }
        tracing::info!(node = %node.id, name = %node.name, %remote, version = %node.version, platform = %node.platform, enrolling, "节点上线");
        // 注册时那一条连接不算"最近在线"：节点真的跑起来、带着 Hello 连上来才算。
        let outcome = if enrolling { Ok(()) } else { self.store.set_node_last_seen(node.id, self.clock.now()).map_err(Error::from) };
        let outcome = outcome.and_then(|()| self.refresh(node.network, seeds, Some(node.id)));
        if let Err(err) = outcome {
            self.live.remove(&node.id);
            return Err(err);
        }
        // 更新的指令不在这里发：要等它的第一次上报，才知道它是带着什么回来的（见 `rollout`）。
        self.relay_is_back(&node);
        Ok(Admitted { node: node.id, conn })
    }

    fn rehello(&mut self, id: NodeId, hello: &Hello, remote: IpAddr) -> Result<(), Error> {
        let Some(mut node) = self.store.node(id)? else { return Ok(()) };
        let seeds = self.apply_hello(&mut node, hello, remote)?;
        if let Some(live) = self.live.get_mut(&id) {
            live.protocol = hello.protocol;
            live.throughput_tests = hello.throughput_tests;
        }
        self.refresh(node.network, seeds, None)?;
        self.roll_out()
    }

    /// 记下节点的自我介绍。节点撤回了某项声明时，已经启用的那项能力随之关闭。返回因此要重算的节点。
    fn apply_hello(&mut self, node: &mut NodeRecord, hello: &Hello, remote: IpAddr) -> Result<Seeds, Error> {
        let before = node.clone();
        let now = self.clock.now();
        node.declared = hello.declared;
        node.platform = clip(&hello.platform);
        node.version = clip(&hello.version);
        node.hostname = hello.hostname.chars().filter(|ch| !ch.is_control()).take(MAX_HOSTNAME).collect();
        node.self_update = hello.self_update;
        // 节点自报的版本号不清它的失败记录，哪怕它说自己已经在跑那个版本（或者更新的）：否则它报一个高版本、
        // 再报回原来的版本，就能把同一次失败报成"新的"，让管理员已经继续的版本再被暂停一次。
        // 已经用不上的失败记录在管理 API 里不显示（见 `Inner::node_update`），管理员点"立即更新"时清掉。

        let mut whole_network = false;
        let mut affected = BTreeSet::from([node.id]);
        if node.relay.is_some() && !node.declared.relay {
            node.relay = None;
            whole_network = true;
            tracing::warn!(node = %node.id, name = %node.name, "节点撤回了可以作为 relay 的声明，关闭它的 relay");
            let detail = format!("节点 {} 撤回了声明，relay 已关闭", node.name);
            let params = json!({ "node": node.id, "node_name": node.name });
            self.store.log_refs(now, Some(node.network), "node.relay.withdrawn", &detail, &[LogRef::node(node.id)], params)?;
        }
        if node.exit.is_some() && !node.declared.exit {
            node.exit = None;
            affected.extend(self.holders_of(node.id));
            tracing::warn!(node = %node.id, name = %node.name, "节点撤回了可以作为出口的声明，关闭它的出口");
            let detail = format!("节点 {} 撤回了声明，出口已关闭", node.name);
            let params = json!({ "node": node.id, "node_name": node.name });
            self.store.log_refs(now, Some(node.network), "node.exit.withdrawn", &detail, &[LogRef::node(node.id)], params)?;
        }
        if !node.declared.forward {
            let pointing_here = self.store.forwards(node.network)?.into_iter().filter(|forward| forward.config.target_node == node.id);
            let listeners: Vec<NodeId> =
                pointing_here.filter(|forward| forward.config.enabled).map(|forward| forward.config.listen_node).collect();
            if !listeners.is_empty() {
                let disabled = self.store.disable_forwards_to(node.id)?;
                affected.extend(listeners);
                tracing::warn!(node = %node.id, name = %node.name, disabled, "节点没有声明可以作为端口转发的目标，停用指向它的转发");
                let detail = format!("节点 {} 没有声明可以作为端口转发的目标，指向它的 {disabled} 条转发已停用", node.name);
                let params = json!({ "node": node.id, "node_name": node.name, "forwards": disabled });
                self.store.log_refs(now, Some(node.network), "node.forward.withdrawn", &detail, &[LogRef::node(node.id)], params)?;
            }
        }
        if *node != before {
            self.store.update_node(node)?;
        }

        // 节点自报的 UDP 端口加上这条连接的来源 IP，是它的一个候选端点。
        let observed = (hello.port != 0).then(|| SocketAddr::new(remote.to_canonical(), hello.port));
        if observed != node.observed {
            node.observed = observed;
            self.store.set_node_observed(node.id, observed)?;
            let holders = self.holders_of(node.id);
            self.defer(node.network, holders, self.timing.report_coalesce);
        }
        Ok(if whole_network { Seeds::All } else { Seeds::Nodes(affected) })
    }

    fn report(&mut self, id: NodeId, report: Report) -> Result<(), Error> {
        let Some(node) = self.store.node(id)? else { return Ok(()) };
        if let Some(live) = self.live.get_mut(&id) {
            live.mapping_varies = report.mapping_varies;
            live.flows = report.flows;
            live.tx_bytes = report.tx_bytes;
            live.rx_bytes = report.rx_bytes;
            live.lanes = report.lanes.as_deref().cloned();
        }
        let Report { run, credentials, exit, exits, listeners, forwards, .. } = report;
        self.usage_reported(&node, UsageReport { run, credentials, exit, exits, listeners, forwards })?;
        let report =
            Report { credentials: Vec::new(), exits: Vec::new(), listeners: Vec::new(), forwards: Vec::new(), exit: None, ..report };
        self.routes_reported(id, report.routes, report.route_kinds, report.routes_full);
        if let Some(status) = report.update {
            self.update_reported(&node, status)?;
        }

        let mut endpoints: Vec<SocketAddr> = Vec::with_capacity(report.endpoints.len().min(MAX_ENDPOINTS));
        for endpoint in report.endpoints {
            if endpoints.len() < MAX_ENDPOINTS && !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
        let mut attached = report.attached;
        attached.truncate(MAX_ATTACHMENTS);

        if same_endpoints(&node.endpoints, &endpoints) && !attachments_differ(&node.attached, &attached) {
            return Ok(());
        }
        tracing::debug!(node = %id, endpoints = endpoints.len(), attached = attached.len(), "节点的端点或挂靠关系变了");
        self.store.set_node_report(id, &endpoints, &attached)?;
        let holders = self.holders_of(id);
        self.defer(node.network, holders, self.timing.report_coalesce);
        Ok(())
    }

    fn ack(&mut self, id: NodeId, version: u64) -> Result<(), Error> {
        let Some(sync) = self.sync.get_mut(&id) else { return Ok(()) };
        // 节点确认不了还没发给它的版本。
        let version = version.min(sync.state.sent);
        if version <= sync.state.acked {
            return Ok(());
        }
        sync.state.acked = version;
        let released = sync.confirm(version);
        self.store.set_node_acked(id, version)?;
        if let Some(network) = self.live.get(&id).map(|live| live.network) {
            self.defer(network, released, self.timing.ack_coalesce);
        }
        Ok(())
    }

    /// 把一条信令转给目的节点。只在两个节点互为对端时转，否则丢掉：信令不能用来探测或打扰看不见的节点。
    fn signal(&mut self, from: NodeId, to: NodeId, signal: Signal) {
        let knows = |a: NodeId, b: NodeId| self.sync.get(&a).is_some_and(|sync| sync.knows(b));
        if !(knows(from, to) && knows(to, from)) {
            tracing::debug!(%from, %to, "丢掉信令：两个节点不是彼此的对端");
            return;
        }
        let Some(live) = self.live.get(&to) else {
            tracing::debug!(%from, %to, "丢掉信令：目的节点不在线");
            return;
        };
        // 信令是尽力而为的。目的节点的队列已经积了一半时不再往里放：
        // 不能让一个猛发信令的节点把别人的队列塞满，害得它被当成"不读"而断开。
        if live.outbox.capacity() * 2 < live.outbox.max_capacity() {
            tracing::debug!(%from, %to, "丢掉信令：目的节点的发送队列积压");
            return;
        }
        self.send(to, ToNode::Signal { from, signal });
    }
}

/// 节点自报的文本只留开头一段，免得一个乱报的节点往数据库里塞大段内容。
pub(super) fn clip(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).take(MAX_HELLO_TEXT).collect()
}

/// 两组端点是不是同一组，不管顺序。
fn same_endpoints(old: &[SocketAddr], new: &[SocketAddr]) -> bool {
    old.len() == new.len() && new.iter().all(|endpoint| old.contains(endpoint))
}

/// 挂靠关系有没有值得告诉对端的变化：挂靠的 relay 换了，或者到某个 relay 的延迟变了不少。
fn attachments_differ(old: &[Attachment], new: &[Attachment]) -> bool {
    old.len() != new.len()
        || new.iter().any(|attachment| {
            let Some(previous) = old.iter().find(|previous| previous.relay == attachment.relay) else { return true };
            attachment.rtt_ms.abs_diff(previous.rtt_ms) > RTT_NOISE_MS.max(previous.rtt_ms / 4)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(relay: u32, rtt_ms: u32) -> Attachment {
        Attachment { relay: NodeId::new(relay).unwrap(), rtt_ms }
    }

    #[test]
    fn small_rtt_changes_are_noise_but_a_new_relay_or_a_big_change_is_not() {
        let old = [attachment(1, 20), attachment(2, 200)];
        assert!(!attachments_differ(&old, &[attachment(2, 240), attachment(1, 28)]));
        assert!(attachments_differ(&old, &[attachment(1, 31), attachment(2, 200)]));
        assert!(attachments_differ(&old, &[attachment(1, 20), attachment(2, 260)]));
        assert!(attachments_differ(&old, &[attachment(1, 20), attachment(3, 200)]));
        assert!(attachments_differ(&old, &[attachment(1, 20)]));
    }

    #[test]
    fn endpoints_are_compared_as_sets() {
        let a: SocketAddr = "192.168.1.5:20330".parse().unwrap();
        let b: SocketAddr = "203.0.113.7:20330".parse().unwrap();
        assert!(same_endpoints(&[a, b], &[b, a]));
        assert!(!same_endpoints(&[a, b], &[a]));
        assert!(!same_endpoints(&[a], &[b]));
    }

    #[test]
    fn a_package_range_is_clamped_to_the_chunk_limit_and_the_end_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i3t-linux-x86_64");
        let content: Vec<u8> = (0..=u8::MAX).cycle().take(600_000).collect();
        std::fs::write(&path, &content).unwrap();
        let size = u64::try_from(content.len()).unwrap();
        let read = |offset, len| PackageRange { offset, len }.read(&path, size);

        assert_eq!(read(0, 10).unwrap(), content[..10]);
        assert_eq!(read(1000, u32::MAX).unwrap(), content[1000..1000 + 262_144], "一段不超过上限");
        assert_eq!(read(599_990, 4096).unwrap(), content[599_990..], "最后一段只给到文件末尾");
        assert!(read(600_000, 10).is_err() && read(700_000, 10).is_err(), "文件末尾之后没有东西可给");
        assert!(read(0, 0).is_err(), "不给空的段");
        assert!(PackageRange { offset: 0, len: 10 }.read(&dir.path().join("missing"), 100).is_err());
        assert!(read(0, 10).is_ok() && PackageRange { offset: 500_000, len: 10 }.read(&path, 100).is_err());
    }

    #[test]
    fn hello_text_is_clipped() {
        assert_eq!(clip("linux-x86_64"), "linux-x86_64");
        assert_eq!(clip(&"x".repeat(500)).len(), MAX_HELLO_TEXT);
        assert_eq!(clip("a\nb"), "ab");
    }
}
