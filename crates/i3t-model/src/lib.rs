//! ice3tier 控制面的数据。
//!
//! 这里只有类型和它们的 JSON 形式，没有行为。controller 产生它们，节点消费它们，WebUI 和命令行通过管理 API 读写它们，
//! 所以这是几方共同遵守的约定。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`netmap`] | 一个节点运行所需要的全部配置 |
//! | [`control`] | 节点和 controller 之间的消息 |
//!
//! 兼容性的规矩：新增的字段都带 `#[serde(default)]`，收的一方忽略不认识的字段。
//! 这样节点和 controller 的版本可以不一样（节点更新是分批的，§10.5）。

pub mod control;
pub mod netmap;

pub use netmap::Netmap;
