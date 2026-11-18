//! HTTP 代理入口：请求行和请求头（`CONNECT`，和转发普通的 `http://` 请求）、`Proxy-Authorization: Basic`、
//! 请求体的分帧（`Content-Length`、`chunked` 和它的尾部），之后按目标选的路。认证的那个代理两种协议都说，第一个字节决定走哪一种。
//!
//! 输入的第一个字节的第 0 位选代理（0 只说 HTTP、不认证，1 两种都说、认用户名和密码），第二个字节是分两次发时的切点（0 不切），
//! 剩下的是客户端发的字节。
//! 环境见 `i3t_fuzz::access`。

#![no_main]

use i3t_fuzz::access::{AccessWorld, Entry};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let Some((&[mode, split], bytes)) = input.split_first_chunk::<2>() else { return };
    let entry = if mode & 1 == 0 { Entry::Http } else { Entry::HttpCredential };
    AccessWorld::get().exchange(entry, bytes, usize::from(split));
});
