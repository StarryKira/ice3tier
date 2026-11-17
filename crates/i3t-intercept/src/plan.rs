//! 要装进系统里的东西：nft 的脚本和 `ip` 的参数，都由一个 [`Plan`] 算出来。
//!
//! 这里只写文字，不碰系统，所以写出来的每一行都能在单元测试里逐字核对。

use std::collections::BTreeSet;
use std::fmt::Write;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

use crate::{LINK, LINK_ALIAS, MARK, PORT, PROTO, ROUTE_TABLE, RULE_PRIORITY, TABLE};

/// 透明访问在这台机器上要装成什么样。由节点现在看到的网络决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// 网络的虚拟地址段。
    pub cidr: Ipv4Net,
    /// 本节点自己的虚拟 IP：不拦截，放在 [`LINK`] 上。
    pub own: Ipv4Addr,
    /// 访问规则允许本节点访问的对端的虚拟 IP。发往它们的连接被拦截；地址段里别的地址当场被拒绝。
    pub peers: BTreeSet<Ipv4Addr>,
}

/// 拆掉那张表的脚本。先建一张空表再删：表不在的时候也不报错，一个事务里完成。
pub const REMOVE_TABLE: &str = "table ip i3t\ndelete table ip i3t\n";

/// 标记写成 nft 和 `ip` 都认的十六进制。
fn mark() -> String {
    format!("{MARK:#x}")
}

impl Plan {
    /// 整张表的脚本。旧的表（包括崩溃之后留下的）在同一个事务里被换掉：先保证有这张表，删掉，再按现在的样子建。
    /// 中途失败时内核里还是原来的样子。
    #[must_use]
    pub fn ruleset(&self) -> String {
        let (cidr, own, mark, port) = (self.cidr, self.own, mark(), PORT);
        let mut script = format!("table ip {TABLE}\ndelete table ip {TABLE}\ntable ip {TABLE} {{\n");
        script.push_str(&self.set_definition());
        // 每条链为什么在那里、为什么排在那个优先级，见模块文档和 crate 的文档。
        let _ = write!(
            script,
            "\tchain guard {{\n\
             \t\ttype filter hook prerouting priority raw; policy accept;\n\
             \t\tiifname != \"lo\" ip saddr {cidr} drop\n\
             \t}}\n\
             \tchain reroute {{\n\
             \t\ttype route hook output priority mangle; policy accept;\n\
             \t\tmeta mark & {mark} == {mark} ct mark set ct mark | {mark}\n\
             \t\tct direction reply ct mark & {mark} == {mark} meta mark set meta mark | {mark}\n\
             \t\tmeta l4proto udp ip daddr @peers meta mark set meta mark | {mark}\n\
             \t}}\n\
             \tchain intercept {{\n\
             \t\ttype nat hook output priority dstnat; policy accept;\n\
             \t\tmeta mark & {mark} == {mark} return\n\
             \t\tmeta l4proto tcp ip daddr @peers redirect to :{port}\n\
             \t}}\n\
             \tchain refuse {{\n\
             \t\ttype filter hook output priority filter; policy accept;\n\
             \t\tmeta mark & {mark} == {mark} return\n\
             \t\tip daddr {cidr} ip daddr != {own} meta l4proto tcp reject with tcp reset\n\
             \t\tip daddr {cidr} ip daddr != {own} meta l4proto udp reject\n\
             \t\tip daddr {cidr} ip daddr != {own} reject with icmp type admin-prohibited\n\
             \t}}\n\
             \tchain divert {{\n\
             \t\ttype filter hook prerouting priority mangle; policy accept;\n\
             \t\tmeta mark & {mark} == {mark} meta l4proto udp ip daddr @peers tproxy to 127.0.0.1:{port} accept\n\
             \t}}\n\
             }}\n"
        );
        script
    }

    /// 只换掉对端的集合：访问规则变了、对端增减了的时候用。一个事务里完成，换的那一刻不会有连接漏过去。
    #[must_use]
    pub fn peers_script(&self) -> String {
        let mut script = format!("flush set ip {TABLE} peers\n");
        if !self.peers.is_empty() {
            let _ = writeln!(script, "add element ip {TABLE} peers {{ {} }}", self.peer_list());
        }
        script
    }

    fn set_definition(&self) -> String {
        let mut set = String::from("\tset peers {\n\t\ttype ipv4_addr\n");
        if !self.peers.is_empty() {
            let _ = writeln!(set, "\t\telements = {{ {} }}", self.peer_list());
        }
        set.push_str("\t}\n");
        set
    }

    fn peer_list(&self) -> String {
        self.peers.iter().map(Ipv4Addr::to_string).collect::<Vec<_>>().join(", ")
    }

    /// 给 [`LINK`] 配地址的参数：本节点的虚拟 IP，/32。
    #[must_use]
    pub fn link_address_args(&self) -> Vec<String> {
        args(&["addr", "add", &format!("{}/32", self.own), "dev", LINK])
    }
}

/// 那条策略路由规则；`verb` 是 `add` 或者 `del`。删的时候把每一项都写上，只删得到自己的那一条。
#[must_use]
pub fn rule_args(verb: &str) -> Vec<String> {
    let (priority, table, proto, mark) = (RULE_PRIORITY.to_string(), ROUTE_TABLE.to_string(), PROTO.to_string(), mark());
    args(&["rule", verb, "priority", &priority, "fwmark", &format!("{mark}/{mark}"), "lookup", &table, "proto", &proto])
}

