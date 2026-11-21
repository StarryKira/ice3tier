//! 读回系统里现在有的、属于虚拟网卡的防火墙规则。
//!
//! 装之前先读一遍：崩溃留下的规则要认出来换掉，不能多出一份。`i3t network status` 也是读的这一份。
//! 读的是 `nft -j` 的 JSON 和 `iptables -S` 的文字（Linux），PowerShell 数出来的规则数（Windows），不认识的东西一律不管。

use ipnet::Ipv4Net;
use serde_json::Value;

use crate::device::Platform;
use crate::machine::{CommandError, Machine, Program};
use crate::plan::{linux, windows};
use crate::{COMMENT, is_device_name};

/// 读回来的样子。每一项各自读，一项读不了（比如普通用户读不了防火墙规则、内核里没有 `raw` 表）不妨碍别的项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// 自己的防冒充的表（Linux，有 `nft` 时装的：表名就是网卡名），按名字排。
    pub tables: Result<Vec<String>, CommandError>,
    /// 自己的放行规则，按在链里的先后。
    pub accept: Result<Vec<AcceptRule>, CommandError>,
    /// 自己的防冒充的规则（Linux，没有 `nft` 时装的：`raw` 表的 PREROUTING 链里），按在链里的先后。
    pub guard_rules: Result<Vec<GuardRule>, CommandError>,
}

/// 一条自己的放行规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptRule {
    /// 管的是哪块网卡。
    pub device: String,
    /// Linux：在 INPUT 链里排第几（从 1 数）。Windows 上没有先后，是 1。
    pub position: usize,
}

/// 一条自己的防冒充的规则（Linux 上 `raw` 表的 PREROUTING 链里）。每块网卡两条：一条按来源（`-s`），一条按目的（`-d`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardRule {
    /// 管的是哪块网卡（规则里的 `! -i <网卡>`）。
    pub device: String,
    /// 在 PREROUTING 链里排第几（从 1 数）。
    pub position: usize,
    /// 丢掉的来源地址段（`-s`）。没有或者读不出来时是 `None`：这样的规则也是自己的，装的时候按需要换掉。
    pub range: Option<Ipv4Net>,
    /// 丢掉的目的地址（`-d`，目的是本节点虚拟 IP 的那一条，`/32`）。
    pub destination: Option<Ipv4Net>,
    /// `iptables -S` 写出来的那一行去掉 `-A PREROUTING `：删的时候原样交回去（[`linux::delete_guard`]）。
    pub rule: String,
}

impl Survey {
    /// 在 `platform` 上读一遍。`device` 是 Windows 上的网卡名（规则的名字由它来，Linux 上按注释找全部的）。
    pub fn take(machine: &mut impl Machine, platform: Platform, device: &str) -> Self {
        match platform {
            Platform::Linux => {
                let tables = machine.run(Program::Nft, &linux::list_tables(), None).and_then(|json| parse_tables(&json));
                let accept = machine.run(Program::Iptables, &linux::list_input(), None).map(|text| parse_input(&text));
                let guard_rules =
                    machine.run(Program::Iptables, &linux::list_raw_prerouting(), None).map(|text| parse_raw_prerouting(&text));
                Self { tables, accept, guard_rules }
            }
            Platform::Windows => {
                let accept =
                    machine.run(Program::PowerShell, &windows::invocation(&windows::count_script(device)), None).and_then(|text| {
                        let count = text.trim().parse::<usize>().map_err(|_| CommandError::Io {
                            program: Program::PowerShell,
                            detail: format!("数规则时输出了看不懂的东西：{}", text.trim()),
                        })?;
                        Ok((0..count).map(|_| AcceptRule { device: device.to_owned(), position: 1 }).collect())
                    });
                Self { tables: Ok(Vec::new()), accept, guard_rules: Ok(Vec::new()) }
            }
            Platform::MacOs | Platform::Other => Self { tables: Ok(Vec::new()), accept: Ok(Vec::new()), guard_rules: Ok(Vec::new()) },
        }
    }

