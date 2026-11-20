//! 要装进系统里的防火墙规则，写成程序的参数和脚本。这里只写文字，不碰系统，所以每个平台的都能在任何一台机器上逐字核对。
//!
//! | 平台 | 放行从网卡进来的（`accept`） | 丢掉冒充虚拟 IP 的（`guard`） |
//! |---|---|---|
//! | Linux | `iptables -I INPUT 1 -i <网卡> -m comment --comment ice3tier -j ACCEPT`：插在系统 INPUT 链的最前面，ufw 这类默认拒绝入站的防火墙因此不挡别的节点。没有 `iptables` 的机器上不加：没有经它填的 INPUT 链要抢在前面 | 有 `nft` 时是 nftables 的表 `ip <网卡名>`：从别的网卡（回环除外）进来、来源却在网络地址段里的包丢掉。没有 `nft`、有 `iptables` 时是 `raw` 表 PREROUTING 链的第一条 `! -i <网卡> -s <地址段> -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP`，做同样的事 |
//! | Windows | Windows 防火墙的入站规则 `ice3tier-<网卡>`：只管从这块网卡进来的，目的是本节点的虚拟 IP、来源在网络地址段里的放行。新的网卡归在"公用"网络里，默认挡住一切入站 | 不加：Windows（Vista 起）默认每块网卡都按强主机模型收包，发往虚拟 IP 的包只在有这个地址的那块网卡上收，从真网卡进来的被丢掉；上面那条也只认从这块网卡进来的。管理员给真网卡打开 `weakhostreceive` 或者转发时，前一半不成立，后一半还在 |
//! | macOS | 不加：程序不碰包过滤（pf），macOS 上它默认不开、也不挡入站 | 不加：macOS 默认（`net.inet.ip.check_interface` = 1，"混合"模型）对 utun 这样的隧道网卡按强主机模型收包，发往虚拟 IP、从真网卡进来的被丢掉；打开了转发（比如"互联网共享"）时退回弱主机模型，这时没有东西挡 |
//!
//! 每一条都认得出是自己的：Linux 上是注释 [`COMMENT`] 加上 `-i <网卡>`（放行）或者 `! -i <网卡>`（防冒充）、表名就是网卡名；
//! Windows 上是规则的名字（分组是 [`COMMENT`]）。

use std::fmt::Write;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

use crate::COMMENT;

/// Linux 上的两条。
pub mod linux {
    use super::{COMMENT, Ipv4Net, Write, args};

    /// `iptables` 等 xtables 的锁最多等几秒（老的 xtables 后端会有别的程序同时在改；nftables 后端不用锁）。
    const LOCK_WAIT: &str = "5";

    /// 防冒充的那张表：整张换。旧的表（包括崩溃之后留下的）在同一个事务里被换掉：先保证有这张表，删掉，再按现在的样子建；
    /// 中途失败时内核里还是原来的样子。挂在 `prerouting` 最前面（`raw`，连接跟踪之前），转发的包也管。
    #[must_use]
    pub fn guard_script(device: &str, cidr: Ipv4Net) -> String {
        let mut script = format!("table ip {device}\ndelete table ip {device}\ntable ip {device} {{\n");
        let _ = write!(
            script,
            "\tchain guard {{\n\
             \t\ttype filter hook prerouting priority raw; policy accept;\n\
             \t\tiifname \"{device}\" return\n\
             \t\tiifname \"lo\" return\n\
             \t\tip saddr {cidr} drop\n\
             \t}}\n\
             }}\n"
        );
        script
    }

    /// 拆掉那张表的脚本。先建一张空表再删：表不在的时候也不报错。
    #[must_use]
    pub fn remove_table_script(name: &str) -> String {
        format!("table ip {name}\ndelete table ip {name}\n")
    }

    /// 放行的那条规则本身（`iptables -S` 写出来的样子去掉 `-A INPUT`）。
    fn rule(device: &str) -> Vec<String> {
        args(&["-i", device, "-m", "comment", "--comment", COMMENT, "-j", "ACCEPT"])
    }

