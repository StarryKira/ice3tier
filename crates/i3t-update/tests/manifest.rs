//! 清单从发布工具到节点的一个来回：签名、在控制链路上原样传递、验签。只用对外的接口。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use i3t_crypto::NodeKey;
use i3t_model::update::{Artifact, Manifest, ManifestSignature, Platform, Sha256, SignedManifest, Version};
use i3t_update::{MANIFEST_PURPOSE, ManifestError, TrustedKeys, sign_manifest};

fn manifest(version: &str) -> Manifest {
    let artifact = |platform: &str, fill: u8| {
        let platform: Platform = platform.parse().unwrap();
        (platform.to_string(), Artifact { file: platform.program_file("i3t"), size: 12_345_678, sha256: Sha256::from_bytes([fill; 32]) })
    };
    Manifest {
        version: version.parse().unwrap(),
        min_controller: Version::new(0, 1, 0),
        published: 1_763_200_000,
        artifacts: BTreeMap::from([
            artifact("linux-x86_64", 1),
            artifact("linux-aarch64", 2),
            artifact("windows-x86_64", 3),
            artifact("macos-aarch64", 4),
            artifact("macos-x86_64", 5),
        ]),
    }
}

fn signed(key: &NodeKey, version: &str) -> SignedManifest {
    sign_manifest(key, serde_json::to_string_pretty(&manifest(version)).unwrap() + "\n")
}

#[test]
fn manifest_survives_signing_the_wire_and_verification() {
    let key = NodeKey::generate().unwrap();
    let signed = signed(&key, "0.3.0");

    // controller 把它放进一条 JSON 消息里发给节点。
    let wire = serde_json::to_string(&serde_json::json!({ "type": "update", "release": signed })).unwrap();
    let mut message: serde_json::Value = serde_json::from_str(&wire).unwrap();
    let received: SignedManifest = serde_json::from_value(message["release"].take()).unwrap();
    assert_eq!(received, signed);

    let verified = TrustedKeys::from(key.public()).verify(&received).unwrap();
    assert_eq!(verified, manifest("0.3.0"));
    assert!(!verified.is_prerelease());
}

#[test]
fn manifest_read_from_the_two_release_files_verifies() {
    let key = NodeKey::generate().unwrap();
    let signed = signed(&key, "0.3.0");
    let signature_file = format!("{}\n", signed.signature);
    let from_files = SignedManifest::from_files(signed.manifest.clone(), &signature_file).unwrap();
    assert_eq!(TrustedKeys::from(key.public()).verify(&from_files).unwrap(), manifest("0.3.0"));
}

#[test]
fn a_manifest_signed_with_the_wrong_key_does_not_verify() {
    let (key, other) = (NodeKey::generate().unwrap(), NodeKey::generate().unwrap());
    let signed = signed(&other, "0.3.0");
    assert!(matches!(TrustedKeys::from(key.public()).verify(&signed), Err(ManifestError::BadSignature)));
    assert!(TrustedKeys::from(other.public()).verify(&signed).is_ok());
}

#[test]
fn an_altered_manifest_does_not_verify() {
    let key = NodeKey::generate().unwrap();
    let keys = TrustedKeys::from(key.public());
    let original = signed(&key, "0.3.0");

    // 每一种改法得到的都还是一份合法的清单，只有签名能说明它被动过。
    let alterations: [fn(&str) -> String; 5] = [
        |text| text.replace("\"0.3.0\"", "\"0.4.0\""),
        |text| text.replace(&"01".repeat(32), &"ff".repeat(32)),
        |text| text.replace("12345678", "12345679"),
        |text| text.replace("i3t-linux-x86_64", "i3t-linux-x86_65"),
        |text| format!("{text} "),
    ];
    for (index, alter) in alterations.iter().enumerate() {
        let altered = SignedManifest { manifest: alter(&original.manifest), signature: original.signature };
        assert_ne!(altered.manifest, original.manifest, "第 {index} 种改法没有改到东西");
        assert!(serde_json::from_str::<Manifest>(&altered.manifest).is_ok(), "第 {index} 种改法应该得到一份合法的清单");
        assert!(matches!(keys.verify(&altered), Err(ManifestError::BadSignature)), "第 {index} 种改法");
    }
    assert!(keys.verify(&original).is_ok());
}

#[test]
fn a_signature_for_another_purpose_does_not_verify() {
    let key = NodeKey::generate().unwrap();
    let keys = TrustedKeys::from(key.public());
    let text = serde_json::to_string(&manifest("0.3.0")).unwrap();
    for purpose in ["relay-session", "probe", "", "release-manifest "] {
        let signature = ManifestSignature::from_bytes(*key.sign(purpose, text.as_bytes()).as_bytes());
        let signed = SignedManifest { manifest: text.clone(), signature };
        assert!(matches!(keys.verify(&signed), Err(ManifestError::BadSignature)), "{purpose:?}");
    }
    let signature = ManifestSignature::from_bytes(*key.sign(MANIFEST_PURPOSE, text.as_bytes()).as_bytes());
    assert!(keys.verify(&SignedManifest { manifest: text, signature }).is_ok());
}

#[test]
fn unverified_text_is_never_parsed() {
    let key = NodeKey::generate().unwrap();
    let stranger = NodeKey::generate().unwrap();
    // 不是 JSON 的东西配上不对的签名：报的是签名不对，说明没有走到解析那一步。
    let garbage = sign_manifest(&stranger, "这不是 JSON {{{".to_owned());
    assert!(matches!(TrustedKeys::from(key.public()).verify(&garbage), Err(ManifestError::BadSignature)));
    // 签名对了才轮到看内容。
    assert!(matches!(TrustedKeys::from(stranger.public()).verify(&garbage), Err(ManifestError::Malformed(_))));
}

#[test]
fn a_prerelease_manifest_is_recognisable_after_verification() {
    let key = NodeKey::generate().unwrap();
    let verified = TrustedKeys::from(key.public()).verify(&signed(&key, "0.3.0-rc.1")).unwrap();
    assert!(verified.is_prerelease());
    assert!(verified.version < "0.3.0".parse().unwrap());
    assert!(verified.version > "0.2.9".parse().unwrap());
}

#[test]
fn a_platform_the_release_was_not_built_for_has_no_artifact() {
    let key = NodeKey::generate().unwrap();
    let verified = TrustedKeys::from(key.public()).verify(&signed(&key, "0.3.0")).unwrap();
    let known: Platform = "linux-aarch64".parse().unwrap();
    assert_eq!(verified.artifact(&known).unwrap().file, "i3t-linux-aarch64");
    for unknown in ["linux-riscv64", "freebsd-x86_64", "windows-aarch64"] {
        assert_eq!(verified.artifact(&unknown.parse().unwrap()), None, "{unknown}");
    }
}