    /// 读到了至少一样自己的东西。读不了的项不算。
    #[must_use]
    pub fn found_any(&self) -> bool {
        self.counts().into_iter().any(|count| count.is_some_and(|count| count > 0))
    }

    /// 自己的东西一样都没有了。读不了的项不算"没有"。
    ///
    /// 要读的东西根本不在（[`CommandError::means_absent`]）的项算"没有"：没有 `nft` 就建不了自己的表，没有 `iptables`
    /// 就加不了自己的规则，内核里没有 `raw` 表，里面也就没有规则。装好表之后才卸掉 `nft` 的话，表还在内核里，可这台机器上
    /// 也没有东西拆得掉它了（拆的时候同样当作没有）；它丢的只是冒充虚拟 IP 的包，留着不碍事，重启就没了。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.counts().into_iter().all(|count| count == Some(0))
    }

    /// 每一项里有几样自己的东西。读不了的项是 `None`，要读的东西根本不在的项是 0。
    fn counts(&self) -> [Option<usize>; 3] {
        [count(&self.tables), count(&self.accept), count(&self.guard_rules)]
    }
}

fn count<T>(item: &Result<Vec<T>, CommandError>) -> Option<usize> {
    match item {
        Ok(found) => Some(found.len()),
        Err(error) => error.means_absent().then_some(0),
    }
}

/// `nft -j list tables ip` 的输出里自己的表：名字是网卡名的样子。以前的透明访问的表叫 `i3t`（没有编号），不在这里，见 [`crate::legacy`]。
fn parse_tables(json: &str) -> Result<Vec<String>, CommandError> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| CommandError::Io { program: Program::Nft, detail: format!("读不懂 nft 的输出：{error}") })?;
    let objects = value.get("nftables").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut names: Vec<String> = objects
        .iter()
        .filter_map(|object| object.get("table")?.get("name")?.as_str())
        .filter(|name| is_device_name(name))
        .map(str::to_owned)
        .collect();
    names.sort();
    Ok(names)
}

/// `nft -j list tables ip` 的输出里有没有叫 `name` 的表。
pub(crate) fn has_table(json: &str, name: &str) -> Result<bool, CommandError> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| CommandError::Io { program: Program::Nft, detail: format!("读不懂 nft 的输出：{error}") })?;
    let objects = value.get("nftables").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    Ok(objects.iter().any(|object| object.get("table").and_then(|table| table.get("name")).and_then(Value::as_str) == Some(name)))
}

/// `iptables -S INPUT` 的输出里自己的规则：`-A INPUT` 开头、注释是 [`COMMENT`]、放行、管的是一块像是自己建的网卡。
fn parse_input(text: &str) -> Vec<AcceptRule> {
    Listed::all(text, "INPUT")
        .filter(|rule| rule.is_ours("ACCEPT"))
        .filter_map(|rule| {
            let device = rule.after("-i").filter(|device| is_device_name(device))?;
            Some(AcceptRule { device: device.to_owned(), position: rule.position })
        })
        .collect()
}

/// `iptables -t raw -S PREROUTING` 的输出里自己的规则：`-A PREROUTING` 开头、注释是 [`COMMENT`]、丢掉、
/// 管的是不从一块像是自己建的网卡进来的包（`! -i <网卡>`）。地址段不在认的条件里：网络的地址段会变，旧的那条也是自己的。
fn parse_raw_prerouting(text: &str) -> Vec<GuardRule> {
    Listed::all(text, "PREROUTING")
        .filter(|rule| rule.is_ours("DROP"))
        .filter_map(|rule| {
            let device = rule.after_negated("-i").filter(|device| is_device_name(device))?;
            let range = rule.after("-s").and_then(|source| source.parse().ok());
            let destination = rule.after("-d").and_then(|destination| destination.parse().ok());
            Some(GuardRule { device: device.to_owned(), position: rule.position, range, destination, rule: rule.text.to_owned() })
        })
        .collect()
}

