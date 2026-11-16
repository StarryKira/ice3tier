//! HTTP 代理的 CONNECT 请求和应答。只有解析和编码，不碰 socket。
//!
//! 只认 `CONNECT 主机:端口 HTTP/1.x`，以及请求头里的 `Proxy-Authorization: Basic`。别的请求头都跳过。
//! 明文 HTTP 的转发代理（`GET http://…`）不支持，回 405。

use std::net::{IpAddr, Ipv6Addr};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use i3t_proto::flow::Host;

use crate::message::{Parsed, Password, ProxyAuth};

/// 请求行加请求头最长多少字节。读到这么多还没有见到空行就拒绝，客户端没办法让读的一方无限地攒数据。
pub(crate) const MAX_HEAD_LEN: usize = 8 * 1024;

const HEAD_END: &[u8] = b"\r\n\r\n";

/// 请求不被接受的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum HttpError {
    #[error("请求不合法：{0}")]
    Malformed(&'static str),
    #[error("请求头超过了 {MAX_HEAD_LEN} 字节")]
    TooLarge,
    #[error("只支持 CONNECT 方法")]
    MethodNotAllowed,
}

impl HttpError {
    /// 出这个错时回给客户端的应答。
    pub(crate) const fn response(self) -> Response {
        match self {
            Self::Malformed(_) => Response::BadRequest,
            Self::TooLarge => Response::HeaderTooLarge,
            Self::MethodNotAllowed => Response::MethodNotAllowed,
        }
    }
}

/// 客户端的 CONNECT 请求。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Connect {
    pub host: Host,
    pub port: u16,
    /// `Proxy-Authorization: Basic` 里的用户名和密码；没有带，或者用的不是 Basic 时是 `None`。
    pub auth: Option<ProxyAuth>,
}

impl Connect {
    /// 解析从连接上读到的字节。请求头还没有读完（没有见到空行）时返回 `None`。
    ///
    /// 返回的长度包含结尾的空行，它后面的字节已经是客户端发给目标的数据。
    pub(crate) fn parse(buf: &[u8]) -> Result<Option<Parsed<Self>>, HttpError> {
        let window = &buf[..buf.len().min(MAX_HEAD_LEN)];
        let Some(end) = window.windows(HEAD_END.len()).position(|bytes| bytes == HEAD_END) else {
            return if buf.len() >= MAX_HEAD_LEN { Err(HttpError::TooLarge) } else { Ok(None) };
        };
        let head = str::from_utf8(&buf[..end]).map_err(|_| HttpError::Malformed("请求头不是 UTF-8"))?;
        if head.split("\r\n").any(|line| line.contains(['\r', '\n', '\0'])) {
            return Err(HttpError::Malformed("行里有多余的换行或者 NUL"));
        }
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();

        let mut parts = request_line.split(' ');
        let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(HttpError::Malformed("请求行不是「方法 目标 版本」"));
        };
        if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
            return Err(HttpError::Malformed("不是 HTTP/1.x"));
        }
        if method != "CONNECT" {
            return Err(if is_token(method) { HttpError::MethodNotAllowed } else { HttpError::Malformed("方法不合法") });
        }
        let (host, port) = parse_authority(target)?;

        let mut auth = None;
        let mut seen_authorization = false;
        for line in lines {
            let (name, value) = line.split_once(':').ok_or(HttpError::Malformed("请求头缺少冒号"))?;
            // 名字里不能有空白：续行（以空白开头的行）和「名字 :」都在这里被拒绝。
            if !is_token(name) {
                return Err(HttpError::Malformed("请求头的名字不合法"));
            }
            if name.eq_ignore_ascii_case("proxy-authorization") {
                if seen_authorization {
                    return Err(HttpError::Malformed("Proxy-Authorization 出现了两次"));
                }
                seen_authorization = true;
                auth = parse_basic(value.trim_matches([' ', '\t']))?;
            }
        }
        Ok(Some(Parsed { value: Self { host, port, auth }, len: end + HEAD_END.len() }))
    }
}

