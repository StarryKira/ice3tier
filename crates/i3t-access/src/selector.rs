//! 代理用户名的写法。
//!
//! ```text
//! <出口节点>[+c-<等级>]
//!
//! 出口节点   虚拟 IPv4（10.66.0.5）或者节点名（nas-01，或者全名 nas-01.home.i3t）；可以不写
//! +c-<等级>  可选的服务等级：a（auto）、i（interactive）、r（reliable）、b（bulk）
//! ```
//!
//! 虚拟 IPv6 不能当用户名：HTTP 的 Basic 认证按第一个冒号切分用户名和密码。附加标记用 `+` 分隔，因为 IP 和全名里都有点号。

use std::net::Ipv4Addr;
use std::str::FromStr;

use i3t_proto::flow::{Class, DomainName};

/// 用户名里写的内容。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Selector {
    /// 流量从哪个节点出去；没有写时由目标地址和监听器的默认动作决定。
    pub exit: Option<ExitName>,
    pub class: Option<Class>,
}

/// 用户名里指代出口节点的方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitName {
    Ip(Ipv4Addr),
    /// 节点的短名字或者全名。字符集和域名相同。
    Name(DomainName),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum InvalidSelector {
    #[error("出口节点既不是 IPv4 地址，也不是合法的节点名")]
    Exit,
    #[error("不认识的标记，只支持 +c-a、+c-i、+c-r、+c-b")]
    UnknownTag,
    #[error("服务等级写了两次")]
    DuplicateClass,
}

impl FromStr for Selector {
    type Err = InvalidSelector;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('+');
        let exit = match parts.next().unwrap_or_default() {
            "" => None,
            text => Some(match text.parse::<Ipv4Addr>() {
                Ok(ip) => ExitName::Ip(ip),
                Err(_) => ExitName::Name(text.parse().map_err(|_| InvalidSelector::Exit)?),
            }),
        };
        let mut class = None;
        for tag in parts {
            let parsed = match tag {
                "c-a" => Class::Auto,
                "c-i" => Class::Interactive,
                "c-r" => Class::Reliable,
                "c-b" => Class::Bulk,
                _ => return Err(InvalidSelector::UnknownTag),
            };
            if class.replace(parsed).is_some() {
                return Err(InvalidSelector::DuplicateClass);
            }
        }
        Ok(Self { exit, class })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Option<ExitName> {
        Some(ExitName::Name(text.parse().unwrap()))
    }

    #[test]
    fn exit_is_a_virtual_ip_or_a_node_name() {
        assert_eq!("10.66.0.5".parse(), Ok(Selector { exit: Some(ExitName::Ip(Ipv4Addr::new(10, 66, 0, 5))), class: None }));
        assert_eq!("nas-01".parse(), Ok(Selector { exit: name("nas-01"), class: None }));
        assert_eq!("nas-01.home.i3t".parse(), Ok(Selector { exit: name("nas-01.home.i3t"), class: None }));
        // 不是完整的 IPv4 地址时只能是名字，查不到就按"没有这个节点"失败。
        assert_eq!("10.66.0".parse(), Ok(Selector { exit: name("10.66.0"), class: None }));
    }

    #[test]
    fn class_tag_follows_a_plus() {
        let classes = [("c-a", Class::Auto), ("c-i", Class::Interactive), ("c-r", Class::Reliable), ("c-b", Class::Bulk)];
        for (tag, class) in classes {
            let selector: Selector = format!("10.66.0.5+{tag}").parse().unwrap();
            assert_eq!(selector, Selector { exit: Some(ExitName::Ip(Ipv4Addr::new(10, 66, 0, 5))), class: Some(class) });
        }
        assert_eq!("nas-01+c-b".parse(), Ok(Selector { exit: name("nas-01"), class: Some(Class::Bulk) }));
    }

    #[test]
    fn exit_may_be_omitted() {
        assert_eq!("".parse(), Ok(Selector::default()));
        assert_eq!("+c-i".parse(), Ok(Selector { exit: None, class: Some(Class::Interactive) }));
    }

    #[test]
    fn malformed_usernames_are_rejected() {
        for (text, error) in [
            ("fd00::5", InvalidSelector::Exit),
            ("nas 01", InvalidSelector::Exit),
            ("alice@example", InvalidSelector::Exit),
            ("nas-01+", InvalidSelector::UnknownTag),
            ("nas-01+c-x", InvalidSelector::UnknownTag),
            ("nas-01+C-I", InvalidSelector::UnknownTag),
            ("nas-01+s-abc", InvalidSelector::UnknownTag),
            ("nas-01+c-i+c-b", InvalidSelector::DuplicateClass),
            ("nas-01+c-i+c-i", InvalidSelector::DuplicateClass),
        ] {
            assert_eq!(text.parse::<Selector>(), Err(error), "{text:?}");
        }
    }
}
