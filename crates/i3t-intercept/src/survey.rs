//! 读回系统里现在有的、属于透明访问的东西。
//!
//! 装之前先读一遍：崩溃留下的旧东西要认出来换掉，挡路的别人的东西要说清楚。`i3t intercept status` 也是读的这一份。
//! 读的是 `nft -j` 和 `ip -j` 的 JSON，不认识的字段一律不管。

use std::net::Ipv4Addr;

use serde_json::Value;

use crate::machine::{CommandError, Machine, Program};
use crate::{LINK, LINK_ALIAS, PROTO, ROUTE_TABLE, RULE_PRIORITY, TABLE};

/// 读回来的样子。每一项各自读，一项读不了（比如普通用户读不了包过滤规则）不妨碍别的项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// nftables 里的那张表；不在时是 `None`。
    pub table: Result<Option<Table>, CommandError>,
    /// 自己加的策略路由规则。正常时是一条；多出来的是以前留下的，装的时候会删掉。
    pub rules: Result<Vec<Rule>, CommandError>,
    /// 表 [`ROUTE_TABLE`] 里的路由，自己的和别人的都在里面（[`Route::ours`]）。
    pub routes: Result<Vec<Route>, CommandError>,
    /// 名字叫 [`LINK`] 的网卡；没有时是 `None`。
    pub link: Result<Option<Link>, CommandError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// 链的名字，按读到的顺序。
    pub chains: Vec<String>,
    /// 对端集合里的地址。
    pub peers: Vec<Ipv4Addr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub priority: u32,
    /// `ip rule` 写出来的那一行，给人看。
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// `ip route` 写出来的那一行，给人看。
    pub text: String,
    /// `proto` 是 [`PROTO`]：是自己加的。
    pub ours: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// 是一块 dummy 网卡，别名是 [`LINK_ALIAS`]：是自己建的。
    pub ours: bool,
    /// 网卡的种类（`dummy`、`veth`……），普通的网卡没有。
    pub kind: Option<String>,
    /// IPv4 地址，写成 `地址/前缀`。
    pub addresses: Vec<String>,
}

impl Survey {
    /// 读一遍。
    pub fn take(machine: &mut impl Machine) -> Self {
        let table = match machine.run(Program::Nft, &args(&["-j", "list", "table", "ip", TABLE]), None) {
            Ok(json) => parse_table(&json).map(Some),
            Err(error) if error.says("no such file or directory") => Ok(None),
            Err(error) => Err(error),
        };
        let rules = machine.run(Program::Ip, &args(&["-j", "rule", "show"]), None).map(|json| parse_rules(&json));
        let routes = match machine.run(Program::Ip, &args(&["-j", "route", "show", "table", &ROUTE_TABLE.to_string()]), None) {
            Ok(json) => Ok(parse_routes(&json)),
            Err(error) if error.says("does not exist") => Ok(Vec::new()),
            Err(error) => Err(error),
        };
        let link = match machine.run(Program::Ip, &args(&["-j", "-d", "addr", "show", "dev", LINK]), None) {
            Ok(json) => Ok(parse_link(&json)),
            Err(error) if error.says("does not exist") => Ok(None),
            Err(error) => Err(error),
        };
        Self { table, rules, routes, link }
    }

