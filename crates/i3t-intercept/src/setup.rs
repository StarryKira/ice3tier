//! 装、更新、拆。
//!
//! 装（[`install`]）的规矩：
//!
//! 1. 先读一遍系统（[`Survey`]）。表 [`ROUTE_TABLE`] 里有别人的路由、或者有一块叫 [`LINK`] 却不是自己建的网卡时不装，
//!    说明是谁挡着：这些不是自己的东西，不能替别人改。网络的地址段和这台机器所在的真实网络重叠时（`overlap` 模块）
//!    也不装：装上会把它从那个网络上切下来。这时上次留下的自己的东西照样拆掉，它们一样会切断那个网络。
//! 2. nft 的表整张换（[`Plan::ruleset`]）：上次崩溃留下的表在同一个事务里被换掉。
//! 3. 策略路由规则正好一条：已经有一条自己的就留着，多了（以前留下的）就都删掉重加一条。路由用 `replace`。
//! 4. 网卡：没有就建；已经有自己的就沿用，地址不对就换成现在的虚拟 IP。
//!
//! 中途哪一步失败，已经装上的全部拆掉（[`remove`]），系统回到没装的样子，再把失败原因交给调用方。
//!
//! 拆（[`remove`]）每一项各自做，一项失败不妨碍别的项；只动认得出是自己的东西。

use std::net::Ipv4Addr;
use std::thread;
use std::time::Duration;

use crate::machine::{CommandError, Machine, Program};
use crate::overlap;
use crate::plan::{self, Plan, REMOVE_TABLE};
use crate::survey::{Link, Survey};
use crate::{LINK, ROUTE_TABLE};

/// 装不上的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Command(#[from] CommandError),
    /// 表 [`ROUTE_TABLE`] 里有别的程序的路由。
    #[error("路由表 {ROUTE_TABLE} 里有 {routes} 条不是 ice3tier 加的路由，这张表被别的程序占着")]
    TableInUse { routes: usize },
    /// 已经有一块叫 [`LINK`] 的网卡，不是自己建的。
    #[error("已经有一块叫 {LINK} 的网卡，不是 ice3tier 建的")]
    LinkInUse,
    /// 网络的地址段和这台机器上的一个地址或者一条路由重叠（`with` 是 `ip` 的写法：`192.168.1.5/24 dev eth0`）。
    #[error("网络的地址段和这台机器上的 {with} 重叠，装上会把它从那个网络上切下来")]
    RangeInUse { with: String },
}

/// 装好了。`replaced` 为真时系统里本来就有上次留下的东西（进程崩溃、被杀掉），这次把它换掉了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installed {
    pub replaced: bool,
}

/// 按 `plan` 装上透明访问。规矩见模块文档。
///
/// # Errors
///
/// 没有权限、少了程序、内核不认某条规则（`nft` 报的原话在错误里），或者有别人的东西挡着。
pub fn install(machine: &mut impl Machine, plan: &Plan) -> Result<Installed, SetupError> {
    let survey = Survey::take(machine);
    // 读不了表通常是没有权限，这时后面每一步都会失败，直接报这个。
    if let Err(error) = &survey.table {
        return Err(error.clone().into());
    }
    let routes = survey.foreign_routes();
    if routes > 0 {
        return Err(SetupError::TableInUse { routes });
    }
    if survey.foreign_link() {
        return Err(SetupError::LinkInUse);
    }
    if let Some(with) = overlap::find(machine, plan.cidr)? {
        if survey.found_any() {
            // 上次留下的规则同样会把那个网络切掉；拆它出的错不如"重叠"有用，只交后者。
            let _ = remove(machine);
        }
        return Err(SetupError::RangeInUse { with });
    }
    let replaced = !survey.is_clean();
    match apply(machine, plan, &survey) {
        Ok(()) => Ok(Installed { replaced }),
        Err(error) => {
            // 拆的时候再出的错不如第一个错有用，只交第一个。
            let _ = remove(machine);
            Err(error)
        }
    }
}