    /// 插在 INPUT 链的第一条。
    #[must_use]
    pub fn insert_accept(device: &str) -> Vec<String> {
        [args(&["-w", LOCK_WAIT, "-I", "INPUT", "1"]), rule(device)].concat()
    }

    /// 删一条（写全了，只删得到自己的那一条）。
    #[must_use]
    pub fn delete_accept(device: &str) -> Vec<String> {
        [args(&["-w", LOCK_WAIT, "-D", "INPUT"]), rule(device)].concat()
    }

    /// 列出 INPUT 链。
    #[must_use]
    pub fn list_input() -> Vec<String> {
        args(&["-w", LOCK_WAIT, "-S", "INPUT"])
    }

    /// 列出 `ip` 族的表。
    #[must_use]
    pub fn list_tables() -> Vec<String> {
        args(&["-j", "list", "tables", "ip"])
    }

    /// 没有 `nft` 时的防冒充，一条 iptables 规则：不是从网卡 `device` 进来、来源却在地址段 `cidr` 里的包丢掉。
    /// 在 `raw` 表的 PREROUTING 链里，和那张表一样在连接跟踪之前，转发的包也管。
    ///
    /// 一条 iptables 规则只写得了一块网卡，回环没法像那张表里（`iifname "lo" return`）那样另外放过；可本机的程序用本节点的虚拟 IP
    /// 连本机时，包走回环、来源在地址段里，不能丢。这样的包来源是本机自己的地址，`addrtype ! --src-type LOCAL` 把它们留下。
    /// 从别的网卡进来、冒充本机地址的包内核自己就丢（martian，`accept_local` 默认关着）。
    ///
    /// `iptables` 存的是地址段的网络地址，这里也写网络地址，读回来才比得上。
    fn guard_rule(device: &str, cidr: Ipv4Net) -> Vec<String> {
        let spoofed = args(&["!", "-i", device, "-s", &cidr.trunc().to_string()]);
        let not_local = args(&["-m", "addrtype", "!", "--src-type", "LOCAL"]);
        [spoofed, not_local, args(&["-m", "comment", "--comment", COMMENT, "-j", "DROP"])].concat()
    }

    /// 防冒充的那条插在 `raw` 表 PREROUTING 链的第一条。
    #[must_use]
    pub fn insert_guard(device: &str, cidr: Ipv4Net) -> Vec<String> {
        [args(&["-w", LOCK_WAIT, "-t", "raw", "-I", "PREROUTING", "1"]), guard_rule(device, cidr)].concat()
    }

    /// 删一条防冒充的规则：`listed` 是 `iptables -S` 写出来的那一行去掉 `-A PREROUTING`，原样交回去。
    /// 按读到的样子删，旧的地址段、以前的写法也删得掉，`iptables` 写出来的次序和 [`insert_guard`] 的不一样也不要紧。
    #[must_use]
    pub fn delete_guard(listed: &str) -> Vec<String> {
        [args(&["-w", LOCK_WAIT, "-t", "raw", "-D", "PREROUTING"]), split_listed(listed)].concat()
    }

    /// 列出 `raw` 表的 PREROUTING 链。
    #[must_use]
    pub fn list_raw_prerouting() -> Vec<String> {
        args(&["-w", LOCK_WAIT, "-t", "raw", "-S", "PREROUTING"])
    }

    /// `iptables -S` 写出来的一条规则拆回参数。词之间是空白；带空格、引号这类字符的值（注释）写在双引号里，
    /// 里面的 `"`、`\`、`'` 前面加了 `\`（`iptables-save` 的写法）。拆出来的词不带引号，正是执行程序时的参数。
    #[must_use]
    pub fn split_listed(listed: &str) -> Vec<String> {
        let mut words = Vec::new();
        // 正在拼的词。一对空的双引号也是一个词（空的），所以用 `Option` 而不是看字符串空不空。
        let mut word: Option<String> = None;
        let mut quoted = false;
        let mut chars = listed.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    quoted = !quoted;
                    word.get_or_insert_default();
                }
                '\\' if quoted => word.get_or_insert_default().extend(chars.next()),
                c if c.is_whitespace() && !quoted => words.extend(word.take()),
                c => word.get_or_insert_default().push(c),
            }
        }
        words.extend(word);
        words
    }
}