/// 表 [`ROUTE_TABLE`] 里那一条路由：什么地址都在本机投递。用 `replace`，已经有了也不会多一条。
#[must_use]
pub fn route_args() -> Vec<String> {
    let (table, proto) = (ROUTE_TABLE.to_string(), PROTO.to_string());
    args(&["route", "replace", "local", "0.0.0.0/0", "dev", "lo", "table", &table, "proto", &proto])
}

/// 删掉表 [`ROUTE_TABLE`] 里自己加的路由，别人的（`proto` 不同的）不动。
#[must_use]
pub fn flush_routes_args() -> Vec<String> {
    let (table, proto) = (ROUTE_TABLE.to_string(), PROTO.to_string());
    args(&["route", "flush", "table", &table, "proto", &proto])
}

/// 建 [`LINK`]：一块 dummy 网卡。
#[must_use]
pub fn link_create_args() -> Vec<String> {
    args(&["link", "add", LINK, "type", "dummy"])
}

/// 给 [`LINK`] 写上认得出来的别名，同时打开它。建的时候写别名不算数（`ip link add` 不认），要另外设。
#[must_use]
pub fn link_up_args() -> Vec<String> {
    args(&["link", "set", LINK, "alias", LINK_ALIAS, "up"])
}

/// 去掉 [`LINK`] 上的全部地址。只对自己建的那块网卡用。
#[must_use]
pub fn link_flush_args() -> Vec<String> {
    args(&["addr", "flush", "dev", LINK])
}

#[must_use]
pub fn link_delete_args() -> Vec<String> {
    args(&["link", "del", LINK])
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(peers: &[&str]) -> Plan {
        Plan {
            cidr: "10.66.0.0/16".parse().unwrap(),
            own: [10, 66, 0, 1].into(),
            peers: peers.iter().map(|ip| ip.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn the_ruleset_replaces_the_whole_table_in_one_transaction() {
        let script = plan(&["10.66.0.5", "10.66.0.2"]).ruleset();
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(
            lines[..3],
            ["table ip i3t", "delete table ip i3t", "table ip i3t {"],
            "先保证有表、删掉，再建：旧的表不管在不在都被换掉"
        );
        assert!(script.contains("\t\telements = { 10.66.0.2, 10.66.0.5 }\n"), "对端按地址排好：{script}");
        let expected = [
            "\t\tiifname != \"lo\" ip saddr 10.66.0.0/16 drop",
            "\t\tmeta mark & 0x20000000 == 0x20000000 ct mark set ct mark | 0x20000000",
            "\t\tct direction reply ct mark & 0x20000000 == 0x20000000 meta mark set meta mark | 0x20000000",
            "\t\tmeta l4proto udp ip daddr @peers meta mark set meta mark | 0x20000000",
            "\t\tmeta l4proto tcp ip daddr @peers redirect to :1",
            "\t\tip daddr 10.66.0.0/16 ip daddr != 10.66.0.1 meta l4proto tcp reject with tcp reset",
            "\t\tip daddr 10.66.0.0/16 ip daddr != 10.66.0.1 meta l4proto udp reject",
            "\t\tip daddr 10.66.0.0/16 ip daddr != 10.66.0.1 reject with icmp type admin-prohibited",
            "\t\tmeta mark & 0x20000000 == 0x20000000 meta l4proto udp ip daddr @peers tproxy to 127.0.0.1:1 accept",
        ];
        for line in expected {
            assert!(lines.contains(&line), "少了 {line:?}：\n{script}");
        }
        assert_eq!(script.matches("chain ").count(), 5);
        assert_eq!(lines.iter().filter(|line| line.ends_with(" return")).count(), 2, "本节点自己带标记的包不被拦截，也不被拒绝");
        assert!(script.ends_with("}\n"));
    }

    #[test]
    fn an_empty_peer_set_has_no_elements_line() {
        let script = plan(&[]).ruleset();
        assert!(script.contains("\tset peers {\n\t\ttype ipv4_addr\n\t}\n"), "{script}");
        assert!(!script.contains("elements"));
        assert_eq!(plan(&[]).peers_script(), "flush set ip i3t peers\n");
    }

    #[test]
    fn updating_peers_flushes_and_refills_the_set_in_one_script() {
        assert_eq!(
            plan(&["10.66.0.9", "10.66.0.3"]).peers_script(),
            "flush set ip i3t peers\nadd element ip i3t peers { 10.66.0.3, 10.66.0.9 }\n"
        );
    }

    #[test]
    fn ip_arguments_name_every_selector_so_only_our_own_entries_are_touched() {
        assert_eq!(rule_args("del").join(" "), "rule del priority 2033 fwmark 0x20000000/0x20000000 lookup 2033 proto 203");
        assert_eq!(route_args().join(" "), "route replace local 0.0.0.0/0 dev lo table 2033 proto 203");
        assert_eq!(flush_routes_args().join(" "), "route flush table 2033 proto 203");
        assert_eq!(link_create_args().join(" "), "link add i3t0 type dummy");
        assert_eq!(link_up_args().join(" "), "link set i3t0 alias ice3tier up");
        assert_eq!(plan(&[]).link_address_args().join(" "), "addr add 10.66.0.1/32 dev i3t0");
    }
}
