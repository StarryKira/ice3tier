//! 装、拆两条防火墙规则，去掉崩溃留下的；Linux 上让 systemd-resolved 把网络的域名交给本节点的名字服务。
//!
//! 规矩：
//!
//! - 每条规则各自装：一条装不上（没有 `iptables`、内核不认）不妨碍另一条和网卡，状态里说清楚哪条为什么（[`RuleState::Failed`]）。
//! - 不会多出一份。Linux 的表整张换（一个 nft 事务）；放行的那条正好一条、排在 INPUT 的第一条，不是的话（崩溃留下的、
//!   被别人挤到了后面的）全删掉再插一条。Windows 的规则按名字先删再加，在同一段脚本里。
//! - 只动认得出是自己的东西（见 [`crate::plan`]）。拆的时候每一样各自拆，一样失败不妨碍别的。
//! - 崩溃留下的、它的网卡已经不在了的规则（这台机器上没有在跑的节点用着它），下次启动或者关着启动时拆掉（[`remove_leftovers`]）。

use std::net::Ipv4Addr;
use std::thread;
use std::time::Duration;

use i3t_model::control::{Firewall, NameService, RuleState, VnetCause};

use crate::device::Platform;
use crate::machine::{CommandError, Machine, Program};
use crate::plan::{Plan, linux, windows};
use crate::survey::Survey;

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
            let guard = machine.run(Program::Nft, &args(&["-f", "-"]), Some(&linux::guard_script(&plan.device, plan.cidr))).map(drop);
            let accept = survey.accept.and_then(|rules| {
                let ours: Vec<usize> = rules.iter().filter(|rule| rule.device == plan.device).map(|rule| rule.position).collect();
                if ours == [1] {
                    return Ok(());
                }
                for _ in &ours {
                    delete_accept(machine, &plan.device)?;
                }
                machine.run(Program::Iptables, &linux::insert_accept(&plan.device), None).map(drop)
            });
            Firewall { accept: state(accept), guard: state(guard) }
        }
        Platform::Windows => {
            let script = windows::install_script(&plan.device, plan.own, plan.cidr);
            let accept = machine.run(Program::PowerShell, &windows::invocation(&script), None).map(drop);
            Firewall { accept: state(accept), guard: RuleState::Unsupported }
        }
        Platform::MacOs | Platform::Other => Firewall { accept: RuleState::Unsupported, guard: RuleState::Unsupported },
    }
}

/// 拆掉 `device` 的两条规则。返回第一个失败（多半是没有权限）。
///
/// # Errors
///
/// 某一条拆不掉，或者读不了现在有哪些。别的照样拆了。
pub fn remove_firewall(machine: &mut impl Machine, platform: Platform, device: &str) -> Result<(), CommandError> {
    match platform {
        Platform::Linux => {
            let survey = Survey::take(machine, platform, device);
            let mut first = None;
            match survey.tables {
                Ok(tables) if tables.iter().any(|table| table == device) => {
                    if let Err(error) = machine.run(Program::Nft, &args(&["-f", "-"]), Some(&linux::remove_table_script(device))) {
                        first.get_or_insert(error);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
            match survey.accept {
                Ok(rules) => {
                    for _ in rules.iter().filter(|rule| rule.device == device) {
                        if let Err(error) = delete_accept(machine, device) {
                            first.get_or_insert(error);
                        }
                    }
                }
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
            first.map_or(Ok(()), Err)
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
/// 读不了或者拆不掉。
pub fn remove_leftovers(machine: &mut impl Machine, platform: Platform, present: &[String]) -> Result<usize, CommandError> {
    match platform {
        Platform::Linux => {
            let survey = Survey::take(machine, platform, "");
            let orphan = |device: &str| !present.iter().any(|name| name == device);
            let mut removed = 0;
            for table in survey.tables?.iter().filter(|table| orphan(table)) {
                machine.run(Program::Nft, &args(&["-f", "-"]), Some(&linux::remove_table_script(table)))?;
                removed += 1;
            }
            for rule in survey.accept?.iter().filter(|rule| orphan(&rule.device)) {
                delete_accept(machine, &rule.device)?;
                removed += 1;
            }
            Ok(removed)
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

/// 删一条 Linux 上自己的放行规则。规则已经不在了不算错。
fn delete_accept(machine: &mut impl Machine, device: &str) -> Result<(), CommandError> {
    match machine.run(Program::Iptables, &linux::delete_accept(device), None) {
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
        /// Windows 防火墙里的规则名。
        pub(crate) windows: Vec<String>,
        pub(crate) resolver: Vec<String>,
        /// 没有这个程序。
        pub(crate) missing: Vec<Program>,
        /// 没有权限：写的命令一律失败。
        pub(crate) unprivileged: bool,
        pub(crate) no_resolver: bool,
        pub(crate) log: Vec<String>,
    }

    fn failed(program: Program, status: i32, detail: &str) -> CommandError {
        CommandError::Failed { program, status: Some(status), detail: detail.to_owned() }
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
                Program::Iptables if line == "-w 5 -S INPUT" => Ok(std::iter::once("-P INPUT ACCEPT".to_owned())
                    .chain(self.input.iter().map(|rule| format!("-A INPUT {rule}")))
                    .collect::<Vec<_>>()
                    .join("\n")),
                Program::Iptables => {
                    if let Some(rule) = line.strip_prefix("-w 5 -I INPUT 1 ") {
                        self.input.insert(0, rule.to_owned());
                        return Ok(String::new());
                    }
                    let rule = line.strip_prefix("-w 5 -D INPUT ").expect("只会插和删");
                    match self.input.iter().position(|existing| existing == rule) {
                        Some(at) => {
                            self.input.remove(at);
                            Ok(String::new())
                        }
                        None => Err(failed(program, 1, "iptables: Bad rule (does a matching rule exist in that chain?).")),
                    }
                }
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
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &["i3t1".into()]).unwrap(), 2, "i3t0 的表和规则");
        assert_eq!(fake.input, ["-i i3t1 -m comment --comment ice3tier -j ACCEPT"]);
        assert_eq!(fake.tables.iter().collect::<Vec<_>>(), ["i3t1"]);
        assert_eq!(remove_leftovers(&mut fake, Platform::Linux, &["i3t1".into()]).unwrap(), 0);
    }

    #[test]
    fn without_iptables_the_guard_still_goes_in_and_the_state_says_which_rule_failed() {
        let mut fake = Fake { missing: vec![Program::Iptables], ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert_eq!(firewall.guard, RuleState::On);
        assert_eq!(firewall.accept, RuleState::Failed { cause: VnetCause::ProgramMissing { program: "iptables".into() } });
        let mut fake = Fake { unprivileged: true, ..Fake::default() };
        let firewall = install_firewall(&mut fake, Platform::Linux, &plan("i3t0"));
        assert!(matches!(firewall.guard, RuleState::Failed { cause: VnetCause::CommandFailed { .. } }));
        assert!(
            remove_firewall(&mut Fake { unprivileged: true, input: vec![OURS.into()], ..Fake::default() }, Platform::Linux, "i3t0")
                .is_err()
        );
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
