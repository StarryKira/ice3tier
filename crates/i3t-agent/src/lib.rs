//! 节点的装配层：把各层装成一个可以运行的节点。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`state`] | 本地状态目录：节点密钥、controller 的地址和公钥、本地声明、最后一份 netmap |
//! | [`config`] | 使用者手改的配置文件 `config.toml` |
//! | [`control`] | 连 controller 的那条长连接：注册、收 netmap、上报、转发信令 |

pub mod config;
pub mod control;
pub mod state;
