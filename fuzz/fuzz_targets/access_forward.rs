//! TCP 端口转发的入口：连上来的客户端先说的那一段就是首批数据，随 OPEN 一起发出。是 TLS 的 ClientHello 时要把那条记录读完整，
//! 别的有多少读多少（`first_data` 的读法；SOCKS5 和 HTTP 的乐观回复用的是同一段代码）。
//!
//! 输入的第一个字节是分两次发时的切点（0 不切），剩下的是客户端发的字节。环境见 `i3t_fuzz::access`。

#![no_main]

use i3t_fuzz::access::{AccessWorld, Entry};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let Some((&split, bytes)) = input.split_first() else { return };
    AccessWorld::get().exchange(Entry::Forward, bytes, usize::from(split));
});
