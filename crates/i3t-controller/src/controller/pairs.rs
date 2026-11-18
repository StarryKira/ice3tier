//! 节点对设置：连接常驻、ice3proxy（lane）和拥塞控制。

use i3t_model::admin::{LaneBlock, LaneEffect, LaneOpening, LogRef, NetworkId, PairInput, PairLaneSide, PairSetting, PairView, action};
use i3t_model::control::LANE_REVISION;
use i3t_model::netmap::{Congestion, LaneMode};
use i3t_proto::NodeId;
use serde_json::{Value, json};

use super::nodes::require_node;
use super::{Controller, Inner, Seeds, switch};
use crate::error::{Entity, Error, InField};
use crate::node_addresses;
use crate::store::{NodeRecord, PairOptions, Store};

impl Controller {
    /// 一个网络里全部的节点对设置，带着 ice3proxy 起不起作用、两边报上来的状况。
    ///
    /// # Errors
    ///
    /// 没有这个网络，或者读存储失败。
    pub fn pairs(&self, network: NetworkId) -> Result<Vec<PairView>, Error> {
        let inner = self.lock();
        inner.require_network(network)?;
        let settings = inner.store.pairs(network)?;
        settings.into_iter().map(|setting| inner.pair_view(network, setting)).collect()
    }

    /// 改一对节点的设置：请求里写了的项改掉，没写的不变（这一对还没有设置时取默认值）。两个节点的先后顺序无所谓。
    ///
    /// # Errors
    ///
    /// 两个节点不都在这个网络里，或者是同一个节点。
    pub fn set_pair(&self, network: NetworkId, a: NodeId, b: NodeId, input: PairInput) -> Result<PairView, Error> {
        if a == b {
            return Err(Error::invalid_field("b", "same_node", "一对节点得是两个不同的节点"));
        }
        let mut inner = self.lock();
        let now = inner.clock.now();
        let setting = inner.store.atomically(|store| {
            require_node(store, network, a).in_field("a")?;
            require_node(store, network, b).in_field("b")?;
            let before = store.pair(a, b)?;
            let keep_alive = input.keep_alive.unwrap_or(before.as_ref().is_some_and(|pair| pair.keep_alive));
            let lane = input.lane.unwrap_or(before.as_ref().map_or(LaneMode::Off, |pair| pair.lane));
            let congestion = input.congestion.unwrap_or(before.as_ref().and_then(|pair| pair.congestion));
            store.set_pair(a, b, PairOptions { keep_alive, lane, congestion })?;
            let lane_text = match lane {
                LaneMode::Off => "不用",
                LaneMode::UdpFallback => "UDP 不通时用",
                LaneMode::Always => "总是用",
            };
            let congestion_text = congestion.map_or("节点的默认", Congestion::as_str);
            let detail = format!("节点 #{a} 和 #{b}：连接常驻 {}，ice3proxy {lane_text}，拥塞控制 {congestion_text}", switch(keep_alive));
            let params = json!({ "keep_alive": keep_alive, "lane": lane, "congestion": congestion });
            store.log_refs(
                now,
                Some(network),
                action::PAIR_SET,
                &detail,
                &[LogRef::node(a), LogRef::node(b)],
                pair_params(store, a, b, params)?,
            )?;
            Ok::<_, Error>(PairSetting { a: a.min(b), b: a.max(b), keep_alive, lane, congestion })
        })?;
        inner.refresh(network, Seeds::of([a, b]), None)?;
        self.wake_if_needed(&inner);
        inner.pair_view(network, setting)
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
            store.log_refs(
                now,
                Some(network),
                action::PAIR_DELETE,
                &format!("节点 #{a} 和 #{b}：去掉节点对设置"),
                &[LogRef::node(a), LogRef::node(b)],
                pair_params(store, a, b, json!({}))?,
            )?;
            Ok(())
        })?;
        inner.refresh(network, Seeds::of([a, b]), None)?;
        self.wake_if_needed(&inner);
        Ok(())
    }
}

/// 操作记录里一对节点的参数：`a` 是编号小的那个，两个都带着当时的名字；`extra` 里的项加在后面。
fn pair_params(store: &Store, a: NodeId, b: NodeId, extra: Value) -> Result<Value, Error> {
    let (a, b) = (a.min(b), a.max(b));
    let name = |node| Ok::<_, Error>(store.node(node)?.map(|node| node.name));
    let mut params = json!({ "a": a, "a_name": name(a)?, "b": b, "b_name": name(b)? });
    if let (Value::Object(params), Value::Object(extra)) = (&mut params, extra) {
        params.extend(extra);
    }
    Ok(params)
}

