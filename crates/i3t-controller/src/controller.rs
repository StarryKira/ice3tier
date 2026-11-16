//! controller 的核心：存储之上的全部规则，和每个节点的 netmap 的下发。
//!
//! [`Controller`] 的方法都是同步的，内部只有一把锁：一次操作要么完整地做完（写存储、重算受影响的 netmap、
//! 放进各个节点的发送队列），要么不做。节点协议和管理 API 只是它的两个入口。
//!
//! 方法里有数据库读写，重算一个大网络也要花时间，所以异步代码要经 `spawn_blocking` 调用它们。

mod address;
mod credentials;
mod diag;
mod enrollment;
mod forwards;
mod listeners;
mod metrics;
mod naming;
mod networks;
mod nodes;
mod pairs;
mod protocol;
mod publish;
mod rollout;
mod routes;
mod rules;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
mod update;
mod usage;
mod views;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use i3t_crypto::{NodeKey, tls};
use i3t_model::Netmap;
use i3t_model::admin::{LogEntry, NetworkId, Node, NodeRef, Status, Timestamp};
use i3t_model::control::{ForwardReport, LaneReport, ListenerReport, ToNode};
use i3t_model::update::{UpdateStatus, Version};
use i3t_proto::{NodeId, PublicKey};
use tokio::sync::{Notify, mpsc};

use crate::error::{Entity, Error};
use crate::gate::Throttle;
use crate::netmap::NetworkView;
use crate::secret;
use crate::store::{DIGEST_LEN, LogFilter, NetworkRecord, NodeRecord, Store};
use crate::sync::NodeSync;

pub use protocol::Admitted;
pub(crate) use publish::Seeds;
pub use routes::RouteAnswers;

/// 节点协议在 TLS 上的 ALPN。
pub const ALPN: &[u8] = b"i3t-ctl/1";

const KEY_FILE: &str = "controller.key";
const DATABASE_FILE: &str = "controller.db";
const ADMIN_TOKEN_META: &str = "admin_token_hash";

/// 删掉的节点的虚拟 IP 和名字多久之后才能再分出去。
const COOLDOWN_SECS: u64 = 90 * 24 * 60 * 60;
/// 拒绝没有注册的公钥、无效的令牌这一类日志多久最多记一条。谁都能拿一把新生成的公钥连上来，不限的话日志会被刷满。
const REFUSAL_LOG_EVERY: Duration = Duration::from_secs(10);

/// controller 里的几个时间。默认值是协议约定的，测试里会改短。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// 节点确认之后等多久，再把因此可以继续的变更发给别的节点。一批确认并成一次下发。
    pub ack_coalesce: Duration,
    /// 节点上报的端点或挂靠关系变了之后等多久，再发给它的对端。
    /// 这段时间里同一个对端只会被重算一次，所以一个反复变化的节点引不起下发风暴。
    pub report_coalesce: Duration,
    /// 在线的节点这么久不确认新的 netmap，就不再等它，免得它卡住别的节点。
    pub ack_timeout: Duration,
    /// controller 连续这么久没有东西要发给一个节点时，发一个心跳。
    pub heartbeat_idle: Duration,
    /// 这么久收不到节点的任何消息，就把它记为离线。
    pub liveness_timeout: Duration,
    /// 新连接要在这么久之内完成 TLS 握手并发来第一条消息。
    pub handshake_timeout: Duration,
    /// 一个 relay 更新完重新连上来之后，过这么久才轮到同一个网络里的下一个 relay 更新：
    /// 挂靠它的节点要重新建好中转会话，换路的时候才有路可换。
    pub relay_settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ack_coalesce: Duration::from_millis(50),
            report_coalesce: Duration::from_secs(1),
            ack_timeout: Duration::from_secs(10),
            heartbeat_idle: Duration::from_secs(20),
            liveness_timeout: Duration::from_secs(35),
            handshake_timeout: Duration::from_secs(10),
            relay_settle: Duration::from_secs(30),
        }
    }
}

pub struct Controller {
    key: NodeKey,
    public_addr: String,
    /// 从数据目录打开时是那个目录。
    data_dir: Option<PathBuf>,
    timing: Timing,
    /// 管理令牌的哈希。每个管理请求都要查它，所以不放在大锁里。
    admin_token: Mutex<Option<[u8; DIGEST_LEN]>>,
    inner: Mutex<Inner>,
    /// 有延后的下发时叫醒后台任务。
    wake: Notify,
    /// 有包要取时叫醒取包的后台任务。
    release_wake: Notify,
    /// 设置变了、要尽快重新查发布源时叫醒查发布源的后台任务。
    check_wake: Notify,
    /// 给还没认出来的连接的拒绝日志限速。
    pub(crate) refusal_log: Throttle,
}

