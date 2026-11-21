//! 装、拆两条防火墙规则，去掉崩溃留下的；Linux 上让 systemd-resolved 把网络的域名交给本节点的名字服务。
//!
//! 规矩：
//!
//! - 每条规则各自装：一条装不上（内核不认、没有权限）不妨碍另一条和网卡，状态里说清楚哪条为什么（[`RuleState::Failed`]）。
//! - 开箱即用，不要求装哪个程序。Linux 上防冒充有 `nft` 用 nftables 的表，没有就用 `iptables` 在 `raw` 表里加一条，
//!   两个都没有才装不上；放行的那条没有 `iptables` 时用不着（[`RuleState::NotNeeded`]），不算失败。
//! - 不会多出一份。Linux 的表整张换（一个 nft 事务）；放行的那条正好一条、排在链的第一条，raw 表里防冒充的正好两条、排在链的
//!   第一、二条，不是的话（崩溃留下的、地址段变了的、被别人挤到了后面的、以前的版本只有一条的）全删掉再插。表装上了，
//!   以前（还没有 `nft` 时）加的 raw 规则拆掉。
//!   Windows 的规则按名字先删再加，在同一段脚本里。
//! - 只动认得出是自己的东西（见 [`crate::plan`]）。拆的时候每一样各自拆，一样失败不妨碍别的；没有那个程序、链所在的表不在时，
//!   那一类东西本来就没有，不算失败（[`CommandError::means_absent`]）。
//! - 崩溃留下的、它的网卡已经不在了的规则（这台机器上没有在跑的节点用着它），下次启动或者关着启动时拆掉（[`remove_leftovers`]）。

use std::net::Ipv4Addr;
use std::thread;
use std::time::Duration;

use i3t_model::control::{Firewall, NameService, RuleState, VnetCause};
use ipnet::Ipv4Net;

use crate::device::Platform;
use crate::machine::{CommandError, Machine, Program};
use crate::plan::{Plan, linux, windows};
use crate::survey::{AcceptRule, GuardRule, Survey};

/// 一个程序的失败写成状态里的原因。
#[must_use]
pub fn cause(error: CommandError) -> VnetCause {
    match error {
        CommandError::Missing(program) => VnetCause::ProgramMissing { program: program.name().to_owned() },
        CommandError::Failed { program, detail, .. } | CommandError::Io { program, detail } => {
            VnetCause::CommandFailed { program: program.name().to_owned(), detail }
        }
    }
}

fn state(result: Result<(), CommandError>) -> RuleState {
    match result {
        Ok(()) => RuleState::On,
        Err(error) => RuleState::Failed { cause: cause(error) },
    }
}

/// 在 `platform` 上按 `plan` 装上两条规则（本机的开关打开着时才调用）。
pub fn install_firewall(machine: &mut impl Machine, platform: Platform, plan: &Plan) -> Firewall {
    match platform {
        Platform::Linux => {
            let survey = Survey::take(machine, platform, &plan.device);
            let guard = match machine.run(Program::Nft, &args(&["-f", "-"]), Some(&linux::guard_script(&plan.device, plan.own, plan.cidr)))
            {
                Ok(_) => {
                    // 表装上了：还没有 nft 时加的那条 raw 规则拆掉。读不了 raw 表（没有 iptables、内核里没有这张表、没有权限）时
                    // 那里也就没有拆得了的；拆不掉也不要紧，它和表管的是同一件事。表照样算装上了。
                    if let Ok(found) = &survey.guard_rules {
                        for rule in found.iter().filter(|rule| rule.device == plan.device) {
                            let _stale = delete_guard_rule(machine, rule);
                        }
                    }
                    Ok(())
                }
                Err(CommandError::Missing(Program::Nft)) => install_raw_guard(machine, plan, survey.guard_rules),
                Err(error) => Err(error),
            };
            let accept = match survey.accept {
                Err(CommandError::Missing(Program::Iptables)) => RuleState::NotNeeded,
                found => state(found.and_then(|rules| install_accept(machine, &plan.device, &rules))),
            };
            Firewall { accept, guard: state(guard) }
        }
        Platform::Windows => {
            let script = windows::install_script(&plan.device, plan.own, plan.cidr);
            let accept = machine.run(Program::PowerShell, &windows::invocation(&script), None).map(drop);
            Firewall { accept: state(accept), guard: RuleState::Unsupported }
        }
        Platform::MacOs | Platform::Other => Firewall { accept: RuleState::Unsupported, guard: RuleState::Unsupported },
    }
}

