//! 从流上读一个头（OPEN 或 REPLY）。

use std::io;

use i3t_conn::{StreamAborted, StreamCode};
use i3t_proto::flow::{DecodeError, Decoded, Reason, Reply, Status};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::FlowError;

/// 读到头完整为止，一个字节也不多读：头后面紧跟着数据，多读了就得有人把它们还回去。
///
/// 每次只读解析器要的那么多字节，而解析器要的不会超过头的上限，所以对端乱写也不能让这里多分配内存。
pub(crate) async fn read<T>(
    recv: &mut (impl AsyncRead + Unpin),
    decode: fn(&[u8]) -> Result<Decoded<T>, DecodeError>,
) -> Result<T, FlowError> {
    let mut buf = Vec::new();
    loop {
        match decode(&buf)? {
            Decoded::Complete { value, .. } => return Ok(value),
            Decoded::Incomplete { more } => {
                let have = buf.len();
                buf.resize(have + more, 0);
                recv.read_exact(&mut buf[have..]).await.map_err(|error| match error.kind() {
                    io::ErrorKind::UnexpectedEof => FlowError::Truncated,
                    _ => FlowError::Read(error),
                })?;
            }
        }
    }
}

/// 读对端的 REPLY。
///
/// 接受方可以不读 OPEN 就带着 [`Reason::AccessRevoked`] 重置整条流：访问规则不允许本节点访问它（见 [`crate::Handler::screen`]）。
/// 这和它读了 OPEN 再回一个 [`Status::AccessDenied`] 是同一个结论，所以也当作这样一个 REPLY 交出去，
/// 发起方给客户端的失败和别的拒绝一样，不会因此多透露什么。
pub(crate) async fn read_reply(recv: &mut (impl AsyncRead + Unpin)) -> Result<Reply, FlowError> {
    match read(recv, Reply::decode).await {
        Err(FlowError::Read(error)) if refused_unread(&error) => {
            Ok(Reply::new(Status::AccessDenied, "对端没有读建流请求就重置了流：访问规则不允许本节点访问它"))
        }
        other => other,
    }
}

fn refused_unread(error: &io::Error) -> bool {
    StreamAborted::of(error) == Some(StreamAborted::Reset(StreamCode(Reason::AccessRevoked.to_wire())))
}

#[cfg(test)]
mod tests {
    use i3t_proto::flow::{Reply, Status};
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[tokio::test]
    async fn header_is_read_without_consuming_the_data_behind_it() {
        let (mut near, mut far) = tokio::io::duplex(1024);
        let mut bytes = Vec::new();
        Reply::new(Status::ConnectionRefused, "端口没开").encode(&mut bytes);
        bytes.extend_from_slice(b"data");
        far.write_all(&bytes).await.unwrap();

        let reply = read(&mut near, Reply::decode).await.unwrap();
        assert_eq!(reply, Reply::new(Status::ConnectionRefused, "端口没开"));
        let mut data = [0; 4];
        near.read_exact(&mut data).await.unwrap();
        assert_eq!(&data, b"data");
    }

    #[tokio::test]
    async fn clean_end_in_the_middle_of_a_header_is_truncation() {
        let (mut near, mut far) = tokio::io::duplex(1024);
        far.write_all(&[0x00]).await.unwrap();
        drop(far);
        assert!(matches!(read(&mut near, Reply::decode).await, Err(FlowError::Truncated)));
    }

    #[tokio::test]
    async fn malformed_header_is_reported_as_soon_as_it_is_seen() {
        let (mut near, mut far) = tokio::io::duplex(1024);
        far.write_all(&[0xff, 200]).await.unwrap();
        // 对端没有再写任何东西：错在前两个字节，不需要等那 200 字节的说明。
        assert!(matches!(read(&mut near, Reply::decode).await, Err(FlowError::Malformed(DecodeError::UnknownStatus(0xff)))));
    }
}
