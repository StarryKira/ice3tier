//! 包从哪里来。

use std::future::Future;

use i3t_model::update::{Platform, Version};

/// 请给出某个版本、某个平台的包里从 `offset` 开始的一段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRequest<'a> {
    pub version: &'a Version,
    pub platform: &'a Platform,
    pub offset: u64,
    /// 最多要多少字节。
    pub len: u32,
}

/// 节点取包的来路。节点不连发布源，包由 controller 缓存，节点经控制链路一段一段地取；
/// 这个 trait 就是那条链路在更新这一层眼里的样子。
pub trait PackageSource: Send + Sync {
    /// 取 `[offset, offset + len)` 这一段。
    ///
    /// 约定：返回的字节数不多于 `len`，也不能是 0；可以比 `len` 少，下载的一方会接着要剩下的。
    /// 给出的内容不需要可信，取完之后会和签过名的清单核对大小和 SHA-256。
    fn fetch(&self, request: ChunkRequest<'_>) -> impl Future<Output = Result<Vec<u8>, SourceError>> + Send;
}

/// 一段没有取到。两种情况下载的一方处理不同，所以要分开报。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// 链路断了、超时这一类：过一会儿从同一个位置再要，不算更新失败。
    #[error("取包被打断：{0}")]
    Interrupted(String),
    /// 对方明确表示给不了（没有这个版本、没有这个平台的包）：再要也没有用，这次更新到此为止。
    #[error("对方给不了这个包：{0}")]
    Unavailable(String),
}