/// 拆掉 `device` 的规则：放行的那条，防冒充的表和 raw 规则（哪种都可能在：`nft` 可能是后来装上的）。返回第一个失败（多半是没有权限）。
///
/// # Errors
///
/// 某一条拆不掉，或者读不了现在有哪些。别的照样拆了。没有那个程序、链所在的表不在不算：那一类东西本来就没有。
pub fn remove_firewall(machine: &mut impl Machine, platform: Platform, device: &str) -> Result<(), CommandError> {
    match platform {
        Platform::Linux => {
            let survey = Survey::take(machine, platform, device);
            let tables = remove_each(survey.tables, |table| table == device, |table| remove_table(machine, table));
            let accept = remove_each(survey.accept, |rule| rule.device == device, |_| delete_accept(machine, device));
            let guards = remove_each(survey.guard_rules, |rule| rule.device == device, |rule| delete_guard_rule(machine, rule));
            tables.and(accept).and(guards).map(drop)
        }
        Platform::Windows => delete_windows_rule(machine, device),
        Platform::MacOs | Platform::Other => Ok(()),
    }
}

/// 拆掉崩溃留下的规则：Linux 上是网卡已经不在了的那些（`present` 是这台机器上现在有的网卡的名字：在的网卡有节点程序用着，
/// 它的规则不归这里管）；Windows 上网卡的名字是固定的，`present` 里没有它时它的那条就是留下的。返回拆了几样。
///
/// # Errors
///
/// 读不了或者拆不掉（Linux 上别的项照样拆了）。没有那个程序、链所在的表不在不算：那一类东西本来就没有。
pub fn remove_leftovers(machine: &mut impl Machine, platform: Platform, present: &[String]) -> Result<usize, CommandError> {
    match platform {
        Platform::Linux => {
            let survey = Survey::take(machine, platform, "");
            let orphan = |device: &str| !present.iter().any(|name| name == device);
            let tables = remove_each(survey.tables, |table| orphan(table), |table| remove_table(machine, table));
            let accept = remove_each(survey.accept, |rule| orphan(&rule.device), |rule| delete_accept(machine, &rule.device));
            let guards = remove_each(survey.guard_rules, |rule| orphan(&rule.device), |rule| delete_guard_rule(machine, rule));
            Ok(tables? + accept? + guards?)
        }
        Platform::Windows => {
            let device = crate::device::WINDOWS_NAME;
            if present.iter().any(|name| name == device) {
                return Ok(0);
            }
            let found = Survey::take(machine, platform, device).accept?.len();
            if found > 0 {
                delete_windows_rule(machine, device)?;
            }
            Ok(found)
        }
        Platform::MacOs | Platform::Other => Ok(0),
    }
}

/// Linux 上放行的那条正好一条、排在 INPUT 的第一条；不是的话（崩溃留下的、被别人挤到了后面的）全删掉再插一条。
fn install_accept(machine: &mut impl Machine, device: &str, found: &[AcceptRule]) -> Result<(), CommandError> {
    let ours: Vec<usize> = found.iter().filter(|rule| rule.device == device).map(|rule| rule.position).collect();
    if ours == [1] {
        return Ok(());
    }
    for _ in &ours {
        delete_accept(machine, device)?;
    }
    machine.run(Program::Iptables, &linux::insert_accept(device), None).map(drop)
}

/// 没有 `nft` 时的防冒充：`raw` 表 PREROUTING 链的第一、二条正好是这块网卡的、地址段和虚拟 IP 是现在的那两条（按来源的在前，
/// 按目的的在后）；不是的话（崩溃留下的、地址段或者虚拟 IP 变了的、被别人挤到了后面的、以前的版本只有按来源的一条）这块网卡的
/// 全删掉再插。
/// `found` 是读回来的这条链。
///
/// 两个程序都没有时说缺 `nft`：装上它就是本来的做法，状态里写的是要装 nftables。
fn install_raw_guard(machine: &mut impl Machine, plan: &Plan, found: Result<Vec<GuardRule>, CommandError>) -> Result<(), CommandError> {
    let found = found.map_err(|error| match error {
        CommandError::Missing(_) => CommandError::Missing(Program::Nft),
        other => other,
    })?;
    let ours: Vec<&GuardRule> = found.iter().filter(|rule| rule.device == plan.device).collect();
    let (range, own) = (Some(plan.cidr.trunc()), Some(Ipv4Net::from(plan.own)));
    if let [spoofed, to_us] = ours.as_slice()
        && (spoofed.position, spoofed.range, spoofed.destination) == (1, range, None)
        && (to_us.position, to_us.range, to_us.destination) == (2, None, own)
    {
        return Ok(());
    }
    for rule in ours {
        delete_guard_rule(machine, rule)?;
    }
    for insert in linux::insert_guards(&plan.device, plan.own, plan.cidr) {
        machine.run(Program::Iptables, &insert, None)?;
    }
    Ok(())
}

