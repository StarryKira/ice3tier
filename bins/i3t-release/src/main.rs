//! `i3t-release`：发布时用的工具。生成发布签名密钥，为一个发布目录生成清单并签名，核对一个发布目录。
//!
//! 发布目录是平铺的，里面的东西原样上传到 GitHub Release，镜像也照这个样子放（见 `docs/releasing.md`）：
//!
//! ```text
//! i3t-linux-x86_64            各个平台的节点程序，名字是 i3t-<平台>，Windows 上加 .exe
//! i3t-windows-x86_64.exe
//! …
//! manifest.json               清单：版本号，以及上面每个文件的大小和 SHA-256
//! manifest.json.sig           清单的签名
//! ```
//!
//! 目录里别的文件（controller、命令行工具）不进清单：它们不走节点的自动更新。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail, ensure};
use clap::{Parser, Subcommand};
use i3t_crypto::NodeKey;
use i3t_model::update::{Artifact, MANIFEST_FILE, Manifest, NODE_PROGRAM, Platform, SIGNATURE_FILE, SignedManifest, Version};
use i3t_proto::PublicKey;
use i3t_update::{FileDigest, TrustedKeys, sign_manifest};

#[derive(Parser)]
#[command(name = "i3t-release", version, about = "ice3tier 的发布工具")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 生成一把新的发布签名密钥，写进文件（只有属主可读），并打印它的公钥
    Keygen {
        /// 私钥写到哪个文件。已经存在的文件不会被覆盖
        #[arg(long, value_name = "文件")]
        out: PathBuf,
    },
    /// 为发布目录里的节点程序生成 manifest.json 并签名，得到 manifest.json.sig
    Manifest {
        /// 这次发布的版本号，例如 0.3.0；预发布版写成 0.3.0-rc.1
        #[arg(long, value_name = "版本号")]
        version: Version,
        /// 发布目录。里面的节点程序要叫 i3t-<平台>，例如 i3t-linux-x86_64、i3t-windows-x86_64.exe
        #[arg(long, value_name = "目录")]
        dir: PathBuf,
        /// 发布签名密钥的文件（keygen 生成的那个）
        #[arg(long, value_name = "文件")]
        key: PathBuf,
        /// 这个版本的节点需要的 controller 最低版本。比它旧的 controller 不会下发这个版本
        #[arg(long, value_name = "版本号", default_value = "0.0.0")]
        min_controller: Version,
    },
    /// 核对一个发布目录：清单的签名，以及清单里每个文件的大小和 SHA-256
    Verify {
        /// 发布目录
        #[arg(long, value_name = "目录")]
        dir: PathBuf,
        /// 只认这一把公钥（base64url）。不给的话用编译在本程序里的发布公钥，和节点认的一样
        // base64url 的字符里有 `-`，公钥可能以它开头，不能被当成另一个选项。
        #[arg(long, value_name = "公钥", allow_hyphen_values = true)]
        key: Option<PublicKey>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Keygen { out } => keygen(&out),
        Command::Manifest { version, dir, key, min_controller } => manifest(version, min_controller, &dir, &key),
        Command::Verify { dir, key } => verify(&dir, key),
    }
}

fn keygen(out: &Path) -> anyhow::Result<()> {
    let key = NodeKey::generate().context("生成密钥")?;
    let mut file = create_private(out).with_context(|| format!("创建 {}（已经存在的文件不会被覆盖）", out.display()))?;
    file.write_all(key.to_pem().as_bytes()).and_then(|()| file.sync_all()).with_context(|| format!("写 {}", out.display()))?;
    // 标准输出上只有公钥，方便脚本接；给人看的话走标准错误。
    println!("{}", key.public());
    eprintln!("私钥已经写到 {}。上面这一行是公钥：节点只认编译在程序里的公钥，", out.display());
    eprintln!("所以要么把它加进 i3t-update 的公钥列表，要么编译时设 I3T_RELEASE_EXTRA_KEY=<公钥>。");
    Ok(())
}

