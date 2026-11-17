//! 网络的地址段是不是和这台机器所在的真实网络重叠。
//!
//! 透明访问装上之后，发往地址段的包一律被拦截或者当场拒绝，从真网卡进来、自称来自地址段的包一律丢掉（`guard`）。
//! 这台机器要是本来就在一个和地址段重叠的真实网络里（家里的局域网、云上的私有网络、容器的网桥），装上就等于把它从那个网络
//! 上切下来，远程的服务器连自己的 SSH 会话都可能断掉。所以装之前先看一遍：本机网卡上的地址、`main` 表里的路由，
//! 和地址段有没有重叠。默认路由不算：它只是"别的都往这里走"，包括 VPN 拆成两半的那种（`0.0.0.0/1` 加 `128.0.0.0/1`）；
//! 自己的网卡 [`LINK`] 上的不算。
//!
//! 这是一道保险，所以读不懂系统的回答时按"不能装"处理，不按"没有重叠"处理。

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use serde_json::Value;

use crate::LINK;
use crate::machine::{CommandError, Machine, Program};

/// `cidr` 和本机的哪个地址或者路由重叠；不重叠时是 `None`。说出来的写法像 `ip` 的：`192.168.1.5/24 dev eth0`。
///
/// # Errors
///
/// 读不了本机的地址或者路由。
pub(crate) fn find(machine: &mut impl Machine, cidr: Ipv4Net) -> Result<Option<String>, CommandError> {
    let addresses = machine.run(Program::Ip, &args(&["-j", "-4", "addr", "show"]), None)?;
    if let Some(found) = addresses_overlapping(&listing(&addresses)?, cidr) {
        return Ok(Some(found));
    }
    let routes = machine.run(Program::Ip, &args(&["-j", "-4", "route", "show", "table", "main"]), None)?;
    Ok(routes_overlapping(&listing(&routes)?, cidr))
}

/// `ip -j` 的输出：一个 JSON 数组。不是的话当作命令失败：查不清有没有重叠就不装。
fn listing(json: &str) -> Result<Vec<Value>, CommandError> {
    serde_json::from_str(json).map_err(|error| CommandError::Failed {
        program: Program::Ip,
        status: None,
        detail: format!("读不懂它的输出：{error}"),
    })
}

/// 路由宽到算不上"某个网络"：默认路由，以及把它拆成两半的写法。
const CATCH_ALL_PREFIX: u8 = 1;

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// 两个地址段有没有共同的地址。
fn overlaps(a: Ipv4Net, b: Ipv4Net) -> bool {
    a.contains(&b.network()) || b.contains(&a.network())
}

/// `ip -j -4 addr show` 的输出里，和 `cidr` 重叠的第一个地址（[`LINK`] 上的除外）。
fn addresses_overlapping(links: &[Value], cidr: Ipv4Net) -> Option<String> {
    links.iter().filter(|link| link.get("ifname").and_then(Value::as_str) != Some(LINK)).find_map(|link| {
        let name = link.get("ifname").and_then(Value::as_str).unwrap_or("?");
        let addresses = link.get("addr_info").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        addresses.iter().find_map(|address| {
            let local: Ipv4Addr = address.get("local")?.as_str()?.parse().ok()?;
            let prefix = u8::try_from(address.get("prefixlen")?.as_u64()?).ok()?;
            let net = Ipv4Net::new(local, prefix).ok()?;
            overlaps(net.trunc(), cidr).then(|| format!("{net} dev {name}"))
        })
    })
}

