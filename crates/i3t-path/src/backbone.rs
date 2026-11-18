//! relay 和 relay 之间走 TCP 的骨干连接（docs/architecture.md §5.2 的"骨干"）。
//!
//! 骨干上的一条连接是两个会话：本节点在对方 relay 上的，对方在本节点 relay 上的。UDP 通的时候两个会话各走各的 UDP，和以前一样。
//! 两个 relay 之间的 UDP 有一个方向不通时，两个会话都放进**同一条** TCP：一方拨号（路径层到 relay 的 TCP，[`crate::tcp`]），
//! 在上面照常建自己的会话；另一方的 relay 接进这条 TCP、认出拨号的是 netmap 里的 relay 之后告诉本节点的路径层（[`BackboneInbox`]），
//! 路径层就在这条 TCP 上反过来建自己的会话。所以一对 relay 之间最多一条 TCP，连不进来的那一方拨出去也一样成。
//!
//! 谁拨号、什么时候改走 TCP，见 [`crate::relays`] 里 `RelayLink` 的骨干部分。帧在两头怎么分：
//!
//! | 收到的 | 拨号的一头（路径层读这条 TCP） | 接进来的一头（relay 读这条 TCP） |
//! |---|---|---|
//! | 会话的请求（INIT、AUTH、PING） | 交给本节点的 relay（[`crate::LocalRelay::handle_leg_frame`]） | relay 自己处理 |
//! | 会话的应答、错误帧、矩阵行 | 路径层自己处理 | 交给路径层（这里） |
//! | 发给本节点的包 | 交给连接层 | relay 交给连接层 |
//! | 要往下转的包 | 交给本节点的 relay | relay 自己转 |

use std::net::SocketAddr;

use bytes::Bytes;
use i3t_proto::session::{Frame, Refused};
use i3t_proto::{Datagram, NodeId};
use i3t_underlay::tcp_leg::FrameSender;
use tokio::sync::mpsc;

use crate::control::Ctrl;
use crate::tcp::TcpEvent;

/// 本节点的 relay 把别的 relay 拨进来的 TCP 上属于路径层的事交到这里（见模块文档）。
///
/// 由 [`crate::Paths::backbone_inbox`] 给出，装配的一方交给本节点的 relay。它只拿着控制任务的队列，不让 relay 和路径层互相拿着对方不放。
/// 队列满了的时候丢掉：交过来的都是有重试的握手、保活和矩阵行。
#[derive(Debug, Clone)]
pub struct BackboneInbox {
    pub(crate) ctrl: mpsc::Sender<Ctrl>,
}

/// relay 接进来的一条骨干 TCP 上发生的事。
#[derive(Debug)]
pub(crate) enum Accepted {
    /// 那个 relay 在这条 TCP 上和本节点的 relay 建好了会话：路径层可以在同一条 TCP 上反过来建自己的会话。
    Opened {
        addr: SocketAddr,
        sender: FrameSender,
    },
    Event(TcpEvent),
    Closed,
}

impl BackboneInbox {
    /// relay `relay` 在本节点的 relay 接进来的第 `conn` 条 TCP 上建好了会话。`sender` 往这条 TCP 里写，`addr` 是它的对端地址。
    pub fn opened(&self, relay: NodeId, conn: u64, addr: SocketAddr, sender: FrameSender) {
        self.send(relay, conn, Accepted::Opened { addr, sender });
    }

    /// 这条 TCP 上来了一帧会话的应答、错误帧或者矩阵行（是不是这几种由这里看）。返回它是不是归路径层的。
    pub fn frame(&self, relay: NodeId, conn: u64, frame: Bytes) -> bool {
        let event = match Datagram::parse(&frame) {
            Ok(Datagram::Session(session)) if !session.is_request() => match (session, Refused::echo(&frame)) {
                (Frame::Error(refused), Some(echo)) => TcpEvent::Refusal(refused, echo),
                (session, _) => TcpEvent::Frame(session),
            },
            Ok(Datagram::Row(_)) => TcpEvent::Row(frame),
            _ => return false,
        };
        self.send(relay, conn, Accepted::Event(event));
        true
    }

    /// 这条 TCP 断了。
    pub fn closed(&self, relay: NodeId, conn: u64) {
        self.send(relay, conn, Accepted::Closed);
    }

    fn send(&self, relay: NodeId, conn: u64, event: Accepted) {
        if self.ctrl.try_send(Ctrl::Accepted { relay, conn, event }).is_err() {
            tracing::debug!(%relay, conn, "控制任务忙不过来，丢掉骨干 TCP 上的一件事");
        }
    }
}
