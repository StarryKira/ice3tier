//! relay 上的会话表：哪个节点现在挂在哪个地址（或者哪条 TCP）上。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use i3t_proto::NodeId;
use i3t_underlay::tcp_leg::FrameSender;

/// 会话多久收不到 PING 就删掉。节点每 5 秒发一次，中间丢几个也够不着这个数。
pub(crate) const SESSION_TTL: Duration = Duration::from_secs(30);

/// 一个数据报是从哪里到 relay 的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Udp(SocketAddr),
    /// relay 给每条接进来的 TCP 编的号。
    Tcp(u64),
}

/// 会话绑定在什么上面，也就是发给这个节点的包往哪里送。
#[derive(Debug, Clone)]
pub(crate) enum Binding {
    Udp(SocketAddr),
    Tcp { conn: u64, sender: FrameSender },
}

impl Binding {
    fn is(&self, origin: Origin) -> bool {
        match (self, origin) {
            (Self::Udp(bound), Origin::Udp(from)) => *bound == from,
            (Self::Tcp { conn, .. }, Origin::Tcp(from)) => *conn == from,
            (Self::Udp(_), Origin::Tcp(_)) | (Self::Tcp { .. }, Origin::Udp(_)) => false,
        }
    }
}

#[derive(Debug)]
struct Session {
    binding: Binding,
    expires: Instant,
}

/// 会话表。一个节点最多一个会话：新的握手成功就顶掉旧的，所以节点从 TCP 换回 UDP 只要在 UDP 上再握一次手。
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    by_node: HashMap<NodeId, Session>,
}

impl Sessions {
    /// 握手成功：给 `node` 建会话，或者把它已有的会话改绑到新的来源上。
    pub(crate) fn open(&mut self, node: NodeId, binding: Binding, now: Instant) {
        self.by_node.insert(node, Session { binding, expires: now + SESSION_TTL });
    }

    /// `node` 有没有会话，并且就是绑定在 `origin` 上的。转发之前靠它核对来源：
    /// 来源节点是别人的会话，或者数据报不是从会话绑定的地方来的，都不算。
    pub(crate) fn owned_by(&self, node: NodeId, origin: Origin, now: Instant) -> bool {
        self.live(node, now).is_some_and(|session| session.binding.is(origin))
    }

    /// 发给 `node` 的包往哪里送。没有会话或者会话过期了返回 `None`。
    pub(crate) fn binding(&self, node: NodeId, now: Instant) -> Option<&Binding> {
        self.live(node, now).map(|session| &session.binding)
    }

    /// 收到 PING：会话是绑定在 `origin` 上的就续期，返回 `true`。
    pub(crate) fn refresh(&mut self, node: NodeId, origin: Origin, now: Instant) -> bool {
        match self.by_node.get_mut(&node) {
            Some(session) if session.expires > now && session.binding.is(origin) => {
                session.expires = now + SESSION_TTL;
                true
            }
            _ => false,
        }
    }

    /// 一条 TCP 断了：绑定在它上面的会话跟着没有。节点这期间已经换到别处的会话不受影响。
    pub(crate) fn close_tcp(&mut self, conn: u64) -> Vec<NodeId> {
        self.remove_where(|_, session| session.binding.is(Origin::Tcp(conn)))
    }

    /// 删掉过期的会话，返回是哪些节点的。
    pub(crate) fn sweep(&mut self, now: Instant) -> Vec<NodeId> {
        self.remove_where(|_, session| session.expires <= now)
    }

    /// 删掉不再被允许的节点的会话（节点被吊销、从网络里删除），返回是哪些节点的。
    pub(crate) fn retain_clients(&mut self, is_client: impl Fn(NodeId) -> bool) -> Vec<NodeId> {
        self.remove_where(|node, _| !is_client(node))
    }

    pub(crate) fn clear(&mut self) {
        self.by_node.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.by_node.len()
    }

    fn live(&self, node: NodeId, now: Instant) -> Option<&Session> {
        self.by_node.get(&node).filter(|session| session.expires > now)
    }

