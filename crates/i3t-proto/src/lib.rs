//! ice3tier 的线上格式。
//!
//! 这里只有格式：字节怎么排、怎么编码和解析。不做 I/O，不依赖运行时，所以 relay、节点和测试都可以直接用。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`node`] | 节点编号 |
//! | [`datagram`] | 一个端口上收到的 UDP 数据报怎么分类；经 relay 转发时加在包前面的中转头 |
//! | [`tcp_frame`] | UDP 用不了的节点把同样的包装进一条 TCP 时的分帧 |

pub mod datagram;
pub mod node;
pub mod tcp_frame;

pub use datagram::{Datagram, DatagramError, MAX_CONN_PACKET_LEN, RelayHeader};
pub use node::NodeId;
