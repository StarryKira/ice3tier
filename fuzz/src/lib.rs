//! fuzz 目标共用的环境：一个跑在回环上的入口（[`access`]），一个握好了手的 relay（[`relay`]）。
//!
//! 这两处读客户端字节的代码不是 crate 的公开接口，fuzz 输入只能从公开的入口进去：入口是一条连到它监听器上的 TCP 连接，
//! relay 是 [`i3t_relay::Relay::handle_udp`]。环境在第一个输入到来时建好，之后所有输入共用。

pub mod access;
pub mod relay;
