//! 明文 HTTP 代理转发的请求体在连接上到哪里为止。
//!
//! 请求头随 OPEN 发出之后，入口只把属于这个请求的请求体搬给目标（[`i3t_flow::Flow::relay_framed`]），
//! 客户端在它后面发的东西（流水线里的下一个请求）不会到达目标，里面的 `Proxy-Authorization` 也就不会。
//! 所以边界必须算得和目标算的一模一样，任何含糊的写法都当作坏请求：块大小只能是十六进制数字（最多 16 位，
//! 不能有前导的空白、`+`、`0x`），行一律以 CRLF 结尾，块数据后面必须紧跟 CRLF；块大小那一行和尾部字段都有长度上限。

use i3t_flow::Framing;
use i3t_proto::flow::Reason;

use super::MAX_HEAD_LEN;

/// 块大小那一行（连同块扩展）最长多少字节。
const MAX_CHUNK_LINE: usize = 4096;
/// 块大小最多几位十六进制数字：再多就超过 64 位了。
const MAX_CHUNK_DIGITS: u8 = 16;

/// 请求体怎么划界：请求头里的 `Content-Length` 或者 `Transfer-Encoding: chunked`，都没有就是没有请求体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Body {
    Empty,
    Length(u64),
    Chunked,
}

impl Body {
    /// 从请求体的第一个字节开始划界。
    pub(crate) fn framing(self) -> BodyFraming {
        let state = match self {
            Self::Empty | Self::Length(0) => State::Done,
            Self::Length(left) => State::Length { left },
            Self::Chunked => State::Size(SizeLine::default()),
        };
        BodyFraming { state }
    }
}

