//! 流上的第一样东西：OPEN（流、数据报上下文、测量流）、REPLY，以及 OPEN 和每个数据报里共用的目标。
//! 接受流的一方读的是对端写的任意字节，发起流的一方读的是对端写的 REPLY。
//!
//! 除了不能 panic，还查模块文档里写的读法：从空的缓冲区开始，每次照 [`Decoded::Incomplete`] 说的字节数去读，
//!
//! - 要的字节数不是 0，加上已经读到的也不超过头的上限（对端乱写不能让读的一方多分配内存）；
//! - 头读完时正好读到头的末尾，一个数据字节都没有多读；
//! - 结果和一次拿到全部字节时解析的一样；
//! - 解析出来的值重新编码，就是原来的那些字节。

#![no_main]

use std::fmt::Debug;

use i3t_proto::flow::{DecodeError, Decoded, Open, OpenDatagrams, Opening, Reply, Target};
use libfuzzer_sys::fuzz_target;

/// 一种头：怎么解析、怎么编码、最长多少字节。
struct Header<T> {
    name: &'static str,
    max_len: usize,
    decode: fn(&[u8]) -> Result<Decoded<T>, DecodeError>,
    encode: fn(&T, &mut Vec<u8>),
}

fuzz_target!(|data: &[u8]| {
    check(data, &Header { name: "Opening", max_len: Opening::MAX_LEN, decode: Opening::decode, encode: encode_opening });
    check(data, &Header { name: "Open", max_len: Open::MAX_LEN, decode: Open::decode, encode: Open::encode });
    check(
        data,
        &Header { name: "OpenDatagrams", max_len: OpenDatagrams::MAX_LEN, decode: OpenDatagrams::decode, encode: OpenDatagrams::encode },
    );
    check(data, &Header { name: "Reply", max_len: Reply::MAX_LEN, decode: Reply::decode, encode: Reply::encode });
    check(data, &Header { name: "Target", max_len: Target::MAX_LEN, decode: Target::decode, encode: Target::encode });
});

fn encode_opening(opening: &Opening, buf: &mut Vec<u8>) {
    match opening {
        Opening::Stream(open) => open.encode(buf),
        Opening::Datagrams(open) => open.encode(buf),
        Opening::Measure(open) => open.encode(buf),
        Opening::Ping(open) => open.encode(buf),
    }
}

fn check<T: PartialEq + Debug>(data: &[u8], header: &Header<T>) {
    let name = header.name;
    let whole = (header.decode)(data);
    match &whole {
        Ok(Decoded::Complete { value, len }) => {
            assert!(
                *len <= data.len() && *len <= header.max_len,
                "{name}：头占 {len} 字节，缓冲区 {}，上限 {}",
                data.len(),
                header.max_len
            );
            let mut encoded = Vec::new();
            (header.encode)(value, &mut encoded);
            assert_eq!(encoded, data[..*len], "{name}：解析出来的 {value:?} 重新编码之后不是原来的字节");
        }
        Ok(Decoded::Incomplete { more }) => {
            assert!(*more > 0, "{name}：不完整却说不用再读");
            assert!(
                data.len() + more <= header.max_len,
                "{name}：已经有 {} 字节，还要 {more} 字节，超过了上限 {}",
                data.len(),
                header.max_len
            );
        }
        Err(_) => {}
    }

    // 照着 `more` 一点一点读，像从流里读那样。
    let mut read = 0;
    let stepped = loop {
        let step = (header.decode)(&data[..read]);
        match step {
            Ok(Decoded::Incomplete { more }) if read < data.len() => {
                assert!(more > 0, "{name}：读到 {read} 字节时不完整，却说不用再读");
                read = (read + more).min(data.len());
            }
            other => break other,
        }
    };
    if let Ok(Decoded::Complete { len, .. }) = &stepped {
        assert_eq!(*len, read, "{name}：照 more 读到 {read} 字节时头完整了，却说头只占 {len} 字节");
    }
    assert_eq!(stepped, whole, "{name}：一点一点读和一次读完的结果不一样（照 more 读到 {read} 字节）");
}