/// `iptables -S <链>` 写出来的一条规则。
struct Listed<'a> {
    /// 在链里排第几（从 1 数）。
    position: usize,
    /// 那一行去掉 `-A <链> `。
    text: &'a str,
    /// 拆开的词，引号去掉了（[`linux::split_listed`]）：带空格的注释是一个词，认不错。
    words: Vec<String>,
}

impl<'a> Listed<'a> {
    /// `iptables -S <chain>` 的输出里这条链的规则，按先后。`-P`（默认策略）那一行不是规则，不占位置。
    fn all(output: &'a str, chain: &str) -> impl Iterator<Item = Self> {
        let prefix = format!("-A {chain} ");
        output.lines().filter_map(move |line| line.strip_prefix(prefix.as_str())).enumerate().map(|(index, text)| Self {
            position: index + 1,
            text,
            words: linux::split_listed(text),
        })
    }

    /// `flag` 后面的那个词。
    fn after(&self, flag: &str) -> Option<&str> {
        self.words.windows(2).find(|pair| pair[0] == flag).map(|pair| pair[1].as_str())
    }

    /// 前面是 `!` 的 `flag` 后面的那个词：`! -i i3t0` 里的 `i3t0`。
    fn after_negated(&self, flag: &str) -> Option<&str> {
        self.words.windows(3).find(|three| three[0] == "!" && three[1] == flag).map(|three| three[2].as_str())
    }