/// RFC 9110 的 token：方法和请求头的名字只能由这些字符组成。
fn is_token(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// CONNECT 的目标：`主机:端口`，IPv6 地址要写在方括号里。
fn parse_authority(target: &str) -> Result<(Host, u16), HttpError> {
    const BAD_TARGET: HttpError = HttpError::Malformed("CONNECT 的目标不是「主机:端口」");
    let (host, port) = target.rsplit_once(':').ok_or(BAD_TARGET)?;
    // `u16` 的解析接受前面的加号，这里不接受。
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BAD_TARGET);
    }
    let port = port.parse::<u16>().ok().filter(|&port| port != 0).ok_or(BAD_TARGET)?;
    let host = match host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        Some(bracketed) => Host::Ip(IpAddr::V6(bracketed.parse::<Ipv6Addr>().map_err(|_| BAD_TARGET)?)),
        // 不带方括号的冒号只能是端口前面那一个，所以这里出现 IPv6 地址说明目标写错了。
        None => host.parse::<Host>().ok().filter(|host| !matches!(host, Host::Ip(IpAddr::V6(_)))).ok_or(BAD_TARGET)?,
    };
    Ok((host, port))
}

/// `Basic <base64(用户名:密码)>`。用的不是 Basic 时当作没有带认证。
///
/// 按第一个冒号切分，所以用户名里不能有冒号，这也是虚拟 IPv6 不能当用户名的原因。
fn parse_basic(value: &str) -> Result<Option<ProxyAuth>, HttpError> {
    let Some((scheme, token)) = value.split_once(' ') else { return Ok(None) };
    if !scheme.eq_ignore_ascii_case("basic") {
        return Ok(None);
    }
    let decoded = STANDARD.decode(token.trim_matches(' ')).map_err(|_| HttpError::Malformed("Basic 认证不是合法的 base64"))?;
    let (username, password) = match decoded.iter().position(|&byte| byte == b':') {
        Some(colon) => (&decoded[..colon], &decoded[colon + 1..]),
        None => (&decoded[..], &[][..]),
    };
    let username = str::from_utf8(username).map_err(|_| HttpError::Malformed("用户名不是 UTF-8"))?.to_owned();
    Ok(Some(ProxyAuth { username, password: Password::new(password) }))
}

/// 代理回给客户端的应答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Response {
    /// 隧道已经建好，之后的字节原样转发。
    Established,
    BadRequest,
    ProxyAuthenticationRequired,
    Forbidden,
    MethodNotAllowed,
    TooManyRequests,
    HeaderTooLarge,
    BadGateway,
    ServiceUnavailable,
    /// 节点正在排空：503，并且告诉客户端过一会儿再试。
    Draining,
    GatewayTimeout,
}

