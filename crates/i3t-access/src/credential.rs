//! 接入凭据：核对令牌，限制每个凭据的并发连接数。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use i3t_model::netmap::{Credential, TokenHash};
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

/// 每个凭据现在有多少条连接在用。
#[derive(Debug, Default)]
pub(crate) struct Slots {
    in_use: Mutex<HashMap<TokenHash, u32>>,
}

impl Slots {
    /// 给 `credential` 占一个并发名额；已经到了它的上限时返回 `None`。名额在 [`Slot`] 被丢弃时归还。
    ///
    /// 上限按当时的 netmap 取：上限调低之后，已经在用的连接不受影响，新的连接要等用量降下来。
    pub(crate) fn acquire(self: &Arc<Self>, credential: &Credential) -> Option<Slot> {
        let mut in_use = self.in_use.lock().expect("credential slots mutex poisoned");
        let count = in_use.entry(credential.token).or_default();
        if credential.max_concurrent.is_some_and(|max| *count >= max) {
            if *count == 0 {
                in_use.remove(&credential.token);
            }
            return None;
        }
        *count += 1;
        Some(Slot { slots: Arc::clone(self), token: credential.token })
    }
}

/// 一条连接占着的并发名额。
#[derive(Debug)]
pub(crate) struct Slot {
    slots: Arc<Slots>,
    token: TokenHash,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut in_use = self.slots.in_use.lock().expect("credential slots mutex poisoned");
        if let Some(count) = in_use.get_mut(&self.token) {
            *count -= 1;
            if *count == 0 {
                in_use.remove(&self.token);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(name: &str, token: &str, max_concurrent: Option<u32>) -> Credential {
        let hash = digest::digest(&digest::SHA256, token.as_bytes());
        Credential { name: name.into(), token: TokenHash::from_bytes(hash.as_ref().try_into().unwrap()), max_concurrent }
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

    #[test]
    fn concurrency_is_limited_per_credential_and_slots_come_back() {
        let slots = Arc::new(Slots::default());
        let limited = credential("gw-a", "token-a", Some(2));
        let other = credential("gw-b", "token-b", Some(1));

        let first = slots.acquire(&limited).unwrap();
        let _second = slots.acquire(&limited).unwrap();
        assert!(slots.acquire(&limited).is_none(), "第三条连接超过了上限 2");
        assert!(slots.acquire(&other).is_some(), "别的凭据不受影响");

        drop(first);
        assert!(slots.acquire(&limited).is_some(), "名额归还之后可以再用");
    }

    #[test]
    fn no_limit_means_unlimited_and_zero_means_none() {
        let slots = Arc::new(Slots::default());
        let unlimited = credential("gw-a", "token-a", None);
        let held: Vec<Slot> = (0..100).map(|_| slots.acquire(&unlimited).unwrap()).collect();
        assert_eq!(held.len(), 100);

        let blocked = credential("gw-b", "token-b", Some(0));
        assert!(slots.acquire(&blocked).is_none());
        drop(held);
        assert!(slots.in_use.lock().unwrap().is_empty(), "没有连接在用时不留记录");
    }
}