/// [`Controller::open`] 的结果。
pub struct Opened {
    pub controller: Controller,
    /// 第一次启动时生成的管理令牌。只有这一次能拿到，controller 只存它的哈希。
    pub admin_token: Option<String>,
}

pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) timing: Timing,
    pub(crate) clock: Clock,
    /// 每个节点的 netmap 下发到了哪一步。存储里有的节点这里都有。
    pub(crate) sync: BTreeMap<NodeId, NodeSync>,
    /// 现在连着的节点。
    pub(crate) live: BTreeMap<NodeId, Live>,
    /// 等着延后重算的节点。
    pub(crate) dirty: BTreeMap<NodeId, Deferred>,
    /// 各个节点最近上报的路由表。节点离线之后留着，界面上显示成过期的；controller 重启就没有了。
    pub(crate) routes: BTreeMap<NodeId, routes::Reported>,
    /// 出口和用出口的节点报的计数，算健康和交叉核对用。节点离线之后留着；controller 重启就没有了。
    pub(crate) usage: usage::Usage,
    /// 已经发给节点、还在等回答的路由查询。
    pub(crate) route_queries: BTreeMap<u64, routes::PendingQuery>,
    /// 进行中的主动测试和全网矩阵。
    pub(crate) diag: diag::Diag,
    /// 节点更新：已知的发布、包、设置、轮次。
    pub(crate) updates: update::Updates,
    pub(crate) counters: metrics::Counters,
    next_conn: u64,
    next_request: u64,
    last_purge: Option<Instant>,
}

/// 一个在线节点的连接，和它在这次连接期间上报的东西。
pub(crate) struct Live {
    /// 连接的序号。旧连接收尾时靠它知道自己已经被新连接顶替了。
    pub(crate) conn: u64,
    pub(crate) network: NetworkId,
    pub(crate) outbox: mpsc::Sender<ToNode>,
    pub(crate) mapping_varies: Option<bool>,
    pub(crate) flows: u32,
    pub(crate) tx_bytes: u64,
    pub(crate) rx_bytes: u64,
    /// 节点说的节点协议是第几版。后来加的消息只发给够新的节点。
    pub(crate) protocol: u32,
    /// 节点这次连接上最近一次报的更新进度；还没有上报过（或者它不懂更新）时是 `None`。
    pub(crate) update: Option<UpdateStatus>,
    /// 这次连接上发给它的更新指令。
    pub(crate) order: Option<rollout::Order>,
    /// 这条连接是 `i3t join` 注册时的那一条，不是节点真的跑起来连上的：它不算"最近在线"。
    pub(crate) enrolling: bool,
    /// 这次连接上最近一次上报的时刻；还没有上报过时是 `None`。
    pub(crate) reported_at: Option<Instant>,
    /// 最近一次上报里的监听器和端口转发的状态。
    pub(crate) listeners: Vec<ListenerReport>,
    pub(crate) forwards: Vec<ForwardReport>,
    /// 最近一次上报里各个凭据（按编号）现在的连接数。
    pub(crate) credential_connections: BTreeMap<u32, u32>,
    /// 最近一次上报里独占 TCP 的状况。
    pub(crate) lanes: Option<LaneReport>,
    /// 节点说它接不接受吞吐测试（本机配置文件里的开关）。
    pub(crate) throughput_tests: bool,
}

pub(crate) struct Deferred {
    pub(crate) network: NetworkId,
    pub(crate) due: Instant,
}

impl Controller {
    /// 打开数据目录：第一次启动时在里面生成 controller 的密钥、数据库和管理令牌。
    ///
    /// `public_addr` 是节点连 controller 用的 `主机:端口`，会写进每个注册令牌。
    ///
    /// # Errors
    ///
    /// 目录建不了、密钥文件或数据库读不出来。
    pub fn open(data_dir: &Path, public_addr: String) -> Result<Opened, Error> {
        fs::create_dir_all(data_dir).map_err(|source| Error::Io { path: data_dir.to_owned(), source })?;
        let key = load_or_create_key(&data_dir.join(KEY_FILE))?;
        let store = Store::open(&data_dir.join(DATABASE_FILE))?;
        let mut controller = Self::new(store, key, public_addr, Timing::default())?.with_release_cache(data_dir.join(update::CACHE_DIR));
        controller.data_dir = Some(data_dir.to_owned());
        let admin_token = if controller.has_admin_token() { None } else { Some(controller.issue_admin_token()?) };
        Ok(Opened { controller, admin_token })
    }

