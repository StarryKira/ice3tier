//! 透明访问（Linux）：本机的程序不配代理，直接连别的节点的虚拟 IP（或者名字）和端口就能用。
//!
//! 没有虚拟网卡，也不在程序里逐包处理。拦截发生在建立连接的那一刻，由内核做：
//!
//! | 东西 | 做什么 |
//! |---|---|
//! | nftables 的表 `ip i3t` | TCP：发往能访问的对端的虚拟 IP 的连接在 `output` 的 NAT 里改到本机入口（[`PORT`]），入口用 `SO_ORIGINAL_DST` 取回原来的目标；发往网络地址段里别的地址的连接当场回 RST；发往地址段的别的协议（比如 ping）一律当场拒绝，不会从真网卡出去。UDP：发往对端的报文打上标记、经回环绕回来，在 `prerouting` 里用 TPROXY 交给入口，每个报文的原目标随报文带着（`IP_ORIGDSTADDR`）。另外丢掉从真网卡进来、却自称来自网络地址段的包 |
//! | 策略路由：一条规则（`fwmark` [`MARK`] → 表 [`ROUTE_TABLE`]）和表里一条 `local default dev lo` | 带标记的包在本机投递。用在两处：上面绕回来的 UDP；目标一侧以发起节点的虚拟 IP 去连本机服务时，服务回的包 |
//! | 一块 dummy 网卡 [`LINK`]，上面是本节点自己的虚拟 IP | 本节点的虚拟 IP 在本机上可达；名字服务听在它的 53 端口上；systemd-resolved 按这块网卡把网络的域名分给这个名字服务 |
//!
//! 每一样都认得出是自己的（表名、规则的 `proto` [`PROTO`]、网卡的别名 [`LINK_ALIAS`]），别的东西一概不碰。
//! 进程崩溃留下的旧东西下次装的时候被整个换掉，不会多出一份。
//!
//! # 模块
//!
//! - [`plan`]：要装什么，写成 nft 的脚本和 `ip` 的参数；
//! - [`survey`]：读回系统里现在有的、属于自己的东西；
//! - [`machine`]：和系统之间的接缝（执行 `nft`、`ip`、`resolvectl`，看本进程有没有需要的能力）；
//! - [`setup`]：装、更新、拆，和遇到旧东西时怎么办；
//! - `overlap`：网络的地址段和这台机器所在的真实网络重不重叠（重叠时不装）；
//! - [`names`]：名字服务的应答，和 `/etc/hosts` 里的一段；
//! - `socket`（只在 Linux 上）：透明访问要用的几种套接字。

pub mod machine;
pub mod names;
mod overlap;
pub mod plan;
pub mod setup;
#[cfg(target_os = "linux")]
pub mod socket;
pub mod survey;

pub use machine::{Capability, CommandError, Machine, Program};
pub use plan::Plan;
pub use setup::{ResolverOutcome, SetupError};
pub use survey::Survey;

/// nftables 里那张表的名字（`ip` 族）。
pub const TABLE: &str = "i3t";

/// 本节点自己做的、要在本机投递的包的标记（`SO_MARK` 和 nft 里的 `meta mark`）。只用这一位，别的位留给别的程序。
pub const MARK: u32 = 0x2000_0000;

/// 带 [`MARK`] 的包查哪张路由表。
pub const ROUTE_TABLE: u32 = 2033;

/// 那条策略路由规则的优先级。比 `main`（32766）和 wg-quick、Tailscale 这些常见程序加的规则都靠前：
/// 本节点带标记的包必须在本机投递，不能被别人的规则先拿去送进隧道。
pub const RULE_PRIORITY: u32 = 2033;

/// 本节点加的路由规则和路由的 `proto` 字段，认自己的东西靠它。
pub const PROTO: u8 = 203;

/// 本机入口接拦截下来的连接和报文的端口（TCP 和 UDP 都是这个号，在 127.0.0.1 上）。
///
/// 取 1024 以下的端口：普通用户绑不了它。节点崩溃之后规则还留着的那段时间里，别的用户也没法抢先占住这个端口，
/// 接走本来要进组网的连接。
pub const PORT: u16 = 1;

/// dummy 网卡的名字。
pub const LINK: &str = "i3t0";

/// dummy 网卡的别名（`ip link set … alias`）。名字叫 [`LINK`]、别名也对得上的才是自己建的。
pub const LINK_ALIAS: &str = "ice3tier";

/// 名字服务的端口，在本节点自己的虚拟 IP 上。
pub const DNS_PORT: u16 = 53;
