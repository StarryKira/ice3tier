//! `i3t tune`：检查会影响节点表现的内核参数，按要求把它们改过来。只支持 Linux。
//!
//! 不加参数时只看不改：列出每一项现在的值、建议的值和原因。加 `--apply` 时把建议的值写进内核（立刻生效），
//! 并记到 `/etc/sysctl.d/` 下的一个文件里，重启之后还在。只往"更宽"的方向改：已经比建议值大的数不动。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// `--apply` 把改过的参数记在这里。
const PERSIST_FILE: &str = "/etc/sysctl.d/90-i3t.conf";
/// 节点给自己的 UDP 端口申请的缓冲区大小（`i3t-underlay` 里的 `SOCKET_BUFFER_BYTES`）。
const UDP_BUFFER_BYTES: u64 = 4 * 1024 * 1024;

/// 读写内核参数的地方。真的是 `/proc/sys`，测试里是一张表。
pub(crate) trait Sysctl {
    /// 参数现在的值；这个内核没有这个参数时是 `None`。
    fn read(&self, key: &str) -> Option<String>;

    fn write(&mut self, key: &str, value: &str) -> io::Result<()>;
}

/// `/proc/sys` 下的内核参数：`net.core.rmem_max` 就是文件 `/proc/sys/net/core/rmem_max`。
pub(crate) struct ProcSys;

impl ProcSys {
    fn path(key: &str) -> PathBuf {
        Path::new("/proc/sys").join(key.replace('.', "/"))
    }
}

impl Sysctl for ProcSys {
    fn read(&self, key: &str) -> Option<String> {
        std::fs::read_to_string(Self::path(key)).ok().map(|value| value.trim().to_owned())
    }

    fn write(&mut self, key: &str, value: &str) -> io::Result<()> {
        std::fs::write(Self::path(key), value)
    }
}

/// 一个参数应该是什么样。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// 数值不小于这个数。
    AtLeast(u64),
    /// 就是这个值。
    Exactly(&'static str),
    /// 端口列表里有这个端口。列表的写法是逗号分隔，每一项是一个端口或者 `起-止`。
    IncludesPort(u16),
}

impl Want {
    /// 现在的值是 `current` 时该改成什么；已经符合就是 `None`。
    fn change(self, current: &str) -> Option<String> {
        match self {
            Self::AtLeast(least) => {
                let now: u64 = current.split_whitespace().next()?.parse().ok()?;
                (now < least).then(|| least.to_string())
            }
            Self::Exactly(value) => (current != value).then(|| value.to_owned()),
            Self::IncludesPort(port) => {
                let listed = current.split(',').filter_map(port_range).any(|(from, to)| (from..=to).contains(&port));
                (!listed).then(|| if current.is_empty() { port.to_string() } else { format!("{current},{port}") })
            }
        }
    }
}

/// 把 `8080` 或者 `8000-8100` 读成一段端口。
fn port_range(item: &str) -> Option<(u16, u16)> {
    let item = item.trim();
    match item.split_once('-') {
        Some((from, to)) => Some((from.trim().parse().ok()?, to.trim().parse().ok()?)),
        None => item.parse().ok().map(|port| (port, port)),
    }
}

/// 对一个参数的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Fine,
    /// 建议改成这个值。
    Change(String),
    /// 不适用，原因写给人看。
    Skipped(&'static str),
}

/// 一个参数检查下来的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) key: &'static str,
    pub(crate) current: Option<String>,
    pub(crate) verdict: Verdict,
    pub(crate) why: &'static str,
}

