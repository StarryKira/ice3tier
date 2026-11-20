//! 可达层。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`port`] | 节点唯一的那个 UDP 端口：绑定、批量收发 |
//! | [`ifaddr`] | 本机网卡地址，用来当局域网里的候选端点 |
//! | [`addr_query`] | 向别人问"你看到我的地址是什么" |
//! | [`tcp_leg`] | TCP 中转的一条 TCP：限制积压的写、按帧读。节点和 relay 两头共用 |
//! | [`tcp_recovery`] | 节点之间的 TCP 断网之后尽快恢复：重传上限和放弃时限一起设 |
//! | [`tcp_mss`] | 本进程开出去、接进来的 TCP 都通告一个压低的 MSS，躲开路径 MTU 黑洞 |
//! | [`tcp_reset`] | 异常结束一条 TCP：关闭时发 RST，并且让对端认它 |
//! | [`lan`] | 局域网发现：组播信标 |
//! | [`portmap`] | 自动端口映射：PCP、NAT-PMP、UPnP-IGD |

pub mod addr_query;
pub mod ifaddr;
pub mod lan;
pub mod port;
pub mod portmap;
pub mod tcp_leg;
pub mod tcp_mss;
pub mod tcp_recovery;
pub mod tcp_reset;

pub use addr_query::{AddrQuery, Observation};
pub use port::{BATCH_SIZE, Direction, Ecn, PacketFilter, RecvMeta, Transmit, UdpPort};

use ring::rand::{SecureRandom, SystemRandom};

/// 取 `N` 个随机字节，用作各种帧里的随机数。
///
/// # Panics
///
/// 操作系统的随机数源不可用。这种情况下进程没法安全地做任何握手，也没有可以退回的办法。
#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    SystemRandom::new().fill(&mut bytes).expect("操作系统的随机数源不可用，进程没法继续安全地运行");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_differ_between_calls() {
        assert_ne!(random_bytes::<16>(), random_bytes::<16>());
    }
}
