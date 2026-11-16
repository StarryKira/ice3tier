//! 路由层：包怎么送到对端。见 docs/architecture.md §5.2～§5.4。
//!
//! 为每个对端维护"现在走哪条路"：直连某个验证过的地址，经某个 relay（UDP 或 TCP），或者没有路。
//! 连接层只和节点编号打交道，所以路可以随时换，连接和上面的流不受影响。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`paths`] | 对外的入口 [`Paths`]：收发包（按首字节分类）、状态查询、信令 |
//! | `control` | 控制任务：处理 relay 的应答和探测、应用 netmap、选路 |
//! | `direct` | 到一个对端的直连：候选地址、验证、打洞的重试、保活 |
//! | `attach` | 节点一侧的中转会话：握手、保活 |
//! | `relays` | 到一个 relay 的联系（UDP 和 TCP 同时试，优先 UDP），挂靠哪几个 relay |
//! | `tcp` | 到 relay 的那条 TCP |
//! | `table` | 收发每个包时查的表 |
//!
//! 现在只有默认的选路模式：直连可用就走直连，否则经一个 relay。两个节点之间有直接的中转会话时用它
//! （对端自己是 relay，或者对端挂在本节点的 relay 上），不然用对端挂靠的 relay 里第一个能用的。
//! 延迟优先、多级中转和 relay 之间的骨干还没有做；对外的路由类型（`i3t_model::control::Route`）里 relay 是一个列表，
//! 以后加上它们不用改接口。

mod attach;
mod control;
mod direct;
pub mod paths;
mod relays;
mod table;
mod tcp;
mod timing;

use std::net::SocketAddr;

use i3t_model::control::{Route, RouteEntry};
use i3t_model::netmap::Attachment;
use i3t_proto::NodeId;
use i3t_underlay::Ecn;

pub use paths::{PacketMeta, PathConfig, Paths};
pub use table::Packets;

/// 本节点的 relay 角色，路径层要它做的几件事。实现在 `i3t-relay`。
///
/// 节点只有一个 UDP 端口，由路径层收包，所以归 relay 的数据报要由路径层转交；反过来，
/// 挂在本节点自己的 relay 上的对端，路径层要经 relay 的会话才送得到。
pub trait LocalRelay: Send + Sync + 'static {
    /// UDP 上收到的、归 relay 管的数据：节点发来的中转会话帧、发给别的节点的中转帧、地址查询。
    /// `data` 里是来自 `from` 的若干个数据报，每 `stride` 字节一个（最后一个可以短）。
    /// 本节点没有启用 relay 时全部丢掉，不回应。
    fn handle_udp(&self, from: SocketAddr, ecn: Option<Ecn>, data: &[u8], stride: usize);

    /// `client` 现在在本节点的 relay 上有没有会话。
    fn has_session(&self, client: NodeId) -> bool;

    /// `client` 的会话是不是绑定在 UDP 地址 `from` 上。
    fn session_at(&self, client: NodeId, from: SocketAddr) -> bool;

    /// 把带着中转头的数据报送到 `client` 的会话上。`contents` 可以是若干个等长的首尾相接，
    /// 这时 `segment_size` 是每个的长度。返回有没有送出去。
    fn send_to_client(&self, client: NodeId, contents: &[u8], segment_size: Option<usize>, ecn: Option<Ecn>) -> bool;
}

/// 到一个对端的路换了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteChange {
    pub peer: NodeId,
    /// 换成了哪条路。
    pub route: Route,
    /// 为什么换，给人看的。
    pub reason: String,
}

/// 对外公布的状态，由控制任务定期刷新。
#[derive(Debug, Default)]
pub(crate) struct Status {
    pub(crate) routes: Vec<RouteEntry>,
    pub(crate) endpoints: Vec<SocketAddr>,
    pub(crate) mapping_varies: Option<bool>,
    pub(crate) attached: Vec<Attachment>,
}
