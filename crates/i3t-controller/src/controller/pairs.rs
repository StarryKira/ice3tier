//! 节点对设置。

use i3t_model::admin::{NetworkId, PairInput, PairSetting};
use i3t_proto::NodeId;

use super::nodes::require_node;
use super::{Controller, Seeds};
use crate::error::{Entity, Error};

impl Controller {
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn pairs(&self, network: NetworkId) -> Result<Vec<PairSetting>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        Ok(inner.store.pairs(network)?)
    }

    /// 写下一对节点的设置，已经有就覆盖。两个节点的先后顺序无所谓。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，或者是同一个节点。
    pub fn set_pair(&self, network: NetworkId, a: NodeId, b: NodeId, input: PairInput) -> Result<PairSetting, Error> {
        if a == b {
            return Err(Error::invalid("一对节点得是两个不同的节点"));
        }
        let mut inner = self.lock();
        let now = inner.clock.now();
        inner.store.atomically(|store| {
            require_node(store, network, a)?;
            require_node(store, network, b)?;
            store.set_pair(a, b, input.keep_alive)?;
            store.log(now, Some(network), "pair.set", &format!("节点 #{a} 和 #{b}：连接常驻 {}", input.keep_alive))?;
            Ok::<_, Error>(())
        })?;
        inner.refresh(network, Seeds::of([a, b]), None)?;
        self.wake_if_needed(&inner);
        Ok(PairSetting { a: a.min(b), b: a.max(b), keep_alive: input.keep_alive })
    }

    /// 去掉一对节点的设置，回到默认。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，或者这一对没有设置。
    pub fn delete_pair(&self, network: NetworkId, a: NodeId, b: NodeId) -> Result<(), Error> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        inner.store.atomically(|store| {
            require_node(store, network, a)?;
            require_node(store, network, b)?;
            if !store.delete_pair(a, b)? {
                return Err(Error::NotFound(Entity::PairSetting));
            }
            store.log(now, Some(network), "pair.delete", &format!("节点 #{a} 和 #{b}：去掉节点对设置"))?;
            Ok(())
        })?;
        inner.refresh(network, Seeds::of([a, b]), None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}
