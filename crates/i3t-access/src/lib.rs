//! 入口：把本机或者别处的应用连进组网。
//!
//! | 入口 | 说明 |
//! |---|---|
//! | SOCKS5 监听器 | `CONNECT` 和 `UDP ASSOCIATE`，不认证或者用户名密码认证 |
//! | HTTP 代理监听器 | `CONNECT`，`Proxy-Authorization: Basic` |
//! | 端口转发 | 监听一个端口。TCP 的每条连接变成到目标节点的一条流；UDP 的每个客户端地址是一个会话，走数据报上下文 |
//! | 透明访问（Linux） | 内核改到本机的、发往对端虚拟 IP 的连接（[`Access::intercept_tcp`]），每条变成到那个对端的一条流；UDP 报文（[`Access::intercept_udp`]）按（应用，目标）分成会话，走数据报上下文 |
//!
//! 代理的用户名写的是出口节点和服务等级（`<出口节点>[+c-<等级>]`），不是调用者的身份；
//! 要求凭据的监听器上，调用者把接入凭据放在密码字段里。一个请求最后往哪里去的规则在 `route` 模块的文档里。
//!
//! 用法是造一个 [`Access`]，然后让它的 [`run`](Access::run) 一直跑着：监听什么由 netmap 决定，netmap 变了它自己跟着调整。
//! 就地出站要用的拨号器（[`LocalEgress`]）由出口模块实现，装配时交进来。
//!
//! 入口按凭据、按监听、按出口记用量（`usage` 模块），装配层取快照上报；凭据的流量上限由 controller 汇总各入口的用量判断，
//! 用完时 netmap 里的凭据被标成用完，入口拒绝它的新连接并断开正在进行的。

mod credential;
mod egress;
mod failure;
mod first_data;
mod http;
mod intercept;
mod manager;
mod message;
mod route;
mod selector;
mod session;
mod socks5;
mod udp;
mod usage;

pub use egress::{EgressPolicy, LocalEgress};
pub use intercept::InterceptStatus;
pub use manager::{Access, AccessStatus, Active, ForwardStatus, ListenState, ListenerStatus, NoPortBook, PortBook, Traffic};
pub use udp::intercept::Diversion;
pub use usage::{CredentialSnapshot, ExitSnapshot, Tally};