/// Windows 上的一条：Windows 防火墙的入站规则，经 PowerShell 的 `NetSecurity` 模块（Windows 8 起自带）。
///
/// 不用 `netsh advfirewall`：它的规则只能按网卡的类型（有线、无线、拨号）限定，不能限定到一块网卡上；
/// `New-NetFirewallRule -InterfaceAlias` 可以，规则因此只管从这块网卡进来的包，别的网卡上冒充虚拟 IP 的包它不放。
pub mod windows {
    use super::{COMMENT, Ipv4Addr, Ipv4Net, args};

    /// 规则的名字（`-Name`，Windows 防火墙里规则的编号，不会重名）和显示的名字：`ice3tier-i3t0`。分组是 [`COMMENT`]。
    #[must_use]
    pub fn rule_name(device: &str) -> String {
        format!("{COMMENT}-{device}")
    }

    /// 执行一段脚本的参数。脚本里没有双引号：Windows 上的参数靠双引号分开，脚本整个作为一个参数交给 PowerShell。
    /// 出错时（`$ErrorActionPreference = 'Stop'`）脚本停下，PowerShell 以 1 退出，错误写在标准错误上。
    #[must_use]
    pub fn invocation(script: &str) -> Vec<String> {
        debug_assert!(!script.contains('"'), "脚本里不能有双引号：{script}");
        let mut parts = args(&["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
        parts.push(format!("$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; {script}"));
        parts
    }

    /// 装上那条规则：先删掉同名的（崩溃留下的），再加。只管从网卡 `device` 进来、目的是本节点的虚拟 IP、
    /// 来源在网络地址段里的包，任何协议，任何网络类型（新的网卡归在"公用"网络里，那里默认挡住一切入站）。
    #[must_use]
    pub fn install_script(device: &str, own: Ipv4Addr, cidr: Ipv4Net) -> String {
        let name = rule_name(device);
        format!(
            "{remove}; New-NetFirewallRule -Name '{name}' -DisplayName '{name}' -Group '{COMMENT}' -Direction Inbound -Action Allow \
             -Protocol Any -InterfaceAlias '{device}' -LocalAddress {own} -RemoteAddress {cidr} -Profile Any | Out-Null",
            remove = remove_script(device)
        )
    }

    /// 删掉这个名字的规则。没有这条规则不算错。
    #[must_use]
    pub fn remove_script(device: &str) -> String {
        format!("Get-NetFirewallRule -Name '{}' -ErrorAction SilentlyContinue | Remove-NetFirewallRule", rule_name(device))
    }

    /// 数一数有没有这个名字的规则：输出 `0` 或者 `1`。"没有这条规则"之外的错误（比如防火墙服务没在跑）照样报错。
    #[must_use]
    pub fn count_script(device: &str) -> String {
        format!(
            "$rules = @(Get-NetFirewallRule -Name '{}' -ErrorAction SilentlyContinue -ErrorVariable failures); \
             $other = @($failures | Where-Object {{ $_.CategoryInfo.Category -ne 'ObjectNotFound' }}); \
             if ($other.Count -gt 0) {{ throw $other[0] }}; $rules.Count",
            rule_name(device)
        )
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// 本节点的网卡要装成什么样：地址、地址段，和网卡的名字（建好之后才知道）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// 网卡实际的名字。
    pub device: String,
    /// 本节点的虚拟 IP。
    pub own: Ipv4Addr,
    /// 网络的地址段。
    pub cidr: Ipv4Net,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linux_guard_replaces_its_whole_table_in_one_transaction() {
        let script = linux::guard_script("i3t0", "10.66.0.0/16".parse().unwrap());
        assert_eq!(
            script,
            "table ip i3t0\ndelete table ip i3t0\ntable ip i3t0 {\n\tchain guard {\n\t\ttype filter hook prerouting priority raw; policy accept;\n\
             \t\tiifname \"i3t0\" return\n\t\tiifname \"lo\" return\n\t\tip saddr 10.66.0.0/16 drop\n\t}\n}\n"
        );
        assert_eq!(linux::remove_table_script("i3t1"), "table ip i3t1\ndelete table ip i3t1\n");
    }

    #[test]
    fn the_linux_accept_rule_is_inserted_first_and_deleted_by_its_full_spelling() {
        assert_eq!(linux::insert_accept("i3t0").join(" "), "-w 5 -I INPUT 1 -i i3t0 -m comment --comment ice3tier -j ACCEPT");
        assert_eq!(linux::delete_accept("i3t0").join(" "), "-w 5 -D INPUT -i i3t0 -m comment --comment ice3tier -j ACCEPT");
        assert_eq!(linux::list_input().join(" "), "-w 5 -S INPUT");
        assert_eq!(linux::list_tables().join(" "), "-j list tables ip");
    }

    #[test]
    fn without_nft_the_linux_guard_is_the_first_rule_in_raw_prerouting_and_is_deleted_as_listed() {
        let cidr = "10.66.0.0/16".parse().unwrap();
        assert_eq!(
            linux::insert_guard("i3t0", cidr).join(" "),
            "-w 5 -t raw -I PREROUTING 1 ! -i i3t0 -s 10.66.0.0/16 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP"
        );
        assert_eq!(linux::list_raw_prerouting().join(" "), "-w 5 -t raw -S PREROUTING");
        let listed = "-s 10.66.0.0/16 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP";
        assert_eq!(
            linux::delete_guard(listed).join(" "),
            "-w 5 -t raw -D PREROUTING -s 10.66.0.0/16 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP"
        );
        assert!(
            linux::insert_guard("i3t0", "10.66.3.4/16".parse().unwrap()).join(" ").contains(" -s 10.66.0.0/16 "),
            "写的是网络地址，和 iptables 读回来的一样"
        );
    }

    #[test]
    fn a_listed_rule_splits_back_into_arguments_and_a_quoted_value_is_one_argument() {
        assert_eq!(
            linux::split_listed(r#"-s 10.0.0.0/8 -m comment --comment "a \"b\" \\c" -j  DROP"#),
            ["-s", "10.0.0.0/8", "-m", "comment", "--comment", r#"a "b" \c"#, "-j", "DROP"]
        );
        assert_eq!(linux::split_listed(r#"--comment "" -j ACCEPT"#), ["--comment", "", "-j", "ACCEPT"]);
        assert!(linux::split_listed("  ").is_empty());
    }

    #[test]
    fn the_windows_rule_is_scoped_to_our_adapter_and_known_by_its_name() {
        let install = windows::install_script("i3t0", Ipv4Addr::new(10, 66, 0, 5), "10.66.0.0/16".parse().unwrap());
        assert_eq!(
            install,
            "Get-NetFirewallRule -Name 'ice3tier-i3t0' -ErrorAction SilentlyContinue | Remove-NetFirewallRule; \
             New-NetFirewallRule -Name 'ice3tier-i3t0' -DisplayName 'ice3tier-i3t0' -Group 'ice3tier' -Direction Inbound -Action Allow \
             -Protocol Any -InterfaceAlias 'i3t0' -LocalAddress 10.66.0.5 -RemoteAddress 10.66.0.0/16 -Profile Any | Out-Null"
        );
        let call = windows::invocation(&install);
        assert_eq!(call[..5].join(" "), "-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command");
        assert_eq!(call.len(), 6, "脚本是一个参数");
        assert!(call[5].starts_with("$ErrorActionPreference = 'Stop'; "), "出错就停下、以 1 退出：{}", call[5]);
        for script in [install, windows::remove_script("i3t0"), windows::count_script("i3t0")] {
            assert!(!script.contains('"'), "没有双引号，整个脚本是一个参数：{script}");
            assert!(script.contains("-Name 'ice3tier-i3t0'"), "{script}");
        }
        assert!(windows::count_script("i3t0").contains("ObjectNotFound"), "只有“没有这条规则”当作 0");
    }
}
