//! 独占 TCP（lane）在节点里的接法。
//!
//! - 入口和出口用的连接层是共用的连接（[`Endpoint`]）和 lane 装在一起的（[`LaneConnector`]、[`LaneAcceptor`]）：
//!   承载 TCP 连接的流按节点对的设置走 lane，别的流和数据报一律走共用的连接；两种流进同一个 `serve`，
//!   计量、配额、带宽、凭据、每个对端的上限、访问规则和排空对它们一视同仁。
//! - 跟着 netmap：设置不是 `off` 的对端交给 [`Lanes::set_peers`]。本节点拨的是 netmap 里启用了 relay、
//!   有 TCP 地址的对端（relay 在节点端口上听 TCP）；本节点自己是 relay 时，relay 的 TCP 端口把以 TLS 握手开头的连接交给 lane。
//!   两边都不能被拨的一对当作 `off`：开不出 lane 的流不该被记成"改走了共用的连接"。
//! - 两个节点都能拨对方时，各自用自己拨的 lane 开流：补 lane 是本地的事，不用经过对方的 POOL 往返，
//!   每边的池跟着它自己开流的速率走；代价是两边各有一条控制 lane。
//! - `udp_fallback`：开流的一方看共用连接到对端的路上有没有 UDP（问路径层），没有才走 lane。
//!   拨号方不管这个，照样拨着控制 lane 和几条空闲的：UDP 断掉的那一刻 lane 已经是热的，
//!   而且经别的 relay 时拨号方只看得见自己那一段，判断不了对方那一段。
//! - 本节点的公网地址变了（家宽重拨）：拨出去的 lane 换新的 epoch 重拨（[`Lanes::address_changed`]）。
//! - 排空：告诉对端不再在和本节点之间的 lane 上开流（[`Lanes::go_away`]）；排空撤销了就恢复（[`Lanes::come_back`]）。

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use i3t_conn::Endpoint;
use i3t_crypto::NodeKey;
use i3t_lane::{Choose, LaneAcceptor, LaneConfig, LaneConnector, LanePeer, LaneStatus, Lanes};
use i3t_model::Netmap;
use i3t_model::control::{LaneCounts, LaneHandshakeFailures, LaneReport, PeerLanes};
use i3t_model::netmap::LaneMode;
use i3t_proto::NodeId;
use tokio::sync::watch;

/// 入口和出口用的连接层。
pub(crate) type Connector = LaneConnector<Endpoint>;
/// 接受流的连接层。
pub(crate) type Acceptor = LaneAcceptor<Endpoint>;

/// 多久看一次本节点的公网地址变了没有。
const ADDRESS_CHECK: Duration = Duration::from_secs(1);

/// 一个节点上的 lane 和它们的设置。
pub(crate) struct NodeLanes {
    lanes: Arc<Lanes>,
    endpoint: Arc<Endpoint>,
    /// 每个打开了 lane、并且至少一边能被拨的对端的设置。开流时按它选。
    modes: Arc<RwLock<HashMap<NodeId, LaneMode>>>,
    /// relay 的 TCP 端口现在是不是把 lane 交过来。
    handoff: std::sync::Mutex<bool>,
}

impl NodeLanes {
    /// # Errors
    ///
    /// 用节点密钥签 lane 的证书失败。
    pub(crate) fn new(key: &NodeKey, endpoint: Arc<Endpoint>) -> anyhow::Result<Self> {
        let lanes = Lanes::new(key, LaneConfig::default()).context("用节点密钥给独占 TCP 签证书")?;
        Ok(Self { lanes: Arc::new(lanes), endpoint, modes: Arc::default(), handoff: std::sync::Mutex::new(false) })
    }

    /// 入口和出口开流用的连接层。
    pub(crate) fn connector(&self) -> Arc<Connector> {
        Arc::new(LaneConnector::new(Arc::clone(&self.endpoint), Arc::clone(&self.lanes), self.choose()))
    }

    /// 接受流用的连接层。
    pub(crate) fn acceptor(&self) -> Arc<Acceptor> {
        Arc::new(LaneAcceptor::new(Arc::clone(&self.endpoint), Arc::clone(&self.lanes)))
    }

