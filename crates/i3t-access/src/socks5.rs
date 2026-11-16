//! SOCKS5（RFC 1928）和它的用户名密码认证（RFC 1929）的报文。只有解析和编码，不碰 socket。
//!
//! 每个 `parse` 接受从连接上读到的字节：报文完整时返回它和它占的长度，还不完整时返回 `None`，内容不对时返回错误。
//! 每个报文的长度都由它自己的长度字节决定，最长的也只有五百多字节，所以"还不完整"时缓冲区一定比这个短，
//! 客户端没办法让读的一方无限地攒数据。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use i3t_proto::flow::{Host, InvalidDomainName};

use crate::message::{Parsed, Password, ProxyAuth};

const VERSION: u8 = 0x05;
const AUTH_VERSION: u8 = 0x01;

const METHOD_NONE: u8 = 0x00;
const METHOD_PASSWORD: u8 = 0x02;
const METHOD_UNACCEPTABLE: u8 = 0xff;

const COMMAND_CONNECT: u8 = 0x01;

const ATYP_V4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_V6: u8 = 0x04;

/// 报文不合法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Socks5Error {
    #[error("不是 SOCKS5：版本字节是 {0:#04x}")]
    Version(u8),
    #[error("问候里一种认证方法都没有")]
    NoMethods,
    #[error("认证报文的版本字节是 {0:#04x}")]
    AuthVersion(u8),
    #[error("用户名不是 UTF-8")]
    Username,
    #[error("不支持的命令 {0:#04x}，只支持 CONNECT")]
    Command(u8),
    #[error("请求里的保留字节是 {0:#04x}，应该是 0")]
    Reserved(u8),
    #[error("不支持的地址类型 {0:#04x}")]
    AddressType(u8),
    #[error("目标域名不合法")]
    Domain(#[source] InvalidDomainName),
}

impl Socks5Error {
    /// 请求阶段出这个错时回给客户端的应答码。
    pub(crate) const fn reply(self) -> ReplyCode {
        match self {
            Self::Command(_) => ReplyCode::CommandNotSupported,
            Self::AddressType(_) => ReplyCode::AddressTypeNotSupported,
            Self::Version(_) | Self::NoMethods | Self::AuthVersion(_) | Self::Username | Self::Reserved(_) | Self::Domain(_) => {
                ReplyCode::GeneralFailure
            }
        }
    }
}

/// 首字节不对时不用等后面的字节，立刻就能拒绝。
fn check_first(buf: &[u8], expected: u8, error: fn(u8) -> Socks5Error) -> Result<(), Socks5Error> {
    match buf.first() {
        Some(&byte) if byte != expected => Err(error(byte)),
        _ => Ok(()),
    }
}

/// 认证方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    None,
    Password,
}

/// 客户端的问候：它支持哪些认证方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Greeting {
    offers_none: bool,
    offers_password: bool,
}

impl Greeting {
    /// ```text
    /// | 版本 0x05 | 方法数 n（1～255）| n 个方法 |
    /// ```
    pub(crate) fn parse(buf: &[u8]) -> Result<Option<Parsed<Self>>, Socks5Error> {
        check_first(buf, VERSION, Socks5Error::Version)?;
        let Some((&[_, count], rest)) = buf.split_first_chunk::<2>() else { return Ok(None) };
        if count == 0 {
            return Err(Socks5Error::NoMethods);
        }
        let Some(methods) = rest.get(..usize::from(count)) else { return Ok(None) };
        let greeting = Self { offers_none: methods.contains(&METHOD_NONE), offers_password: methods.contains(&METHOD_PASSWORD) };
        Ok(Some(Parsed { value: greeting, len: 2 + methods.len() }))
    }

    /// 选一种认证方法；没有可用的时返回 `None`。
    ///
    /// 客户端肯给用户名密码就选它，即使监听器不要求凭据：出口节点写在用户名里，选了"不认证"就拿不到了。
    pub(crate) const fn choose(self, credential_required: bool) -> Option<Method> {
        if self.offers_password {
            Some(Method::Password)
        } else if self.offers_none && !credential_required {
            Some(Method::None)
        } else {
            None
        }
    }
}

/// 对问候的回答：`| 0x05 | 选中的方法 |`，没有可用的方法时是 `0xff`。
pub(crate) const fn method_selection(method: Option<Method>) -> [u8; 2] {
    let method = match method {
        Some(Method::None) => METHOD_NONE,
        Some(Method::Password) => METHOD_PASSWORD,
        None => METHOD_UNACCEPTABLE,
    };
    [VERSION, method]
}

