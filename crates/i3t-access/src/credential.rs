//! 接入凭据：核对令牌。并发上限和用量在 `usage` 模块里。

use i3t_model::netmap::Credential;
use ring::digest;
use subtle::ConstantTimeEq;

use crate::message::Password;

/// 在 netmap 下发的凭据里找出密码字段对应的那一个。
///
/// 入口只有令牌的 SHA-256：这里对收到的密码做一次哈希，再和每个凭据做常量时间比较。
/// 不提前返回，所以花的时间和凭据排在第几个、有没有匹配上都无关。
pub(crate) fn find<'a>(credentials: &'a [Credential], password: &Password) -> Option<&'a Credential> {
    let presented = digest::digest(&digest::SHA256, password.as_bytes());
    let mut found = None;
    for credential in credentials {
        if bool::from(presented.as_ref().ct_eq(credential.token.as_bytes())) {
            found = Some(credential);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use i3t_model::netmap::TokenHash;

    use super::*;

    fn credential(name: &str, token: &str, max_concurrent: Option<u32>) -> Credential {
        let hash = digest::digest(&digest::SHA256, token.as_bytes());
        Credential {
            name: name.into(),
            token: TokenHash::from_bytes(hash.as_ref().try_into().unwrap()),
            max_concurrent,
            id: 0,
            exhausted: false,
        }
    }

    fn password(token: &str) -> Password {
        Password::new(token.as_bytes())
    }

    #[test]
    fn the_token_is_matched_by_its_sha256() {
        let credentials = [credential("gw-a", "token-a", None), credential("gw-b", "token-b", None)];
        assert_eq!(find(&credentials, &password("token-a")).map(|found| found.name.as_str()), Some("gw-a"));
        assert_eq!(find(&credentials, &password("token-b")).map(|found| found.name.as_str()), Some("gw-b"));
    }

    #[test]
    fn wrong_empty_or_hashed_tokens_do_not_match() {
        let credentials = [credential("gw-a", "token-a", None)];
        assert_eq!(find(&credentials, &password("token-A")), None);
        assert_eq!(find(&credentials, &password("")), None);
        // 拿到哈希的人（比如读到了 netmap）不能直接把哈希当令牌用。
        assert_eq!(find(&credentials, &password(&credentials[0].token.to_string())), None);
        assert_eq!(find(&credentials, &Password::new(credentials[0].token.as_bytes())), None);
        assert_eq!(find(&[], &password("token-a")), None);
    }
}