/// 一个失败的应答：没有正文，发完就关连接。`$extra` 是额外的响应头，每个以 `\r\n` 结尾。
macro_rules! failure {
    ($status:literal $(, $extra:literal)?) => {
        concat!("HTTP/1.1 ", $status, "\r\n", $($extra,)? "Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()
    };
}

impl Response {
    pub(crate) const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Established => b"HTTP/1.1 200 Connection established\r\n\r\n",
            Self::BadRequest => failure!("400 Bad Request"),
            Self::ProxyAuthenticationRequired => {
                failure!("407 Proxy Authentication Required", "Proxy-Authenticate: Basic realm=\"i3t\"\r\n")
            }
            Self::Forbidden => failure!("403 Forbidden"),
            Self::MethodNotAllowed => failure!("405 Method Not Allowed", "Allow: CONNECT\r\n"),
            Self::TooManyRequests => failure!("429 Too Many Requests"),
            Self::HeaderTooLarge => failure!("431 Request Header Fields Too Large"),
            Self::BadGateway => failure!("502 Bad Gateway"),
            Self::ServiceUnavailable => failure!("503 Service Unavailable"),
            Self::Draining => failure!("503 Service Unavailable", "Retry-After: 60\r\n"),
            Self::GatewayTimeout => failure!("504 Gateway Timeout"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Option<Parsed<Connect>>, HttpError> {
        Connect::parse(text.as_bytes())
    }

    fn connect(text: &str) -> Connect {
        parse(text).unwrap().unwrap().value
    }

    fn basic(credentials: &[u8]) -> String {
        format!("CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n", STANDARD.encode(credentials))
    }

    #[test]
    fn connect_with_a_domain_target_is_parsed() {
        let text = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nUser-Agent: curl/8.0\r\n\r\n";
        let parsed = parse(text).unwrap().unwrap();
        assert_eq!(parsed.len, text.len());
        assert_eq!(parsed.value, Connect { host: "example.com".parse().unwrap(), port: 443, auth: None });
    }

    #[test]
    fn connect_accepts_ipv4_and_bracketed_ipv6_targets() {
        assert_eq!(connect("CONNECT 10.66.0.5:22 HTTP/1.0\r\n\r\n").host, Host::Ip("10.66.0.5".parse().unwrap()));
        let v6 = connect("CONNECT [2001:db8::1]:8443 HTTP/1.1\r\n\r\n");
        assert_eq!((v6.host, v6.port), (Host::Ip("2001:db8::1".parse().unwrap()), 8443));
    }

    #[test]
    fn bytes_after_the_blank_line_belong_to_the_tunnel() {
        let head = "CONNECT example.com:443 HTTP/1.1\r\n\r\n";
        let parsed = Connect::parse(format!("{head}\x16\x03\x01early hello\r\n\r\n").as_bytes()).unwrap().unwrap();
        assert_eq!(parsed.len, head.len());
    }

    #[test]
    fn every_strict_prefix_of_a_request_is_incomplete() {
        let text = basic(b"nas-01:token");
        for len in 0..text.len() {
            assert_eq!(parse(&text[..len]), Ok(None), "前 {len} 字节");
        }
    }

    #[test]
    fn basic_authorization_is_split_at_the_first_colon() {
        let auth = connect(&basic(b"10.66.0.5+c-i:to:ken")).auth.unwrap();
        assert_eq!(auth.username, "10.66.0.5+c-i");
        assert_eq!(auth.password.as_bytes(), b"to:ken");

        let no_password = connect(&basic(b"nas-01")).auth.unwrap();
        assert_eq!((no_password.username.as_str(), no_password.password.as_bytes()), ("nas-01", &[][..]));

        let no_username = connect(&basic(b":token")).auth.unwrap();
        assert_eq!((no_username.username.as_str(), no_username.password.as_bytes()), ("", &b"token"[..]));
    }

    #[test]
    fn header_name_and_scheme_are_case_insensitive() {
        let text = format!("CONNECT example.com:443 HTTP/1.1\r\nproxy-AUTHORIZATION:\tbASIC {} \r\n\r\n", STANDARD.encode("u:p"));
        assert_eq!(connect(&text).auth.unwrap().username, "u");
    }

    #[test]
    fn other_authorization_schemes_count_as_no_credentials() {
        let text = "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Bearer abc.def\r\n\r\n";
        assert_eq!(connect(text).auth, None);
        let text = "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic\r\n\r\n";
        assert_eq!(connect(text).auth, None);
    }

    #[test]
    fn other_methods_are_not_allowed() {
        for text in
            ["GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n", "POST / HTTP/1.1\r\n\r\n", "connect a:1 HTTP/1.1\r\n\r\n"]
        {
            assert_eq!(parse(text), Err(HttpError::MethodNotAllowed), "{text:?}");
        }
        assert_eq!(HttpError::MethodNotAllowed.response(), Response::MethodNotAllowed);
    }

    #[test]
    fn malformed_requests_are_rejected() {
        let cases = [
            "CONNECT example.com:443\r\n\r\n",
            "CONNECT  example.com:443 HTTP/1.1\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1 extra\r\n\r\n",
            "CONNECT example.com:443 HTTP/2.0\r\n\r\n",
            "CONNECT example.com:443 http/1.1\r\n\r\n",
            "CONNECT example.com HTTP/1.1\r\n\r\n",
            "CONNECT example.com:0 HTTP/1.1\r\n\r\n",
            "CONNECT example.com:+443 HTTP/1.1\r\n\r\n",
            "CONNECT example.com:65536 HTTP/1.1\r\n\r\n",
            "CONNECT example.com:44a HTTP/1.1\r\n\r\n",
            "CONNECT :443 HTTP/1.1\r\n\r\n",
            "CONNECT exa mple.com:443 HTTP/1.1\r\n\r\n",
            "CONNECT 2001:db8::1:443 HTTP/1.1\r\n\r\n",
            "CONNECT [example.com]:443 HTTP/1.1\r\n\r\n",
            "CONNECT [2001:db8::1:443 HTTP/1.1\r\n\r\n",
            "CONNECT http://example.com:443/ HTTP/1.1\r\n\r\n",
            "(CONNECT) example.com:443 HTTP/1.1\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nno colon here\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nHost : example.com\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nHost: a\r\n folded: b\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nHost: a\nX: b\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nHost: a\0b\r\n\r\n",
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic !!!not-base64!!!\r\n\r\n",
            "\r\n\r\n",
        ];
        for text in cases {
            assert!(matches!(parse(text), Err(HttpError::Malformed(_))), "{text:?} → {:?}", parse(text));
        }
        assert_eq!(Connect::parse(b"CONNECT \xff\xfe:443 HTTP/1.1\r\n\r\n"), Err(HttpError::Malformed("请求头不是 UTF-8")));
        assert_eq!(HttpError::Malformed("").response(), Response::BadRequest);
    }

    #[test]
    fn duplicate_authorization_headers_are_rejected() {
        let text = format!(
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {0}\r\nProxy-Authorization: Basic {0}\r\n\r\n",
            STANDARD.encode("u:p")
        );
        assert!(matches!(parse(&text), Err(HttpError::Malformed(_))));
    }

    #[test]
    fn a_username_that_is_not_utf8_is_rejected() {
        assert!(matches!(parse(&basic(b"\xff\xfe:token")), Err(HttpError::Malformed(_))));
    }

    #[test]
    fn a_head_that_never_ends_is_cut_off_at_the_limit() {
        let mut flood = b"CONNECT example.com:443 HTTP/1.1\r\n".to_vec();
        while flood.len() < MAX_HEAD_LEN - 1 {
            flood.extend_from_slice(b"X-Padding: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
            flood.truncate(MAX_HEAD_LEN - 1);
        }
        assert_eq!(Connect::parse(&flood), Ok(None));
        flood.push(b'a');
        assert_eq!(Connect::parse(&flood), Err(HttpError::TooLarge));
        // 空行落在上限之外也不算数。
        flood.extend_from_slice(b"\r\n\r\n");
        assert_eq!(Connect::parse(&flood), Err(HttpError::TooLarge));
        assert_eq!(HttpError::TooLarge.response(), Response::HeaderTooLarge);
    }

    #[test]
    fn responses_are_complete_http_messages() {
        assert_eq!(Response::Established.as_bytes(), b"HTTP/1.1 200 Connection established\r\n\r\n");
        let unauthorized = str::from_utf8(Response::ProxyAuthenticationRequired.as_bytes()).unwrap();
        assert!(unauthorized.starts_with("HTTP/1.1 407 "));
        assert!(unauthorized.contains("\r\nProxy-Authenticate: Basic realm=\"i3t\"\r\n"));
        assert!(Response::MethodNotAllowed.as_bytes().windows(16).any(|bytes| bytes == b"\r\nAllow: CONNECT"));

        let failures = [
            (Response::BadRequest, "400"),
            (Response::ProxyAuthenticationRequired, "407"),
            (Response::Forbidden, "403"),
            (Response::MethodNotAllowed, "405"),
            (Response::TooManyRequests, "429"),
            (Response::HeaderTooLarge, "431"),
            (Response::BadGateway, "502"),
            (Response::ServiceUnavailable, "503"),
            (Response::GatewayTimeout, "504"),
        ];
        for (response, status) in failures {
            let text = str::from_utf8(response.as_bytes()).unwrap();
            assert!(text.starts_with(&format!("HTTP/1.1 {status} ")), "{text}");
            assert!(text.ends_with("\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"), "{text}");
        }
    }
}