fn apply(machine: &mut impl Machine, plan: &Plan, survey: &Survey) -> Result<(), SetupError> {
    machine.run(Program::Nft, &args(&["-f", "-"]), Some(&plan.ruleset()))?;

    let rules = survey.rules.clone()?;
    if rules.len() != 1 {
        for _ in &rules {
            delete_rule(machine)?;
        }
        machine.run(Program::Ip, &plan::rule_args("add"), None)?;
    }
    machine.run(Program::Ip, &plan::route_args(), None)?;

    let wanted = format!("{}/32", plan.own);
    match &survey.link {
        Ok(Some(Link { ours: true, addresses, .. })) if *addresses == [wanted.clone()] => {}
        Ok(Some(Link { ours: true, .. })) => {
            machine.run(Program::Ip, &plan::link_flush_args(), None)?;
            machine.run(Program::Ip, &plan.link_address_args(), None)?;
        }
        Ok(_) => {
            machine.run(Program::Ip, &plan::link_create_args(), None)?;
            machine.run(Program::Ip, &plan.link_address_args(), None)?;
        }
        Err(error) => return Err(error.clone().into()),
    }
    machine.run(Program::Ip, &plan::link_up_args(), None)?;
    Ok(())
}

/// 删一条自己的规则。规则已经不在了不算错。
fn delete_rule(machine: &mut impl Machine) -> Result<(), CommandError> {
    match machine.run(Program::Ip, &plan::rule_args("del"), None) {
        Err(error) if !error.says("no such file") => Err(error),
        Ok(_) | Err(_) => Ok(()),
    }
}

/// 只换掉对端的集合（访问规则或者对端变了）。表不在时（被别人删掉了）报错，调用方应该整个重装。
///
/// # Errors
///
/// `nft` 失败。
pub fn update_peers(machine: &mut impl Machine, plan: &Plan) -> Result<(), SetupError> {
    machine.run(Program::Nft, &args(&["-f", "-"]), Some(&plan.peers_script()))?;
    Ok(())
}

/// 拆掉透明访问装的全部东西，只动认得出是自己的。每一项各自做；返回第一个失败（多半是没有权限）。
///
/// # Errors
///
/// 某一项拆不掉。别的项照样拆了。
pub fn remove(machine: &mut impl Machine) -> Result<(), SetupError> {
    let survey = Survey::take(machine);
    let mut first: Option<SetupError> = None;
    let mut keep = |result: Result<(), SetupError>| {
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    };

    if !matches!(survey.table, Ok(None)) {
        keep(machine.run(Program::Nft, &args(&["-f", "-"]), Some(REMOVE_TABLE)).map(drop).map_err(SetupError::from));
    }
    let rules = survey.rules.clone().unwrap_or_default();
    for _ in &rules {
        keep(delete_rule(machine).map_err(SetupError::from));
    }
    if survey.routes.as_ref().is_ok_and(|routes| routes.iter().any(|route| route.ours)) {
        keep(machine.run(Program::Ip, &plan::flush_routes_args(), None).map(drop).map_err(SetupError::from));
    }
    if matches!(survey.link, Ok(Some(Link { ours: true, .. }))) {
        // 网卡一删，systemd-resolved 就忘了它上面的设置；先撤一下只是更干净，撤不了不要紧。
        let _ = machine.run(Program::Resolvectl, &args(&["revert", LINK]), None);
        keep(machine.run(Program::Ip, &plan::link_delete_args(), None).map(drop).map_err(SetupError::from));
    }
    first.map_or(Ok(()), Err)
}

/// 让 systemd-resolved 把网络的域名分给本节点的名字服务的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverOutcome {
    /// 设好了：`<名字>.<域名>` 的查询发到 [`LINK`] 上的名字服务，别的查询不经过它。
    Configured,
    /// 这台机器没有在跑 systemd-resolved（没有 `resolvectl`，或者连不上它）。名字要靠 `/etc/hosts`（`i3t hosts`）。
    Absent,
    /// systemd-resolved 在，但是不接受设置（多半是权限）。
    Failed(CommandError),
}

/// systemd-resolved 刚知道一块新网卡要一点时间，这期间说"不认识这块网卡"。最多等这么多次、每次这么久。
const LINK_KNOWN_TRIES: u32 = 20;
const LINK_KNOWN_PAUSE: Duration = Duration::from_millis(100);

