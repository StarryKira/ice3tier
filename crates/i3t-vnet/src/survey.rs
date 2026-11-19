//! 读回系统里现在有的、属于虚拟网卡的防火墙规则。
//!
//! 装之前先读一遍：崩溃留下的规则要认出来换掉，不能多出一份。`i3t network status` 也是读的这一份。
//! 读的是 `nft -j` 的 JSON 和 `iptables -S` 的文字（Linux），PowerShell 数出来的规则数（Windows），不认识的东西一律不管。

use serde_json::Value;

use crate::device::Platform;
use crate::machine::{CommandError, Machine, Program};
use crate::plan::{linux, windows};
use crate::{COMMENT, is_device_name};

/// 读回来的样子。每一项各自读，一项读不了（比如普通用户读不了防火墙规则）不妨碍别的项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// 自己的防冒充的表（Linux：表名就是网卡名），按名字排。
    pub tables: Result<Vec<String>, CommandError>,
    /// 自己的放行规则，按在链里的先后。
    pub accept: Result<Vec<AcceptRule>, CommandError>,
}

/// 一条自己的放行规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptRule {
    /// 管的是哪块网卡。
    pub device: String,
    /// Linux：在 INPUT 链里排第几（从 1 数）。Windows 上没有先后，是 1。
    pub position: usize,
}

impl Survey {
    /// 在 `platform` 上读一遍。`device` 是 Windows 上的网卡名（规则的名字由它来，Linux 上按注释找全部的）。
    pub fn take(machine: &mut impl Machine, platform: Platform, device: &str) -> Self {
        match platform {
            Platform::Linux => {
                let tables = machine.run(Program::Nft, &linux::list_tables(), None).and_then(|json| parse_tables(&json));
                let accept = machine.run(Program::Iptables, &linux::list_input(), None).map(|text| parse_input(&text));
                Self { tables, accept }
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
                Self { tables: Ok(Vec::new()), accept }
            }
            Platform::MacOs | Platform::Other => Self { tables: Ok(Vec::new()), accept: Ok(Vec::new()) },
        }
    }

    /// 读到了至少一样自己的东西。读不了的项不算。
    #[must_use]
    pub fn found_any(&self) -> bool {
        self.tables.as_ref().is_ok_and(|tables| !tables.is_empty()) || self.accept.as_ref().is_ok_and(|rules| !rules.is_empty())
    }

    /// 自己的东西一样都没有了。读不了的项不算"没有"。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.tables.as_ref().is_ok_and(Vec::is_empty) && self.accept.as_ref().is_ok_and(Vec::is_empty)
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
    text.lines()
        .filter_map(|line| line.strip_prefix("-A INPUT "))
        .enumerate()
        .filter_map(|(index, rule)| {
            let words: Vec<&str> = rule.split_whitespace().collect();
            let after = |flag: &str| words.windows(2).find(|pair| pair[0] == flag).map(|pair| pair[1].trim_matches('"'));
            let ours = after("--comment") == Some(COMMENT) && after("-j") == Some("ACCEPT");
            let device = after("-i").filter(|device| is_device_name(device))?;
            ours.then(|| AcceptRule { device: device.to_owned(), position: index + 1 })
        })
        .collect()
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

    #[derive(Default)]
    struct Answers(Vec<(Program, Result<String, CommandError>)>);

    impl Machine for Answers {
        fn run(&mut self, program: Program, _args: &[String], _input: Option<&str>) -> Result<String, CommandError> {
            let at = self.0.iter().position(|(wanted, _)| *wanted == program).expect("每个程序都给了回答");
            self.0.remove(at).1
        }
    }

    #[test]
    fn a_survey_reads_each_item_on_its_own_and_unreadable_items_are_not_taken_for_none() {
        let mut machine = Answers(vec![
            (Program::Nft, Ok(TABLES.to_owned())),
            (
                Program::Iptables,
                Err(CommandError::Failed { program: Program::Iptables, status: Some(4), detail: "Permission denied".into() }),
            ),
        ]);
        let survey = Survey::take(&mut machine, Platform::Linux, "i3t0");
        assert_eq!(survey.tables.as_deref(), Ok(&["i3t0".to_owned()][..]));
        assert!(survey.accept.is_err());
        assert!(survey.found_any() && !survey.is_clean());

        let mut windows = Answers(vec![(Program::PowerShell, Ok("0\r\n".into()))]);
        assert!(Survey::take(&mut windows, Platform::Windows, "i3t0").is_clean(), "数出来是 0：没有这条规则");
        let mut windows = Answers(vec![(Program::PowerShell, Ok("1\r\n".into()))]);
        assert_eq!(Survey::take(&mut windows, Platform::Windows, "i3t0").accept.unwrap().len(), 1);
        let mut windows = Answers(vec![(Program::PowerShell, Ok("WARNING: something\r\n".into()))]);
        assert!(Survey::take(&mut windows, Platform::Windows, "i3t0").accept.is_err(), "看不懂的输出不当作没有");
        assert!(Survey::take(&mut Answers::default(), Platform::MacOs, "utun7").is_clean(), "macOS 上什么都不加，也什么都不读");
    }
}
