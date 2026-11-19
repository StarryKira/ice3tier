//! 虚拟网卡的名字服务（[`Names::answer`]）：本机任何程序发到节点虚拟 IP 53 端口的 UDP 报文。
//!
//! 除了不能 panic，还查：有回答时编号照抄，标着"是应答"，并且不比请求长出一条 A 记录以上（拿它放大不了流量）。

#![no_main]

use std::net::Ipv4Addr;
use std::sync::OnceLock;

use i3t_vnet::names::Names;
use libfuzzer_sys::fuzz_target;

/// 一条 A 记录在应答里占的字节：名字的指针 2、类型 2、类 2、TTL 4、长度 2、地址 4。
const A_RECORD_LEN: usize = 16;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| {
        Names::new("home.i3t", [("nas".to_owned(), Ipv4Addr::new(10, 66, 0, 5)), ("Laptop".to_owned(), Ipv4Addr::new(10, 66, 0, 1))])
    })
}

fuzz_target!(|query: &[u8]| {
    let Some(answer) = names().answer(query) else { return };
    assert!(answer.len() >= 12, "应答至少有一个头");
    assert_eq!(answer[..2], query[..2], "应答的编号要照抄请求的");
    assert!(answer[2] & 0x80 != 0, "应答要标着是应答");
    assert!(answer.len() <= query.len() + A_RECORD_LEN, "应答 {} 字节，请求只有 {} 字节", answer.len(), query.len());
});
