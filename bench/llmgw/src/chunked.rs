//! HTTP/1.1 的分块传输编码（RFC 9112 §7.1）：上游用它一段一段地写流式回答，负载端增量地解开。

use std::fmt::Write as _;

/// 最后一块：长度 0，没有尾部字段。
pub const LAST_CHUNK: &[u8] = b"0\r\n\r\n";

/// 把一块数据编码追加到 `out`。空的数据不编码：长度 0 的块表示结束。
pub fn encode(out: &mut Vec<u8>, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let mut size = String::with_capacity(8);
    write!(size, "{:x}\r\n", data.len()).expect("写 String 不会失败");
    out.extend_from_slice(size.as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkError {
    BadSize(String),
    MissingCrlf,
    LineTooLong,
    DataAfterEnd,
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSize(line) => write!(f, "块长度那一行不对：{line:?}"),
            Self::MissingCrlf => f.write_str("块的数据后面不是 CRLF"),
            Self::LineTooLong => f.write_str("块长度或者尾部字段的一行太长"),
            Self::DataAfterEnd => f.write_str("最后一块之后还有数据"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// 在读块长度那一行。
    Size,
    /// 块里还剩这么多字节的数据。
    Data(u64),
    /// 数据之后的 CRLF，已经读到了几个字节。
    DataEnd(u8),
    /// 最后一块之后的尾部字段，一行一行地读，读到空行结束。
    Trailer,
    Done,
}

/// 增量解码器：字节可以按任意边界分批喂进来。
#[derive(Debug)]
pub struct Decoder {
    state: State,
    line: Vec<u8>,
}

impl Decoder {
    /// 块长度、尾部字段的一行最长多少。正常的不过十几个字节。
    const MAX_LINE: usize = 4096;

    pub fn new() -> Self {
        Self { state: State::Size, line: Vec::new() }
    }

    /// 读到了最后一块和它后面的空行。
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// 喂一批字节，解出来的数据追加到 `out`。
    ///
    /// # Errors
    ///
    /// 编码不对，或者最后一块之后还有数据。
    pub fn feed(&mut self, mut input: &[u8], out: &mut Vec<u8>) -> Result<(), ChunkError> {
        while !input.is_empty() {
            match self.state {
                State::Size | State::Trailer => {
                    let Some(line) = self.take_line(&mut input)? else { break };
                    self.state = match self.state {
                        State::Size => match Self::parse_size(&line)? {
                            0 => State::Trailer,
                            size => State::Data(size),
                        },
                        _ if line.is_empty() => State::Done,
                        other => other,
                    };
                }
                State::Data(remaining) => {
                    let take = usize::try_from(remaining).unwrap_or(usize::MAX).min(input.len());
                    out.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    let left = remaining - u64::try_from(take).expect("usize 放得进 u64");
                    self.state = if left == 0 { State::DataEnd(0) } else { State::Data(left) };
                }
                State::DataEnd(seen) => {
                    let expected = if seen == 0 { b'\r' } else { b'\n' };
                    if input[0] != expected {
                        return Err(ChunkError::MissingCrlf);
                    }
                    input = &input[1..];
                    self.state = if seen == 0 { State::DataEnd(1) } else { State::Size };
                }
                State::Done => return Err(ChunkError::DataAfterEnd),
            }
        }
        Ok(())
    }

    /// 从 `input` 里取一整行（不含 CRLF）；这一批里没有行尾时攒起来，返回 `None`。
    fn take_line(&mut self, input: &mut &[u8]) -> Result<Option<Vec<u8>>, ChunkError> {
        match input.iter().position(|&byte| byte == b'\n') {
            Some(at) => {
                self.line.extend_from_slice(&input[..at]);
                *input = &input[at + 1..];
                let mut line = std::mem::take(&mut self.line);
                if line.pop_if(|last| *last == b'\r').is_none() {
                    return Err(ChunkError::MissingCrlf);
                }
                Ok(Some(line))
            }
            None => {
                self.line.extend_from_slice(input);
                *input = &[];
                if self.line.len() > Self::MAX_LINE {
                    return Err(ChunkError::LineTooLong);
                }
                Ok(None)
            }
        }
    }

    fn parse_size(line: &[u8]) -> Result<u64, ChunkError> {
        let text = String::from_utf8_lossy(line);
        // 分号后面是块扩展，按规范忽略。
        let digits = text.split(';').next().unwrap_or_default().trim();
        u64::from_str_radix(digits, 16).map_err(|_| ChunkError::BadSize(text.into_owned()))
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_in_pieces(bytes: &[u8], piece: usize) -> (Vec<u8>, bool) {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        for chunk in bytes.chunks(piece) {
            decoder.feed(chunk, &mut out).unwrap();
        }
        (out, decoder.is_done())
    }

    #[test]
    fn encoded_chunks_decode_back_whatever_the_split() {
        let mut wire = Vec::new();
        encode(&mut wire, b"hello ");
        encode(&mut wire, b"");
        encode(&mut wire, &[b'x'; 300]);
        wire.extend_from_slice(LAST_CHUNK);
        let mut expected = b"hello ".to_vec();
        expected.extend_from_slice(&[b'x'; 300]);
        for piece in [1, 2, 5, 17, 10_000] {
            assert_eq!(decode_in_pieces(&wire, piece), (expected.clone(), true), "按 {piece} 字节一批喂");
        }
    }

    #[test]
    fn extensions_and_trailers_are_skipped() {
        let wire = b"4;name=value\r\nabcd\r\n0\r\nExpires: never\r\n\r\n";
        assert_eq!(decode_in_pieces(wire, 3), (b"abcd".to_vec(), true));
    }

    #[test]
    fn a_stream_cut_inside_a_chunk_is_not_done() {
        let wire = b"a\r\n01234";
        assert_eq!(decode_in_pieces(wire, 2), (b"01234".to_vec(), false));
    }

    #[test]
    fn malformed_input_is_an_error() {
        let mut out = Vec::new();
        assert!(matches!(Decoder::new().feed(b"zz\r\n", &mut out), Err(ChunkError::BadSize(_))));
        assert_eq!(Decoder::new().feed(b"2\r\nabX", &mut out), Err(ChunkError::MissingCrlf));
        assert_eq!(Decoder::new().feed(b"2\nab\r\n", &mut out), Err(ChunkError::MissingCrlf));
        let mut done = Decoder::new();
        done.feed(LAST_CHUNK, &mut out).unwrap();
        assert_eq!(done.feed(b"x", &mut out), Err(ChunkError::DataAfterEnd));
        assert_eq!(Decoder::new().feed(&[b'1'; 5000], &mut out), Err(ChunkError::LineTooLong));
    }
}
