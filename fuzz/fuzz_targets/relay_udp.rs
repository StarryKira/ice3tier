//! relay 收到的 UDP 数据报（[`Relay::handle_udp`](i3t_relay::Relay::handle_udp)）：握手、保活、一级和多级中转帧、
//! 一批里混着发往不同节点的数据报。来源可以是有会话的节点、骨干上的另一个 relay，或者没有会话的陌生地址。
//!
//! 输入的第一个字节选来源（低 2 位）和 ECN（再 2 位），接着 2 字节是一批里每个数据报的长度（0 表示整个是一个），
//! 剩下的是数据。环境见 `i3t_fuzz::relay`。

#![no_main]

use i3t_fuzz::relay::{RelayWorld, Source};
use i3t_underlay::Ecn;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let Some((&[selector, hi, lo], data)) = input.split_first_chunk::<3>() else { return };
    if data.is_empty() {
        return;
    }
    let source = match selector & 0b11 {
        3 => Source::Stranger,
        member => Source::Member(usize::from(member)),
    };
    let ecn = match (selector >> 2) & 0b11 {
        0 => None,
        1 => Some(Ecn::Ect0),
        2 => Some(Ecn::Ect1),
        _ => Some(Ecn::Ce),
    };
    let stride = match usize::from(u16::from_be_bytes([hi, lo])) {
        0 => data.len(),
        stride => stride,
    };
    RelayWorld::get().deliver(source, ecn, data, stride);
});