/// 让 systemd-resolved 只把 `domain` 下面的名字发给 `server`（本节点虚拟 IP 上的名字服务），挂在 [`LINK`] 上：
/// 这块网卡不是默认的 DNS 路线，别的名字照旧由原来的服务器解析，本节点不会变成这台机器的解析器。
///
/// 设置只在内存里，网卡删掉就没了，不写任何文件。
pub fn configure_resolver(machine: &mut impl Machine, domain: &str, server: Ipv4Addr) -> ResolverOutcome {
    // 先定路由域、再说"不是默认路线"，最后才给服务器：有了服务器的那一刻，这块网卡就只收这个域名的查询。
    let steps = [
        args(&["domain", LINK, &format!("~{domain}")]),
        args(&["default-route", LINK, "false"]),
        args(&["dns", LINK, &server.to_string()]),
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
                Err(error) if resolver_absent(&error) => return ResolverOutcome::Absent,
                Err(error) => {
                    // 做了一半的设置撤掉：太旧的 systemd-resolved（没有 default-route）会把这块网卡当成默认路线。
                    let _ = machine.run(Program::Resolvectl, &args(&["revert", LINK]), None);
                    return ResolverOutcome::Failed(error);
                }
            }
        }
    }
    ResolverOutcome::Configured
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
    //! 一台假的机器：按收到的命令改自己的状态，读的时候按真的程序的样子写 JSON。
    use std::collections::BTreeSet;
    use std::net::Ipv4Addr;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct FakeLink {
        pub(crate) kind: &'static str,
        pub(crate) alias: Option<&'static str>,
        pub(crate) addresses: Vec<String>,
        pub(crate) up: bool,
    }

    #[derive(Debug, Default)]
    pub(crate) struct Fake {
        /// 那张表在不在，在的话集合里有哪些对端。
        pub(crate) table: Option<BTreeSet<Ipv4Addr>>,
        /// `ip rule` 的条目，按（优先级，写法）记。
        pub(crate) rules: Vec<(u32, String)>,
        /// 表 2033 里的路由：（写法，是不是自己的）。
        pub(crate) routes: Vec<(String, bool)>,
        pub(crate) link: Option<FakeLink>,
        pub(crate) resolver: Vec<String>,
        /// 没有权限：读写包过滤规则、改路由一律失败。
        pub(crate) unprivileged: bool,
        /// nft 不认含有这个词的脚本（比如内核没有 tproxy）。
        pub(crate) nft_rejects: Option<&'static str>,
        /// 没有 systemd-resolved。
        pub(crate) no_resolver: bool,
        /// systemd-resolved 太旧，不认 `default-route`。
        pub(crate) old_resolver: bool,
        /// 别的网卡上的 IPv4 地址：（网卡，`地址/前缀`）。[`LINK`] 上的地址在 `link` 里。
        pub(crate) addresses: Vec<(&'static str, &'static str)>,
        /// `main` 表里的路由：（目的，网卡）。
        pub(crate) main_routes: Vec<(&'static str, &'static str)>,
        pub(crate) log: Vec<String>,
    }

    fn failed(program: Program, detail: &str) -> CommandError {
        CommandError::Failed { program, status: Some(1), detail: detail.to_owned() }
    }

    const OUR_RULE: &str = "fwmark 0x20000000/0x20000000 lookup 2033 proto 203";

    impl Machine for Fake {
        fn run(&mut self, program: Program, args: &[String], input: Option<&str>) -> Result<String, CommandError> {
            let line = args.join(" ");
            self.log.push(format!("{program} {line}"));
            let writes = !line.starts_with("-j");
            if self.unprivileged && program == Program::Nft {
                return Err(failed(program, "Error: Could not process rule: Operation not permitted"));
            }
            if self.unprivileged && program == Program::Ip && writes {
                return Err(failed(program, "RTNETLINK answers: Operation not permitted"));
            }
            match (program, line.as_str()) {
                (Program::Nft, "-j list table ip i3t") => match &self.table {
                    None => Err(failed(program, "Error: No such file or directory\nlist table ip i3t")),
                    Some(peers) => {
                        let elem: Vec<String> = peers.iter().map(|ip| format!("\"{ip}\"")).collect();
                        Ok(format!(
                            r#"{{"nftables": [{{"table": {{"family": "ip", "name": "i3t"}}}}, {{"chain": {{"name": "intercept"}}}}, {{"set": {{"name": "peers", "elem": [{}]}}}}]}}"#,
                            elem.join(", ")
                        ))
                    }
                },
                (Program::Nft, "-f -") => {
                    let script = input.unwrap_or_default();
                    if self.nft_rejects.is_some_and(|word| script.contains(word)) {
                        return Err(failed(program, "Error: Could not process rule: No such file or directory"));
                    }
                    if script == REMOVE_TABLE {
                        self.table = None;
                    } else if script.starts_with("flush set") {
                        let Some(peers) = self.table.as_mut() else { return Err(failed(program, "Error: No such file or directory")) };
                        *peers = elements(script);
                    } else {
                        assert!(script.starts_with("table ip i3t\ndelete table ip i3t\ntable ip i3t {"), "整张表要在一个事务里换：{script}");
                        self.table = Some(elements(script));
                    }
                    Ok(String::new())
                }
                (Program::Ip, "-j rule show") => Ok(format!(
                    "[{}]",
                    self.rules
                        .iter()
                        .map(|(priority, text)| {
                            let ours = text == OUR_RULE;
                            let protocol = if ours { r#","protocol":"203""# } else { "" };
                            format!(r#"{{"priority":{priority},"src":"all","fwmark":"0x20000000","fwmask":"0x20000000","table":"2033"{protocol}}}"#)
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                (Program::Ip, rule) if rule.starts_with("rule add priority 2033 ") => {
                    self.rules.push((2033, rule.trim_start_matches("rule add priority 2033 ").to_owned()));
                    Ok(String::new())
                }
                (Program::Ip, rule) if rule.starts_with("rule del priority 2033 ") => {
                    let text = rule.trim_start_matches("rule del priority 2033 ");
                    match self.rules.iter().position(|(priority, existing)| *priority == 2033 && existing == text) {
                        Some(at) => {
                            self.rules.remove(at);
                            Ok(String::new())
                        }
                        None => Err(failed(program, "RTNETLINK answers: No such file or directory")),
                    }
                }
                (Program::Ip, "-j -4 addr show") => {
                    let link = self.link.iter().flat_map(|link| link.addresses.iter().map(|address| ("i3t0", address.as_str())));
                    let entries: Vec<String> = self
                        .addresses
                        .iter()
                        .copied()
                        .chain(link)
                        .map(|(name, address)| {
                            let (ip, prefix) = address.split_once('/').unwrap();
                            format!(r#"{{"ifname":"{name}","addr_info":[{{"family":"inet","local":"{ip}","prefixlen":{prefix}}}]}}"#)
                        })
                        .collect();
                    Ok(format!("[{}]", entries.join(",")))
                }
                (Program::Ip, "-j -4 route show table main") => {
                    let entries: Vec<String> =
                        self.main_routes.iter().map(|(dst, dev)| format!(r#"{{"dst":"{dst}","dev":"{dev}"}}"#)).collect();
                    Ok(format!("[{}]", entries.join(",")))
                }
                (Program::Ip, "-j route show table 2033") => {
                    if self.routes.is_empty() {
                        return Err(failed(program, "Error: ipv4: FIB table does not exist.\nDump terminated"));
                    }
                    let routes: Vec<String> = self
                        .routes
                        .iter()
                        .map(|(dst, ours)| {
                            let protocol = if *ours { r#","protocol":"203""# } else { "" };
                            format!(r#"{{"type":"local","dst":"{dst}","dev":"lo"{protocol}}}"#)
                        })
                        .collect();
                    Ok(format!("[{}]", routes.join(",")))
                }
                (Program::Ip, "route replace local 0.0.0.0/0 dev lo table 2033 proto 203") => {
                    if !self.routes.iter().any(|(dst, ours)| *ours && dst == "default") {
                        self.routes.push(("default".to_owned(), true));
                    }
                    Ok(String::new())
                }
                (Program::Ip, "route flush table 2033 proto 203") => {
                    self.routes.retain(|(_, ours)| !ours);
                    Ok(String::new())
                }
                (Program::Ip, "-j -d addr show dev i3t0") => match &self.link {
                    None => Err(failed(program, "Device \"i3t0\" does not exist.")),
                    Some(link) => {
                        let alias = link.alias.map(|alias| format!(r#""ifalias":"{alias}","#)).unwrap_or_default();
                        let addresses: Vec<String> = link
                            .addresses
                            .iter()
                            .map(|address| {
                                let (ip, prefix) = address.split_once('/').unwrap();
                                format!(r#"{{"family":"inet","local":"{ip}","prefixlen":{prefix}}}"#)
                            })
                            .collect();
                        Ok(format!(r#"[{{"ifname":"i3t0",{alias}"linkinfo":{{"info_kind":"{}"}},"addr_info":[{}]}}]"#, link.kind, addresses.join(",")))
                    }
                },
                (Program::Ip, "link add i3t0 type dummy") => {
                    assert!(self.link.is_none(), "网卡已经有了还在建");
                    self.link = Some(FakeLink { kind: "dummy", alias: None, addresses: Vec::new(), up: false });
                    Ok(String::new())
                }
                (Program::Ip, "link set i3t0 alias ice3tier up") => {
                    let link = self.link.as_mut().expect("网卡要先建");
                    (link.alias, link.up) = (Some("ice3tier"), true);
                    Ok(String::new())
                }
                (Program::Ip, "addr flush dev i3t0") => {
                    self.link.as_mut().expect("网卡要先建").addresses.clear();
                    Ok(String::new())
                }
                (Program::Ip, address) if address.starts_with("addr add ") && address.ends_with(" dev i3t0") => {
                    let ip = address.trim_start_matches("addr add ").trim_end_matches(" dev i3t0");
                    self.link.as_mut().expect("网卡要先建").addresses.push(ip.to_owned());
                    Ok(String::new())
                }
                (Program::Ip, "link del i3t0") => {
                    self.link = None;
                    Ok(String::new())
                }
                (Program::Resolvectl, _) if self.no_resolver => {
                    Err(failed(program, "Failed to set DNS configuration: Unit dbus-org.freedesktop.resolve1.service not found."))
                }
                (Program::Resolvectl, step) if self.old_resolver && step.starts_with("default-route") => {
                    Err(failed(program, "Unknown command verb default-route."))
                }
                (Program::Resolvectl, step) => {
                    self.resolver.push(step.to_owned());
                    Ok(String::new())
                }
                (program, line) => panic!("假机器不认识这条命令：{program} {line}"),
            }
        }
    }

    /// 脚本里 `{ … }` 中列出的对端地址。
    fn elements(script: &str) -> BTreeSet<Ipv4Addr> {
        script
            .lines()
            .filter(|line| line.contains("elements = {") || line.starts_with("add element"))
            .flat_map(|line| line.split(['{', '}', ',']).filter_map(|part| part.trim().parse().ok()).collect::<Vec<Ipv4Addr>>())
            .collect()
    }

    pub(crate) fn our_rule() -> (u32, String) {
        (2033, OUR_RULE.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Fake, FakeLink, our_rule};
    use super::*;
    use crate::Survey;

    fn plan(own: [u8; 4], peers: &[[u8; 4]]) -> Plan {
        Plan { cidr: "10.66.0.0/16".parse().unwrap(), own: own.into(), peers: peers.iter().map(|ip| Ipv4Addr::from(*ip)).collect() }
    }

    fn installed_everything(fake: &Fake, own: &str) {
        assert!(fake.table.is_some(), "表应该在");
        assert_eq!(fake.rules, [our_rule()], "正好一条自己的规则");
        assert_eq!(fake.routes, [("default".to_owned(), true)]);
        let link = fake.link.as_ref().expect("网卡应该在");
        assert_eq!((link.kind, link.alias, link.up), ("dummy", Some("ice3tier"), true));
        assert_eq!(link.addresses, [format!("{own}/32")]);
    }

    #[test]
    fn installing_on_a_clean_machine_puts_up_every_piece_and_removing_takes_them_all_away() {
        let mut fake = Fake::default();
        let done = install(&mut fake, &plan([10, 66, 0, 1], &[[10, 66, 0, 5]])).unwrap();
        assert!(!done.replaced);
        installed_everything(&fake, "10.66.0.1");
        assert_eq!(fake.table.as_ref().unwrap().iter().copied().collect::<Vec<_>>(), [Ipv4Addr::new(10, 66, 0, 5)]);
        let survey = Survey::take(&mut fake);
        assert!(!survey.is_clean());
        assert_eq!(survey.table.unwrap().unwrap().peers, [Ipv4Addr::new(10, 66, 0, 5)]);

        remove(&mut fake).unwrap();
        assert!(Survey::take(&mut fake).is_clean());
        assert_eq!((fake.table.as_ref(), fake.rules.len(), fake.routes.len(), fake.link.as_ref()), (None, 0, 0, None));
        remove(&mut fake).unwrap();
    }

    #[test]
    fn a_leftover_after_a_crash_is_replaced_not_duplicated() {
        let mut fake = Fake::default();
        install(&mut fake, &plan([10, 66, 0, 1], &[[10, 66, 0, 5]])).unwrap();
        // 进程被杀掉，什么都没拆；重启之后虚拟 IP 被管理员改了，对端也变了。
        let done = install(&mut fake, &plan([10, 66, 0, 7], &[[10, 66, 0, 9], [10, 66, 0, 2]])).unwrap();
        assert!(done.replaced, "要认出这是上次留下的");
        installed_everything(&fake, "10.66.0.7");
        assert_eq!(fake.table.as_ref().unwrap().len(), 2);
        assert!(!fake.log.iter().any(|line| line == "ip link add i3t0 type dummy" && fake.log.iter().filter(|l| *l == line).count() > 1));
    }

    #[test]
    fn duplicated_rules_from_an_older_leftover_are_collapsed_to_one() {
        let mut fake = Fake { rules: vec![our_rule(), our_rule(), (2033, "fwmark 0x1 lookup 7".to_owned())], ..Fake::default() };
        install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap();
        assert_eq!(fake.rules.iter().filter(|rule| **rule == our_rule()).count(), 1);
        assert!(fake.rules.contains(&(2033, "fwmark 0x1 lookup 7".to_owned())), "同一个优先级上别人的规则不动");
        remove(&mut fake).unwrap();
        assert_eq!(fake.rules, [(2033, "fwmark 0x1 lookup 7".to_owned())]);
    }

    #[test]
    fn a_link_or_table_that_belongs_to_someone_else_blocks_the_installation_and_is_left_alone() {
        let veth = FakeLink { kind: "veth", alias: None, addresses: vec!["192.168.9.1/24".into()], up: true };
        let mut fake = Fake { link: Some(veth.clone()), ..Fake::default() };
        assert_eq!(install(&mut fake, &plan([10, 66, 0, 1], &[])), Err(SetupError::LinkInUse));
        assert_eq!((fake.table.as_ref(), fake.rules.len(), fake.link.as_ref()), (None, 0, Some(&veth)), "什么都没装，别人的网卡没动");
        remove(&mut fake).unwrap();
        assert_eq!(fake.link.as_ref(), Some(&veth), "拆的时候也不碰它");

        let unmarked = FakeLink { kind: "dummy", alias: None, addresses: Vec::new(), up: false };
        let mut fake = Fake { link: Some(unmarked), ..Fake::default() };
        assert_eq!(install(&mut fake, &plan([10, 66, 0, 1], &[])), Err(SetupError::LinkInUse), "没有自己的别名的 dummy 也不是自己的");

        let mut fake = Fake { routes: vec![("10.9.0.0/24".into(), false)], ..Fake::default() };
        assert_eq!(install(&mut fake, &plan([10, 66, 0, 1], &[])), Err(SetupError::TableInUse { routes: 1 }));
        assert_eq!(fake.routes, [("10.9.0.0/24".to_owned(), false)]);
    }

    #[test]
    fn a_failure_part_way_leaves_the_machine_as_it_was() {
        let mut fake = Fake { nft_rejects: Some("tproxy"), ..Fake::default() };
        let error = install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap_err();
        assert!(matches!(error, SetupError::Command(CommandError::Failed { program: Program::Nft, .. })), "{error:?}");
        assert!(Survey::take(&mut fake).is_clean());
    }

    #[test]
    fn without_permission_nothing_is_installed_and_the_error_says_so() {
        let mut fake = Fake { unprivileged: true, ..Fake::default() };
        let error = install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap_err();
        assert!(error.to_string().contains("Operation not permitted"), "{error}");
        assert!(fake.rules.is_empty() && fake.link.is_none());
        assert!(remove(&mut fake).is_err(), "拆不掉时要说出来");
    }

    /// 一台普通的机器：上网的网卡，一条默认路由，一条到它所在网段的路由。
    fn ordinary() -> Fake {
        Fake {
            addresses: vec![("lo", "127.0.0.1/8"), ("eth0", "192.168.1.5/24")],
            main_routes: vec![("default", "eth0"), ("192.168.1.0/24", "eth0")],
            ..Fake::default()
        }
    }

    #[test]
    fn an_address_inside_the_range_stops_the_installation_and_nothing_is_touched() {
        let mut fake = Fake { addresses: vec![("eth0", "192.168.1.5/24"), ("lan1", "10.66.200.1/24")], ..ordinary() };
        let error = install(&mut fake, &plan([10, 66, 0, 1], &[[10, 66, 0, 5]])).unwrap_err();
        assert_eq!(error, SetupError::RangeInUse { with: "10.66.200.1/24 dev lan1".into() });
        assert!(Survey::take(&mut fake).is_clean(), "什么都没装");
        assert!(
            !fake.log.iter().any(|line| line.starts_with("nft -f") || line.contains(" add ") || line.contains("replace")),
            "{:?}",
            fake.log
        );
    }

    #[test]
    fn a_route_inside_or_around_the_range_stops_the_installation() {
        let mut fake = Fake { main_routes: vec![("default", "eth0"), ("10.0.0.0/8", "tun0")], ..ordinary() };
        let error = install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap_err();
        assert_eq!(error, SetupError::RangeInUse { with: "10.0.0.0/8 dev tun0".into() });
        assert!(Survey::take(&mut fake).is_clean());
    }

    #[test]
    fn the_default_route_and_our_own_link_are_not_overlaps() {
        let mut fake = ordinary();
        install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap();
        // 装好之后再装一次（比如崩溃之后）：自己的网卡上的虚拟 IP 不算重叠。
        assert!(fake.link.as_ref().unwrap().addresses.contains(&"10.66.0.1/32".to_owned()));
        install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap();
        let mut only_default =
            Fake { addresses: vec![("eth0", "203.0.113.5/32")], main_routes: vec![("default", "eth0")], ..Fake::default() };
        install(&mut only_default, &plan([10, 66, 0, 1], &[])).unwrap();
    }

    #[test]
    fn an_overlap_found_later_also_removes_what_an_earlier_run_left_behind() {
        let mut fake = ordinary();
        install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap();
        // 进程被杀掉之后，这台机器换到了一个和地址段重叠的网络里。
        fake.addresses.push(("wlan0", "10.66.7.20/24"));
        let error = install(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap_err();
        assert!(matches!(error, SetupError::RangeInUse { .. }), "{error:?}");
        assert!(Survey::take(&mut fake).is_clean(), "留下的规则同样会切断那个网络，要拆掉");
    }

    #[test]
    fn peers_are_replaced_in_place() {
        let mut fake = Fake::default();
        install(&mut fake, &plan([10, 66, 0, 1], &[[10, 66, 0, 5]])).unwrap();
        update_peers(&mut fake, &plan([10, 66, 0, 1], &[[10, 66, 0, 6], [10, 66, 0, 7]])).unwrap();
        assert_eq!(fake.table.as_ref().unwrap().len(), 2);
        update_peers(&mut fake, &plan([10, 66, 0, 1], &[])).unwrap();
        assert!(fake.table.as_ref().unwrap().is_empty());
        remove(&mut fake).unwrap();
        assert!(update_peers(&mut fake, &plan([10, 66, 0, 1], &[])).is_err(), "表被删掉了要说出来，调用方整个重装");
    }

    #[test]
    fn the_resolver_gets_a_routing_domain_on_our_link_and_is_never_the_default_route() {
        let mut fake = Fake::default();
        assert_eq!(configure_resolver(&mut fake, "home.i3t", [10, 66, 0, 1].into()), ResolverOutcome::Configured);
        assert_eq!(fake.resolver, ["domain i3t0 ~home.i3t", "default-route i3t0 false", "dns i3t0 10.66.0.1"], "服务器最后才给");
        let mut fake = Fake { no_resolver: true, ..Fake::default() };
        assert_eq!(configure_resolver(&mut fake, "home.i3t", [10, 66, 0, 1].into()), ResolverOutcome::Absent);
        let mut fake = Fake { old_resolver: true, ..Fake::default() };
        assert!(matches!(configure_resolver(&mut fake, "home.i3t", [10, 66, 0, 1].into()), ResolverOutcome::Failed(_)));
        assert_eq!(fake.resolver, ["domain i3t0 ~home.i3t", "revert i3t0"], "太旧的不认 default-route：撤掉，不给服务器");
    }
}