    /// 一条到 `peer` 的 TCP 流该不该走 lane。
    fn choose(&self) -> Choose {
        let (modes, endpoint) = (Arc::clone(&self.modes), Arc::clone(&self.endpoint));
        Arc::new(move |peer| {
            let mode = modes.read().expect("lane modes lock poisoned").get(&peer).copied();
            match mode {
                Some(LaneMode::Always) => true,
                Some(LaneMode::UdpFallback) => !endpoint.path_has_udp(peer),
                Some(LaneMode::Off) | None => false,
            }
        })
    }

    /// 按 netmap 设置要开 lane 的对端、本节点拨谁，以及 relay 的 TCP 端口交不交 lane。
    pub(crate) fn apply(&self, netmap: &Netmap) {
        let plan = plan(netmap);
        let modes = plan.iter().map(|(peer, mode)| (peer.id, *mode)).collect();
        let peers: Vec<LanePeer> = plan.into_iter().map(|(peer, _)| peer).collect();
        // 只在本节点是 relay、并且真有对端要拨过来时才让 relay 分辨：不用 lane 的节点上 relay 的 TCP 端口和原来一模一样。
        let handoff = netmap.me.relay.is_some() && !peers.is_empty();
        if let Err(error) = self.lanes.set_peers(peers) {
            // 签不了 TLS 配置是本机的问题，不会因为重试好起来；保留原来的设置，流照常走共用的连接。
            tracing::error!(%error, "独占 TCP 的设置没能生效");
            return;
        }
        *self.modes.write().expect("lane modes lock poisoned") = modes;
        let mut current = self.handoff.lock().expect("lane handoff mutex poisoned");
        if *current != handoff {
            *current = handoff;
            self.endpoint.set_lane_handoff(handoff.then(|| {
                let lanes = Arc::clone(&self.lanes);
                Arc::new(move |stream, from| lanes.accept_tcp(stream, from, &[])) as i3t_conn::TcpHandoff
            }));
        }
    }

    /// 跟着 netmap 改设置，盯着本节点的公网地址，直到 netmap 没有人再发。
    pub(crate) async fn follow(self: Arc<Self>, mut netmap: watch::Receiver<Arc<Netmap>>) {
        let first = Arc::clone(&netmap.borrow_and_update());
        self.apply(&first);
        let mut addresses = PublicAddresses::default();
        let mut tick = tokio::time::interval(ADDRESS_CHECK);
        loop {
            tokio::select! {
                changed = netmap.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let current = Arc::clone(&netmap.borrow_and_update());
                    self.apply(&current);
                }
                _ = tick.tick() => {
                    if addresses.changed(&self.endpoint) {
                        self.lanes.address_changed();
                    }
                }
            }
        }
    }

    /// 排空开始或者撤销。
    pub(crate) fn set_draining(&self, draining: bool) {
        if draining {
            self.lanes.go_away();
        } else {
            self.lanes.come_back();
        }
    }

    pub(crate) fn status(&self) -> LaneStatus {
        self.lanes.status()
    }

    /// lane 的状况写成上报的样子，带上每个对端的共用连接现在有没有 UDP 的路（`udp_fallback` 按它决定开流走不走 lane）。
    pub(crate) fn report(&self) -> LaneReport {
        report(&self.lanes.status(), |peer| Some(self.endpoint.path_has_udp(peer)))
    }
}

/// 要开 lane 的对端和它们的设置。两边都不能被拨的对端不在里面。
fn plan(netmap: &Netmap) -> Vec<(LanePeer, LaneMode)> {
    let me_relay = netmap.me.relay.is_some();
    let wanted = netmap.peers.iter().filter(|peer| peer.lane != LaneMode::Off);
    wanted
        .filter_map(|peer| {
            let dial: Vec<_> = netmap.relays.iter().find(|relay| relay.id == peer.id).map(|relay| relay.tcp.clone()).unwrap_or_default();
            (me_relay || !dial.is_empty()).then_some((LanePeer { id: peer.id, key: peer.key, dial }, peer.lane))
        })
        .collect()
}

/// 本节点的公网 IP（端口不算：TCP 的源端口每条连接都不一样）。换了一批才算变了，暂时一个都不知道不算。
#[derive(Debug, Default)]
struct PublicAddresses {
    known: BTreeSet<IpAddr>,
}

impl PublicAddresses {
    fn changed(&mut self, endpoint: &Endpoint) -> bool {
        let now: BTreeSet<IpAddr> = endpoint.public_addresses().iter().map(|addr| addr.ip().to_canonical()).collect();
        if now.is_empty() || now == self.known {
            return false;
        }
        let first = self.known.is_empty();
        self.known = now;
        !first
    }
}

