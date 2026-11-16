//! 出口：替别的节点连接目标。
//!
//! 别的节点开过来的每条流带着一个建流请求，说明它要去哪里：互联网、本节点自己的服务、本节点所在的局域网，
//! 或者下发给本节点的一条端口转发。[`Exit`] 按发起节点查访问规则，按目标查本节点的能力和出站策略，
//! 通过了就连上目标，在流和目标之间搬运。UDP 应用的数据走数据报上下文：每个上下文在本节点上是一个 UDP 端口，
//! 每个数据报的目标按同样的规则检查。
//!
//! [`Outbound`] 是其中"检查策略、解析域名、连接目标"的那一部分。入口模块的就地出站也用它，
//! 所以装配时把同一个 [`Outbound`] 既交给 [`Exit`]，也交给入口。
//!
//! [`System`] 是和操作系统之间的接缝（网卡地址、域名解析、TCP 连接），正常运行时用 [`Os`]。

mod datagrams;
mod dns_cache;
mod handler;
mod outbound;
mod policy;
mod stats;
mod system;
mod upstream;

pub use handler::Exit;
pub use outbound::Outbound;
pub use stats::ExitStats;
pub use system::{Os, ResolveError, System};
