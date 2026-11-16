//! 节点协议的分帧：4 字节大端长度，后面是一个 JSON 对象。

use std::io;

use i3t_model::control::MAX_FRAME_LEN;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    #[error("连接读写失败")]
    Io(#[from] io::Error),
    #[error("一帧有 {len} 字节，超过上限 {max}")]
    TooLong { len: usize, max: usize },
    /// 帧是完整的，只是内容解析不了。连接还可以接着用。
    #[error("帧的内容不是期望的 JSON")]
    Json(#[from] serde_json::Error),
}

/// 读一帧。对端在两帧之间正常关闭连接时返回 `None`。
///
/// `max_len` 是这一帧最长多少字节：还没认出对方是谁时用小的上限，免得随便一个连接就让 controller 分配一大块内存。
///
/// # Errors
///
/// 读失败、长度超过上限，或者内容解析不了。
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(reader: &mut R, max_len: usize) -> Result<Option<T>, FrameError> {
    let len = match reader.read_u32().await {
        // 放不进 usize 的长度一定超过了上限。
        Ok(len) => usize::try_from(len).unwrap_or(usize::MAX),
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let max = max_len.min(MAX_FRAME_LEN);
    if len > max {
        return Err(FrameError::TooLong { len, max });
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

/// 写一帧并刷出去。
///
/// # Errors
///
/// 消息超过 [`MAX_FRAME_LEN`]，或者写失败。
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(writer: &mut W, message: &T) -> Result<(), FrameError> {
    let body = serde_json::to_vec(message)?;
    let too_long = || FrameError::TooLong { len: body.len(), max: MAX_FRAME_LEN };
    let len = u32::try_from(body.len()).ok().filter(|_| body.len() <= MAX_FRAME_LEN).ok_or_else(too_long)?;
    let mut frame = Vec::with_capacity(size_of::<u32>() + body.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&body);
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use i3t_model::control::ToController;

    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_a_clean_close_reads_as_none() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &ToController::Ack { version: 7 }).await.unwrap();
        write_frame(&mut wire, &ToController::Heartbeat).await.unwrap();
        assert_eq!(&wire[..4], [0, 0, 0, 26]);
        let mut reader = wire.as_slice();
        assert_eq!(read_frame::<_, ToController>(&mut reader, 1024).await.unwrap(), Some(ToController::Ack { version: 7 }));
        assert_eq!(read_frame::<_, ToController>(&mut reader, 1024).await.unwrap(), Some(ToController::Heartbeat));
        assert_eq!(read_frame::<_, ToController>(&mut reader, 1024).await.unwrap(), None);
    }

    #[tokio::test]
    async fn oversized_truncated_and_malformed_frames_are_told_apart() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &ToController::Ack { version: 7 }).await.unwrap();
        assert!(matches!(read_frame::<_, ToController>(&mut wire.as_slice(), 8).await, Err(FrameError::TooLong { len: 26, max: 8 })));
        assert!(matches!(read_frame::<_, ToController>(&mut &wire[..10], 1024).await, Err(FrameError::Io(_))));

        let mut unknown = Vec::new();
        write_frame(&mut unknown, &serde_json::json!({ "type": "added_in_a_newer_version" })).await.unwrap();
        write_frame(&mut unknown, &ToController::Heartbeat).await.unwrap();
        let mut reader = unknown.as_slice();
        assert!(matches!(read_frame::<_, ToController>(&mut reader, 1024).await, Err(FrameError::Json(_))));
        assert_eq!(
            read_frame::<_, ToController>(&mut reader, 1024).await.unwrap(),
            Some(ToController::Heartbeat),
            "解析不了的一帧不影响下一帧"
        );
    }
}
