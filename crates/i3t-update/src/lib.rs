//! 节点更新。
//!
//! 节点不连发布源。controller 把带签名的发布清单发下来，节点用编译在程序里的公钥验过之后，经控制链路把包一段一段取回来，
//! 核对、自检、换上去，重启；新程序连上 controller 才算成功，否则换回旧的。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | `trust` | 认哪些发布公钥，怎么给清单签名和验签（[`TrustedKeys`]、[`sign_manifest`]） |
//! | `source` | 包从哪里来：调用方要实现的 [`PackageSource`] |
//! | `download` | 分块取包、断点续传、核对大小和 SHA-256 |
//! | [`mod@self_check`] | 新程序的自检：旧程序怎么运行它，它要确认什么 |
//! | `program` | 程序文件的替换与换回，以及 Windows 和 Unix 的差别 |
//! | `state` | 持久状态：哪个版本待确认，哪些版本失败过 |
//! | `timing` | 等 controller 指定的时机：空闲、立刻、排空之后 |
//! | `blocker` | 这个节点能不能自己更新（[`self_update_blocker`]） |
//! | `updater` | 把上面这些连成一次更新的 [`Updater`] |
//!
//! 清单的格式和节点报给 controller 的状态在 `i3t_model::update` 里。

mod blocker;
mod digest;
mod download;
mod error;
mod program;
pub mod self_check;
mod source;
mod state;
mod timing;
mod trust;
mod updater;

pub use blocker::{current_program, self_update_blocker};
pub use digest::FileDigest;
pub use error::{ManifestError, Refusal, UpdateError};
pub use self_check::{SelfCheckFailure, self_check};
pub use source::{ChunkRequest, PackageSource, SourceError};
pub use state::FailedVersion;
pub use trust::{MANIFEST_PURPOSE, TrustedKeys, sign_manifest};
pub use updater::{
    CONFIRMATION_WINDOW, MAX_UNCONFIRMED_STARTS, Offer, Prepared, RESTART_EXIT_CODE, RestartRequired, RolledBack, Startup, Updater,
    UpdaterConfig,
};