/// `ip -j -4 route show table main` 的输出里，和 `cidr` 重叠的第一条路由（默认路由和经 [`LINK`] 的除外）。
fn routes_overlapping(routes: &[Value], cidr: Ipv4Net) -> Option<String> {
    routes.iter().find_map(|route| {
        let dev = route.get("dev").and_then(Value::as_str);
        let dst = route.get("dst")?.as_str()?;
        if dst == "default" || dev == Some(LINK) {
            return None;
        }
        // 只有一个地址的路由不带前缀长度。
        let net: Ipv4Net = dst.parse().or_else(|_| dst.parse::<Ipv4Addr>().map(Ipv4Net::from)).ok()?;
        if net.prefix_len() <= CATCH_ALL_PREFIX || !overlaps(net, cidr) {
            return None;
        }
        let mut words: Vec<&str> = Vec::new();
        words.extend(route.get("type").and_then(Value::as_str));
        words.push(dst);
        if let Some(dev) = dev {
            words.extend(["dev", dev]);
        }
        Some(words.join(" "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // iproute2 6.17 在一个网络命名空间里真的输出：up0 是上网的网卡，lan0 在一个和覆盖网重叠的局域网里，i3t0 是自己的。
    const ADDRESSES: &str = r#"[{"ifindex":1,"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8,"scope":"host","label":"lo"}]},{"ifindex":2,"ifname":"up0","addr_info":[{"family":"inet","local":"11.0.0.6","prefixlen":30,"scope":"global","label":"up0"}]},{"ifindex":3,"ifname":"lan0","addr_info":[{"family":"inet","local":"10.66.200.1","prefixlen":24,"scope":"global","label":"lan0"}]},{"ifindex":4,"ifname":"i3t0","addr_info":[{"family":"inet","local":"10.66.0.1","prefixlen":32,"scope":"global","label":"i3t0"}]}]"#;
    const ROUTES: &str = r#"[{"dst":"default","gateway":"11.0.0.5","dev":"up0","flags":[]},{"dst":"10.66.200.0/24","dev":"lan0","protocol":"kernel","scope":"link","prefsrc":"10.66.200.1","flags":[]},{"dst":"10.67.0.0/16","gateway":"11.0.0.5","dev":"up0","flags":[]},{"type":"blackhole","dst":"10.68.1.0/24","flags":[]},{"dst":"11.0.0.4/30","dev":"up0","protocol":"kernel","scope":"link","prefsrc":"11.0.0.6","flags":[]}]"#;

    fn net(text: &str) -> Ipv4Net {
        text.parse().unwrap()
    }

    #[test]
    fn an_address_on_another_interface_inside_the_range_is_found() {
        assert_eq!(addresses_overlapping(&listing(ADDRESSES).unwrap(), net("10.66.0.0/16")).as_deref(), Some("10.66.200.1/24 dev lan0"));
        assert_eq!(
            addresses_overlapping(&listing(ADDRESSES).unwrap(), net("10.0.0.0/8")).as_deref(),
            Some("10.66.200.1/24 dev lan0"),
            "地址段包住网卡的网段"
        );
        assert_eq!(
            addresses_overlapping(&listing(ADDRESSES).unwrap(), net("10.66.200.128/25")).as_deref(),
            Some("10.66.200.1/24 dev lan0"),
            "网卡的网段包住地址段"
        );
        assert_eq!(addresses_overlapping(&listing(ADDRESSES).unwrap(), net("10.70.0.0/16")), None);
        assert_eq!(addresses_overlapping(&listing(ADDRESSES).unwrap(), net("10.66.0.0/24")), None, "只有自己的网卡在里面");
    }

    #[test]
    fn routes_inside_or_around_the_range_are_found_but_not_the_default_route() {
        assert_eq!(routes_overlapping(&listing(ROUTES).unwrap(), net("10.67.0.0/16")).as_deref(), Some("10.67.0.0/16 dev up0"));
        assert_eq!(routes_overlapping(&listing(ROUTES).unwrap(), net("10.68.0.0/16")).as_deref(), Some("blackhole 10.68.1.0/24"));
        assert_eq!(routes_overlapping(&listing(ROUTES).unwrap(), net("10.70.0.0/16")), None, "默认路由不算");
        let host_route = r#"[{"dst":"10.71.0.9","dev":"tun0","scope":"link"},{"dst":"10.66.0.0/16","dev":"i3t0"}]"#;
        assert_eq!(
            routes_overlapping(&listing(host_route).unwrap(), net("10.71.0.0/16")).as_deref(),
            Some("10.71.0.9 dev tun0"),
            "没有前缀长度的是一个地址"
        );
        assert_eq!(routes_overlapping(&listing(host_route).unwrap(), net("10.66.0.0/16")), None, "经自己的网卡的不算");
    }

    #[test]
    fn a_default_route_split_in_two_halves_is_not_a_network() {
        // VPN 常见的写法：不动原来的默认路由，另加两条各管一半的。
        let halves = r#"[{"dst":"0.0.0.0/1","dev":"tun0"},{"dst":"128.0.0.0/1","dev":"tun0"},{"dst":"default","dev":"eth0"}]"#;
        assert_eq!(routes_overlapping(&listing(halves).unwrap(), net("10.66.0.0/16")), None);
        let quarter = r#"[{"dst":"0.0.0.0/2","dev":"tun0"}]"#;
        assert_eq!(routes_overlapping(&listing(quarter).unwrap(), net("10.66.0.0/16")).as_deref(), Some("0.0.0.0/2 dev tun0"));
    }

    #[test]
    fn an_answer_that_cannot_be_read_is_an_error_not_a_clean_bill() {
        assert!(matches!(listing("Object \"-j\" is unknown, try \"ip help\"."), Err(CommandError::Failed { .. })));
        assert!(matches!(listing("{}"), Err(CommandError::Failed { .. })), "不是数组");
        assert_eq!(listing("[]").unwrap(), Vec::<Value>::new());
    }
}
