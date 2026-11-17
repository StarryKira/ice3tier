//! 数据报上下文里的一帧（对端发来的数据报的头、分片头）、测量流结束时对端写回的摘要，以及发送一侧的分片。
//!
//! 除了不能 panic，还查：头重新编码接上后面的字节就是原来的帧，`Recipient::peek` 和完整的解析说的一样；
//! 用输入的后半段当内容切片，每一片都不超过要求的长度，按片序号拼回来就是原来的内容。

#![no_main]

use i3t_proto::flow::{DatagramHeader, Frames, MeasureSteady, MeasureSummary, Recipient};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match DatagramHeader::decode(data) {
        Ok((header, rest)) => {
            let mut encoded = Vec::new();
            header.encode(&mut encoded);
            encoded.extend_from_slice(rest);
            assert_eq!(encoded, data, "数据报的头重新编码之后对不上原来的帧");
            assert_eq!(Recipient::peek(data), Some(header.recipient), "只看首字节的判断和完整的解析不一样");
        }
        Err(_) => {
            let _ = Recipient::peek(data);
        }
    }

    if let Some(summary) = data.first_chunk::<{ MeasureSummary::LEN }>().and_then(MeasureSummary::from_bytes) {
        let mut encoded = Vec::new();
        summary.encode(&mut encoded);
        assert_eq!(encoded, data[..MeasureSummary::LEN], "测量的摘要重新编码之后对不上");
    }
    if let Some(steady) = data.first_chunk::<{ MeasureSteady::LEN }>() {
        assert_eq!(MeasureSteady::from_bytes(steady).to_bytes(), *steady);
    }

    split_and_join(data);
});

/// 前 2 字节是每帧的上限，再 2 字节是报文号，剩下的是内容。
fn split_and_join(data: &[u8]) {
    let Some((&[hi, lo, m0, m1], body)) = data.split_first_chunk::<4>() else { return };
    let max_len = usize::from(u16::from_be_bytes([hi, lo]));
    let recipient = if m1 & 1 == 0 { Recipient::Acceptor } else { Recipient::Initiator };
    let Ok(frames) = Frames::split(recipient, 0x0102_0304, u16::from_be_bytes([m0, m1]), body, max_len) else { return };
    let (mut joined, mut frames_seen, mut announced) = (Vec::new(), 0, None);
    for (index, frame) in frames.enumerate() {
        assert!(frame.len() <= max_len, "第 {index} 帧有 {} 字节，上限是 {max_len}", frame.len());
        let (header, piece) = DatagramHeader::decode(&frame).expect("自己切出来的帧一定解析得了");
        assert_eq!((header.recipient, header.context), (recipient, 0x0102_0304));
        match header.fragment {
            None => assert_eq!(index, 0, "不分片的报文只有一帧"),
            Some(fragment) => {
                let count = *announced.get_or_insert(fragment.count);
                assert_eq!((fragment.count, usize::from(fragment.index)), (count, index), "片数或者片序号不对");
            }
        }
        joined.extend_from_slice(piece);
        frames_seen += 1;
    }
    if let Some(count) = announced {
        assert_eq!(frames_seen, usize::from(count), "切出来的片数和头里写的不一样");
    }
    assert_eq!(joined, body, "拼回来的内容和原来的不一样");
}
