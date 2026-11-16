//! 端口转发的增删改，和每条转发现在的状态。

use i3t_model::admin::{Forward, ForwardInput, ForwardStatus, NetworkId};

use super::nodes::{is_connectable, require_node};
use super::{Controller, Inner, Seeds, naming};
use crate::access::AccessMap;
use crate::error::{Capability, Entity, Error};
use crate::netmap::NetworkView;
use crate::store::{ForwardRecord, Store};

impl Controller {
    /// 一个网络的全部端口转发和它们现在的状态。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn forwards(&self, network: NetworkId) -> Result<Vec<Forward>, Error> {
        let inner = self.lock();
        let view = NetworkView::load(&inner.store, network)?.ok_or(Error::NotFound(Entity::Network))?;
        let access = view.access();
        Ok(view.forwards.iter().map(|record| inner.forward_view(&view, &access, record)).collect())
    }

    /// # Errors
    ///
    /// 两个节点不都在这个网络里；要启用而目标节点没有声明可以作为端口转发的目标；
    /// 地址不合法；监听节点上同一个地址已经有一条同协议的转发。
    pub fn create_forward(&self, network: NetworkId, input: ForwardInput) -> Result<Forward, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let record = inner.store.atomically(|store| {
            let config = check(store, network, None, input)?;
            let id = store.insert_forward(network, &config)?;
            store.log(now, Some(network), "forward.create", &describe(id, &config))?;
            Ok::<_, Error>(ForwardRecord { id, config })
        })?;
        inner.refresh(network, Seeds::of([record.config.listen_node, record.config.target_node]), None)?;
        self.wake_if_needed(&inner);
        inner.forward_status(network, &record)
    }

    /// 整个替换一条转发。启用和停用也走这里。
    ///
    /// # Errors
    ///
    /// 这个网络里没有这条转发；其余同 [`Controller::create_forward`]。
    pub fn replace_forward(&self, network: NetworkId, id: u32, input: ForwardInput) -> Result<Forward, Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let (before, record) = inner.store.atomically(|store| {
            let before = store.forward(network, id)?.ok_or(Error::NotFound(Entity::Forward))?;
            let config = check(store, network, Some(id), input)?;
            store.update_forward(id, &config)?;
            store.log(now, Some(network), "forward.update", &describe(id, &config))?;
            Ok::<_, Error>((before, ForwardRecord { id, config }))
        })?;
        let ends = [before.config.listen_node, before.config.target_node, record.config.listen_node, record.config.target_node];
        inner.refresh(network, Seeds::of(ends), None)?;
        self.wake_if_needed(&inner);
        inner.forward_status(network, &record)
    }

    /// # Errors
    ///
    /// 这个网络里没有这条转发。
    pub fn delete_forward(&self, network: NetworkId, id: u32) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let record = inner.store.atomically(|store| {
            let record = store.forward(network, id)?.ok_or(Error::NotFound(Entity::Forward))?;
            store.delete_forward(id)?;
            store.log(now, Some(network), "forward.delete", &describe(id, &record.config))?;
            Ok::<_, Error>(record)
        })?;
        inner.refresh(network, Seeds::of([record.config.listen_node, record.config.target_node]), None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

impl Inner {
    fn forward_status(&self, network: NetworkId, record: &ForwardRecord) -> Result<Forward, Error> {
        let view = NetworkView::load(&self.store, network)?.ok_or(Error::NotFound(Entity::Network))?;
        Ok(self.forward_view(&view, &view.access(), record))
    }

    fn forward_view(&self, view: &NetworkView, access: &AccessMap<'_>, record: &ForwardRecord) -> Forward {
        let config = &record.config;
        let declared = view.nodes.get(&config.target_node).is_some_and(|target| target.declared.forward);
        let status = if !declared {
            ForwardStatus::TargetNotDeclared
        } else if !config.enabled {
            ForwardStatus::Disabled
        } else if !view.forward_is_deliverable(access, record) {
            ForwardStatus::AccessDenied
        } else if !self.is_online(config.listen_node) {
            ForwardStatus::ListenerOffline
        } else if !self.is_online(config.target_node) {
            ForwardStatus::TargetOffline
        } else {
            ForwardStatus::Active
        };
        Forward {
            id: record.id,
            name: config.name.clone(),
            protocol: config.protocol,
            listen_node: config.listen_node,
            bind: config.bind,
            target_node: config.target_node,
            target_addr: config.target_addr,
            allow_from: config.allow_from.clone(),
            enabled: config.enabled,
            status,
        }
    }
}

/// 检查一条转发的配置。`replacing` 是正在被替换的那一条，查冲突时不和它自己比。
fn check(store: &Store, network: NetworkId, replacing: Option<u32>, mut config: ForwardInput) -> Result<ForwardInput, Error> {
    config.name = naming::title("端口转发", &config.name, false)?;
    require_node(store, network, config.listen_node)?;
    let target = require_node(store, network, config.target_node)?;
    // 目标节点要自己声明过才行：controller 不能把一台没有声明的机器的回环和局域网转发出去。
    if config.enabled && !target.declared.forward {
        return Err(Error::NotDeclared { node: target.name, capability: Capability::Forward });
    }
    if config.bind.port() == 0 {
        return Err(Error::invalid("监听端口不能是 0"));
    }
    if !is_connectable(&config.target_addr) {
        return Err(Error::invalid(format!("目标地址 {} 不是一个能连上的地址", config.target_addr)));
    }
    let clash = store.forwards(network)?.into_iter().find(|other| {
        Some(other.id) != replacing
            && other.config.listen_node == config.listen_node
            && other.config.protocol == config.protocol
            && other.config.bind == config.bind
    });
    match clash {
        Some(other) => Err(Error::Conflict(format!("监听节点上的转发 #{} 已经监听在 {}", other.id, other.config.bind))),
        None => Ok(config),
    }
}

fn describe(id: u32, config: &ForwardInput) -> String {
    let state = if config.enabled { "启用" } else { "停用" };
    format!(
        "转发 #{id}：节点 #{} 的 {} → 节点 #{} 的 {}（{state}）",
        config.listen_node, config.bind, config.target_node, config.target_addr
    )
}
