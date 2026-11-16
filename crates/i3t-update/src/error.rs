//! 更新过程里会出的错。

use std::error::Error;
use std::fmt::Write;
use std::io;
use std::path::{Path, PathBuf};

use i3t_model::update::{FailureKind, Platform, SelfUpdateBlocker, Sha256, Version};

/// 一份带签名的清单没有通过检查。
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("清单的签名不是任何一把受信任的发布公钥签的")]
    BadSignature,
    /// 签名是对的，所以这是发布的人写错了清单，或者清单是给更新的程序看的。
    #[error("清单的签名正确，但内容读不懂")]
    Malformed(#[source] serde_json::Error),
}

/// 节点不肯装一份清单的原因。这时什么都还没有动。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Refusal {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("{0}")]
    CannotSelfUpdate(SelfUpdateBlocker),
    #[error("清单里的版本 {offered} 不比正在运行的 {running} 新")]
    NotNewer { offered: Version, running: Version },
    #[error("{offered} 是预发布版，而正在运行的 {running} 是正式版")]
    Prerelease { offered: Version, running: Version },
    #[error("版本 {version} 在这台机器上失败过，不再自动重试（上次的原因：{reason}）")]
    FailedBefore { version: Version, reason: String },
    #[error("版本 {version} 没有 {platform} 平台的包")]
    NoArtifact { version: Version, platform: Platform },
    /// 这个进程已经换上了一个版本，还没有重启。重启之前不能再换。
    #[error("已经换上了 {installed}，在等进程重启")]
    RestartPending { installed: Version },
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateError {
    #[error("不装这份清单")]
    Refused(#[from] Refusal),
    /// 给包的一方明确说没有，或者给出来的东西不合约定。链路中断不在这里：那种情况一直重试，不返回。
    #[error("取不到包：{0}")]
    Unavailable(String),
    #[error("包的内容和清单对不上：大小 {actual_size} 字节、SHA-256 {actual}，清单上是 {expected_size} 字节、{expected}")]
    Corrupt { expected_size: u64, expected: Sha256, actual_size: u64, actual: Sha256 },
    #[error("版本 {version} 的自检没有通过：{reason}")]
    SelfCheck { version: Version, reason: String },
    #[error("调用方不再通报流的数量，等不到换程序的时机")]
    Cancelled,
    #[error("{action} {} 失败", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("更新状态文件 {} 读不懂", path.display())]
    State {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl UpdateError {
    /// 这次失败算不算"这个版本在这台机器上不能用"。只有自检不通过算；
    /// 另一种（装上之后没能连上 controller）不经过这个类型，由回退的那一步直接记下。
    #[must_use]
    pub const fn kind(&self) -> FailureKind {
        match self {
            Self::SelfCheck { .. } => FailureKind::VersionRejected,
            Self::Refused(_) | Self::Unavailable(_) | Self::Corrupt { .. } | Self::Cancelled | Self::Io { .. } | Self::State { .. } => {
                FailureKind::NotInstalled
            }
        }
    }

    /// 给 `map_err` 用：把一个 I/O 错误连同"在对哪个文件做什么"一起包起来。
    pub(crate) fn io(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Self {
        let path = path.to_owned();
        move |source| Self::Io { action, path, source }
    }
}

/// 一个错误连同它的全部来由，写成一句话。上报给 controller 的失败原因只有一个字符串，所以要把来由链接进去。
pub(crate) fn describe(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        // 往 String 里写不会失败。
        let _ = write!(text, "：{cause}");
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_includes_every_cause_once() {
        let error = UpdateError::Refused(Refusal::Manifest(ManifestError::BadSignature));
        assert_eq!(describe(&error), "不装这份清单：清单的签名不是任何一把受信任的发布公钥签的");
        let error = UpdateError::Io { action: "写", path: PathBuf::from("/x/y"), source: io::Error::other("磁盘满了") };
        assert_eq!(describe(&error), "写 /x/y 失败：磁盘满了");
    }

    #[test]
    fn only_a_failed_self_check_blames_the_version() {
        let version = Version::new(1, 0, 0);
        assert_eq!(UpdateError::SelfCheck { version, reason: String::new() }.kind(), FailureKind::VersionRejected);
        assert_eq!(UpdateError::Unavailable(String::new()).kind(), FailureKind::NotInstalled);
        assert_eq!(UpdateError::Refused(Refusal::Manifest(ManifestError::BadSignature)).kind(), FailureKind::NotInstalled);
    }
}
