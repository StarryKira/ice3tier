//! 把发布工具当成一个程序来跑：生成密钥、生成并签署清单、核对，以及各种被改动之后的核对。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use i3t_model::update::{MANIFEST_FILE, Manifest, Platform, SIGNATURE_FILE, SignedManifest};
use i3t_proto::PublicKey;
use i3t_update::{FileDigest, ManifestError, TrustedKeys};

fn tool(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_i3t-release")).args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// 一次准备好的发布：一把刚生成的钥匙，和一个放着各种文件的发布目录。
struct Release {
    _scratch: tempfile::TempDir,
    key_file: PathBuf,
    public_key: String,
    dir: PathBuf,
}

impl Release {
    /// 目录里有三个平台的节点程序，还有不该进清单的别的文件。
    fn prepared() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let key_file = scratch.path().join("release.key");
        let keygen = tool(&["keygen", "--out", text(&key_file)]);
        assert!(keygen.status.success(), "{}", stderr(&keygen));
        let public_key = stdout(&keygen).trim().to_owned();

        let dir = scratch.path().join("dist");
        fs::create_dir(&dir).unwrap();
        let files = [
            ("i3t-linux-x86_64", "linux 上的节点程序"),
            ("i3t-linux-aarch64", "arm 上的节点程序，内容不一样"),
            ("i3t-windows-x86_64.exe", "windows 上的节点程序"),
            ("i3t-controller-linux-x86_64", "controller 不走自动更新"),
            ("i3tctl-linux-x86_64", "命令行工具也不走"),
            ("README.txt", "别的文件"),
        ];
        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }
        Self { _scratch: scratch, key_file, public_key, dir }
    }

    fn sign(&self, version: &str) -> Output {
        tool(&["manifest", "--version", version, "--dir", text(&self.dir), "--key", text(&self.key_file), "--min-controller", "0.1.0"])
    }

    /// 签好的发布。
    fn signed(version: &str) -> Self {
        let release = Self::prepared();
        let output = release.sign(version);
        assert!(output.status.success(), "{}", stderr(&output));
        release
    }

    fn verify(&self) -> Output {
        tool(&["verify", "--dir", text(&self.dir), "--key", &self.public_key])
    }

    fn manifest_text(&self) -> String {
        fs::read_to_string(self.dir.join(MANIFEST_FILE)).unwrap()
    }

    /// 节点那一边会怎么读这个目录里的清单。
    fn as_a_node_sees_it(&self) -> Result<Manifest, ManifestError> {
        let signature = fs::read_to_string(self.dir.join(SIGNATURE_FILE)).unwrap();
        let signed = SignedManifest::from_files(self.manifest_text(), &signature).unwrap();
        TrustedKeys::from(self.public_key.parse::<PublicKey>().unwrap()).verify(&signed)
    }
}

#[test]
fn keygen_writes_a_private_key_file_and_prints_only_the_public_key() {
    let release = Release::prepared();
    release.public_key.parse::<PublicKey>().expect("标准输出上应该只有一行公钥");
    let pem = fs::read_to_string(&release.key_file).unwrap();
    assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"), "{pem}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&release.key_file).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn keygen_does_not_overwrite_an_existing_key() {
    let release = Release::prepared();
    let before = fs::read(&release.key_file).unwrap();
    let again = tool(&["keygen", "--out", text(&release.key_file)]);
    assert!(!again.status.success());
    assert_eq!(stdout(&again), "", "失败时不应该打印出一把没有存下来的钥匙的公钥");
    assert_eq!(fs::read(&release.key_file).unwrap(), before);
}

#[test]
fn manifest_lists_the_node_programs_and_the_node_accepts_it() {
    let release = Release::signed("0.3.0");
    let manifest = release.as_a_node_sees_it().unwrap();
    assert_eq!(manifest.version.to_string(), "0.3.0");
    assert_eq!(manifest.min_controller.to_string(), "0.1.0");
    assert!(!manifest.is_prerelease());
    assert!(manifest.published > 1_700_000_000);
    assert_eq!(manifest.artifacts.keys().collect::<Vec<_>>(), ["linux-aarch64", "linux-x86_64", "windows-x86_64"]);
    for (platform, artifact) in &manifest.artifacts {
        let platform: Platform = platform.parse().unwrap();
        assert_eq!(artifact.file, platform.program_file("i3t"));
        assert!(FileDigest::of_file(&release.dir.join(&artifact.file)).unwrap().matches(artifact), "{platform}");
        assert_eq!(manifest.artifact(&platform), Some(artifact));
    }
    assert!(release.manifest_text().ends_with("}\n"));
}

