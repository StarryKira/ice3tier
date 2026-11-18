//! SOCKS5 入口：问候、用户名密码认证、CONNECT / UDP ASSOCIATE 请求，之后按目标选的路（经代理的出口 `gw` 出去、组网目标），
//! 乐观回复时还有首批数据的读法；以及 UDP 关联里每个报文的头。认证的那个代理两种协议都说，第一个字节决定走哪一种。
//!
//! 输入的第一个字节选做什么：
//!
//! - 第 1 位是 0：一条 TCP 连接。第 0 位选代理（0 只说 SOCKS5、不认证，1 两种都说、认用户名和密码），
//!   第二个字节是分两次发时的切点（0 不切），剩下的是客户端发的字节。
//! - 第 1 位是 1：在不认证的代理上做一次合法的 UDP ASSOCIATE，剩下的是若干个报文，每个前面 2 字节的长度（大端）。
//!
//! 环境见 `i3t_fuzz::access`。

#![no_main]

use i3t_fuzz::access::{AccessWorld, Entry};
use libfuzzer_sys::fuzz_target;

/// 一次最多发几个 UDP 报文。
const MAX_DATAGRAMS: usize = 8;

fuzz_target!(|input: &[u8]| {
    let Some((&mode, rest)) = input.split_first() else { return };
    let world = AccessWorld::get();
    if mode & 0b10 == 0 {
        let Some((&split, bytes)) = rest.split_first() else { return };
        let entry = if mode & 1 == 0 { Entry::Socks5 } else { Entry::Socks5Credential };
        world.exchange(entry, bytes, usize::from(split));
    } else {
        world.associate(&datagrams(rest));
    }
});

/// 按 2 字节的长度前缀切出报文；最后一个不够长时有多少算多少。
fn datagrams(mut rest: &[u8]) -> Vec<&[u8]> {
    let mut datagrams = Vec::new();
    while let Some((&[hi, lo], tail)) = rest.split_first_chunk::<2>() {
        if datagrams.len() == MAX_DATAGRAMS {
            break;
        }
        let (datagram, tail) = tail.split_at(usize::from(u16::from_be_bytes([hi, lo])).min(tail.len()));
        datagrams.push(datagram);
        rest = tail;
    }
    datagrams
}