    fn remove_where(&mut self, doomed: impl Fn(NodeId, &Session) -> bool) -> Vec<NodeId> {
        let removed: Vec<NodeId> = self.by_node.iter().filter(|&(&node, session)| doomed(node, session)).map(|(&node, _)| node).collect();
        for node in &removed {
            self.by_node.remove(node);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use i3t_underlay::tcp_leg;

    use super::*;

    fn node(raw: u32) -> NodeId {
        NodeId::new(raw).unwrap()
    }

    fn udp(port: u16) -> SocketAddr {
        SocketAddr::from(([203, 0, 113, 7], port))
    }

    fn tcp(conn: u64) -> Binding {
        Binding::Tcp { conn, sender: tcp_leg::frame_queue().0 }
    }

    #[test]
    fn a_session_belongs_only_to_the_address_it_was_opened_from() {
        let now = Instant::now();
        let mut sessions = Sessions::default();
        sessions.open(node(1), Binding::Udp(udp(4000)), now);
        assert!(sessions.owned_by(node(1), Origin::Udp(udp(4000)), now));
        assert!(!sessions.owned_by(node(1), Origin::Udp(udp(4001)), now), "别的地址冒充不了");
        assert!(!sessions.owned_by(node(1), Origin::Tcp(1), now));
        assert!(!sessions.owned_by(node(2), Origin::Udp(udp(4000)), now), "同一个地址也不能冒充别的节点");
    }

    #[test]
    fn a_session_expires_without_keepalive_and_ping_extends_it() {
        let start = Instant::now();
        let mut sessions = Sessions::default();
        sessions.open(node(1), Binding::Udp(udp(4000)), start);

        let later = start + SESSION_TTL - Duration::from_secs(1);
        assert!(sessions.refresh(node(1), Origin::Udp(udp(4000)), later));
        assert!(!sessions.refresh(node(1), Origin::Udp(udp(4001)), later), "别的地址来的 PING 不续期");
        assert!(sessions.binding(node(1), start + SESSION_TTL).is_some(), "续过期了");

        let expired = later + SESSION_TTL;
        assert!(sessions.binding(node(1), expired).is_none());
        assert!(!sessions.owned_by(node(1), Origin::Udp(udp(4000)), expired));
        assert!(!sessions.refresh(node(1), Origin::Udp(udp(4000)), expired), "过期之后要重新握手");
        assert_eq!(sessions.sweep(expired), [node(1)]);
        assert_eq!(sessions.len(), 0);
    }

    #[test]
    fn a_new_handshake_rebinds_the_session() {
        let now = Instant::now();
        let mut sessions = Sessions::default();
        sessions.open(node(1), tcp(7), now);
        assert!(sessions.owned_by(node(1), Origin::Tcp(7), now));
        sessions.open(node(1), Binding::Udp(udp(4000)), now);
        assert!(!sessions.owned_by(node(1), Origin::Tcp(7), now));
        assert!(matches!(sessions.binding(node(1), now), Some(Binding::Udp(_))));
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn closing_a_tcp_connection_removes_only_sessions_still_bound_to_it() {
        let now = Instant::now();
        let mut sessions = Sessions::default();
        sessions.open(node(1), tcp(7), now);
        sessions.open(node(2), tcp(8), now);
        sessions.open(node(3), tcp(9), now);
        // 节点 3 已经换回了 UDP，它原来那条 TCP 断开不该影响它。
        sessions.open(node(3), Binding::Udp(udp(4000)), now);
        assert_eq!(sessions.close_tcp(7), [node(1)]);
        assert_eq!(sessions.close_tcp(9), []);
        assert!(sessions.binding(node(2), now).is_some() && sessions.binding(node(3), now).is_some());
    }

    #[test]
    fn sessions_of_removed_clients_are_dropped() {
        let now = Instant::now();
        let mut sessions = Sessions::default();
        sessions.open(node(1), Binding::Udp(udp(4000)), now);
        sessions.open(node(2), Binding::Udp(udp(4001)), now);
        assert_eq!(sessions.retain_clients(|id| id == node(2)), [node(1)]);
        assert!(sessions.binding(node(1), now).is_none() && sessions.binding(node(2), now).is_some());
        sessions.clear();
        assert_eq!(sessions.len(), 0);
    }
}