impl Inner {
    fn pair_view(&self, network: NetworkId, setting: PairSetting) -> Result<PairView, Error> {
        let lane_effect = match setting.lane {
            LaneMode::Off => None,
            LaneMode::UdpFallback | LaneMode::Always => Some(self.lane_effect(network, &setting)?),
        };
        // 两边各自报的、和对方之间的 lane。设置是 off 的一对也报（刚关掉时还有在跑的流）。
        let side = |node: NodeId, other: NodeId| {
            let live = self.live.get(&node)?;
            let lanes = live.lanes.as_ref()?.peers.iter().find(|peer| peer.peer == other)?.clone();
            Some(PairLaneSide { node, lanes, reported_secs_ago: live.reported_at.map(|at| at.elapsed().as_secs()) })
        };
        let lanes = [side(setting.a, setting.b), side(setting.b, setting.a)].into_iter().flatten().collect();
        Ok(PairView { setting, lane_effect, lanes })
    }

    /// ice3proxy 的设置在这一对节点上起不起作用。
    fn lane_effect(&self, network: NetworkId, setting: &PairSetting) -> Result<LaneEffect, Error> {
        let (a, b) = (self.require_node(network, setting.a)?, self.require_node(network, setting.b)?);
        let cidr = self.require_network(network)?.cidr;
        let mut blocked = Vec::new();
        // 能拨对方的一边：对方启用了 relay（在它的端口上听 TCP），并且有已知的地址（controller 找出来的，见 `node_addresses`）。
        let has_address = |node: &NodeRecord| !node_addresses::relay_addresses(node, cidr).is_empty();
        let dialable = |node: &NodeRecord| node.relay.is_some() && has_address(node);
        let mut dialers = Vec::new();
        if dialable(&b) {
            dialers.push(a.id);
        }
        if dialable(&a) {
            dialers.push(b.id);
        }
        if a.relay.is_none() && b.relay.is_none() {
            blocked.push(LaneBlock::NoRelay);
        }
        for node in [&a, &b] {
            if node.relay.is_some() && !has_address(node) && dialers.is_empty() {
                blocked.push(LaneBlock::NoTcpAddress { node: node.id });
            }
        }
        for node in [&a, &b] {
            // 只有在线的节点知道它说的协议是第几版；离线的不拦，等它上线再说。
            if self.live.get(&node.id).is_some_and(|live| live.protocol < LANE_REVISION) {
                blocked.push(LaneBlock::NodeTooOld { node: node.id });
            }
        }
        let view = self.view(network)?;
        let access = view.access();
        let connected = access.rules_allowing(a.id, b.id).next().is_some() || access.rules_allowing(b.id, a.id).next().is_some();
        if !connected {
            blocked.push(LaneBlock::NotConnected);
        }
        let effective = blocked.is_empty();
        let summary = if effective {
            let dialers = dialers.iter().map(|id| format!("#{id}")).collect::<Vec<_>>().join("、");
            format!(
                "由 {dialers} 拨对方的 TCP 端口；{when}新的 TCP 流（代理的 CONNECT、TCP 端口转发）每条独占一条 TCP，取不到时悄悄改走共用的连接。\
                 这一对节点之间共用的连接和它的路照常保持：UDP 应用的数据和别的流走它，路由视图显示的是它的路；走 ice3proxy 的流不换路。",
                when = if setting.lane == LaneMode::UdpFallback { "共用的连接到对方没有 UDP 的路时，" } else { "" }
            )
        } else {
            "设置不起作用，这一对节点的流都走共用的连接（见 blocked）。".to_owned()
        };
        let opening = if setting.lane == LaneMode::UdpFallback { self.lane_opening(setting, effective) } else { Vec::new() };
        Ok(LaneEffect { effective, blocked, dialers, summary, opening })
    }

    /// `udp_fallback` 时两个节点各自开新的 TCP 流现在走不走 lane：按它自己报的"到对方的共用连接上有没有 UDP 的路"。
    fn lane_opening(&self, setting: &PairSetting, effective: bool) -> Vec<LaneOpening> {
        let side = |node: NodeId, other: NodeId| {
            let report = self.live.get(&node)?.lanes.as_ref()?;
            let udp_path = report.peers.iter().find(|peer| peer.peer == other)?.udp_path?;
            Some(LaneOpening { node, udp_path, lanes: effective && !udp_path })
        };
        [side(setting.a, setting.b), side(setting.b, setting.a)].into_iter().flatten().collect()
    }
}
