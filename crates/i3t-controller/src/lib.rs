//! ice3tier 的 controller。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`store`] | 存储：一个 SQLite 文件 |
//! | [`access`] | 访问规则的编译：谁能访问谁 |
//! | [`netmap`] | 给一个节点算它的 netmap |
//! | [`Controller`] | 存储之上的全部规则，和 netmap 的下发。节点协议和管理 API 都只是它的入口 |
//! | [`node_server`]、[`frame`] | 节点协议的服务端和分帧 |
//! | [`addr_server`] | 地址查询的应答 |
//! | [`admin`] | 管理 API |
//! | [`Server`] | 把上面这些装在一起跑 |
//!
//! ```no_run
//! # async fn example() -> Result<(), i3t_controller::Error> {
//! use std::sync::Arc;
//! use i3t_controller::{Controller, ServeOptions, Server};
//!
//! let opened = Controller::open("/var/lib/i3t-controller".as_ref(), "ctl.example.com:20331".to_owned())?;
//! if let Some(token) = opened.admin_token {
//!     println!("管理令牌（只显示这一次）：{token}");
//! }
//! let options = ServeOptions {
//!     node_listen: "0.0.0.0:20331".parse().expect("合法的地址"),
//!     admin_listen: "127.0.0.1:20332".parse().expect("合法的地址"),
//!     static_dir: None,
//! };
//! let server = Server::bind(Arc::new(opened.controller), options).await?;
//! server.run(async { tokio::signal::ctrl_c().await.ok(); }).await
//! # }
//! ```

pub mod access;
pub mod addr_server;
pub mod admin;
mod controller;
mod error;
pub mod frame;
pub mod netmap;
pub mod node_server;
mod secret;
mod server;
pub mod store;
mod sync;

pub use controller::{ALPN, Admitted, Controller, Opened, Timing};
pub use error::{Capability, Entity, Error, Rejection};
pub use server::{ServeOptions, Server};