    /// 注释是 [`COMMENT`]、目标是 `target`：程序自己加的那一类。
    fn is_ours(&self, target: &str) -> bool {
        self.after("--comment") == Some(COMMENT) && self.after("-j") == Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // nftables 1.1.6 在一个网络命名空间里真的输出（另有一张 ufw 的表、一张以前的透明访问的表）。
    const TABLES: &str = r#"{"nftables": [{"metainfo": {"version": "1.1.6", "release_name": "Commodore Bullmoose #7", "json_schema_version": 1}}, {"table": {"family": "ip", "name": "filter", "handle": 1}}, {"table": {"family": "ip", "name": "i3t0", "handle": 2}}, {"table": {"family": "ip", "name": "i3t", "handle": 3}}]}"#;
    // iptables 1.8.11（nf_tables 后端）的 -S INPUT：ufw 风格的规则夹着自己的。
    const INPUT: &str = "-P INPUT DROP\n-A INPUT -i i3t0 -m comment --comment ice3tier -j ACCEPT\n-A INPUT -j ufw-before-input\n\
                         -A INPUT -i i3t1 -m comment --comment ice3tier -j ACCEPT\n-A INPUT -i eth0 -m comment --comment ice3tier -j ACCEPT\n\
                         -A INPUT -i i3t0 -m comment --comment \"someone else\" -j ACCEPT\n-A INPUT -i i3t0 -m comment --comment ice3tier -j DROP\n";

    #[test]
    fn our_tables_are_the_ones_named_like_our_devices() {
        assert_eq!(parse_tables(TABLES).unwrap(), ["i3t0"]);
        assert!(has_table(TABLES, "i3t").unwrap());
        assert!(!has_table(TABLES, "i3t9").unwrap());
        assert!(parse_tables(r#"{"nftables": []}"#).unwrap().is_empty());
        assert!(parse_tables("not json").is_err(), "读不懂的是错误，不是没有");
    }

    #[test]
    fn our_input_rules_carry_our_comment_accept_and_a_device_of_ours_and_keep_their_position() {
        assert_eq!(
            parse_input(INPUT),
            [AcceptRule { device: "i3t0".into(), position: 1 }, AcceptRule { device: "i3t1".into(), position: 3 }]
        );
        assert!(parse_input("-P INPUT ACCEPT\n").is_empty());
    }

    // -t raw -S PREROUTING：Docker 的、别人的、注释带引号的规则夹着自己的三条（i3t0 的两条，i3t1 那条是地址段变了之前、
    // 还没有按目的的那条时的）。按 iptables 1.8 写规则的次序（-s、-d、! -i、各个 -m、-j）写的。
    const RAW: &str = "-P PREROUTING ACCEPT\n\
                       -A PREROUTING -d 172.17.0.2/32 ! -i docker0 -j DROP\n\
                       -A PREROUTING -s 10.66.0.0/16 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP\n\
                       -A PREROUTING -d 10.66.0.1/32 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP\n\
                       -A PREROUTING -p udp -m udp --dport 53 -j CT --notrack\n\
                       -A PREROUTING -s 10.66.0.0/16 ! -i i3t0 -m comment --comment \"ice3tier \\\"old\\\"\" -j DROP\n\
                       -A PREROUTING -s 10.99.0.0/16 ! -i i3t1 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP\n\
                       -A PREROUTING -s 10.66.0.0/16 -i i3t0 -m comment --comment ice3tier -j DROP\n\
                       -A PREROUTING -s 10.66.0.0/16 ! -i eth0 -m comment --comment ice3tier -j DROP\n\
                       -A PREROUTING -s 10.66.0.0/16 ! -i i3t2 -m comment --comment ice3tier -j ACCEPT\n";

    #[test]
    fn our_raw_rules_carry_our_comment_drop_and_not_a_device_of_ours_and_keep_their_position_range_and_spelling() {
        let rule = |device: &str, position, range: &str, text: &str| GuardRule {
            device: device.into(),
            position,
            range: Some(range.parse().unwrap()),
            destination: None,
            rule: text.into(),
        };
        let to_us = "-d 10.66.0.1/32 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP";
        assert_eq!(
            parse_raw_prerouting(RAW),
            [
                rule(
                    "i3t0",
                    2,
                    "10.66.0.0/16",
                    "-s 10.66.0.0/16 ! -i i3t0 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP"
                ),
                GuardRule { range: None, destination: Some("10.66.0.1/32".parse().unwrap()), ..rule("i3t0", 3, "0.0.0.0/0", to_us) },
                rule(
                    "i3t1",
                    6,
                    "10.99.0.0/16",
                    "-s 10.99.0.0/16 ! -i i3t1 -m addrtype ! --src-type LOCAL -m comment --comment ice3tier -j DROP"
                ),
            ],
            "注释带引号的那条（ice3tier \"old\"）不是自己的；-i（没有 !）、eth0、ACCEPT 的也不是"
        );
        assert!(parse_raw_prerouting("-P PREROUTING ACCEPT\n").is_empty());
        let without_source = parse_raw_prerouting("-A PREROUTING ! -i i3t0 -m comment --comment ice3tier -j DROP\n");
        assert_eq!(without_source.len(), 1, "没有地址段的也是自己的：装的时候换掉");
        assert_eq!((without_source[0].range, without_source[0].destination), (None, None));
    }

    #[derive(Default)]
    struct Answers(Vec<(Program, Result<String, CommandError>)>);

    impl Machine for Answers {
        fn run(&mut self, program: Program, _args: &[String], _input: Option<&str>) -> Result<String, CommandError> {
            let at = self.0.iter().position(|(wanted, _)| *wanted == program).expect("每个程序都给了回答");
            self.0.remove(at).1
        }
    }

    fn refused(detail: &str) -> Result<String, CommandError> {
        Err(CommandError::Failed { program: Program::Iptables, status: Some(4), detail: detail.into() })
    }

    #[test]
    fn a_survey_reads_each_item_on_its_own_and_unreadable_items_are_not_taken_for_none() {
        // iptables 先读 INPUT，再读 raw 的 PREROUTING。
        let mut machine = Answers(vec![
            (Program::Nft, Ok(TABLES.to_owned())),
            (Program::Iptables, refused("Permission denied")),
            (Program::Iptables, Ok(RAW.to_owned())),
        ]);
        let survey = Survey::take(&mut machine, Platform::Linux, "i3t0");
        assert_eq!(survey.tables.as_deref(), Ok(&["i3t0".to_owned()][..]));
        assert!(survey.accept.is_err());
        assert_eq!(survey.guard_rules.as_ref().map(Vec::len), Ok(3));
        assert!(survey.found_any() && !survey.is_clean());

        let no_raw_table = "iptables v1.8.9 (legacy): can't initialize iptables table `raw': Table does not exist (do you need to insmod?)";
        let mut machine = Answers(vec![
            (Program::Nft, Ok(r#"{"nftables": []}"#.into())),
            (Program::Iptables, Ok(INPUT.to_owned())),
            (Program::Iptables, refused(no_raw_table)),
        ]);
        let survey = Survey::take(&mut machine, Platform::Linux, "i3t0");
        assert!(survey.guard_rules.is_err(), "raw 表读不了是这一项的错误");
        assert_eq!(survey.accept.map(|rules| rules.len()), Ok(2), "不妨碍别的项");

        let mut machine = Answers(vec![
            (Program::Nft, Ok(r#"{"nftables": []}"#.into())),
            (Program::Iptables, Ok("-P INPUT ACCEPT\n".into())),
            (Program::Iptables, refused("Permission denied (you must be root)")),
        ]);
        assert!(!Survey::take(&mut machine, Platform::Linux, "i3t0").is_clean(), "读不了的项不算没有");

        let mut windows = Answers(vec![(Program::PowerShell, Ok("0\r\n".into()))]);
        assert!(Survey::take(&mut windows, Platform::Windows, "i3t0").is_clean(), "数出来是 0：没有这条规则");
        let mut windows = Answers(vec![(Program::PowerShell, Ok("1\r\n".into()))]);
        assert_eq!(Survey::take(&mut windows, Platform::Windows, "i3t0").accept.unwrap().len(), 1);
        let mut windows = Answers(vec![(Program::PowerShell, Ok("WARNING: something\r\n".into()))]);
        assert!(Survey::take(&mut windows, Platform::Windows, "i3t0").accept.is_err(), "看不懂的输出不当作没有");
        assert!(Survey::take(&mut Answers::default(), Platform::MacOs, "utun7").is_clean(), "macOS 上什么都不加，也什么都不读");
    }

    #[test]
    fn a_missing_program_or_table_means_none_of_its_kind_is_there() {
        let missing = |program| Err(CommandError::Missing(program));
        let tables = || Ok(r#"{"nftables": []}"#.to_owned());
        let input = || Ok("-P INPUT ACCEPT\n".to_owned());
        let raw = || Ok("-P PREROUTING ACCEPT\n".to_owned());
        let no_raw_table = || refused("can't initialize iptables table `raw': Table does not exist (do you need to insmod?)");
        let iptables = Program::Iptables;
        let machines = [
            vec![(Program::Nft, missing(Program::Nft)), (iptables, input()), (iptables, raw())],
            vec![(Program::Nft, tables()), (iptables, missing(iptables)), (iptables, missing(iptables))],
            vec![(Program::Nft, missing(Program::Nft)), (iptables, missing(iptables)), (iptables, missing(iptables))],
            vec![(Program::Nft, tables()), (iptables, input()), (iptables, no_raw_table())],
        ];
        for answers in machines {
            let survey = Survey::take(&mut Answers(answers), Platform::Linux, "i3t0");
            assert!(survey.is_clean() && !survey.found_any(), "{survey:?}");
        }
        let mut machine = Answers(vec![(Program::Nft, missing(Program::Nft)), (iptables, input()), (iptables, Ok(RAW.into()))]);
        let survey = Survey::take(&mut machine, Platform::Linux, "i3t0");
        assert!(survey.found_any() && !survey.is_clean(), "没有 nft 时 raw 表里的规则照样算");
    }
}
