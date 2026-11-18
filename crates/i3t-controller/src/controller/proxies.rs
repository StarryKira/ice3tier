//! 代理的增删改，和每个代理现在的状态。
//!
//! 一个代理是"在节点 A 上监听，经出口节点 B 出去，认一对用户名和密码"。B 必须是另一个、启用了出口的节点，
//! 访问规则要让 A 能访问 B。这几条在新建和修改时查（修改时只在会让它生效的那一下查，见 [`check`]）；
//! 之后出口被停用、规则变了，代理留着、照样下发、照样监听，状态说明它为什么用不了。

use i3t_model::admin::{CreateProxy, LogRef, NetworkId, Proxy, ProxyStatus, RefKind, UpdateProxy};
use i3t_model::control::{ListenState, ListenTraffic};
use i3t_model::netmap::Verifier;
use i3t_proto::NodeId;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};

use super::nodes::require_node;
use super::{Controller, Inner, Seeds, naming};
use crate::access::AccessMap;
use crate::error::{Entity, Error, InField};
use crate::netmap::NetworkView;
use crate::store::{ProxyRecord, Store};

/// 用户名和密码最长多少字节：SOCKS5 的认证报文里各有一个字节的长度。
const MAX_CREDENTIAL_LEN: usize = 255;

impl Controller {
    /// 一个网络里的代理；给了 `node` 时只列监听在那个节点上的。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn proxies(&self, network: NetworkId, node: Option<NodeId>) -> Result<Vec<Proxy>, Error> {
        let inner = self.lock();
        let view = NetworkView::load(&inner.store, network)?.ok_or(Error::NotFound(Entity::Network))?;
        let access = view.access();
        let wanted = view.proxies.iter().filter(|proxy| node.is_none_or(|node| proxy.node == node));
        Ok(wanted.map(|record| inner.proxy_view(&view, &access, record)).collect())
    }

    /// 一个代理。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个代理，或者读存储失败。
    pub fn proxy(&self, network: NetworkId, id: u32) -> Result<Proxy, Error> {
        let inner = self.lock();
        let record = inner.store.proxy(network, id)?.ok_or(Error::NotFound(Entity::Proxy))?;
        inner.proxy_status(network, &record)
    }

    /// # Errors
    ///
    /// 节点或出口不在这个网络里；出口就是监听的节点（`exit_is_node`）、没有启用出口（`exit_not_enabled`）、
    /// 访问规则不让监听的节点访问它（`access_denied`）；用户名和密码只写了一个（`credentials_incomplete`）；
    /// 名字、地址和同一个节点上的另一个代理冲突；地址的端口是 0。
    pub fn create_proxy(&self, network: NetworkId, input: CreateProxy) -> Result<Proxy, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let record = ProxyRecord {
            id: 0,
            node: input.node,
            name: input.name,
            bind: input.bind,
            protocol: input.protocol,
            exit: Some(input.exit),
            username: input.username,
            password: input.password,
            salt: salt()?,
            enabled: input.enabled,
            allow_from: input.allow_from,
            class: input.class,
            optimistic: input.optimistic,
        };
        let record = inner.store.atomically(|store| {
            let mut record = check(store, network, None, record)?;
            record.id = store.insert_proxy(&record)?;
            let detail = format!("节点 #{} 上新建代理 {}（{}）", record.node, record.name, record.bind);
            store.log_refs(now, Some(network), "proxy.create", &detail, &refs(&record), params(store, &record)?)?;
            Ok::<_, Error>(record)
        })?;
        inner.refresh(network, Seeds::of([record.node]), None)?;
        self.wake_if_needed(&inner);
        inner.proxy_status(network, &record)
    }

    /// 改一个代理：只改写了的字段。可以换到另一个节点上。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个代理；没有出口的代理要启用却没有选出口（`invalid_request`，`what` 是 `required`）；
    /// 其余同 [`Controller::create_proxy`]。
    pub fn update_proxy(&self, network: NetworkId, id: u32, input: UpdateProxy) -> Result<Proxy, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let fresh_salt = salt()?;
        let (before, record) = inner.store.atomically(|store| {
            let before = store.proxy(network, id)?.ok_or(Error::NotFound(Entity::Proxy))?;
            let mut next = before.clone();
            let UpdateProxy { node, name, bind, exit, protocol, username, password, enabled, allow_from, class, optimistic } = input;
            next.node = node.unwrap_or(next.node);
            next.name = name.unwrap_or(next.name);
            next.bind = bind.unwrap_or(next.bind);
            next.exit = exit.or(next.exit);
            next.protocol = protocol.unwrap_or(next.protocol);
            next.username = username.unwrap_or(next.username);
            next.password = password.unwrap_or(next.password);
            next.enabled = enabled.unwrap_or(next.enabled);
            next.allow_from = allow_from.unwrap_or(next.allow_from);
            next.class = class.unwrap_or(next.class);
            next.optimistic = optimistic.unwrap_or(next.optimistic);
            let record = check(store, network, Some(&before), next)?;
            // 用户名或者密码变了就换一个盐：旧的校验值对新的一对没有用，也看不出两次是不是同一个密码。
            let record = if (&record.username, &record.password) == (&before.username, &before.password) {
                record
            } else {
                ProxyRecord { salt: fresh_salt, ..record }
            };
            store.update_proxy(&record)?;
            let detail = format!("修改代理 {}（{}）", record.name, record.bind);
            store.log_refs(now, Some(network), "proxy.update", &detail, &refs(&record), params(store, &record)?)?;
            Ok::<_, Error>((before, record))
        })?;
        inner.refresh(network, Seeds::of([before.node, record.node]), None)?;
        self.wake_if_needed(&inner);
        inner.proxy_status(network, &record)
    }

    /// # Errors
    ///
    /// 这个网络里没有这个代理。
    pub fn delete_proxy(&self, network: NetworkId, id: u32) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let record = inner.store.atomically(|store| {
            let record = store.proxy(network, id)?.ok_or(Error::NotFound(Entity::Proxy))?;
            let params = params(store, &record)?;
            store.delete_proxy(id)?;
            store.log_refs(
                now,
                Some(network),
                "proxy.delete",
                &format!("删除代理 {}（{}）", record.name, record.bind),
                &refs(&record),
                params,
            )?;
            Ok::<_, Error>(record)
        })?;
        inner.refresh(network, Seeds::of([record.node]), None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

impl Inner {
    fn proxy_status(&self, network: NetworkId, record: &ProxyRecord) -> Result<Proxy, Error> {
        let view = NetworkView::load(&self.store, network)?.ok_or(Error::NotFound(Entity::Network))?;
        Ok(self.proxy_view(&view, &view.access(), record))
    }

    /// 管理接口里的一个代理：存着的设置，加上按 controller 知道的算出来的状态和监听节点报的用量。
    pub(crate) fn proxy_view(&self, view: &NetworkView, access: &AccessMap<'_>, record: &ProxyRecord) -> Proxy {
        let report = self.live.get(&record.node).and_then(|live| live.proxies.iter().find(|report| report.id == record.id));
        // 节点报的是它手上那一版的：地址刚改过、它还没换过来时，那份状态说的是旧地址。
        let report = report.filter(|report| report.bind == record.bind);
        Proxy {
            id: record.id,
            name: record.name.clone(),
            node: record.node,
            bind: record.bind,
            protocol: record.protocol,
            exit: record.exit,
            username: record.username.clone(),
            password: record.password.clone(),
            enabled: record.enabled,
            allow_from: record.allow_from.clone(),
            class: record.class,
            optimistic: record.optimistic,
            status: self.status_of(view, access, record, report.map(|report| &report.state)),
            usage: report.map_or(ListenTraffic { connections: 0, bytes_up: 0, bytes_down: 0 }, |report| report.traffic),
        }
    }

    /// 排在前面的先看：没有出口、停用了的不看别的；节点不在线、没有报、监听不上的不看出口；在监听才看出口用不用得了。
    fn status_of(&self, view: &NetworkView, access: &AccessMap<'_>, record: &ProxyRecord, reported: Option<&ListenState>) -> ProxyStatus {
        let Some(exit) = record.exit.and_then(|exit| view.nodes.get(&exit)) else { return ProxyStatus::ExitMissing };
        if !record.enabled {
            return ProxyStatus::Disabled;
        }
        if !self.is_online(record.node) {
            return ProxyStatus::NodeOffline;
        }
        let local = match reported {
            Some(ListenState::Listening { local }) => *local,
            Some(ListenState::PortInUse { detail, cause }) => return ProxyStatus::PortInUse { detail: detail.clone(), cause: *cause },
            Some(ListenState::Failed { detail, cause } | ListenState::Unsupported { detail, cause }) => {
                return ProxyStatus::BindFailed { detail: detail.clone(), cause: *cause };
            }
            // 代理不会监听在虚拟 IP 上；比这个 controller 新的节点报的、不认识的状态也当作还没有报。
            Some(ListenState::Virtual | ListenState::Unknown) | None => return ProxyStatus::Pending,
        };
        if exit.exit.is_none() {
            ProxyStatus::ExitDisabled { local }
        } else if !access.allows(record.node, exit.id) {
            ProxyStatus::AccessDenied { local }
        } else if !self.is_online(exit.id) {
            ProxyStatus::ExitOffline { local }
        } else {
            ProxyStatus::Listening { local }
        }
    }
}

/// 检查一个代理的设置，补上省略的名字和要生成的用户名密码。`before` 是改之前的样子（新建时没有）。
fn check(store: &Store, network: NetworkId, before: Option<&ProxyRecord>, mut record: ProxyRecord) -> Result<ProxyRecord, Error> {
    let node = require_node(store, network, record.node).in_field("node")?;
    if record.bind.port() == 0 {
        return Err(Error::invalid_field("bind", "zero_port", "代理的监听端口不能是 0"));
    }
    let siblings: Vec<ProxyRecord> = store
        .proxies(network)?
        .into_iter()
        .filter(|other| other.node == record.node && Some(other.id) != before.map(|before| before.id))
        .collect();
    record.name = if record.name.trim().is_empty() {
        default_name(&node.name, record.bind.port(), &siblings)
    } else {
        naming::title("代理", &record.name, true).in_field("name")?
    };

    match record.exit {
        Some(exit) if exit == record.node => return Err(Error::ExitIsNode.in_field("exit")),
        Some(exit) => {
            let exit = require_node(store, network, exit).in_field("exit")?;
            // 出口和规则只在会让它生效的那一下查：新建、启用、换了出口或者监听的节点。只改名字、改密码不因为出口后来被停用而被拦住，
            // 那时它照样留着，状态说明为什么用不了。
            let unchanged = before.is_some_and(|before| before.enabled && before.exit == record.exit && before.node == record.node);
            if record.enabled && !unchanged {
                if exit.exit.is_none() {
                    return Err(Error::ExitNotEnabled(exit.name).in_field("exit"));
                }
                let view = NetworkView::load(store, network)?.ok_or(Error::NotFound(Entity::Network))?;
                if !view.access().allows(record.node, exit.id) {
                    return Err(Error::AccessDenied { node: node.name, exit: exit.name }.in_field("exit"));
                }
            }
        }
        None if record.enabled => return Err(Error::invalid_field("exit", "required", "这个代理还没有出口：选好出口才能启用")),
        None => {}
    }

    for (field, value) in [("username", &record.username), ("password", &record.password)] {
        if value.len() > MAX_CREDENTIAL_LEN {
            return Err(Error::invalid_field(field, "too_long", format!("最长 {MAX_CREDENTIAL_LEN} 字节")));
        }
        if value.chars().any(char::is_control) {
            return Err(Error::invalid_field(field, "malformed", "不能有控制字符"));
        }
    }
    // HTTP 的 Basic 认证按第一个冒号切分用户名和密码。
    if record.username.contains(':') {
        return Err(Error::invalid_field("username", "malformed", "用户名里不能有冒号"));
    }
    match (record.username.is_empty(), record.password.is_empty()) {
        (true, false) => return Err(Error::CredentialsIncomplete.in_field("username")),
        (false, true) => return Err(Error::CredentialsIncomplete.in_field("password")),
        // 不在回环上的端口谁都连得到，一定要认证：没写就生成一对，管理员在应答里拿到。
        (true, true) if !record.bind.ip().is_loopback() => (record.username, record.password) = generated_credentials()?,
        (true, true) | (false, false) => {}
    }

    if let Some(other) = siblings.iter().find(|other| other.name == record.name) {
        return Err(Error::NameTaken(other.name.clone()).in_field("name"));
    }
    if let Some(other) = siblings.iter().find(|other| other.bind == record.bind) {
        return Err(Error::Conflict {
            with: Entity::Proxy, message: format!("这个节点上的代理 {} 已经监听在 {}", other.name, other.bind)
        }
        .in_field("bind"));
    }
    Ok(record)
}

/// 没有写名字时的名字：`<节点名>-<端口>`，这个节点上已经有了就接 `-2`、`-3`……
fn default_name(node: &str, port: u16, siblings: &[ProxyRecord]) -> String {
    let base = format!("{node}-{port}");
    let taken = |name: &str| siblings.iter().any(|other| other.name == name);
    std::iter::once(base.clone()).chain((2..).map(|n| format!("{base}-{n}"))).find(|name| !taken(name)).expect("序号用不完")
}

/// 一个新的盐。
fn salt() -> Result<[u8; Verifier::SALT_LEN], Error> {
    let mut salt = [0; Verifier::SALT_LEN];
    SystemRandom::new().fill(&mut salt).map_err(|_| Error::Random)?;
    Ok(salt)
}

/// 生成的用户名（`u-` 加 8 个十六进制字符）和密码（32 个十六进制字符，128 位）。只用字母和数字，放进代理的网址里不用转义。
/// 升级时数据库迁移给要求过凭据的监听器生成的是同样的写法。
fn generated_credentials() -> Result<(String, String), Error> {
    let random = SystemRandom::new();
    let mut name = [0; 4];
    let mut secret = [0; 16];
    random.fill(&mut name).map_err(|_| Error::Random)?;
    random.fill(&mut secret).map_err(|_| Error::Random)?;
    let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    Ok((format!("u-{}", hex(&name)), hex(&secret)))
}

/// 操作记录里一个代理说到的东西：它自己、监听的节点、出口。
fn refs(record: &ProxyRecord) -> Vec<LogRef> {
    let mut refs = vec![LogRef::new(RefKind::Proxy, record.id), LogRef::node(record.node)];
    refs.extend(record.exit.map(LogRef::node));
    refs
}

/// 操作记录里一个代理的参数：节点带着当时的名字。密码不记。
fn params(store: &Store, record: &ProxyRecord) -> Result<Value, Error> {
    let name = |node: Option<NodeId>| Ok::<_, Error>(node.map(|node| store.node(node)).transpose()?.flatten().map(|node| node.name));
    Ok(json!({
        "proxy": record.id,
        "proxy_name": record.name,
        "node": record.node,
        "node_name": name(Some(record.node))?,
        "bind": record.bind,
        "protocol": record.protocol,
        "exit": record.exit,
        "exit_name": name(record.exit)?,
        "enabled": record.enabled,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_credentials_are_plain_and_differ() {
        let (first, second) = (generated_credentials().unwrap(), generated_credentials().unwrap());
        assert!(first.0.starts_with("u-") && first.0.len() == 10, "{}", first.0);
        assert_eq!(first.1.len(), 32);
        assert!(first.1.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn a_default_name_is_the_node_and_port_with_a_number_when_taken() {
        let proxy = |name: &str| ProxyRecord {
            id: 1,
            node: NodeId::new(1).unwrap(),
            name: name.to_owned(),
            bind: "127.0.0.1:1".parse().unwrap(),
            protocol: Default::default(),
            exit: None,
            username: String::new(),
            password: String::new(),
            salt: [0; Verifier::SALT_LEN],
            enabled: false,
            allow_from: Vec::new(),
            class: None,
            optimistic: None,
        };
        assert_eq!(default_name("gw", 1080, &[]), "gw-1080");
        assert_eq!(default_name("gw", 1080, &[proxy("gw-1080"), proxy("gw-1080-2")]), "gw-1080-3");
    }
}
