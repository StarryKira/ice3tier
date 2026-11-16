//! 流层：把一条用户连接变成端到端连接上的一条流。
//!
//! 流上的格式在 [`i3t_proto::flow`]：发起方先写 OPEN，后面直接跟数据；接受方先写 REPLY，后面直接跟数据。
//! 这里做的是格式之外的事：
//!
//! | 谁用 | 入口 | 做什么 |
//! |---|---|---|
//! | 发起方（入口模块） | [`open`]、[`Pending`] | 开流，把 OPEN 和首批数据一次写出去；等 REPLY，或者不等就开始搬运 |
//! | 接受方（出口模块） | [`serve`]、[`Handler`]、[`Request`] | 限时读 OPEN，交给处理者，由它接受或者拒绝 |
//! | 两边 | [`Flow::relay`] | 在流和一条 TCP 连接之间双向搬运，如实传递半关闭和异常结束 |
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
mod error;
mod header;
mod initiator;
#[cfg(feature = "test-util")]
pub mod memory;
mod relay;

pub use acceptor::{Handler, OPEN_DEADLINE, Request, serve};
pub use error::FlowError;
pub use initiator::{Pending, open};
pub use relay::{End, Flow, Summary, Traffic};
