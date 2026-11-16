//! 流层：把一条用户连接变成端到端连接上的一条流。
//!
//! 流上的格式在 [`i3t_proto::flow`]：发起方先写 OPEN，后面直接跟数据；接受方先写 REPLY，后面直接跟数据。
//! 这里做的是格式之外的事：
//!
//! | 谁用 | 入口 | 做什么 |
//! |---|---|---|
//! | 发起方（入口模块） | [`open`]、[`Pending`] | 开流，把 OPEN 和首批数据一次写出去；等 REPLY，或者不等就开始搬运 |
//! | 接受方（出口模块） | [`serve`]、[`Handler`]、[`Request`] | 先按发起节点筛一遍，限时读 OPEN，交给处理者，由它接受或者拒绝 |
//! | 两边 | [`Flow::relay`]、[`keep_alive`] | 在流和一条 TCP 连接之间双向搬运，如实传递半关闭和异常结束；TCP 那一头靠内核保活判断死活 |
//! | 两边 | [`Meter`] | 搬运时每一段数据要过的关卡：计数、限速、叫停 |
//! | 中间的节点 | [`Flow::splice`] | 把两条流接起来：监听在虚拟 IP 上、目标是别的节点的端口转发 |
//! | 两边 | [`engine`] | 流底下是一条不再加密的独占 TCP 时，在它和另一条 TCP 之间直接搬（Linux 上用 splice） |
//!
//! UDP 应用的数据不走流，走数据报上下文，见 [`datagram`]。接受方那一侧两种都从 [`serve`] 进来。
//!
//! 服务等级在这一层落到实处：流的发送优先级跟着等级走（`auto` 的流发得多了会降级），
//! 数据报按等级选不可靠的通道还是逐个可靠送达。
//!
//! 这一层只依赖连接层的契约（[`i3t_conn::contract`]），不知道下面是不是 QUIC。
//! 打开 `test-util` 特性之后有一个内存里的连接层（`memory` 模块），测试不需要真的连接。

mod acceptor;
pub mod datagram;
pub mod engine;
mod error;
mod header;
mod initiator;
#[cfg(feature = "test-util")]
pub mod memory;
mod meter;
mod relay;
mod splice;

pub use acceptor::{Handler, OPEN_DEADLINE, Request, Screening, serve};
pub use error::FlowError;
pub use initiator::{Pending, open};
pub use meter::{Direction, Meter, Unmetered};
pub use relay::{End, Flow, KEEPALIVE, Summary, Traffic, keep_alive};
pub use splice::Spliced;
