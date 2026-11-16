//! 节点的装配层：把各层装成一个可以运行的节点。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`state`] | 本地状态目录：节点密钥、controller 的地址和公钥、本地声明、最后一份 netmap |
//! | [`config`] | 使用者手改的配置文件 `config.toml` |
//! | [`program`] | 这个节点程序自己的情况：版本、程序文件、能不能自己更新 |
//! | [`control`] | 连 controller 的那条长连接：注册、收 netmap、上报、转发信令、取更新包、回答路由查询 |
//! | [`join`] | 用注册令牌加入网络 |
//! | [`node`] | 把各层装成一个运行着的节点 |
//! | [`drain`] | 排空：更新之前不再接新的流 |
//! | [`selfcheck`] | 新程序的自检（`i3t self-check`） |
//! | [`status`]、[`local`] | 运行着的节点的状态，和本机问它的接口（`i3t status`、`i3t routes`、`/metrics`） |
//!
//! 节点更新的验签、取包、替换和回退在 `i3t-update` 里，这里只是把它接到长连接和数据面上。

pub mod config;
pub mod control;
pub mod drain;
mod flows;
mod http;
pub mod join;
pub mod local;
mod metrics;
pub mod node;
pub mod program;
mod runtime;
pub mod selfcheck;
pub mod state;
pub mod status;
mod update;

pub use i3t_model::update::Version;
pub use i3t_update::RESTART_EXIT_CODE;
