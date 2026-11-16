//! 节点密钥、签名，以及按公钥互相认证的 TLS 配置。
//!
//! ice3tier 里只有一种密钥：每个节点一对 Ed25519 密钥。它有两个用处：
//!
//! - 签名（[`NodeKey::sign`]、[`verify`]）：中转会话的握手、探测包、发布清单；
//! - TLS 1.3 双向认证（[`tls`]）：节点之间的 QUIC 连接、节点到 controller 的连接。
//!   证书是用节点密钥自签的，校验时不看证书链，只把证书里的公钥和期望的公钥比对。

pub mod key;
pub mod tls;

pub use key::{BadSignature, KeyError, NodeKey, Signature, verify};
