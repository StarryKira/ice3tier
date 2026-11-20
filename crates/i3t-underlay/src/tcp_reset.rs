//! 异常结束一条 TCP 连接：关闭时发 RST 而不是 FIN，并且让对端认这个 RST。
//!
//! RST 的序号是本端下一个要发的字节。对端只认序号正好对得上的 RST（RFC 5961），对不上的不理，回一个"挑战 ACK"。
//! Linux 拿它和自己下一个要收的字节比，本端发出去的它都收到了就对得上。macOS 拿它和自己最后一次确认到的位置比：
//! 对端刚收到数据、还欠着一个延迟确认（最多一两百毫秒）时就对不上，RST 被丢掉。挑战 ACK 回到本端：Linux 上连接已经没了，
//! 回一个 RST，这一个对端认；macOS 上刚关掉的连接还在表里，挑战 ACK 被悄悄丢掉。所以本端和对端都是 macOS 时
//! （本机的代理和本机的程序就是这样），刚发完数据就被重置的连接，对端不知道它被重置了，一直等下去。
//!
//! 所以 macOS 上关闭之前先等对端确认完本端发出去的所有数据（发送缓冲空了：`SO_NWRITE` 是 0），这时它最后一次确认到的
//! 位置就是 RST 的序号。最多等 [`ACK_WAIT`]，等不到（对端不读了、断了）也照样关。等的时候连接在后台留着，调用方不用等。
//! 别的系统上马上关。

use tokio::net::TcpStream;

/// 最多等对端确认多久。macOS 的延迟确认最多一两百毫秒，再加上一个往返。
#[cfg(target_os = "macos")]
const ACK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// 等的时候隔多久看一次发送缓冲。
#[cfg(target_os = "macos")]
const POLL: std::time::Duration = std::time::Duration::from_millis(10);

/// 关掉 `stream`，对端看到的是重置（RST），不是正常结束。见模块文档。
///
/// 不用等：macOS 上要等对端确认时，连接交给一个后台任务，它在 tokio 运行时里关掉连接；不在运行时里时马上关。
pub fn close(stream: TcpStream) {
    if let Err(error) = stream.set_zero_linger() {
        tracing::debug!(%error, "设置不了 SO_LINGER，这条连接关闭时对端可能看到正常结束");
    }
    #[cfg(target_os = "macos")]
    if unacknowledged(&stream).is_ok_and(|bytes| bytes > 0)
        && let Ok(runtime) = tokio::runtime::Handle::try_current()
    {
        drop(runtime.spawn(async move {
            let deadline = tokio::time::Instant::now() + ACK_WAIT;
            while tokio::time::Instant::now() < deadline && unacknowledged(&stream).is_ok_and(|bytes| bytes > 0) {
                tokio::time::sleep(POLL).await;
            }
            drop(stream);
        }));
        return;
    }
    drop(stream);
}

/// 发送缓冲里还有多少字节：还没发出去的，加上发出去了、对端还没确认的。
#[cfg(target_os = "macos")]
fn unacknowledged(stream: &TcpStream) -> std::io::Result<usize> {
    use std::os::fd::AsRawFd;

    let mut value: libc::c_int = 0;
    let mut len = libc::socklen_t::try_from(size_of::<libc::c_int>()).expect("一个 int 的长度装得进 socklen_t");
    // SAFETY: `stream` 借着，文件描述符在调用期间活着；指针指向栈上的一个 `c_int`，长度就是它的长度，内核最多写这么多，
    // 调用结束之后不再用这个指针。
    let result = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NWRITE, (&raw mut value).cast(), &raw mut len) };
    if result == 0 { Ok(usize::try_from(value).unwrap_or(0)) } else { Err(std::io::Error::last_os_error()) }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn the_peer_reading_right_after_the_data_sees_a_reset() {
        // 对端一直在读：每一段数据刚到，它都可能还欠着确认。读完数据之后读到的必须是重置，而且要及时（macOS 上见模块文档）。
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (dialled, accepted) = tokio::join!(TcpStream::connect(listener.local_addr().unwrap()), listener.accept());
        let (mut reader, (mut writer, _)) = (dialled.unwrap(), accepted.unwrap());
        let block = vec![7u8; 100_000];
        let read = tokio::spawn(async move {
            let mut received = Vec::new();
            let end = tokio::time::timeout(std::time::Duration::from_secs(5), reader.read_to_end(&mut received)).await;
            (received.len(), end.map(|end| end.map_err(|error| error.kind())))
        });
        for chunk in block.chunks(1000) {
            writer.write_all(chunk).await.unwrap();
        }
        close(writer);
        let (received, end) = read.await.unwrap();
        let end = end.expect("对端应该及时知道连接结束了");
        // Windows 上收到 RST 时报的可能是 ConnectionAborted。
        assert!(
            matches!(end, Err(std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted)),
            "读到 {received} 字节之后结束的方式是 {end:?}，应该是重置"
        );
    }
}