/// 用户名密码认证的请求。
///
/// ```text
/// | 版本 0x01 | 用户名长度 | 用户名 | 密码长度 | 密码 |
/// ```
///
/// RFC 1929 要求两个长度都至少是 1，但只带用户名的客户端（比如 curl）发的密码长度就是 0，所以这里接受 0。
pub(crate) fn parse_auth(buf: &[u8]) -> Result<Option<Parsed<ProxyAuth>>, Socks5Error> {
    check_first(buf, AUTH_VERSION, Socks5Error::AuthVersion)?;
    let Some((&[_, username_len], rest)) = buf.split_first_chunk::<2>() else { return Ok(None) };
    let Some((username, rest)) = rest.split_at_checked(usize::from(username_len)) else { return Ok(None) };
    let Some((&password_len, rest)) = rest.split_first() else { return Ok(None) };
    let Some(password) = rest.get(..usize::from(password_len)) else { return Ok(None) };
    let username = str::from_utf8(username).map_err(|_| Socks5Error::Username)?.to_owned();
    let len = 2 + username.len() + 1 + password.len();
    Ok(Some(Parsed { value: ProxyAuth { username, password: Password::new(password) }, len }))
}

/// 对认证的回答：`| 0x01 | 状态 |`，状态 0 是通过。
pub(crate) const fn auth_status(granted: bool) -> [u8; 2] {
    [AUTH_VERSION, if granted { 0x00 } else { 0x01 }]
}

/// 客户端的 CONNECT 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Connect {
    pub host: Host,
    pub port: u16,
}

impl Connect {
    /// ```text
    /// | 版本 0x05 | 命令 | 保留 0x00 | 地址类型 | 地址 | 端口（大端）|
    ///
    /// 地址类型  0x01 IPv4，4 字节；0x04 IPv6，16 字节；0x03 域名，1 字节长度加域名
    /// ```
    ///
    /// 只认 CONNECT。BIND 和 UDP ASSOCIATE 报 [`Socks5Error::Command`]。
    pub(crate) fn parse(buf: &[u8]) -> Result<Option<Parsed<Self>>, Socks5Error> {
        check_first(buf, VERSION, Socks5Error::Version)?;
        let Some((&[_, command, reserved, address_type], rest)) = buf.split_first_chunk::<4>() else { return Ok(None) };
        if command != COMMAND_CONNECT {
            return Err(Socks5Error::Command(command));
        }
        if reserved != 0 {
            return Err(Socks5Error::Reserved(reserved));
        }
        let (host, rest) = match address_type {
            ATYP_V4 => {
                let Some((octets, rest)) = rest.split_first_chunk::<4>() else { return Ok(None) };
                (Host::Ip(IpAddr::V4(Ipv4Addr::from(*octets))), rest)
            }
            ATYP_V6 => {
                let Some((octets, rest)) = rest.split_first_chunk::<16>() else { return Ok(None) };
                (Host::Ip(IpAddr::V6(Ipv6Addr::from(*octets))), rest)
            }
            ATYP_DOMAIN => {
                let Some((&len, rest)) = rest.split_first() else { return Ok(None) };
                let Some((name, rest)) = rest.split_at_checked(usize::from(len)) else { return Ok(None) };
                // 有的客户端把 IP 地址也按域名类型发，`Host` 的解析会把它认回地址。
                let name = str::from_utf8(name).map_err(|error| Socks5Error::Domain(InvalidDomainName::Byte(name[error.valid_up_to()])))?;
                (name.parse::<Host>().map_err(Socks5Error::Domain)?, rest)
            }
            other => return Err(Socks5Error::AddressType(other)),
        };
        let Some((&port, rest)) = rest.split_first_chunk::<2>() else { return Ok(None) };
        Ok(Some(Parsed { value: Self { host, port: u16::from_be_bytes(port) }, len: buf.len() - rest.len() }))
    }
}

/// 对 CONNECT 请求的应答码（RFC 1928 §6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplyCode {
    Succeeded = 0x00,
    GeneralFailure = 0x01,
    NotAllowed = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    CommandNotSupported = 0x07,
    AddressTypeNotSupported = 0x08,
}

