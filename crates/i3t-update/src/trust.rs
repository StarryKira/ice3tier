//! 发布清单的签名：谁签的算数，怎么签，怎么验。

use i3t_crypto::{NodeKey, Signature};
use i3t_model::update::{Manifest, ManifestSignature, SignedManifest};
use i3t_proto::PublicKey;

use crate::error::ManifestError;

/// 清单签名的用途标签。发布私钥就算被拿去签别的东西，签出来的也冒充不了清单；反过来也一样。
pub const MANIFEST_PURPOSE: &str = "release-manifest";

/// 项目的发布公钥。要换钥匙时先把新的加进来发一版，等节点都更新到认识新钥匙的版本，再去掉旧的。
const PROJECT_KEYS: [[u8; PublicKey::LEN]; 1] = [decode_key("kWB8NBoJtLGX7mZwXgMSZ1C8RkUDpkdOphusy_JhEk0")];

/// 编译时用环境变量 `I3T_RELEASE_EXTRA_KEY` 额外加进来的一把公钥（base64url）。
/// 给测试和自己改了代码另行发布的人用：他们没有项目的私钥，要签自己的发布就得让程序认自己的公钥。
/// 值写错了编译不过，不会悄悄地不生效。
const EXTRA_KEY: Option<[u8; PublicKey::LEN]> = match option_env!("I3T_RELEASE_EXTRA_KEY") {
    Some(text) if !text.is_empty() => Some(decode_key(text)),
    _ => None,
};

/// 在编译期解出一把公钥：43 个字符、不带填充的 base64url，32 字节。
///
/// 自己写而不用 `base64` 库，是因为要在常量里算：这样写错的公钥是编译错误，而不是节点跑起来之后才发现更新不了。
///
/// # Panics
///
/// `text` 不是合法的公钥。只在编译期求值，所以表现为编译错误。
const fn decode_key(text: &str) -> [u8; PublicKey::LEN] {
    let text = text.as_bytes();
    assert!(text.len() == 43, "发布公钥应该是 43 个字符的 base64url（32 字节，不带填充）");
    let mut key = [0; PublicKey::LEN];
    let mut written = 0;
    let mut bits: u32 = 0;
    let mut pending_bits = 0;
    let mut index = 0;
    while index < text.len() {
        let sextet = match text[index] {
            letter @ b'A'..=b'Z' => letter - b'A',
            letter @ b'a'..=b'z' => letter - b'a' + 26,
            digit @ b'0'..=b'9' => digit - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => panic!("发布公钥里有不属于 base64url 的字符"),
        };
        // 只留还没写出去的那几位，免得往左移的时候溢出。`as` 在这里是放宽，常量函数里还用不了 `From`。
        bits = ((bits << 6) | sextet as u32) & 0x3fff;
        pending_bits += 6;
        if pending_bits >= 8 {
            pending_bits -= 8;
            key[written] = (bits >> pending_bits).to_le_bytes()[0];
            written += 1;
        }
        index += 1;
    }
    assert!(bits & 0b11 == 0, "发布公钥最后一个字符多出来的两位应该是 0");
    key
}

/// 节点认哪些发布公钥。
///
/// 节点程序只用 [`TrustedKeys::compiled_in`]：公钥是编译进去的，运行时没有任何配置、命令或者 controller 的消息能增加或者替换它们。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKeys {
    keys: Vec<PublicKey>,
}

impl TrustedKeys {
    /// 编译在这个程序里的公钥：项目的发布公钥，加上编译时用 `I3T_RELEASE_EXTRA_KEY` 指定的那一把（如果有）。
    #[must_use]
    pub fn compiled_in() -> Self {
        Self { keys: PROJECT_KEYS.iter().chain(&EXTRA_KEY).copied().map(PublicKey::from_bytes).collect() }
    }

    pub fn iter(&self) -> impl Iterator<Item = &PublicKey> {
        self.keys.iter()
    }

    /// 先验签名，验过了才去读清单的内容。没有通过验证的字节不会被解析。
    ///
    /// # Errors
    ///
    /// 签名不是这里任何一把公钥签的，或者签名正确但内容不是一份清单。
    pub fn verify(&self, signed: &SignedManifest) -> Result<Manifest, ManifestError> {
        let signature = Signature::from_bytes(*signed.signature.as_bytes());
        let text = signed.manifest.as_bytes();
        if !self.keys.iter().any(|key| i3t_crypto::verify(key, MANIFEST_PURPOSE, text, &signature).is_ok()) {
            return Err(ManifestError::BadSignature);
        }
        serde_json::from_slice(text).map_err(ManifestError::Malformed)
    }
}

/// 只认这一把公钥。给发布工具核对自己刚签出来的清单用；节点程序不走这条路。
impl From<PublicKey> for TrustedKeys {
    fn from(key: PublicKey) -> Self {
        Self { keys: vec![key] }
    }
}

/// 给一份清单的原文签名。`manifest` 之后一个字节都不能再动。
#[must_use]
pub fn sign_manifest(key: &NodeKey, manifest: String) -> SignedManifest {
    let signature = key.sign(MANIFEST_PURPOSE, manifest.as_bytes());
    SignedManifest { manifest, signature: ManifestSignature::from_bytes(*signature.as_bytes()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_time_key_decoding_agrees_with_the_runtime_parser() {
        for text in [
            "kWB8NBoJtLGX7mZwXgMSZ1C8RkUDpkdOphusy_JhEk0",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-_-8",
        ] {
            assert_eq!(PublicKey::from_bytes(decode_key(text)), text.parse::<PublicKey>().unwrap(), "{text}");
        }
    }

    #[test]
    fn the_project_key_is_always_trusted() {
        let project: PublicKey = "kWB8NBoJtLGX7mZwXgMSZ1C8RkUDpkdOphusy_JhEk0".parse().unwrap();
        let keys = TrustedKeys::compiled_in();
        assert_eq!(keys.iter().next(), Some(&project));
        // 另外那一把只在编译时指定了才有。
        assert_eq!(keys.iter().count(), 1 + usize::from(EXTRA_KEY.is_some()));
    }

    #[test]
    fn a_manifest_signed_by_a_stranger_is_not_trusted_by_the_compiled_in_keys() {
        let stranger = NodeKey::generate().unwrap();
        let signed = sign_manifest(&stranger, "{}".to_owned());
        assert!(matches!(TrustedKeys::compiled_in().verify(&signed), Err(ManifestError::BadSignature)));
    }

    #[test]
    fn any_one_of_several_trusted_keys_is_enough() {
        // 换钥匙的过渡期里程序同时认新旧两把。
        let (current, next, stranger) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
        let keys = TrustedKeys { keys: vec![current.public(), next.public()] };
        let text = r#"{ "version": "0.3.0", "min_controller": "0.1.0", "published": 1, "artifacts": {} }"#;
        for key in [&current, &next] {
            assert_eq!(keys.verify(&sign_manifest(key, text.to_owned())).unwrap().version.to_string(), "0.3.0");
        }
        assert!(matches!(keys.verify(&sign_manifest(&stranger, text.to_owned())), Err(ManifestError::BadSignature)));
    }
}