/// 检查一遍。`port` 是节点的端口（配置文件里的 `port`）。
pub(crate) fn survey(sysctl: &impl Sysctl, port: u16) -> Vec<Finding> {
    let check = |key: &'static str, want: Want, why: &'static str| {
        let current = sysctl.read(key);
        let verdict = match &current {
            None => Verdict::Skipped("这个内核没有这个参数"),
            Some(current) => want.change(current).map_or(Verdict::Fine, Verdict::Change),
        };
        Finding { key, current, verdict, why }
    };
    let mut findings = vec![
        check(
            "net.core.rmem_max",
            Want::AtLeast(UDP_BUFFER_BYTES),
            "节点给自己的 UDP 端口申请 4MB 的接收缓冲，内核按这个上限截断。很多系统的默认值只有 200KB 左右，流量一突发，包在内核里就被丢掉了",
        ),
        check("net.core.wmem_max", Want::AtLeast(UDP_BUFFER_BYTES), "同上，发送的方向"),
        check(
            "net.ipv4.tcp_slow_start_after_idle",
            Want::Exactly("0"),
            "经过节点的 TCP 连接（出口到目标的、UDP 用不了时到 relay 的）安静一会儿之后不把发送窗口收回去。流式的应答停一下再来时，不用从头慢启动",
        ),
    ];

    let mut congestion = check(
        "net.ipv4.tcp_congestion_control",
        Want::Exactly("bbr"),
        "上面那些 TCP 连接在有丢包的链路上用 BBR 快得多。节点之间的 QUIC 连接用的是程序自带的 BBR，不受这一项影响",
    );
    let available = sysctl.read("net.ipv4.tcp_available_congestion_control").unwrap_or_default();
    if matches!(congestion.verdict, Verdict::Change(_)) && !available.split_whitespace().any(|name| name == "bbr") {
        congestion.verdict = Verdict::Skipped("内核现在没有加载 bbr，先执行 modprobe tcp_bbr，再运行一次 i3t tune");
    }
    findings.push(congestion);

    let mut reserved = check(
        "net.ipv4.ip_local_reserved_ports",
        Want::IncludesPort(port),
        "节点的端口落在系统给出站连接随机分配的端口范围里。节点重启的间隙，别的程序可能正好占到它，节点就起不来了",
    );
    let in_ephemeral_range = sysctl.read("net.ipv4.ip_local_port_range").and_then(|range| {
        let mut bounds = range.split_whitespace().map(str::parse::<u16>);
        Some((bounds.next()?.ok()?..=bounds.next()?.ok()?).contains(&port))
    });
    if matches!(reserved.verdict, Verdict::Change(_)) && in_ephemeral_range != Some(true) {
        reserved.verdict = Verdict::Fine;
    }
    findings.push(reserved);
    findings
}

/// 把建议的值写进内核。返回改了哪些参数、改成了什么。
///
/// # Errors
/// 有一项写不进去就停下返回错误（通常是没有 root 权限）；在它之前的已经生效。
pub(crate) fn apply(sysctl: &mut impl Sysctl, findings: &[Finding]) -> anyhow::Result<Vec<(&'static str, String)>> {
    let mut changed = Vec::new();
    for finding in findings {
        if let Verdict::Change(value) = &finding.verdict {
            sysctl
                .write(finding.key, value)
                .with_context(|| format!("写内核参数 {} 失败。修改内核参数要 root 权限：sudo i3t tune --apply", finding.key))?;
            changed.push((finding.key, value.clone()));
        }
    }
    Ok(changed)
}