    /// 在一个已经打开的存储上建 controller。不生成管理令牌。
    ///
    /// # Errors
    ///
    /// 读存储失败。
    pub fn new(store: Store, key: NodeKey, public_addr: String, timing: Timing) -> Result<Self, Error> {
        let admin_token = store.meta(ADMIN_TOKEN_META)?.and_then(|hash| <[u8; DIGEST_LEN]>::try_from(hash).ok());
        let version: Version = env!("CARGO_PKG_VERSION").parse().expect("Cargo 保证包的版本号是合法的语义化版本");
        let updates = update::Updates::load(&store, version)?;
        let mut inner = Inner {
            store,
            timing,
            clock: Clock::default(),
            sync: BTreeMap::new(),
            live: BTreeMap::new(),
            dirty: BTreeMap::new(),
            routes: BTreeMap::new(),
            usage: usage::Usage::default(),
            route_queries: BTreeMap::new(),
            diag: diag::Diag::default(),
            updates,
            counters: metrics::Counters::default(),
            next_conn: 0,
            next_request: 0,
            last_purge: None,
        };
        inner.load_sync()?;
        Ok(Self {
            key,
            public_addr,
            data_dir: None,
            timing,
            admin_token: Mutex::new(admin_token),
            inner: Mutex::new(inner),
            wake: Notify::new(),
            release_wake: Notify::new(),
            check_wake: Notify::new(),
            refusal_log: Throttle::new(REFUSAL_LOG_EVERY),
        })
    }

    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        self.key.public()
    }

    #[must_use]
    pub fn public_addr(&self) -> &str {
        &self.public_addr
    }

    #[must_use]
    pub const fn timing(&self) -> Timing {
        self.timing
    }

    /// 节点协议的 TLS 配置。任何公钥都能完成握手：还没注册的节点要先连上来才能交注册令牌，
    /// 认不认这把公钥在第一条消息上决定。
    ///
    /// # Errors
    ///
    /// 用 controller 的密钥签证书失败。
    pub fn tls_config(&self) -> Result<rustls::ServerConfig, Error> {
        Ok(tls::server_config(&self.key, |_| true, ALPN)?)
    }

    fn has_admin_token(&self) -> bool {
        self.admin_token.lock().expect("admin token mutex poisoned").is_some()
    }

    /// 生成一个新的管理令牌，原来的随即作废。返回的明文只有这一次能拿到。
    ///
    /// # Errors
    ///
    /// 随机数源不可用，或者写存储失败。
    pub fn issue_admin_token(&self) -> Result<String, Error> {
        let token = secret::generate()?;
        let hash = secret::sha256(token.as_bytes());
        let inner = self.lock();
        inner.store.set_meta(ADMIN_TOKEN_META, &hash)?;
        inner.store.log(inner.clock.now(), None, "admin.token", "生成了新的管理令牌", serde_json::json!({}))?;
        *self.admin_token.lock().expect("admin token mutex poisoned") = Some(hash);
        Ok(token)
    }

    /// `token` 是不是当前的管理令牌。
    #[must_use]
    pub fn is_admin_token(&self, token: &str) -> bool {
        let expected = *self.admin_token.lock().expect("admin token mutex poisoned");
        expected.is_some_and(|expected| secret::digests_match(&expected, &secret::sha256(token.as_bytes())))
    }

    /// # Errors
    ///
    /// 读存储失败。
    pub fn status(&self) -> Result<Status, Error> {
        let inner = self.lock();
        let updates = &inner.updates;
        let newer = updates.release.as_ref().map(|release| &release.manifest.version).filter(|version| **version > updates.version);
        Ok(Status {
            version: updates.version.to_string(),
            controller_key: self.public_key(),
            public_addr: self.public_addr.clone(),
            networks: count(inner.store.networks()?.len()),
            nodes: inner.store.node_count(None)?,
            online: count(inner.live.len()),
            newer_version: newer.cloned(),
            data_dir: self.data_dir.as_ref().map(|dir| dir.display().to_string()),
        })
    }

    /// 最近的操作记录，新的在前。
    ///
    /// # Errors
    ///
    /// 读存储失败。
    pub fn oplog(&self, limit: u32) -> Result<Vec<LogEntry>, Error> {
        Ok(self.lock().store.oplog(limit)?)
    }

    /// 符合条件的操作记录，新的在前。
    ///
    /// # Errors
    ///
    /// 读存储失败。
    pub fn oplog_filtered(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, Error> {
        Ok(self.lock().store.oplog_filtered(filter)?)
    }

    /// 一个节点当前版本的 netmap，和发给它的那一份相同。节点不存在时返回 `None`。
    ///
    /// # Errors
    ///
    /// 读存储失败。
    pub fn netmap_of(&self, node: NodeId) -> Result<Option<Netmap>, Error> {
        let inner = self.lock();
        let Some(record) = inner.store.node(node)? else { return Ok(None) };
        let Some(view) = NetworkView::load(&inner.store, record.network)? else { return Ok(None) };
        let Some(sync) = inner.sync.get(&node) else { return Ok(None) };
        Ok(view.compile(&view.access(), node, &sync.relations).map(|netmap| Netmap { version: sync.state.version, ..netmap }))
    }

    /// 做到期的后台工作：延后的下发、不再等迟迟不确认的节点、清理过期的令牌和冷却记录。
    /// 返回下一次最晚什么时候再调用；有新的延后工作时 [`Controller::wait_for_work`] 会提前返回。
    ///
    /// # Errors
    ///
    /// 读写存储失败。到期的工作留在队列里，下次再试。
    pub fn tick(&self) -> Result<Option<Instant>, Error> {
        let mut inner = self.lock();
        let wall = inner.clock.now();
        inner.tick(Instant::now(), wall)
    }

    /// 等到有新的延后工作。
    pub async fn wait_for_work(&self) {
        self.wake.notified().await;
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("controller state mutex poisoned")
    }

    /// 还没有别人拿着这个 controller 的时候直接改它里面的东西，给 `with_…` 这几个装配用的方法用。
    fn inner_mut(&mut self) -> &mut Inner {
        self.inner.get_mut().expect("controller state mutex poisoned")
    }

    /// 操作留下了要后台任务接着做的事时叫醒它：有延后的下发，有节点欠着确认（要有人盯着它是不是等得太久），
    /// 或者有 relay 更新完在等站稳。有包要取时另外叫醒管发布的后台任务。
    pub(crate) fn wake_if_needed(&self, inner: &Inner) {
        if !inner.dirty.is_empty()
            || inner.sync.values().any(NodeSync::is_waiting)
            || inner.updates.next_rollout.is_some()
            || !inner.diag.is_idle()
        {
            self.wake.notify_one();
        }
        if inner.updates.packages.values().any(|package| *package == update::Package::Wanted) {
            self.release_wake.notify_one();
        }
    }

    #[cfg(test)]
    pub(crate) fn advance_clock(&self, secs: u64) {
        self.lock().clock.advance(secs);
    }
}