/// 一个请求体读到了哪里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyFraming {
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Length {
        left: u64,
    },
    /// 在读块大小那一行。
    Size(SizeLine),
    Data {
        left: u64,
    },
    /// 块数据之后的 CRLF；`cr` 是已经读到了 CR。
    DataEnd {
        cr: bool,
    },
    /// 在读尾部字段。
    Trailer(TrailerLines),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct SizeLine {
    size: u64,
    digits: u8,
    /// 数字后面已经出现了空白或者 `;`：之后不能再有数字。
    after_digits: bool,
    /// 在块扩展里（`;` 之后），到行尾之前什么都可以有。
    extension: bool,
    cr: bool,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TrailerLines {
    /// 这一行读到的字节数（不含 CRLF）。
    line: usize,
    cr: bool,
    total: usize,
}

impl Framing for BodyFraming {
    fn take(&mut self, bytes: &[u8]) -> Result<usize, Reason> {
        let mut used = 0;
        while used < bytes.len() && self.state != State::Done {
            let rest = &bytes[used..];
            used += match &mut self.state {
                State::Length { left } | State::Data { left } => {
                    // 剩下的超过 usize 时一定比 `rest.len()` 多。
                    let taken = usize::try_from(*left).map_or(rest.len(), |left| left.min(rest.len()));
                    // usize 不会比 u64 宽，这里不会截断。
                    *left -= taken as u64;
                    if *left == 0 {
                        self.state = match self.state {
                            State::Data { .. } => State::DataEnd { cr: false },
                            _ => State::Done,
                        };
                    }
                    taken
                }
                State::Size(line) => {
                    self.state = line.next(rest[0])?;
                    1
                }
                State::DataEnd { cr } => {
                    self.state = match (*cr, rest[0]) {
                        (false, b'\r') => State::DataEnd { cr: true },
                        (true, b'\n') => State::Size(SizeLine::default()),
                        _ => return Err(Reason::ProtocolError),
                    };
                    1
                }
                State::Trailer(lines) => {
                    self.state = lines.next(rest[0])?;
                    1
                }
                State::Done => 0,
            };
        }
        Ok(used)
    }

    fn is_complete(&self) -> bool {
        self.state == State::Done
    }
}

impl SizeLine {
    fn next(mut self, byte: u8) -> Result<State, Reason> {
        self.len += 1;
        if self.len > MAX_CHUNK_LINE {
            return Err(Reason::ProtocolError);
        }
        if self.cr {
            return match byte {
                b'\n' if self.size == 0 => Ok(State::Trailer(TrailerLines::default())),
                b'\n' => Ok(State::Data { left: self.size }),
                _ => Err(Reason::ProtocolError),
            };
        }
        match byte {
            b'\r' if self.digits > 0 => self.cr = true,
            b'\r' | b'\n' | 0 => return Err(Reason::ProtocolError),
            _ if self.extension => {}
            b';' if self.digits > 0 => (self.after_digits, self.extension) = (true, true),
            b' ' | b'\t' if self.digits > 0 => self.after_digits = true,
            _ if self.after_digits => return Err(Reason::ProtocolError),
            _ => {
                let digit = char::from(byte).to_digit(16).ok_or(Reason::ProtocolError)?;
                if self.digits == MAX_CHUNK_DIGITS {
                    return Err(Reason::ProtocolError);
                }
                self.size = self.size << 4 | u64::from(digit);
                self.digits += 1;
            }
        }
        Ok(State::Size(self))
    }
}

impl TrailerLines {
    fn next(mut self, byte: u8) -> Result<State, Reason> {
        self.total += 1;
        if self.total > MAX_HEAD_LEN {
            return Err(Reason::ProtocolError);
        }
        match (self.cr, byte) {
            (true, b'\n') if self.line == 0 => return Ok(State::Done),
            (true, b'\n') => (self.line, self.cr) = (0, false),
            (false, b'\r') => self.cr = true,
            (true, _) | (false, b'\n' | 0) => return Err(Reason::ProtocolError),
            (false, _) => self.line += 1,
        }
        Ok(State::Trailer(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一次喂进去，返回属于请求体的字节数和是不是完整了。
    fn feed(body: Body, bytes: &[u8]) -> Result<(usize, bool), Reason> {
        let mut framing = body.framing();
        let used = framing.take(bytes)?;
        Ok((used, framing.is_complete()))
    }

    /// 一个字节一个字节地喂，结果要和一次喂进去一样。
    fn feed_bytewise(body: Body, bytes: &[u8]) -> Result<(usize, bool), Reason> {
        let mut framing = body.framing();
        let mut used = 0;
        for byte in bytes {
            used += framing.take(std::slice::from_ref(byte))?;
        }
        Ok((used, framing.is_complete()))
    }

    #[test]
    fn a_request_without_a_body_is_complete_before_any_byte() {
        assert!(Body::Empty.framing().is_complete());
        assert!(Body::Length(0).framing().is_complete());
        assert_eq!(feed(Body::Empty, b"GET /next HTTP/1.1\r\n"), Ok((0, true)));
    }

    #[test]
    fn a_content_length_body_ends_after_exactly_that_many_bytes() {
        assert_eq!(feed(Body::Length(5), b"helloGET /next"), Ok((5, true)));
        assert_eq!(feed(Body::Length(5), b"hel"), Ok((3, false)));
        assert_eq!(feed_bytewise(Body::Length(5), b"hello!"), Ok((5, true)));
    }

    #[test]
    fn a_chunked_body_ends_after_the_last_chunk_and_the_trailers() {
        let body = b"5\r\nhello\r\n1a;name=value\r\nabcdefghijklmnopqrstuvwxyz\r\n0\r\nX-Sum: 1\r\n\r\n";
        let next = b"GET /next HTTP/1.1\r\n\r\n";
        let bytes = [&body[..], next].concat();
        assert_eq!(feed(Body::Chunked, &bytes), Ok((body.len(), true)));
        assert_eq!(feed_bytewise(Body::Chunked, &bytes), Ok((body.len(), true)));
        assert_eq!(feed(Body::Chunked, b"0\r\n\r\n"), Ok((5, true)));
        assert_eq!(feed(Body::Chunked, b"A \t;ext\r\n0123456789\r\n0\r\n\r\n"), Ok((26, true)), "块大小后面可以有空白再接扩展");
        assert_eq!(feed(Body::Chunked, b"5\r\nhel"), Ok((6, false)));
    }

    #[test]
    fn ambiguous_chunked_bodies_are_rejected() {
        let cases: [&[u8]; 14] = [
            b"0x5\r\nhello\r\n0\r\n\r\n",
            b"+5\r\nhello\r\n0\r\n\r\n",
            b" 5\r\nhello\r\n0\r\n\r\n",
            b"5 5\r\nhello\r\n0\r\n\r\n",
            b"\r\n",
            b"5\nhello\r\n0\r\n\r\n",
            b"5\r\nhello\n0\r\n\r\n",
            b"5\r\nhelloXX0\r\n\r\n",
            b"5\r\rhello\r\n0\r\n\r\n",
            b"5;ext\nhello\r\n0\r\n\r\n",
            b"0\r\nX: a\nY: b\r\n\r\n",
            b"0\r\nX: a\0\r\n\r\n",
            b"10000000000000000\r\n",
            b"g\r\n",
        ];
        for bytes in cases {
            assert_eq!(feed(Body::Chunked, bytes), Err(Reason::ProtocolError), "{:?}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn the_largest_chunk_size_fits_and_a_longer_size_line_or_trailer_does_not() {
        let mut framing = Body::Chunked.framing();
        assert_eq!(framing.take(b"ffffffffffffffff\r\n"), Ok(18));
        assert_eq!(framing.state, State::Data { left: u64::MAX });

        let long_extension = [&b"1;"[..], &vec![b'x'; MAX_CHUNK_LINE]].concat();
        assert_eq!(feed(Body::Chunked, &long_extension), Err(Reason::ProtocolError));
        let long_trailer = [&b"0\r\nX: "[..], &vec![b'x'; MAX_HEAD_LEN]].concat();
        assert_eq!(feed(Body::Chunked, &long_trailer), Err(Reason::ProtocolError));
    }
}
