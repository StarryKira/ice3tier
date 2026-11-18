//! 两种代理协议的解析器共用的类型。

use std::fmt;

/// 从缓冲区开头解析出来的一个完整报文。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Parsed<T> {
    pub value: T,
    /// 报文占缓冲区开头的多少字节。后面的字节属于下一个报文，或者已经是客户端的数据。
    pub len: usize,
}

/// 一个报文的解析器：报文完整时返回它，还不完整时返回 `None`，内容不对时返回错误。
pub(crate) type Parser<T, E> = fn(&[u8]) -> Result<Option<Parsed<T>>, E>;

/// 客户端在代理认证里给出的用户名和密码。
///
/// 代理设了用户名和密码时两个都要对上（[`crate::entry::Gate`]）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProxyAuth {
    pub username: String,
    pub password: Password,
}

/// 密码字段的内容。它是代理的密码，不能进日志：`Debug` 不打印内容。
#[derive(PartialEq, Eq)]
pub(crate) struct Password(Vec<u8>);

impl Password {
    pub(crate) fn new(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Password({} 字节)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_is_not_printed() {
        let auth = ProxyAuth { username: "nas-01".into(), password: Password::new(b"s3cret-token") };
        let printed = format!("{auth:?}");
        assert!(!printed.contains("s3cret"), "{printed}");
        assert!(printed.contains("12 字节"));
    }
}