impl Inner {
    pub(crate) fn require_network(&self, id: NetworkId) -> Result<NetworkRecord, Error> {
        self.store.network(id)?.ok_or(Error::NotFound(Entity::Network))
    }

    /// 找一个节点，并且它必须属于 `network`：别的网络的节点在这里等于不存在。
    pub(crate) fn require_node(&self, network: NetworkId, id: NodeId) -> Result<NodeRecord, Error> {
        self.store.node(id)?.filter(|node| node.network == network).ok_or(Error::NotFound(Entity::Node))
    }

    pub(crate) fn is_online(&self, node: NodeId) -> bool {
        self.live.contains_key(&node)
    }

    pub(crate) fn next_conn(&mut self) -> u64 {
        self.next_conn += 1;
        self.next_conn
    }

    /// 发给节点的查询的编号。
    pub(crate) fn next_request(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    /// 管理 API 里的节点：存储里的记录，加上它现在在不在线、netmap 发到了第几版。
    pub(crate) fn node_view(&self, network: &NetworkRecord, record: &NodeRecord, now: Timestamp) -> Node {
        let online = self.is_online(record.id);
        // 只是注册时连上过（`i3t join`）不算在线过：节点真的跑起来连上了才有"最近在线"。
        let running = self.live.get(&record.id).is_some_and(|live| !live.enrolling);
        let state = self.sync.get(&record.id).map_or(record.netmap, |sync| sync.state);
        Node {
            id: record.id,
            network: record.network,
            key: record.key,
            name: record.name.clone(),
            fqdn: format!("{}.{}", record.name, network.domain()),
            virtual_ip: record.virtual_ip,
            tags: record.tags.iter().cloned().collect(),
            declared: record.declared,
            relay: record.relay.clone(),
            exit: record.exit.clone(),
            dial_out_only: record.dial_out_only,
            routing: record.routing,
            reach: record.reach,
            pinned_relays: record.pinned_relays.clone(),
            platform: record.platform.clone(),
            version: record.version.clone(),
            online,
            last_seen_at: if running { Some(now) } else { record.last_seen_at },
            created_at: record.created_at,
            netmap_version: state.version,
            sent_version: state.sent,
            acked_version: state.acked,
            hostname: record.hostname.clone(),
            update: self.node_update(record),
            attached: record.attached.clone(),
        }
    }

    fn tick(&mut self, now: Instant, wall: Timestamp) -> Result<Option<Instant>, Error> {
        self.give_up_on_slow_acks(now);
        let next = self.flush_due(now)?;
        if self.updates.next_rollout.is_some_and(|at| at <= now) {
            self.roll_out()?;
        }
        if self.last_purge.is_none_or(|last| now.duration_since(last) >= PURGE_INTERVAL) {
            self.last_purge = Some(now);
            self.store.purge_expired_tokens(wall)?;
            self.store.purge_cooldowns(wall)?;
        }
        // 还有节点欠着确认时，隔一会儿要回来看它们是不是等得太久了。
        let waiting = self.sync.values().any(NodeSync::is_waiting).then(|| now + self.timing.ack_timeout.min(SWEEP_INTERVAL));
        let tests = self.tests_tick(now)?;
        Ok([next, waiting, self.updates.next_rollout, tests].into_iter().flatten().min())
    }
}

/// 过期的令牌和冷却记录多久清一次。它们过期之后本来就不起作用，清理只是不让表一直长。
const PURGE_INTERVAL: Duration = Duration::from_secs(60);
/// 有节点欠着确认时多久查一次超时。
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) fn node_ref(record: &NodeRecord) -> NodeRef {
    NodeRef { id: record.id, name: record.name.clone(), virtual_ip: record.virtual_ip }
}

