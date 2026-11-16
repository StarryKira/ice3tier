//! 可达层。见 docs/architecture.md。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`port`] | 节点唯一的那个 UDP 端口：绑定、批量收发 |
//! | [`ifaddr`] | 本机网卡地址，用来当局域网里的候选端点 |
//! | [`addr_query`] | 向别人问"你看到我的地址是什么" |
//! | [`tcp_leg`] | TCP 中转的一条 TCP：限制积压的写、按帧读。节点和 relay 两头共用 |
//!
//! 端口映射（UPnP、NAT-PMP、PCP）和局域网组播发现还没有做。

pub mod addr_query;
pub mod ifaddr;
pub mod port;
pub mod tcp_leg;

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
