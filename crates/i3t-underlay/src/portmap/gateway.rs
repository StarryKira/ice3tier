//! 默认路由的网关，以及本机朝着它的那个地址。

use std::net::Ipv4Addr;

/// 默认路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DefaultRoute {
    pub(super) gateway: Ipv4Addr,
    /// 本机在默认路由那块网卡上的地址，网关把映射转到这里。
    pub(super) local: Ipv4Addr,
}

/// 读系统的默认路由。没有默认路由、网关没有 IPv4 地址时返回 `None`。
///
/// 它要枚举网卡、问路由表，会阻塞，调用方放在阻塞线程里跑。
pub(super) fn default_route() -> Option<DefaultRoute> {
    let interface = match netdev::get_default_interface() {
        Ok(interface) => interface,
        Err(error) => {
            tracing::debug!(error, "找不到默认路由的网卡，不做端口映射");
            return None;
        }
    };
    let gateway = *interface.gateway.as_ref()?.ipv4.first()?;
    // 一块网卡上可以有几个地址，取和网关在同一个网段的那个。
    let same_subnet = interface.ipv4.iter().find(|net| net.contains(&gateway));
    let local = same_subnet.or(interface.ipv4.first())?.addr();
    Some(DefaultRoute { gateway, local })
}

/// 这个本机地址要不要映射。公网地址本来就连得上，用不着；回环和链路本地地址后面没有网关。
///
/// 运营商级 NAT 的那一段（100.64.0.0/10）也算：PCP 本来就是给它设计的。
pub(super) fn needs_mapping(local: Ipv4Addr) -> bool {
    let [first, second, ..] = local.octets();
    local.is_private() || (first == 100 && second & 0xc0 == 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_behind_a_translator_need_a_mapping() {
        for local in ["192.168.1.5", "10.0.0.2", "172.16.3.4", "100.64.0.9", "100.127.255.254"] {
            assert!(needs_mapping(local.parse().unwrap()), "{local}");
        }
        for local in ["203.0.113.7", "8.8.8.8", "100.128.0.1", "100.63.0.1", "127.0.0.1", "169.254.1.2", "0.0.0.0"] {
            assert!(!needs_mapping(local.parse().unwrap()), "{local}");
        }
    }

    #[test]
    fn reading_the_default_route_never_panics() {
        // 测试机器上有没有默认路由都可以，有的话两个地址都得是能用的。
        if let Some(route) = default_route() {
            assert!(!route.gateway.is_unspecified() && !route.local.is_unspecified());
        }
    }
}