/// lane 的状况写成上报的样子。`udp_path` 说到一个对端的共用连接现在有没有 UDP 的路，不知道时是 `None`。
pub(crate) fn report(status: &LaneStatus, udp_path: impl Fn(NodeId) -> Option<bool>) -> LaneReport {
    let failures = &status.handshake_failures;
    LaneReport {
        total: counts(&status.total),
        handshake_failures: LaneHandshakeFailures {
            deadline: failures.deadline,
            unknown_key: failures.unknown_key,
            tls: failures.tls,
            protocol: failures.protocol,
            busy: failures.busy,
        },
        peers: status
            .peers
            .iter()
            .map(|peer| PeerLanes {
                peer: peer.peer,
                dials: peer.dials,
                control: peer.control,
                idle: peer.idle,
                ready: peer.ready,
                parked_at_peer: peer.parked_at_peer,
                udp_path: udp_path(peer.peer),
                counts: counts(&peer.counts),
            })
            .collect(),
    }
}

fn counts(counts: &i3t_lane::LaneCounts) -> LaneCounts {
    LaneCounts {
        in_use: counts.in_use,
        dialling: counts.dialling,
        claims: counts.claims,
        claims_waited: counts.claims_waited,
        claim_wait_ms: u64::try_from(counts.claim_wait.as_millis()).unwrap_or(u64::MAX),
        claims_failed: counts.claims_failed,
        fallbacks: counts.fallbacks,
        dial_failures: counts.dial_failures,
        refused: counts.refused,
        sealed_bytes: counts.sealed_bytes,
        passthrough_bytes: counts.passthrough_bytes,
        spliced_bytes: counts.spliced_bytes,
    }
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::{Peer, Relay, RelayRole};
    use i3t_proto::PublicKey;

    use super::*;
    use crate::control::testing::netmap;

    fn with_peer(netmap: &mut Netmap, raw: u8, lane: LaneMode) -> NodeId {
        let id = NodeId::new(u32::from(raw)).unwrap();
        netmap.peers.push(Peer {
            id,
            key: PublicKey::from_bytes([raw; 32]),
            name: format!("node-{raw}"),
            virtual_ip: std::net::Ipv4Addr::new(10, 66, 0, raw),
            inbound: true,
            outbound: true,
            endpoints: Vec::new(),
            attached: Vec::new(),
            exit: None,
            dial_out_only: false,
            keep_alive: false,
            lane,
        });
        id
    }

    fn relay(netmap: &mut Netmap, id: NodeId, tcp: &[&str]) {
        netmap.relays.push(Relay {
            id,
            key: PublicKey::from_bytes([9; 32]),
            name: format!("relay-{id}"),
            udp: Vec::new(),
            tcp: tcp.iter().map(|addr| addr.parse().unwrap()).collect(),
        });
    }

    #[test]
    fn only_peers_with_lanes_on_that_one_side_can_dial_are_planned() {
        let mut map = netmap(1);
        let off = with_peer(&mut map, 2, LaneMode::Off);
        let public = with_peer(&mut map, 3, LaneMode::Always);
        let nobody = with_peer(&mut map, 4, LaneMode::UdpFallback);
        let relay_without_tcp = with_peer(&mut map, 5, LaneMode::Always);
        relay(&mut map, off, &["203.0.113.2:20330"]);
        relay(&mut map, public, &["203.0.113.3:20330"]);
        relay(&mut map, relay_without_tcp, &[]);

        let planned: Vec<_> = plan(&map).into_iter().map(|(peer, mode)| (peer.id, peer.dial.len(), mode)).collect();
        assert_eq!(planned, vec![(public, 1, LaneMode::Always)], "两边都不能被拨的对端当作 off");

        // 本节点自己是 relay：别人拨过来，不能被拨的对端也要登记（只接受它拨来的）。
        map.me.relay = Some(RelayRole::default());
        let planned: Vec<_> = plan(&map).into_iter().map(|(peer, mode)| (peer.id, peer.dial.len(), mode)).collect();
        assert_eq!(
            planned,
            vec![(public, 1, LaneMode::Always), (nobody, 0, LaneMode::UdpFallback), (relay_without_tcp, 0, LaneMode::Always)]
        );
    }
}