/// 记下改过的参数，让它们重启之后还在。`existing` 是这个文件原来的内容：以前记过的参数保留，同名的换成新值。
fn persisted(existing: &str, changed: &[(&'static str, String)]) -> String {
    let mut settings: BTreeMap<String, String> = existing
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();
    for (key, value) in changed {
        settings.insert((*key).to_owned(), value.clone());
    }
    let mut text = String::from("# 由 i3t tune --apply 写入。每一项的原因见 i3t tune 的输出。\n");
    for (key, value) in settings {
        text.push_str(&format!("{key} = {value}\n"));
    }
    text
}

/// `i3t tune` 的全部：检查，打印，按要求修改。
pub(crate) fn run(port: u16, apply_changes: bool) -> anyhow::Result<()> {
    let mut sysctl = ProcSys;
    let kernel = sysctl.read("kernel.osrelease").unwrap_or_else(|| "版本未知".to_owned());
    println!("内核参数（Linux {kernel}）");
    let findings = survey(&sysctl, port);
    for finding in &findings {
        let current = finding.current.as_deref().unwrap_or("没有");
        match &finding.verdict {
            Verdict::Fine => println!("  正常      {} = {current}", finding.key),
            Verdict::Change(value) => {
                println!("  建议修改  {} = {current}，建议 {value}", finding.key);
                println!("            {}", finding.why);
            }
            Verdict::Skipped(reason) => println!("  没有检查  {}（现在是 {current}）：{reason}", finding.key),
        }
    }
    print_open_file_limit();

    let wanted = findings.iter().filter(|finding| matches!(finding.verdict, Verdict::Change(_))).count();
    if wanted == 0 {
        println!("\n没有要改的。");
        return Ok(());
    }
    if !apply_changes {
        println!("\n有 {wanted} 项建议修改。执行 sudo i3t tune --apply：立刻生效，并写进 {PERSIST_FILE}，重启之后还在。");
        return Ok(());
    }
    let changed = apply(&mut sysctl, &findings)?;
    let existing = std::fs::read_to_string(PERSIST_FILE).unwrap_or_default();
    std::fs::write(PERSIST_FILE, persisted(&existing, &changed))
        .with_context(|| format!("参数已经生效，但写 {PERSIST_FILE} 失败，重启之后会变回去"))?;
    println!("\n改了 {} 项，已经生效，并写进了 {PERSIST_FILE}。", changed.len());
    Ok(())
}

/// 能同时打开的文件数。它不是内核参数，这里只报告：`i3t run` 启动时自己会把软上限抬到硬上限。
fn print_open_file_limit() {
    match rlimit::Resource::NOFILE.get() {
        Ok((soft, hard)) => {
            println!("能同时打开的文件数：软上限 {soft}，硬上限 {hard}。i3t run 启动时会把软上限抬到硬上限；经过节点的每条连接占一个。");
            if hard < 65_536 {
                println!("            硬上限偏低，流多的出口或者 relay 可能不够用。装成系统服务时单元文件里的 LimitNOFILE 管这一项。");
            }
        }
        Err(error) => println!("能同时打开的文件数：读不到（{error}）"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// 一张表当内核。`read_only` 的参数写不进去，像没有 root 权限时那样。
    #[derive(Default)]
    struct Table {
        values: HashMap<&'static str, String>,
        read_only: bool,
    }

    impl Table {
        fn with(values: &[(&'static str, &str)]) -> Self {
            Self { values: values.iter().map(|&(key, value)| (key, value.to_owned())).collect(), read_only: false }
        }
    }

    impl Sysctl for Table {
        fn read(&self, key: &str) -> Option<String> {
            self.values.get(key).cloned()
        }

        fn write(&mut self, key: &str, value: &str) -> io::Result<()> {
            if self.read_only {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let slot = self.values.iter_mut().find(|(known, _)| **known == key).ok_or(io::ErrorKind::NotFound)?;
            *slot.1 = value.to_owned();
            Ok(())
        }
    }

    /// 一台没有调过的机器上常见的值。
    fn stock() -> Table {
        Table::with(&[
            ("net.core.rmem_max", "212992"),
            ("net.core.wmem_max", "212992"),
            ("net.ipv4.tcp_slow_start_after_idle", "1"),
            ("net.ipv4.tcp_congestion_control", "cubic"),
            ("net.ipv4.tcp_available_congestion_control", "reno cubic bbr"),
            ("net.ipv4.ip_local_reserved_ports", ""),
            ("net.ipv4.ip_local_port_range", "32768\t60999"),
        ])
    }

    fn verdict_of<'a>(findings: &'a [Finding], key: &str) -> &'a Verdict {
        &findings.iter().find(|finding| finding.key == key).unwrap_or_else(|| panic!("应该检查了 {key}")).verdict
    }

    #[test]
    fn an_untuned_machine_gets_the_four_changes() {
        let findings = survey(&stock(), 20330);
        assert_eq!(verdict_of(&findings, "net.core.rmem_max"), &Verdict::Change("4194304".to_owned()));
        assert_eq!(verdict_of(&findings, "net.core.wmem_max"), &Verdict::Change("4194304".to_owned()));
        assert_eq!(verdict_of(&findings, "net.ipv4.tcp_slow_start_after_idle"), &Verdict::Change("0".to_owned()));
        assert_eq!(verdict_of(&findings, "net.ipv4.tcp_congestion_control"), &Verdict::Change("bbr".to_owned()));
        assert_eq!(verdict_of(&findings, "net.ipv4.ip_local_reserved_ports"), &Verdict::Fine, "20330 不在随机端口的范围里，不用保留");
    }

    #[test]
    fn values_that_are_already_wide_enough_are_left_alone() {
        let mut machine = stock();
        machine.values.insert("net.core.rmem_max", "16777216".to_owned());
        let findings = survey(&machine, 20330);
        assert_eq!(verdict_of(&findings, "net.core.rmem_max"), &Verdict::Fine, "只往大里改，不把已经更大的数改小");
    }

    #[test]
    fn bbr_is_only_suggested_when_the_kernel_has_it() {
        let mut machine = stock();
        machine.values.insert("net.ipv4.tcp_available_congestion_control", "reno cubic".to_owned());
        let findings = survey(&machine, 20330);
        assert!(
            matches!(verdict_of(&findings, "net.ipv4.tcp_congestion_control"), Verdict::Skipped(reason) if reason.contains("modprobe tcp_bbr"))
        );
    }

    #[test]
    fn a_node_port_inside_the_ephemeral_range_is_reserved() {
        let findings = survey(&stock(), 40000);
        assert_eq!(verdict_of(&findings, "net.ipv4.ip_local_reserved_ports"), &Verdict::Change("40000".to_owned()));

        let mut machine = stock();
        machine.values.insert("net.ipv4.ip_local_reserved_ports", "8080,39000-41000".to_owned());
        assert_eq!(verdict_of(&survey(&machine, 40000), "net.ipv4.ip_local_reserved_ports"), &Verdict::Fine, "已经在一段保留的范围里");
        machine.values.insert("net.ipv4.ip_local_reserved_ports", "8080".to_owned());
        let appended = Verdict::Change("8080,40000".to_owned());
        assert_eq!(verdict_of(&survey(&machine, 40000), "net.ipv4.ip_local_reserved_ports"), &appended, "原来保留的端口留着");
    }

    #[test]
    fn a_parameter_the_kernel_lacks_is_reported_not_guessed() {
        let mut machine = stock();
        machine.values.remove("net.ipv4.tcp_slow_start_after_idle");
        let findings = survey(&machine, 20330);
        assert!(matches!(verdict_of(&findings, "net.ipv4.tcp_slow_start_after_idle"), Verdict::Skipped(_)));
    }

    #[test]
    fn applying_writes_every_suggested_value_and_a_second_survey_is_clean() {
        let mut machine = stock();
        let findings = survey(&machine, 40000);
        let changed = apply(&mut machine, &findings).unwrap();
        assert_eq!(changed.len(), 5);
        assert!(survey(&machine, 40000).iter().all(|finding| finding.verdict == Verdict::Fine));
    }

    #[test]
    fn without_permission_applying_fails_and_says_how_to_run_it() {
        let mut machine = stock();
        machine.read_only = true;
        let findings = survey(&machine, 20330);
        let error = apply(&mut machine, &findings).unwrap_err();
        assert!(format!("{error:#}").contains("sudo i3t tune --apply"));
    }

    #[test]
    fn the_persisted_file_keeps_earlier_entries_and_replaces_changed_ones() {
        let existing = "# 旧的说明\nnet.core.rmem_max = 1048576\nnet.ipv4.tcp_slow_start_after_idle = 0\n";
        let text = persisted(existing, &[("net.core.rmem_max", "4194304".to_owned()), ("net.core.wmem_max", "4194304".to_owned())]);
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines, ["net.core.rmem_max = 4194304", "net.core.wmem_max = 4194304", "net.ipv4.tcp_slow_start_after_idle = 0"]);
        assert!(text.starts_with('#'));
    }

    #[test]
    fn port_lists_are_read_with_ranges_and_spaces() {
        assert_eq!(port_range("8080"), Some((8080, 8080)));
        assert_eq!(port_range(" 8000 - 8100 "), Some((8000, 8100)));
        assert_eq!(port_range("x"), None);
    }
}