#[test]
fn a_fresh_release_verifies() {
    let release = Release::signed("0.3.0");
    let output = release.verify();
    assert!(output.status.success(), "{}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("版本 0.3.0（正式版）"), "{report}");
    assert_eq!(report.matches("通过").count(), 3, "{report}");
}

#[test]
fn a_prerelease_is_reported_as_one() {
    let release = Release::signed("0.3.0-rc.1");
    assert!(release.as_a_node_sees_it().unwrap().is_prerelease());
    let report = stdout(&release.verify());
    assert!(report.contains("版本 0.3.0-rc.1（预发布版）"), "{report}");
}

#[test]
fn manifest_refuses_to_overwrite_a_signed_release() {
    let release = Release::signed("0.3.0");
    let before = (release.manifest_text(), fs::read(release.dir.join(SIGNATURE_FILE)).unwrap());
    let again = release.sign("0.3.1");
    assert!(!again.status.success());
    assert!(stderr(&again).contains("已经存在"), "{}", stderr(&again));
    assert_eq!((release.manifest_text(), fs::read(release.dir.join(SIGNATURE_FILE)).unwrap()), before);

    // 只剩一个文件时也不覆盖：另一个不见了多半是出了岔子，得让人来看。
    fs::remove_file(release.dir.join(MANIFEST_FILE)).unwrap();
    assert!(!release.sign("0.3.1").status.success());
    assert!(!release.dir.join(MANIFEST_FILE).exists());
}

#[test]
fn manifest_needs_a_valid_version_a_key_and_at_least_one_node_program() {
    let release = Release::prepared();
    for bad_version in ["v0.3.0", "0.3", "0.3.0+build"] {
        assert!(!release.sign(bad_version).status.success(), "{bad_version}");
    }
    let not_a_key = release.dir.join("README.txt");
    let output = tool(&["manifest", "--version", "0.3.0", "--dir", text(&release.dir), "--key", text(&not_a_key)]);
    assert!(!output.status.success());

    for name in ["i3t-linux-x86_64", "i3t-linux-aarch64", "i3t-windows-x86_64.exe"] {
        fs::remove_file(release.dir.join(name)).unwrap();
    }
    let output = release.sign("0.3.0");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("没有节点程序"), "{}", stderr(&output));
    assert!(!release.dir.join(MANIFEST_FILE).exists() && !release.dir.join(SIGNATURE_FILE).exists());
}

#[test]
fn a_changed_file_is_detected() {
    let release = Release::signed("0.3.0");
    // 大小不变，只改一个字节。
    let path = release.dir.join("i3t-linux-aarch64");
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, bytes).unwrap();

    let output = release.verify();
    assert!(!output.status.success());
    let report = stdout(&output);
    assert_eq!(report.matches("通过").count(), 2, "{report}");
    assert!(report.contains("不符  linux-aarch64") && report.contains("SHA-256"), "{report}");
}

#[test]
fn a_truncated_or_missing_file_is_detected() {
    let release = Release::signed("0.3.0");
    fs::write(release.dir.join("i3t-linux-x86_64"), "短了").unwrap();
    fs::remove_file(release.dir.join("i3t-windows-x86_64.exe")).unwrap();
    let output = release.verify();
    assert!(!output.status.success());
    let report = stdout(&output);
    assert!(report.contains("不符  linux-x86_64") && report.contains("大小"), "{report}");
    assert!(report.contains("不符  windows-x86_64") && report.contains("读不了"), "{report}");
    assert!(stderr(&output).contains("2 个文件"), "{}", stderr(&output));
}

#[test]
fn a_changed_manifest_is_detected() {
    let release = Release::signed("0.3.0");
    let tampered = release.manifest_text().replace("\"0.3.0\"", "\"9.9.9\"");
    assert_ne!(tampered, release.manifest_text());
    fs::write(release.dir.join(MANIFEST_FILE), tampered).unwrap();
    let output = release.verify();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("签名"), "{}", stderr(&output));
    assert_eq!(stdout(&output), "", "签名不对的清单，里面写的什么都不该被当真");
    assert!(matches!(release.as_a_node_sees_it(), Err(ManifestError::BadSignature)));
}

#[test]
fn a_signature_from_another_release_is_detected() {
    let release = Release::signed("0.3.0");
    let other = Release::signed("0.3.0");
    fs::copy(other.dir.join(SIGNATURE_FILE), release.dir.join(SIGNATURE_FILE)).unwrap();
    assert!(!release.verify().status.success());

    fs::write(release.dir.join(SIGNATURE_FILE), "不是签名\n").unwrap();
    assert!(!release.verify().status.success());
}

#[test]
fn verification_uses_the_compiled_in_keys_unless_told_otherwise() {
    let release = Release::signed("0.3.0");
    // 测试里的钥匙是刚生成的，不在编译进去的公钥里：不指定公钥就应该验不过。
    let output = tool(&["verify", "--dir", text(&release.dir)]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("kWB8NBoJtLGX7mZwXgMSZ1C8RkUDpkdOphusy_JhEk0"), "应该说明认的是哪些公钥：{}", stderr(&output));

    let stranger = Release::prepared();
    let output = tool(&["verify", "--dir", text(&release.dir), "--key", &stranger.public_key]);
    assert!(!output.status.success());
}
