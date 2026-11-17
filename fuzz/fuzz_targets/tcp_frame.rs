//! TCP 中转的分帧：relay 和节点从一条 TCP 上读到的任意字节流。
//!
//! 第一个字节决定把其余的字节切成多大的块一块一块交给解码器（模拟 TCP 每次读到的长短不一），
//! 结果必须和一次全部交进去一样；切出来的每一帧都不空，重新加上长度前缀就是读走的那一段字节。

#![no_main]

use bytes::{Bytes, BytesMut};
use i3t_proto::tcp_frame::{self, FrameDecoder};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&chunk, stream)) = data.split_first() else { return };
    let chunk = usize::from(chunk).max(1);

    let mut whole = FrameDecoder::new();
    whole.extend_from_slice(stream);
    let at_once = drain(&mut whole);

    let mut pieces = FrameDecoder::new();
    let mut in_pieces = Vec::new();
    for piece in stream.chunks(chunk) {
        pieces.extend_from_slice(piece);
        in_pieces.extend(drain(&mut pieces));
    }
    assert_eq!(in_pieces, at_once, "分块交进去和一次交进去切出来的帧不一样（块长 {chunk}）");
    assert_eq!(pieces.buffer_mut().len(), whole.buffer_mut().len(), "剩下的不完整的帧长短不一样");

    let mut rebuilt = BytesMut::new();
    for frame in &at_once {
        assert!(!frame.is_empty(), "长度为 0 的帧应该被跳过");
        tcp_frame::put_frame(&mut rebuilt, frame).expect("读出来的帧不会超过一帧的上限");
    }
    // 读走的字节里除了被跳过的空帧（两个 0 字节），就是这些帧加上它们的前缀。
    let consumed = &stream[..stream.len() - whole.buffer_mut().len()];
    let without_empty = strip_empty_frames(consumed);
    assert_eq!(rebuilt.as_ref(), without_empty.as_slice(), "帧加上长度前缀之后对不上读走的字节");
});

fn drain(decoder: &mut FrameDecoder) -> Vec<Bytes> {
    std::iter::from_fn(|| decoder.next_frame()).collect()
}

/// `stream` 是若干个完整的帧。去掉其中长度为 0 的帧。
fn strip_empty_frames(mut stream: &[u8]) -> Vec<u8> {
    let mut kept = Vec::new();
    while let Some((&[hi, lo], rest)) = stream.split_first_chunk::<2>() {
        let len = usize::from(u16::from_be_bytes([hi, lo]));
        if len > 0 {
            kept.extend_from_slice(&stream[..2 + len]);
        }
        stream = &rest[len..];
    }
    kept
}