/// 拆掉读回来的一项里 `pick` 挑出来的每一样，返回拆了几样。要读的东西根本不在（没有那个程序、链所在的表不在）时一样也没有。
fn remove_each<T>(
    found: Result<Vec<T>, CommandError>,
    pick: impl Fn(&T) -> bool,
    mut remove: impl FnMut(&T) -> Result<(), CommandError>,
) -> Result<usize, CommandError> {
    let found = match found {
        Err(error) if error.means_absent() => Vec::new(),
        found => found?,
    };
    found.iter().filter(|thing| pick(thing)).try_fold(0, |removed, thing| remove(thing).map(|()| removed + 1))
}

/// 拆掉 Linux 上自己的一张表。
fn remove_table(machine: &mut impl Machine, name: &str) -> Result<(), CommandError> {
    machine.run(Program::Nft, &args(&["-f", "-"]), Some(&linux::remove_table_script(name))).map(drop)
}

/// 删一条 Linux 上自己的放行规则。规则已经不在了不算错。
fn delete_accept(machine: &mut impl Machine, device: &str) -> Result<(), CommandError> {
    delete_iptables_rule(machine, &linux::delete_accept(device))
}

/// 删一条 Linux 上自己的防冒充规则，按读回来的样子。规则已经不在了不算错。
fn delete_guard_rule(machine: &mut impl Machine, rule: &GuardRule) -> Result<(), CommandError> {
    delete_iptables_rule(machine, &linux::delete_guard(&rule.rule))
}

/// 删一条 iptables 规则：已经不在了（别人删了，同样的两条删第二遍）不算错。
fn delete_iptables_rule(machine: &mut impl Machine, args: &[String]) -> Result<(), CommandError> {
    match machine.run(Program::Iptables, args, None) {
        Err(error) if !error.says("does a matching rule exist") && !error.says("bad rule") => Err(error),
        Ok(_) | Err(_) => Ok(()),
    }
}

/// 删 Windows 上这个名字的规则。没有这条规则不算错（脚本自己这样写的）。
fn delete_windows_rule(machine: &mut impl Machine, device: &str) -> Result<(), CommandError> {
    machine.run(Program::PowerShell, &windows::invocation(&windows::remove_script(device)), None).map(drop)
}

/// systemd-resolved 刚知道一块新网卡要一点时间，这期间说"不认识这块网卡"。最多等这么多次、每次这么久。
const LINK_KNOWN_TRIES: u32 = 20;
const LINK_KNOWN_PAUSE: Duration = Duration::from_millis(100);

/// Linux：让 systemd-resolved 只把 `domain` 下面的名字发给 `server`（本节点虚拟 IP 上的名字服务），挂在网卡 `device` 上：
/// 这块网卡不是默认的 DNS 路线，别的名字照旧由原来的服务器解析，本节点不会变成这台机器的解析器。
///
/// 设置只在内存里，网卡删掉就没了，不写任何文件。没有 systemd-resolved 时是 [`NameService::NoResolver`]：名字用 `i3t hosts`。
pub fn configure_resolver(machine: &mut impl Machine, device: &str, domain: &str, server: Ipv4Addr) -> NameService {
    // 先定路由域、再说"不是默认路线"，最后才给服务器：有了服务器的那一刻，这块网卡就只收这个域名的查询。
    let steps = [
        args(&["domain", device, &format!("~{domain}")]),
        args(&["default-route", device, "false"]),
        args(&["dns", device, &server.to_string()]),
    ];
    for step in &steps {
        let mut tries = 0;
        loop {
            match machine.run(Program::Resolvectl, step, None) {
                Ok(_) => break,
                Err(error) if error.says("not known") && tries < LINK_KNOWN_TRIES => {
                    tries += 1;
                    thread::sleep(LINK_KNOWN_PAUSE);
                }
                Err(error) if resolver_absent(&error) => return NameService::NoResolver,
                Err(error) => {
                    // 做了一半的设置撤掉：太旧的 systemd-resolved（没有 default-route）会把这块网卡当成默认路线。
                    let _undone = machine.run(Program::Resolvectl, &args(&["revert", device]), None);
                    return NameService::Failed { cause: cause(error) };
                }
            }
        }
    }
    NameService::Resolved
}

