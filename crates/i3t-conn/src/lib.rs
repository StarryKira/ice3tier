//! 连接层：一对节点之间一条端到端的连接。
//!
//! [`contract`] 是这一层对上层的全部承诺：到某个节点开一条流，接受对端开过来的流，流可以正常结束也可以被重置。
//! 上层（`i3t-flow` 及以上）只依赖这些 trait，看不到 QUIC 库的任何类型。

pub mod contract;

pub use contract::{Acceptor, Connector, Incoming, OpenError, RecvStream, SendStream, StreamAborted, StreamCode};