    /// 系统里一样属于透明访问的东西都没有了。读不了的项不算"没有"。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        matches!(self.table, Ok(None))
            && self.rules.as_ref().is_ok_and(Vec::is_empty)
            && self.routes.as_ref().is_ok_and(|routes| !routes.iter().any(|route| route.ours))
            && matches!(&self.link, Ok(None) | Ok(Some(Link { ours: false, .. })))
    }

    /// 读到了至少一样属于透明访问的东西（上次没拆干净）。读不了的项不算。
    #[must_use]
    pub fn found_any(&self) -> bool {
        matches!(self.table, Ok(Some(_)))
            || self.rules.as_ref().is_ok_and(|rules| !rules.is_empty())
            || self.routes.as_ref().is_ok_and(|routes| routes.iter().any(|route| route.ours))
            || matches!(&self.link, Ok(Some(Link { ours: true, .. })))
    }

    /// 表 [`ROUTE_TABLE`] 里有别人的路由：这张表被别的程序占着，不能拿来用。
    #[must_use]
    pub fn foreign_routes(&self) -> usize {
        self.routes.as_ref().map_or(0, |routes| routes.iter().filter(|route| !route.ours).count())
    }

    /// 有一块叫 [`LINK`] 的网卡，但不是自己建的。
    #[must_use]
    pub fn foreign_link(&self) -> bool {
        matches!(&self.link, Ok(Some(Link { ours: false, .. })))
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// `nft -j list table ip i3t` 的输出。
fn parse_table(json: &str) -> Result<Table, CommandError> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| CommandError::Io { program: Program::Nft, detail: format!("读不懂 nft 的输出：{error}") })?;
    let objects = value.get("nftables").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let chains = objects.iter().filter_map(|object| object.get("chain")?.get("name")?.as_str().map(str::to_owned)).collect();
    let peers = objects
        .iter()
        .filter_map(|object| object.get("set").filter(|set| set.get("name").and_then(Value::as_str) == Some("peers")))
        .flat_map(|set| set.get("elem").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|element| element.as_str().and_then(|text| text.parse().ok()))
        .collect();
    Ok(Table { chains, peers })
}

/// 一个 JSON 值写成字符串：字符串原样，数原样写出。`ip -j` 有时把编号写成字符串，有时写成数。
fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// `ip -j rule show` 的输出里属于自己的规则：优先级是 [`RULE_PRIORITY`]，`proto` 是 [`PROTO`]。
fn parse_rules(json: &str) -> Vec<Rule> {
    let rules: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
    rules
        .iter()
        .filter(|rule| rule.get("priority").and_then(Value::as_u64) == Some(u64::from(RULE_PRIORITY)))
        .filter(|rule| text(rule.get("protocol")).as_deref() == Some(PROTO.to_string().as_str()))
        .map(|rule| {
            let mut words = vec![format!("{RULE_PRIORITY}:"), "from".to_owned(), text(rule.get("src")).unwrap_or_else(|| "all".to_owned())];
            if let Some(mark) = text(rule.get("fwmark")) {
                words.push("fwmark".to_owned());
                words.push(text(rule.get("fwmask")).map_or_else(|| mark.clone(), |mask| format!("{mark}/{mask}")));
            }
            words.extend(["lookup".to_owned(), text(rule.get("table")).unwrap_or_default(), "proto".to_owned(), PROTO.to_string()]);
            Rule { priority: RULE_PRIORITY, text: words.join(" ") }
        })
        .collect()
}

/// `ip -j route show table 2033` 的输出。
fn parse_routes(json: &str) -> Vec<Route> {
    let routes: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
    routes
        .iter()
        .map(|route| {
            let protocol = text(route.get("protocol"));
            let mut words: Vec<String> = Vec::new();
            words.extend(text(route.get("type")));
            words.extend(text(route.get("dst")));
            if let Some(dev) = text(route.get("dev")) {
                words.extend(["dev".to_owned(), dev]);
            }
            words.extend(["table".to_owned(), ROUTE_TABLE.to_string()]);
            if let Some(protocol) = &protocol {
                words.extend(["proto".to_owned(), protocol.clone()]);
            }
            Route { text: words.join(" "), ours: protocol.as_deref() == Some(PROTO.to_string().as_str()) }
        })
        .collect()
}