/// 这个失败说明机器上没有可用的 systemd-resolved，而不是它拒绝了。
fn resolver_absent(error: &CommandError) -> bool {
    matches!(error, CommandError::Missing(_))
        || ["connect to bus", "not found", "could not activate", "no such file", "not running", "connection refused"]
            .iter()
            .any(|needle| error.says(needle))
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[cfg(test)]
pub(crate) mod fake {
    //! 一台假的机器：按收到的命令改自己的防火墙，读的时候按真的程序的样子回答。
    use std::collections::BTreeSet;

    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct Fake {
        /// `ip` 族里的表名。
        pub(crate) tables: BTreeSet<String>,
        /// INPUT 链，按先后：每条是 `iptables -S` 去掉 `-A INPUT ` 的样子。
        pub(crate) input: Vec<String>,
        /// `raw` 表的 PREROUTING 链，按先后：每条是 `iptables -S` 去掉 `-A PREROUTING ` 的样子。
        pub(crate) raw: Vec<String>,
        /// Windows 防火墙里的规则名。
        pub(crate) windows: Vec<String>,
        pub(crate) resolver: Vec<String>,
        /// 没有这个程序。
        pub(crate) missing: Vec<Program>,
        /// 没有权限：写的命令一律失败。
        pub(crate) unprivileged: bool,
        /// 内核里没有 `raw` 表（老的 xtables 后端、没有这个模块）：碰它的命令一律失败。
        pub(crate) no_raw_table: bool,
        pub(crate) no_resolver: bool,
        pub(crate) log: Vec<String>,
    }

    fn failed(program: Program, status: i32, detail: &str) -> CommandError {
        CommandError::Failed { program, status: Some(status), detail: detail.to_owned() }
    }

    /// `iptables -S` 写出来的样子：`-s`、`-d` 排在 `! -i` 前面，不管插的时候怎么写。删的时候也按这个样子比，和真的一样不看次序。
    fn as_listed(rule: &str) -> String {
        let words: Vec<&str> = rule.split(' ').collect();
        match words.as_slice() {
            ["!", "-i", device, flag @ ("-s" | "-d"), range, rest @ ..] => {
                [&[*flag, *range, "!", "-i", *device][..], rest].concat().join(" ")
            }
            _ => rule.to_owned(),
        }
    }

    impl Fake {
        /// iptables 的两条链：INPUT，和 `raw` 表的 PREROUTING。读、插在第一条、按写全了的规则删。
        fn iptables(&mut self, line: &str) -> Result<String, CommandError> {
            let (name, chain, command) = match line.strip_prefix("-w 5 -t raw ") {
                Some(_) if self.no_raw_table => {
                    let detail =
                        "iptables v1.8.9 (legacy): can't initialize iptables table `raw': Table does not exist (do you need to insmod?)";
                    return Err(failed(Program::Iptables, 3, detail));
                }
                Some(command) => ("PREROUTING", &mut self.raw, command),
                None => ("INPUT", &mut self.input, line.strip_prefix("-w 5 ").expect("总是等 xtables 的锁")),
            };
            if command == format!("-S {name}") {
                let listed = chain.iter().map(|rule| format!("-A {name} {rule}"));
                return Ok(std::iter::once(format!("-P {name} ACCEPT")).chain(listed).collect::<Vec<_>>().join("\n"));
            }
            if let Some((position, rule)) = command.strip_prefix(&format!("-I {name} ")).and_then(|rest| rest.split_once(' ')) {
                let position: usize = position.parse().expect("插在第几条");
                assert!(position <= chain.len() + 1, "iptables 不让插在链的末尾之后：{line}");
                chain.insert(position - 1, as_listed(rule));
                return Ok(String::new());
            }
            let rule = as_listed(command.strip_prefix(&format!("-D {name} ")).expect("只会读、插和删"));
            match chain.iter().position(|existing| *existing == rule) {
                Some(at) => {
                    chain.remove(at);
                    Ok(String::new())
                }
                None => Err(failed(Program::Iptables, 1, "iptables: Bad rule (does a matching rule exist in that chain?).")),
            }
        }
    }

    impl Machine for Fake {
        fn run(&mut self, program: Program, args: &[String], input: Option<&str>) -> Result<String, CommandError> {
            let line = args.join(" ");
            self.log.push(format!("{program} {line}"));
            if self.missing.contains(&program) {
                return Err(CommandError::Missing(program));
            }
            let reads = line.starts_with("-j list") || line.contains(" -S ") || line.contains("$rules.Count");
            if self.unprivileged && !reads {
                return Err(failed(program, 1, "Operation not permitted"));
            }
            match program {
                Program::Nft if line == "-j list tables ip" => {
                    let tables: Vec<String> =
                        self.tables.iter().map(|name| format!(r#"{{"table": {{"family": "ip", "name": "{name}"}}}}"#)).collect();
                    Ok(format!(r#"{{"nftables": [{}]}}"#, tables.join(", ")))
                }
                Program::Nft if line == "-f -" => {
                    let script = input.unwrap_or_default();
                    let name = script.lines().next().and_then(|first| first.strip_prefix("table ip ")).expect("脚本先保证有表");
                    assert!(script.contains(&format!("delete table ip {name}\n")), "整张表在一个事务里换：{script}");
                    if script.lines().count() > 2 {
                        self.tables.insert(name.to_owned());
                    } else {
                        self.tables.remove(name);
                    }
                    Ok(String::new())
                }
                Program::Iptables => self.iptables(&line),
                Program::PowerShell => {
                    let name = line.split("-Name '").nth(1).and_then(|rest| rest.split('\'').next()).unwrap_or_default().to_owned();
                    if line.contains("$rules.Count") {
                        return Ok(format!("{}\r\n", self.windows.iter().filter(|existing| **existing == name).count()));
                    }
                    assert!(line.contains("Remove-NetFirewallRule"), "装之前先删同名的：{line}");
                    self.windows.retain(|existing| *existing != name);
                    if line.contains("New-NetFirewallRule") {
                        self.windows.push(name);
                    }
                    Ok(String::new())
                }
                Program::Resolvectl if self.no_resolver => {
                    Err(failed(program, 1, "Failed to set DNS configuration: Unit dbus-org.freedesktop.resolve1.service not found."))
                }
                Program::Resolvectl => {
                    self.resolver.push(line);
                    Ok(String::new())
                }
                other => panic!("假机器不认识这条命令：{other} {line}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    fn plan(device: &str) -> Plan {
        Plan { device: device.into(), own: Ipv4Addr::new(10, 66, 0, 1), cidr: "10.66.0.0/16".parse().unwrap() }
    }

    const OURS: &str = "-i i3t0 -m comment --comment ice3tier -j ACCEPT";
    /// 没有 nft 时防冒充的那条，`iptables -S` 写出来的样子。
    const GUARD: &str = "-s 10.66.0.0/16 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP";
    /// 防冒充的第二条：目的是本机的地址。
    const TO_US: &str = "-d 10.66.0.1/32 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP";
    const GUARD_I3T1: &str = "-s 10.66.0.0/16 ! -i i3t1 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP";
    const TO_US_I3T1: &str = "-d 10.66.0.1/32 ! -i i3t1 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP";
    /// Docker 在 raw 表里加的那种规则：别人的，不动。
    const DOCKER: &str = "-d 172.17.0.2/32 ! -i docker0 -j DROP";

    #[test]
    fn on_linux_both_rules_go_in_and_come_out_and_nothing_else_is_touched() {
        let mut fake = Fake { input: vec!["-j ufw-before-input".into()], tables: ["filter".to_owned()].into(), ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(firewall, Firewall { accept: RuleState::On, guard: RuleState::On });
        assert_eq!(fake.input, [OURS, "-j ufw-before-input"], "放行的那条插在最前面");
        assert!(fake.tables.contains("i3t0"));
        remove_firewall(&mut fake, Platform::Linux, "i3t0").unwrap();
        assert_eq!(fake.input, ["-j ufw-before-input"]);
        assert_eq!(fake.tables.iter().collect::<Vec<_>>(), ["filter"], "别人的表不动");
    }

    #[test]
    fn a_leftover_after_a_crash_is_replaced_not_duplicated() {
        let mut fake = Fake::default();
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        // 进程被杀掉，什么都没拆；别的程序之后在最前面插了一条。重启之后再装。
        fake.input.insert(0, "-s 192.0.2.9/32 -j DROP".into());
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(firewall.accept, RuleState::On);
        assert_eq!(fake.input, [OURS, "-s 192.0.2.9/32 -j DROP"], "还是一条，又回到最前面");
        assert_eq!(fake.tables.len(), 1);
        // 已经在最前面的不动。
        fake.log.clear();
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert!(
            !fake.log.iter().any(|line| line.starts_with("iptables -w 5 -I") || line.starts_with("iptables -w 5 -D")),
            "{:?}",
            fake.log
        );
        // 两份（更早的版本被杀掉两次）也收成一份。
        fake.input.push(OURS.into());
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(fake.input.iter().filter(|rule| *rule == OURS).count(), 1);
    }

    #[test]
    fn leftovers_whose_device_is_gone_are_removed_and_a_running_nodes_are_not() {
        let mut fake = Fake::default();
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        install_firewall(&mut fake, Platform::Linux, &plan("i3t1"));
        // 还没有 nft 时 i3t0 加的那条 raw 规则也留着。
        fake.raw.push(GUARD.into());
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &["i3t1".into()]).unwrap(), 3, "i3t0 的表、放行和 raw 规则");
        assert_eq!(fake.input, ["-i i3t1 -m comment --comment ice3tier -j ACCEPT"]);
        assert_eq!(fake.tables.iter().collect::<Vec<_>>(), ["i3t1"]);
        assert!(fake.raw.is_empty());
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &["i3t1".into()]).unwrap(), 0);

        // 没有 nft 的机器：防冒充是 raw 规则，在的网卡的不动。
        let mut fake = Fake { missing: vec![Program::Nft], raw: vec![DOCKER.into()], ..Fake::default() };
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        install_firewall(&mut fake, Platform::Linux, &plan("i3t1"));
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &["i3t1".into()]).unwrap(), 3, "i3t0 的放行和两条 raw 规则");
        assert_eq!(fake.raw, [GUARD_I3T1, TO_US_I3T1, DOCKER]);
        assert_eq!(fake.input, ["-i i3t1 -m comment --comment ice3tier -j ACCEPT"]);
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &[]).unwrap(), 3);
        assert_eq!(fake.raw, [DOCKER], "别人的规则不动");
    }

    #[test]
    fn without_iptables_the_accept_rule_is_not_needed_and_the_guard_goes_in_with_nft() {
        let mut fake = Fake { missing: vec![Program::Iptables], ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(firewall, Firewall { accept: RuleState::NotNeeded, guard: RuleState::On });
        assert!(fake.tables.contains("i3t0"));
        let tried: Vec<&String> = fake.log.iter().filter(|line| line.starts_with("iptables ")).collect();
        assert!(!tried.is_empty() && tried.iter().all(|line| line.contains(" -S ")), "iptables 只读过一下，发现没有：{tried:?}");

        // 别的失败（没有权限）还是失败。
        let mut fake = Fake { unprivileged: true, ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert!(matches!(firewall.guard, RuleState::Failed { cause: VnetCause::CommandFailed { .. } }));
        assert!(matches!(firewall.accept, RuleState::Failed { cause: VnetCause::CommandFailed { .. } }));
        let mut fake = Fake { unprivileged: true, missing: vec![Program::Nft], ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert!(
            matches!(firewall.guard, RuleState::Failed { cause: VnetCause::CommandFailed { ref program, .. } } if program == "iptables")
        );
        assert!(
            remove_firewall(&mut Fake { unprivileged: true, input: vec![OURS.into()], ..Fake::default() }, Platform::Linux, "i3t0")
                .is_err()
        );
        assert!(
            remove_firewall(&mut Fake { unprivileged: true, raw: vec![GUARD.into()], ..Fake::default() }, Platform::Linux, "i3t0").is_err()
        );
    }

    #[test]
    fn without_nft_the_guard_is_two_rules_first_in_raw_prerouting() {
        let mut fake = Fake { missing: vec![Program::Nft], raw: vec![DOCKER.into()], ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(firewall, Firewall { accept: RuleState::On, guard: RuleState::On });
        assert_eq!(fake.raw, [GUARD, TO_US, DOCKER], "插在最前面，按来源的在前");
        for insert in linux::insert_guards("i3t0", plan("i3t0").own, plan("i3t0").cidr) {
            let inserted = format!("iptables {}", insert.join(" "));
            assert!(fake.log.contains(&inserted), "{:?}", fake.log);
        }

        // 再装一遍：已经在最前面、地址段对的不动。
        fake.log.clear();
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(fake.raw, [GUARD, TO_US, DOCKER]);
        assert!(!fake.log.iter().any(|line| line.contains(" -I ") || line.contains(" -D ")), "{:?}", fake.log);

        // 被别人挤到了后面：回到最前面，还是两条。
        fake.raw.insert(0, "-s 192.0.2.9/32 -j DROP".into());
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(fake.raw, [GUARD, TO_US, "-s 192.0.2.9/32 -j DROP", DOCKER]);
        fake.raw.insert(1, "-s 192.0.2.9/32 -j DROP".into());
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(fake.raw, [GUARD, TO_US, "-s 192.0.2.9/32 -j DROP", "-s 192.0.2.9/32 -j DROP", DOCKER], "两条中间插进了别的");
        fake.raw.drain(2..3);
        fake.raw.swap(0, 1);
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(fake.raw, [GUARD, TO_US, "-s 192.0.2.9/32 -j DROP", DOCKER], "次序反了");

        // 两份（更早的版本被杀掉两次）收成一份。
        fake.raw.push(GUARD.into());
        fake.raw.push(TO_US.into());
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(fake.raw, [GUARD, TO_US, "-s 192.0.2.9/32 -j DROP", DOCKER]);

        // 拆：只拆自己的。
        remove_firewall(&mut fake, Platform::Linux, "i3t0").unwrap();
        assert_eq!(fake.raw, ["-s 192.0.2.9/32 -j DROP", DOCKER]);
        assert!(fake.input.is_empty());
    }

    #[test]
    fn without_nft_a_changed_virtual_ip_replaces_the_rule_for_it() {
        let mut fake = Fake { missing: vec![Program::Nft], ..Fake::default() };
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        let renumbered = Plan { own: Ipv4Addr::new(10, 66, 0, 9), ..plan("i3t0") };
        assert_eq!(install_firewall(&mut fake, Platform::Linux, &renumbered).guard, RuleState::On);
        assert_eq!(fake.raw, [GUARD, "-d 10.66.0.9/32 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP"]);
    }

    #[test]
    fn without_nft_the_single_rule_of_an_earlier_version_gets_its_second_rule() {
        let mut fake = Fake { missing: vec![Program::Nft], raw: vec![GUARD.into(), DOCKER.into()], ..Fake::default() };
        assert_eq!(install_firewall(&mut fake, Platform::Linux, &plan("i3t0")).guard, RuleState::On);
        assert_eq!(fake.raw, [GUARD, TO_US, DOCKER]);
    }

    #[test]
    fn without_nft_a_changed_range_replaces_the_raw_rule_and_other_adapters_keep_theirs() {
        let mut fake = Fake { missing: vec![Program::Nft], ..Fake::default() };
        install_firewall(&mut fake, Platform::Linux, &plan("i3t1"));
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        let moved = Plan { own: Ipv4Addr::new(10, 77, 0, 1), cidr: "10.77.0.0/16".parse().unwrap(), ..plan("i3t0") };
        let firewall = install_firewall(&mut fake, Platform::Linux, &moved);
        assert_eq!(firewall.guard, RuleState::On);
        assert_eq!(
            fake.raw,
            [
                "-s 10.77.0.0/16 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP",
                "-d 10.77.0.1/32 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP",
                GUARD_I3T1,
                TO_US_I3T1
            ],
            "旧地址段的两条换掉了，i3t1 的不动"
        );
    }

    #[test]
    fn once_nft_is_there_the_table_takes_over_from_the_raw_rule_left_from_before() {
        let mut fake = Fake { raw: vec![GUARD.into(), TO_US.into(), GUARD_I3T1.into(), DOCKER.into()], ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(firewall, Firewall { accept: RuleState::On, guard: RuleState::On });
        assert!(fake.tables.contains("i3t0"));
        assert_eq!(fake.raw, [GUARD_I3T1, DOCKER], "只拆这块网卡的");

        // raw 表读不了、拆不掉：表照样算装上了。
        let mut fake = Fake { no_raw_table: true, ..Fake::default() };
        assert_eq!(install_firewall(&mut fake, Platform::Linux, &plan("i3t0")), Firewall { accept: RuleState::On, guard: RuleState::On });
        assert!(!fake.log.iter().any(|line| line.contains(" -t raw -D ")), "读不出来就不删：{:?}", fake.log);
    }

    #[test]
    fn with_neither_program_the_guard_fails_for_want_of_nft_and_the_accept_rule_is_not_needed() {
        let mut fake = Fake { missing: vec![Program::Nft, Program::Iptables], ..Fake::default() };
        assert_eq!(
            install_firewall(&mut fake, Platform::Linux, &plan("i3t0")),
            Firewall {
                accept: RuleState::NotNeeded,
                guard: RuleState::Failed { cause: VnetCause::ProgramMissing { program: "nft".into() } }
            }
        );
        remove_firewall(&mut fake, Platform::Linux, "i3t0").unwrap();
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &[]).unwrap(), 0);
    }

    #[test]
    fn removing_where_a_program_or_the_raw_table_is_missing_succeeds_and_removes_what_is_there() {
        let mut fake = Fake { missing: vec![Program::Nft], ..Fake::default() };
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        remove_firewall(&mut fake, Platform::Linux, "i3t0").unwrap();
        assert!(fake.raw.is_empty() && fake.input.is_empty());

        let mut fake = Fake { missing: vec![Program::Iptables], ..Fake::default() };
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        remove_firewall(&mut fake, Platform::Linux, "i3t0").unwrap();
        assert!(fake.tables.is_empty());

        let mut fake = Fake { no_raw_table: true, ..Fake::default() };
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        remove_firewall(&mut fake, Platform::Linux, "i3t0").unwrap();
        assert!(fake.tables.is_empty() && fake.input.is_empty());
        install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &[]).unwrap(), 2, "表和放行的那条");
    }

    #[test]
    fn on_windows_one_named_rule_on_our_adapter_admits_the_range_and_the_guard_is_left_to_the_strong_host_model() {
        let mut fake = Fake { windows: vec!["ice3tier-i3t0".into()], ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Windows, &plan("i3t0"));
        assert_eq!(firewall, Firewall { accept: RuleState::On, guard: RuleState::Unsupported });
        assert_eq!(fake.windows, ["ice3tier-i3t0"], "留下的那条先删掉，不会多一条");
        assert!(
            fake.log.iter().any(|line| line.contains("-InterfaceAlias 'i3t0' -LocalAddress 10.66.0.1 -RemoteAddress 10.66.0.0/16")),
            "{:?}",
            fake.log
        );
        assert_eq!(remove_leftovers(&mut fake, Platform::Windows, &["i3t0".into()]).unwrap(), 0, "网卡在：不是留下的");
        assert_eq!(remove_leftovers(&mut fake, Platform::Windows, &[]).unwrap(), 1);
        assert!(fake.windows.is_empty());
        remove_firewall(&mut fake, Platform::Windows, "i3t0").unwrap();
        let mut fake = Fake { unprivileged: true, ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Windows, &plan("i3t0"));
        assert!(matches!(firewall.accept, RuleState::Failed { cause: VnetCause::CommandFailed { .. } }), "{firewall:?}");
    }

    #[test]
    fn on_macos_nothing_is_added() {
        let mut fake = Fake::default();
        assert_eq!(
            install_firewall(&mut fake, Platform::MacOs, &plan("utun7")),
            Firewall { accept: RuleState::Unsupported, guard: RuleState::Unsupported }
        );
        assert!(fake.log.is_empty());
    }

    #[test]
    fn the_resolver_gets_a_routing_domain_on_our_link_and_is_never_the_default_route() {
        let mut fake = Fake::default();
        assert_eq!(configure_resolver(&mut fake, "i3t0", "home.i3t", [10, 66, 0, 1].into()), NameService::Resolved);
        assert_eq!(fake.resolver, ["domain i3t0 ~home.i3t", "default-route i3t0 false", "dns i3t0 10.66.0.1"], "服务器最后才给");
        let mut fake = Fake { no_resolver: true, ..Fake::default() };
        assert_eq!(configure_resolver(&mut fake, "i3t0", "home.i3t", [10, 66, 0, 1].into()), NameService::NoResolver);
        let mut fake = Fake { missing: vec![Program::Resolvectl], ..Fake::default() };
        assert_eq!(configure_resolver(&mut fake, "i3t0", "home.i3t", [10, 66, 0, 1].into()), NameService::NoResolver);
    }
}