/// 操作记录里一个开关现在是开还是关。
pub(crate) const fn switch(on: bool) -> &'static str {
    if on { "开" } else { "关" }
}

/// 操作记录里的一串名字：用顿号连起来，一个都没有时写"无"。
pub(crate) fn listed<T: std::fmt::Display>(items: impl Iterator<Item = T>) -> String {
    let items: Vec<String> = items.map(|item| item.to_string()).collect();
    if items.is_empty() { "无".to_owned() } else { items.join("、") }
}

/// 管理 API 里的计数都是 `u32`。真的数不下时停在最大值，不回绕。
pub(crate) fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// 墙上时钟。测试里可以往前拨，用来试过期和冷却期。
#[derive(Debug, Default)]
pub(crate) struct Clock {
    #[cfg(test)]
    skew: std::sync::atomic::AtomicU64,
}

impl Clock {
    pub(crate) fn now(&self) -> Timestamp {
        // 系统时钟被拨到 1970 年之前时当作 0：时间戳只用来比较先后和展示，不值得为此让操作失败。
        let system = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
        #[cfg(test)]
        let system = system + self.skew.load(std::sync::atomic::Ordering::Relaxed);
        system
    }

    #[cfg(test)]
    fn advance(&self, secs: u64) {
        self.skew.fetch_add(secs, std::sync::atomic::Ordering::Relaxed);
    }
}

fn load_or_create_key(path: &Path) -> Result<NodeKey, Error> {
    let io_error = |source| Error::Io { path: path.to_owned(), source };
    match fs::read_to_string(path) {
        Ok(pem) => NodeKey::from_pem(&pem).map_err(|source| Error::Key { path: path.to_owned(), source }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let key = NodeKey::generate().map_err(Error::KeyGeneration)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            // 私钥文件只有属主可读。别的平台靠数据目录本身的权限。
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(path).map_err(io_error)?;
            file.write_all(key.to_pem().as_bytes()).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            tracing::info!(path = %path.display(), public = %key.public(), "生成了 controller 的密钥");
            Ok(key)
        }
        Err(err) => Err(io_error(err)),
    }
}
