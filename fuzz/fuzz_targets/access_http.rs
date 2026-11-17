//! HTTP 代理入口：请求行和请求头（`CONNECT`，和转发普通的 `http://` 请求）、`Proxy-Authorization: Basic`（用户名是出口和
//! 服务等级的写法，密码是接入凭据）、请求体的分帧（`Content-Length`、`chunked` 和它的尾部），之后按用户名和目标选的路。
//!
//! 输入的第一个字节的第 0 位选监听器（0 不要凭据，1 要凭据），第二个字节是分两次发时的切点（0 不切），剩下的是客户端发的字节。
//! 环境见 `i3t_fuzz::access`。

#![no_main]

use i3t_fuzz::access::{AccessWorld, Entry};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let Some((&[mode, split], bytes)) = input.split_first_chunk::<2>() else { return };
    let entry = if mode & 1 == 0 { Entry::Http } else { Entry::HttpCredential };
    AccessWorld::get().exchange(entry, bytes, usize::from(split));
});