/// 对 CONNECT 请求的应答。
///
/// ```text
/// | 版本 0x05 | 应答码 | 保留 0x00 | 地址类型 0x01 | 0.0.0.0 | 端口 0 |
/// ```
///
/// 绑定地址一律填 0：连接是在出口节点上建的，它在那边用的本地地址对客户端没有意义。
pub(crate) const fn reply(code: ReplyCode) -> [u8; 10] {
    [VERSION, code as u8, 0x00, ATYP_V4, 0, 0, 0, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Parser;

    fn host(text: &str) -> Host {
        text.parse().unwrap()
    }

    /// 报文的每个真前缀都应该是"还不完整"，而不是错误或者别的报文。
    fn assert_prefixes_incomplete<T: std::fmt::Debug, E: std::fmt::Debug>(bytes: &[u8], parse: Parser<T, E>) {
        for len in 0..bytes.len() {
            assert!(matches!(parse(&bytes[..len]), Ok(None)), "前 {len} 字节：{:?}", parse(&bytes[..len]));
        }
    }

    #[test]
    fn greeting_lists_the_offered_methods() {
        let parsed = Greeting::parse(&[5, 2, 0, 2]).unwrap().unwrap();
        assert_eq!(parsed.len, 4);
        assert_eq!(parsed.value.choose(false), Some(Method::Password));
        assert_eq!(parsed.value.choose(true), Some(Method::Password));

        let none_only = Greeting::parse(&[5, 1, 0]).unwrap().unwrap().value;
        assert_eq!(none_only.choose(false), Some(Method::None));
        assert_eq!(none_only.choose(true), None, "要求凭据的监听器不接受不认证");

        let gssapi_only = Greeting::parse(&[5, 1, 1]).unwrap().unwrap().value;
        assert_eq!(gssapi_only.choose(false), None);

        assert_prefixes_incomplete(&[5, 3, 0, 1, 2], Greeting::parse);
    }

    #[test]
    fn greeting_leaves_the_bytes_behind_it_alone() {
        let parsed = Greeting::parse(&[5, 1, 0, 5, 1, 0, 1]).unwrap().unwrap();
        assert_eq!(parsed.len, 3);
    }

    #[test]
    fn malformed_greeting_is_rejected_as_early_as_possible() {
        assert_eq!(Greeting::parse(&[4]), Err(Socks5Error::Version(4)));
        assert_eq!(Greeting::parse(b"GET / HTTP/1.1\r\n"), Err(Socks5Error::Version(b'G')));
        assert_eq!(Greeting::parse(&[5, 0]), Err(Socks5Error::NoMethods));
    }

    #[test]
    fn method_selection_and_auth_status_have_the_rfc_layout() {
        assert_eq!(method_selection(Some(Method::None)), [5, 0]);
        assert_eq!(method_selection(Some(Method::Password)), [5, 2]);
        assert_eq!(method_selection(None), [5, 0xff]);
        assert_eq!(auth_status(true), [1, 0]);
        assert_eq!(auth_status(false), [1, 1]);
    }

    #[test]
    fn auth_carries_username_and_password() {
        let bytes = [&[1, 6][..], b"nas-01", &[5], b"token", b"next"].concat();
        let parsed = parse_auth(&bytes).unwrap().unwrap();
        assert_eq!(parsed.len, bytes.len() - 4);
        assert_eq!(parsed.value.username, "nas-01");
        assert_eq!(parsed.value.password.as_bytes(), b"token");
        assert_prefixes_incomplete(&bytes[..bytes.len() - 4], parse_auth);
    }

    #[test]
    fn auth_accepts_empty_username_and_empty_password() {
        let parsed = parse_auth(&[1, 0, 0]).unwrap().unwrap();
        assert_eq!((parsed.value.username.as_str(), parsed.value.password.as_bytes(), parsed.len), ("", &[][..], 3));
    }

    #[test]
    fn malformed_auth_is_rejected() {
        assert_eq!(parse_auth(&[5, 1, b'a', 0]), Err(Socks5Error::AuthVersion(5)));
        assert_eq!(parse_auth(&[1, 2, 0xff, 0xfe, 0]), Err(Socks5Error::Username));
    }

    #[test]
    fn longest_auth_is_bounded_by_its_length_bytes() {
        let bytes = [&[1, 255][..], &[b'u'; 255], &[255], &[b'p'; 255]].concat();
        let parsed = parse_auth(&bytes).unwrap().unwrap();
        assert_eq!(parsed.len, 513);
        assert_eq!(parsed.value.username.len(), 255);
    }

    #[test]
    fn connect_accepts_ipv4_ipv6_and_domain_targets() {
        let v4 = [5, 1, 0, 1, 203, 0, 113, 7, 0x01, 0xbb];
        assert_eq!(Connect::parse(&v4), Ok(Some(Parsed { value: Connect { host: host("203.0.113.7"), port: 443 }, len: 10 })));
        assert_prefixes_incomplete(&v4, Connect::parse);

        let mut v6 = vec![5, 1, 0, 4];
        v6.extend_from_slice(&"2001:db8::1".parse::<Ipv6Addr>().unwrap().octets());
        v6.extend_from_slice(&[0, 80]);
        assert_eq!(Connect::parse(&v6), Ok(Some(Parsed { value: Connect { host: host("2001:db8::1"), port: 80 }, len: 22 })));
        assert_prefixes_incomplete(&v6, Connect::parse);

        let domain = [&[5, 1, 0, 3, 11][..], b"example.com", &[0x1f, 0x90], b"early data"].concat();
        let parsed = Connect::parse(&domain).unwrap().unwrap();
        assert_eq!(parsed, Parsed { value: Connect { host: host("example.com"), port: 8080 }, len: domain.len() - 10 });
        assert_prefixes_incomplete(&domain[..domain.len() - 10], Connect::parse);
    }

    #[test]
    fn an_address_sent_as_a_domain_is_still_an_address() {
        let bytes = [&[5, 1, 0, 3, 9][..], b"10.66.0.5", &[0, 22]].concat();
        assert_eq!(Connect::parse(&bytes).unwrap().unwrap().value.host, Host::Ip("10.66.0.5".parse().unwrap()));
    }

    #[test]
    fn unsupported_commands_and_address_types_get_their_own_reply_codes() {
        let bind = Connect::parse(&[5, 2, 0, 1]).unwrap_err();
        assert_eq!(bind, Socks5Error::Command(2));
        assert_eq!(bind.reply(), ReplyCode::CommandNotSupported);

        let udp_associate = Connect::parse(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap_err();
        assert_eq!(udp_associate.reply(), ReplyCode::CommandNotSupported);

        let unknown_type = Connect::parse(&[5, 1, 0, 2, 0, 0]).unwrap_err();
        assert_eq!(unknown_type, Socks5Error::AddressType(2));
        assert_eq!(unknown_type.reply(), ReplyCode::AddressTypeNotSupported);
    }

    #[test]
    fn malformed_connect_is_rejected() {
        assert_eq!(Connect::parse(&[4, 1, 0, 1]), Err(Socks5Error::Version(4)));
        assert_eq!(Connect::parse(&[5, 1, 9, 1]), Err(Socks5Error::Reserved(9)));
        assert_eq!(Connect::parse(&[5, 1, 0, 3, 0, 0, 80]), Err(Socks5Error::Domain(InvalidDomainName::Length(0))));
        let spaced = [&[5, 1, 0, 3, 7][..], b"a b.com", &[0, 80]].concat();
        assert_eq!(Connect::parse(&spaced), Err(Socks5Error::Domain(InvalidDomainName::Byte(b' '))));
        let binary = [5, 1, 0, 3, 2, 0xff, 0xfe, 0, 80];
        assert_eq!(Connect::parse(&binary), Err(Socks5Error::Domain(InvalidDomainName::Byte(0xff))));
        assert_eq!(Connect::parse(&binary).unwrap_err().reply(), ReplyCode::GeneralFailure);
    }

    #[test]
    fn reply_has_the_rfc_layout() {
        assert_eq!(reply(ReplyCode::Succeeded), [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(reply(ReplyCode::NotAllowed)[1], 2);
        assert_eq!(reply(ReplyCode::NetworkUnreachable)[1], 3);
        assert_eq!(reply(ReplyCode::HostUnreachable)[1], 4);
        assert_eq!(reply(ReplyCode::ConnectionRefused)[1], 5);
        assert_eq!(reply(ReplyCode::CommandNotSupported)[1], 7);
        assert_eq!(reply(ReplyCode::AddressTypeNotSupported)[1], 8);
        assert_eq!(reply(ReplyCode::GeneralFailure)[1], 1);
    }
}
