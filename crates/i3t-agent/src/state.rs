//! 节点的本地状态目录。
//!
//! | 文件 | 内容 |
//! |---|---|
//! | `node.key` | 节点私钥，PKCS#8 的 PEM，只有属主可读 |
//! | `controller.json` | controller 的地址和公钥，注册时从令牌里得到 |
//! | `local.json` | 本机的设置：本地声明、端口 |
//! | `netmap.json` | 最后一份成功应用的 netmap。controller 不在线时节点靠它照常工作 |
//!
//! 所有文件都是先写临时文件再改名，所以断电不会留下写了一半的文件。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use i3t_crypto::NodeKey;
use i3t_model::Netmap;
use i3t_model::control::Declared;
use i3t_proto::PublicKey;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const KEY_FILE: &str = "node.key";
const CONTROLLER_FILE: &str = "controller.json";
const LOCAL_FILE: &str = "local.json";
const NETMAP_FILE: &str = "netmap.json";

/// 节点默认绑定的端口。
pub const DEFAULT_PORT: u16 = 20330;

/// 到哪里找 controller，以及凭什么确认连上的是它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerInfo {
    /// `主机:端口`，主机可以是域名。
    pub address: String,
    pub key: PublicKey,
}

/// 只在本机决定、不由 controller 下发的设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSettings {
    /// 这台机器声明自己可以承担什么。启用与否由管理员在 WebUI 上决定，没有声明的启用不了。
    #[serde(default)]
    pub declared: Declared,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self { declared: Declared::default(), port: DEFAULT_PORT }
    }
}

const fn default_port() -> u16 {
    DEFAULT_PORT
}

#[derive(Debug, Clone)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    /// 打开状态目录，不存在就创建（只有属主可以进入）。
    ///
    /// # Errors
    ///
    /// 目录建不出来。
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).with_context(|| format!("创建状态目录 {}", root.display()))?;
        restrict_to_owner(&root, 0o700).with_context(|| format!("收紧状态目录 {} 的权限", root.display()))?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 读出节点私钥；还没有时返回 `None`。
    ///
    /// # Errors
    ///
    /// 文件读不了，或者内容不是一把私钥。
    pub fn key(&self) -> anyhow::Result<Option<NodeKey>> {
        let path = self.root.join(KEY_FILE);
        match fs::read_to_string(&path) {
            Ok(pem) => Ok(Some(NodeKey::from_pem(&pem).with_context(|| format!("解析私钥文件 {}", path.display()))?)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("读私钥文件 {}", path.display())),
        }
    }

    /// 读出节点私钥；还没有就生成一把并存下来。
    ///
    /// # Errors
    ///
    /// 同 [`key`](Self::key)，以及生成或写入失败。
    pub fn key_or_generate(&self) -> anyhow::Result<NodeKey> {
        if let Some(key) = self.key()? {
            return Ok(key);
        }
        let key = NodeKey::generate().context("生成节点私钥")?;
        self.write(KEY_FILE, key.to_pem().as_bytes())?;
        Ok(key)
    }

    /// # Errors
    ///
    /// 文件读不了或者解析不了。
    pub fn controller(&self) -> anyhow::Result<Option<ControllerInfo>> {
        self.read_json(CONTROLLER_FILE)
    }

    /// # Errors
    ///
    /// 写入失败。
    pub fn save_controller(&self, controller: &ControllerInfo) -> anyhow::Result<()> {
        self.write_json(CONTROLLER_FILE, controller)
    }

    /// 本机的设置；文件不存在时是默认值。
    ///
    /// # Errors
    ///
    /// 文件读不了或者解析不了。
    pub fn local(&self) -> anyhow::Result<LocalSettings> {
        Ok(self.read_json(LOCAL_FILE)?.unwrap_or_default())
    }

    /// # Errors
    ///
    /// 写入失败。
    pub fn save_local(&self, local: &LocalSettings) -> anyhow::Result<()> {
        self.write_json(LOCAL_FILE, local)
    }

    /// # Errors
    ///
    /// 文件读不了或者解析不了。
    pub fn netmap(&self) -> anyhow::Result<Option<Netmap>> {
        self.read_json(NETMAP_FILE)
    }

    /// # Errors
    ///
    /// 写入失败。
    pub fn save_netmap(&self, netmap: &Netmap) -> anyhow::Result<()> {
        self.write_json(NETMAP_FILE, netmap)
    }

    fn read_json<T: DeserializeOwned>(&self, name: &str) -> anyhow::Result<Option<T>> {
        let path = self.root.join(name);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| format!("解析 {}", path.display()))?)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("读 {}", path.display())),
        }
    }

    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(value).with_context(|| format!("序列化 {name}"))?;
        self.write(name, &bytes)
    }

    /// 先写临时文件再改名。目录里的文件一律只有属主可读：除了私钥，netmap 里也有对端的地址。
    fn write(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let path = self.root.join(name);
        let temporary = self.root.join(format!("{name}.tmp"));
        let result = (|| -> io::Result<()> {
            let mut file = create_owner_only(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)
        })();
        result.with_context(|| format!("写 {}", path.display()))
    }
}

#[cfg(unix)]
fn create_owner_only(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
}

// Windows 上文件默认继承目录的访问控制，状态目录建在只有本用户可以访问的位置。
#[cfg(not(unix))]
fn create_owner_only(path: &Path) -> io::Result<fs::File> {
    fs::File::create(path)
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (tempfile::TempDir, StateDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path().join("i3t")).unwrap();
        (dir, state)
    }

    #[test]
    fn key_is_generated_once_and_then_reused() {
        let (_dir, state) = state();
        assert!(state.key().unwrap().is_none());
        let first = state.key_or_generate().unwrap().public();
        let second = state.key_or_generate().unwrap().public();
        assert_eq!(first, second);
        assert_eq!(state.key().unwrap().unwrap().public(), first);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_and_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, state) = state();
        state.key_or_generate().unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&state.path().join(KEY_FILE)), 0o600);
        assert_eq!(mode(state.path()), 0o700);
    }

    #[test]
    fn settings_default_until_saved_and_survive_reopening() {
        let (_dir, state) = state();
        assert_eq!(state.local().unwrap(), LocalSettings::default());
        assert_eq!(state.controller().unwrap(), None);

        let local = LocalSettings { declared: Declared { exit: true, relay: false, forward: true }, port: 20444 };
        let controller = ControllerInfo { address: "ctl.example.com:20331".to_owned(), key: PublicKey::from_bytes([5; 32]) };
        state.save_local(&local).unwrap();
        state.save_controller(&controller).unwrap();

        let reopened = StateDir::open(state.path()).unwrap();
        assert_eq!(reopened.local().unwrap(), local);
        assert_eq!(reopened.controller().unwrap(), Some(controller));
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_silent_default() {
        let (_dir, state) = state();
        fs::write(state.path().join(LOCAL_FILE), b"{ not json").unwrap();
        assert!(state.local().is_err());
        fs::write(state.path().join(KEY_FILE), b"garbage").unwrap();
        assert!(state.key().is_err());
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let (_dir, state) = state();
        state.save_local(&LocalSettings::default()).unwrap();
        let names: Vec<_> = fs::read_dir(state.path()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, [LOCAL_FILE]);
    }
}
