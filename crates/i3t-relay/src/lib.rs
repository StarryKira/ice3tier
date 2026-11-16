//! relay。见 docs/architecture.md §5.2。
//!
//! relay 只转发包：节点先和它建一个中转会话，之后带着中转头的数据报被原样送到目的节点的会话上。
//! 它不解密（也解不开）、不重传、不为流记状态，发不出去就丢。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`relay`] | 握手、保活、转发，以及跟随 netmap 的启用与停用 |
//! | `leg` | 同一个端口号上的 TCP 中转 |
//! | `session` | 会话表 |
//! | `cookie` | 握手里不记状态的地址证明 |
//! | `limit` | 带宽上限、错误帧的限速 |
//! | [`counters`] | 计数 |
//!
//! # 对没有认证过的来源
//!
//! - 握手的每一步应答都不比请求大，来源地址得到证明（cookie）之前不记状态、不验签名。格式上的保证见 [`i3t_proto::session`]。
//! - 没有会话的来源发来的中转帧被丢掉，只按来源地址限速地回一个 6 字节的错误帧。
//! - 签名不对、cookie 不对、节点不在名单里、类型不认识的帧一律丢掉，不回应。

mod cookie;
pub mod counters;
mod leg;
mod limit;
pub mod relay;
mod session;

pub use counters::{DropReason, RelayCounters};
pub use relay::Relay;