/// `ip -j -d addr show dev i3t0` 的输出。
fn parse_link(json: &str) -> Option<Link> {
    let links: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
    let link = links.first()?;
    let kind = link.get("linkinfo").and_then(|info| text(info.get("info_kind")));
    let alias = text(link.get("ifalias"));
    let addresses = link
        .get("addr_info")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|address| address.get("family").and_then(Value::as_str) == Some("inet"))
        .filter_map(|address| Some(format!("{}/{}", text(address.get("local"))?, text(address.get("prefixlen"))?)))
        .collect();
    Some(Link { ours: kind.as_deref() == Some("dummy") && alias.as_deref() == Some(LINK_ALIAS), kind, addresses })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 以下几段是 nftables 1.1.6 和 iproute2 6.17 在一个网络命名空间里装好之后真的输出。
    const TABLE_JSON: &str = r#"{"nftables": [{"metainfo": {"version": "1.1.6", "release_name": "Commodore Bullmoose #7", "json_schema_version": 1}}, {"table": {"family": "ip", "name": "i3t", "handle": 1}}, {"chain": {"family": "ip", "table": "i3t", "name": "intercept", "handle": 1, "type": "nat", "hook": "output", "prio": -100, "policy": "accept"}}, {"set": {"family": "ip", "name": "peers", "table": "i3t", "type": "ipv4_addr", "handle": 2, "elem": ["10.66.0.2", "10.66.0.3"]}}, {"rule": {"family": "ip", "table": "i3t", "chain": "intercept", "handle": 3, "expr": [{"match": {"op": "==", "left": {"&": [{"meta": {"key": "mark"}}, 536870912]}, "right": 536870912}}, {"return": null}]}}]}"#;
    const RULES_JSON: &str = r#"[{"priority":0,"src":"all","table":"local"},{"priority":2033,"src":"all","fwmark":"0x20000000","fwmask":"0x20000000","table":"2033","protocol":"203"},{"priority":2033,"src":"all","fwmark":"0x1","table":"7"},{"priority":32766,"src":"all","table":"main"},{"priority":32767,"src":"all","table":"default"}]"#;
    const ROUTES_JSON: &str = r#"[{"type":"local","dst":"default","dev":"lo","protocol":"203","scope":"host","flags":[]},{"dst":"10.9.0.0/24","dev":"eth0","scope":"link","flags":[]}]"#;
    const LINK_JSON: &str = r#"[{"ifindex":2,"ifname":"i3t0","flags":["BROADCAST","NOARP","UP","LOWER_UP"],"mtu":1500,"ifalias":"ice3tier","linkinfo":{"info_kind":"dummy"},"addr_info":[{"family":"inet","local":"10.66.0.1","prefixlen":32,"scope":"global","label":"i3t0"},{"family":"inet6","local":"fe80::a472:60ff:feb0:ea59","prefixlen":64,"scope":"link"}]}]"#;

    #[test]
    fn the_table_is_read_with_its_chains_and_peers() {
        let table = parse_table(TABLE_JSON).unwrap();
        assert_eq!(table.chains, ["intercept"]);
        assert_eq!(table.peers, [Ipv4Addr::new(10, 66, 0, 2), Ipv4Addr::new(10, 66, 0, 3)]);
        let empty_set = r#"{"nftables": [{"table": {"family": "ip", "name": "i3t"}}, {"set": {"name": "peers", "type": "ipv4_addr"}}]}"#;
        assert!(parse_table(empty_set).unwrap().peers.is_empty());
        assert!(parse_table("not json").is_err());
    }

    #[test]
    fn only_rules_with_our_priority_and_protocol_count_as_ours() {
        let rules = parse_rules(RULES_JSON);
        assert_eq!(rules.len(), 1, "同一个优先级上别人的规则不是自己的");
        assert_eq!(rules[0].text, "2033: from all fwmark 0x20000000/0x20000000 lookup 2033 proto 203");
        assert!(parse_rules("[]").is_empty());
    }

    #[test]
    fn routes_in_our_table_are_told_apart_by_protocol() {
        let routes = parse_routes(ROUTES_JSON);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0], Route { text: "local default dev lo table 2033 proto 203".into(), ours: true });
        assert!(!routes[1].ours);
    }

    #[test]
    fn the_link_is_ours_only_if_it_is_a_dummy_with_our_alias() {
        let link = parse_link(LINK_JSON).unwrap();
        assert!(link.ours);
        assert_eq!(link.addresses, ["10.66.0.1/32"], "只列 IPv4 地址");
        let someone_elses = LINK_JSON.replace("\"ifalias\":\"ice3tier\",", "");
        assert!(!parse_link(&someone_elses).unwrap().ours);
        let a_veth = LINK_JSON.replace("dummy", "veth");
        assert!(!parse_link(&a_veth).unwrap().ours);
        assert_eq!(parse_link("[]"), None);
    }
}
