//! 代理监听器的增删改。

use i3t_model::admin::{Listener, ListenerInput, LogRef, NetworkId, RefKind};
use i3t_model::netmap::DefaultAction;
use i3t_proto::NodeId;

use super::nodes::require_node;
use super::{Controller, Inner, Seeds, naming};
use crate::error::{Entity, Error};
use crate::store::Store;

impl Controller {
    /// 一个网络里的监听器；给了 `node` 时只列那个节点上的。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn listeners(&self, network: NetworkId, node: Option<NodeId>) -> Result<Vec<Listener>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        let listeners = inner.store.listeners(network)?.into_iter().filter(|listener| node.is_none_or(|node| listener.node == node));
        Ok(listeners.map(|listener| inner.with_status(listener)).collect())
    }

    /// # Errors
    ///
    /// 节点不在这个网络里；名字或监听地址和同一个节点上的另一个监听器冲突；
    /// 绑定在非回环地址上却不要求凭据；引用的出口节点不在这个网络里。
    pub fn create_listener(&self, network: NetworkId, input: ListenerInput) -> Result<Listener, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let listener = inner.store.atomically(|store| {
            let input = check(store, network, None, input)?;
            let id = store.insert_listener(&input)?;
            store.log_refs(
                now,
                Some(network),
                "listener.create",
                &format!("节点 #{} 上新建监听器 {}（{}）", input.node, input.config.name, input.config.bind),
                &[LogRef::new(RefKind::Listener, id), LogRef::node(input.node)],
            )?;
            Ok::<_, Error>(Listener { id, node: input.node, config: input.config, status: None })
        })?;
        inner.refresh(network, Seeds::of([listener.node]), None)?;
        self.wake_if_needed(&inner);
        Ok(inner.with_status(listener))
    }

    /// 整个替换一个监听器。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这个监听器；其余同 [`Controller::create_listener`]。
    pub fn replace_listener(&self, network: NetworkId, id: u32, input: ListenerInput) -> Result<Listener, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let (before, listener) = inner.store.atomically(|store| {
            let before = store.listener(network, id)?.ok_or(Error::NotFound(Entity::Listener))?;
            let input = check(store, network, Some(id), input)?;
            store.update_listener(id, &input)?;
            let detail = format!("修改监听器 {}（{}）", input.config.name, input.config.bind);
            store.log_refs(
                now,
                Some(network),
                "listener.update",
                &detail,
                &[LogRef::new(RefKind::Listener, id), LogRef::node(input.node)],
            )?;
            Ok::<_, Error>((before, Listener { id, node: input.node, config: input.config, status: None }))
        })?;
        inner.refresh(network, Seeds::of([before.node, listener.node]), None)?;
        self.wake_if_needed(&inner);
        Ok(inner.with_status(listener))
    }

    /// # Errors
    ///
    /// 这个网络里没有这个监听器。
    pub fn delete_listener(&self, network: NetworkId, id: u32) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let listener = inner.store.atomically(|store| {
            let listener = store.listener(network, id)?.ok_or(Error::NotFound(Entity::Listener))?;
            store.delete_listener(id)?;
            store.log_refs(
                now,
                Some(network),
                "listener.delete",
                &format!("删除监听器 {}（{}）", listener.config.name, listener.config.bind),
                &[LogRef::new(RefKind::Listener, id), LogRef::node(listener.node)],
            )?;
            Ok::<_, Error>(listener)
        })?;
        inner.refresh(network, Seeds::of([listener.node]), None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

impl Inner {
    /// 带上节点最近一次报的这个监听器的状态。
    fn with_status(&self, listener: Listener) -> Listener {
        let status = self.listener_status(listener.node, &listener.config.name);
        Listener { status, ..listener }
    }
}

/// 检查一个监听器的配置。`replacing` 是正在被替换的那个监听器，查冲突时不和它自己比。
fn check(store: &Store, network: NetworkId, replacing: Option<u32>, mut input: ListenerInput) -> Result<ListenerInput, Error> {
    require_node(store, network, input.node)?;
    let config = &mut input.config;
    config.name = naming::title("监听器", &config.name, true)?;
    if config.bind.port() == 0 {
        return Err(Error::invalid("监听端口不能是 0"));
    }
    // 回环上的入口只服务本机。别的地址上谁都连得到，调用者是谁只能靠凭据认。
    if !config.bind.ip().is_loopback() && !config.require_credential {
        return Err(Error::invalid(format!("监听地址 {} 不是回环地址，这样的监听器必须要求凭据（require_credential）", config.bind)));
    }
    let exits = [
        config.fixed_exit,
        config.default_action.and_then(|action| match action {
            DefaultAction::Exit { node } => Some(node),
            DefaultAction::Local | DefaultAction::Reject => None,
        }),
    ];
    for exit in exits.into_iter().flatten() {
        require_node(store, network, exit)?;
    }
    let siblings = store.listeners(network)?.into_iter().filter(|other| other.node == input.node && Some(other.id) != replacing);
    for other in siblings {
        if other.config.name == input.config.name {
            return Err(Error::NameTaken(other.config.name));
        }
        if other.config.bind == input.config.bind {
            return Err(Error::Conflict(format!("这个节点上的监听器 {} 已经监听在 {}", other.config.name, other.config.bind)));
        }
    }
    Ok(input)
}