/// 新建一个只有属主可读写的文件。文件已经存在时失败：签名密钥不能被不小心盖掉。
#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

// Windows 上文件的访问控制继承自所在的目录，所以要把密钥放在只有自己能进的目录里。
#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn manifest(version: Version, min_controller: Version, dir: &Path, key_file: &Path) -> anyhow::Result<()> {
    let pem = fs::read_to_string(key_file).with_context(|| format!("读签名密钥 {}", key_file.display()))?;
    let key = NodeKey::from_pem(&pem).with_context(|| format!("解析签名密钥 {}", key_file.display()))?;
    let (manifest_path, signature_path) = (dir.join(MANIFEST_FILE), dir.join(SIGNATURE_FILE));
    for path in [&manifest_path, &signature_path] {
        ensure!(!path.exists(), "{} 已经存在。签过名的清单不覆盖；确实要重做的话先把它删掉", path.display());
    }

    let artifacts = node_programs(dir)?;
    ensure!(
        !artifacts.is_empty(),
        "{} 里没有节点程序。它们应该叫 {NODE_PROGRAM}-<平台>，例如 {NODE_PROGRAM}-linux-x86_64、{NODE_PROGRAM}-windows-x86_64.exe",
        dir.display()
    );
    let published = SystemTime::now().duration_since(UNIX_EPOCH).context("系统时钟早于 1970 年")?.as_secs();
    let manifest = Manifest { version, min_controller, published, artifacts };
    let text = serde_json::to_string_pretty(&manifest).context("生成清单")? + "\n";
    let signed = sign_manifest(&key, text);

    write_new(&manifest_path, signed.manifest.as_bytes())?;
    write_new(&signature_path, format!("{}\n", signed.signature).as_bytes())?;

    println!("版本 {}（{}），签名用的公钥 {}", manifest.version, release_kind(&manifest), key.public());
    for (platform, artifact) in &manifest.artifacts {
        println!("  {platform:<16} {}  {} 字节  {}", artifact.file, artifact.size, artifact.sha256);
    }
    println!("已写出 {} 和 {}", manifest_path.display(), signature_path.display());
    if !TrustedKeys::compiled_in().iter().any(|trusted| *trusted == key.public()) {
        eprintln!("注意：这把钥匙不在本程序编译进去的发布公钥里。同样编译出来的节点不会认这份清单。");
    }
    Ok(())
}

/// 目录里各个平台的节点程序，以平台名为键。别的文件不管。
fn node_programs(dir: &Path) -> anyhow::Result<BTreeMap<String, Artifact>> {
    let mut artifacts = BTreeMap::new();
    for entry in fs::read_dir(dir).with_context(|| format!("读目录 {}", dir.display()))? {
        let entry = entry.with_context(|| format!("读目录 {}", dir.display()))?;
        let Some(file) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Some(platform) = node_program_platform(&file) else { continue };
        let path = entry.path();
        ensure!(path.is_file(), "{} 不是一个普通文件", path.display());
        let digest = FileDigest::of_file(&path).with_context(|| format!("读 {}", path.display()))?;
        ensure!(digest.size > 0, "{} 是空的", path.display());
        artifacts.insert(platform.to_string(), Artifact { file, size: digest.size, sha256: digest.sha256 });
    }
    Ok(artifacts)
}

/// 一个文件名是不是某个平台的节点程序；是的话是哪个平台。
///
/// 反过来核对一遍名字，所以 `i3t-linux-x86_64.exe`、`i3t-windows-x86_64` 这种和平台对不上的写法不算。
fn node_program_platform(file: &str) -> Option<Platform> {
    let rest = file.strip_prefix(NODE_PROGRAM)?.strip_prefix('-')?;
    let platform: Platform = rest.strip_suffix(".exe").unwrap_or(rest).parse().ok()?;
    (platform.program_file(NODE_PROGRAM) == file).then_some(platform)
}

