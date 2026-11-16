//! 令牌的生成和哈希。
//!
//! 管理令牌、注册令牌里的一次性密钥、接入凭据都是同一种东西：32 个随机字节的 base64url。
//! controller 只存它们的 SHA-256；令牌本身熵足够高，不需要加盐和慢哈希。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::Error;
use crate::store::DIGEST_LEN;

const TOKEN_BYTES: usize = 32;

/// 生成一个新的令牌。
///
/// 令牌总是以字母或数字开头：以 `-` 开头的会被命令行当成选项，粘贴到 `--token` 后面就出错。
/// 为此偶尔要重新生成一次，损失的熵不到 0.1 比特。
pub(crate) fn generate() -> Result<String, Error> {
    let random = SystemRandom::new();
    loop {
        let mut bytes = [0; TOKEN_BYTES];
        random.fill(&mut bytes).map_err(|_| Error::Random)?;
        let token = URL_SAFE_NO_PAD.encode(bytes);
        if token.starts_with(|first: char| first.is_ascii_alphanumeric()) {
            return Ok(token);
        }
    }
}

pub(crate) fn sha256(data: &[u8]) -> [u8; DIGEST_LEN] {
    digest(&SHA256, data).as_ref().try_into().expect("SHA-256 的输出固定是 32 字节，这是算法的定义")
}

/// 比较两个哈希，用时不随"前多少个字节相同"变化。
pub(crate) fn digests_match(a: &[u8; DIGEST_LEN], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |difference, (x, y)| difference | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_long_differ_and_never_look_like_an_option() {
        let (first, second) = (generate().unwrap(), generate().unwrap());
        assert_eq!(first.len(), 43);
        assert_ne!(first, second);
        // 64 种开头里有 2 种不行：五百个里一个都没撞上的概率不到千万分之一，所以这确实在测重新生成。
        for _ in 0..500 {
            let token = generate().unwrap();
            assert!(token.starts_with(|first: char| first.is_ascii_alphanumeric()), "{}…", &token[..1]);
        }
    }

    #[test]
    fn digest_comparison_accepts_only_the_same_digest() {
        let digest = sha256(b"token");
        assert!(digests_match(&digest, &sha256(b"token")));
        assert!(!digests_match(&digest, &sha256(b"other")));
        assert!(!digests_match(&digest, &digest[..31]));
    }
}