/// 新建并写入。文件已经存在时失败。
fn write_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let write = || -> io::Result<()> {
        let mut file = File::create_new(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    write().with_context(|| format!("写 {}", path.display()))
}

fn release_kind(manifest: &Manifest) -> &'static str {
    if manifest.is_prerelease() { "预发布版" } else { "正式版" }
}

fn verify(dir: &Path, key: Option<PublicKey>) -> anyhow::Result<()> {
    let read = |name: &str| fs::read_to_string(dir.join(name)).with_context(|| format!("读 {}", dir.join(name).display()));
    let signed =
        SignedManifest::from_files(read(MANIFEST_FILE)?, &read(SIGNATURE_FILE)?).with_context(|| format!("解析 {SIGNATURE_FILE}"))?;
    let keys = key.map_or_else(TrustedKeys::compiled_in, TrustedKeys::from);
    let manifest = keys.verify(&signed).with_context(|| {
        let trusted: Vec<String> = keys.iter().map(ToString::to_string).collect();
        format!("核对 {MANIFEST_FILE} 的签名（认的公钥：{}）", trusted.join("、"))
    })?;
    println!("版本 {}（{}），需要 controller {} 或更新，签名正确", manifest.version, release_kind(&manifest), manifest.min_controller);

    let mut mismatched = 0;
    for (platform, artifact) in &manifest.artifacts {
        match check_file(dir, artifact) {
            Ok(()) => println!("  通过  {platform:<16} {}", artifact.file),
            Err(error) => {
                println!("  不符  {platform:<16} {}：{error:#}", artifact.file);
                mismatched += 1;
            }
        }
    }
    ensure!(mismatched == 0, "清单里有 {mismatched} 个文件和目录里的对不上");
    println!("清单里的 {} 个文件都核对无误", manifest.artifacts.len());
    Ok(())
}

fn check_file(dir: &Path, artifact: &Artifact) -> anyhow::Result<()> {
    // 清单是签过名的，但文件名要拿去拼路径，还是只认不带目录的名字。
    ensure!(Path::new(&artifact.file).file_name().is_some_and(|name| name == artifact.file.as_str()), "文件名里不应该有目录");
    let digest = FileDigest::of_file(&dir.join(&artifact.file)).context("读不了")?;
    if digest.size != artifact.size {
        bail!("大小是 {} 字节，清单上是 {} 字节", digest.size, artifact.size);
    }
    ensure!(digest.sha256 == artifact.sha256, "SHA-256 是 {}，清单上是 {}", digest.sha256, artifact.sha256);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_node_programs_named_for_a_platform_are_recognised() {
        let platform = |file: &str| node_program_platform(file).map(|platform| platform.to_string());
        assert_eq!(platform("i3t-linux-x86_64").as_deref(), Some("linux-x86_64"));
        assert_eq!(platform("i3t-linux-aarch64").as_deref(), Some("linux-aarch64"));
        assert_eq!(platform("i3t-macos-aarch64").as_deref(), Some("macos-aarch64"));
        assert_eq!(platform("i3t-windows-x86_64.exe").as_deref(), Some("windows-x86_64"));
        let others = [
            "i3t",
            "i3t-",
            "i3t-linux",
            "i3t-controller-linux-x86_64",
            "i3tctl-linux-x86_64",
            "i3t-linux-x86_64.exe",
            "i3t-windows-x86_64",
            "i3t-linux-x86_64.sha256",
            "i3t-linux-x86_64.old",
            "manifest.json",
            "manifest.json.sig",
            "I3T-linux-x86_64",
        ];
        for file in others {
            assert_eq!(platform(file), None, "{file} 不应该被当成节点程序");
        }
    }

    #[test]
    fn command_line_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn a_public_key_starting_with_a_hyphen_is_still_a_value() {
        let key = format!("-{}", "A".repeat(42));
        let cli = Cli::try_parse_from(["i3t-release", "verify", "--dir", "dist", "--key", &key]).unwrap();
        let Command::Verify { key: Some(parsed), .. } = cli.command else { panic!("应该解析成 verify") };
        assert_eq!(parsed.to_string(), key);
    }
}
