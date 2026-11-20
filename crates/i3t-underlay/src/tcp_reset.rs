//! 异常结束一条 TCP 连接：关闭时发 RST 而不是 FIN，并且让对端认这个 RST。以及设成这样的连接怎样正常结束（最后一节）。
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
//!
//! # 一开始就设成"关闭时发 RST"的连接怎样正常结束
//!
//! 搬运用户数据的连接一开始就设成"关闭时发 RST"（`SO_LINGER` 为 0），中途出错、任务被取消、进程退出时对端看到的
//! 都是重置；两个方向都正常结束了才改回正常关闭。macOS 上这一步可能来不及：一条 socket 两个方向都关了之后（本端发了
//! FIN，对端的 FIN 也到了），那里的 setsockopt 一律返回 EINVAL，"关闭时发 RST"再也改不回来。于是正常结束的连接照样以
//! RST 关闭，发送缓冲里还没送到的数据被丢掉：对端读到的是重置；对端也是 macOS 并且不认这个 RST 时，它一直等那些数据。
//! 所以这样的连接用 [`shutdown_write`] 发 FIN、用 [`graceful_close`] 改回正常关闭，别直接调 `shutdown` 和 `set_linger`。

use std::io;
use std::net::Shutdown;

use socket2::SockRef;
use tokio::net::TcpStream;

/// 一条 socket 两个方向都关了之后还能不能改 `SO_LINGER`。macOS 不能（见模块文档）。
const LINGER_SETTABLE_ONCE_SHUT: bool = !cfg!(target_os = "macos");

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

/// 正常结束写的一半：发 FIN，排在还没送出去的数据后面。连接设着"关闭时发 RST"、要等两个方向都结束才定怎么关时用它。
///
/// 对端的 FIN 什么时候到由不得本端，到了之后再发 FIN，macOS 上就改不回正常关闭了（模块文档）。所以 macOS 上发 FIN
/// 之前先改回正常关闭：这个方向到这里是完整的。之后整条连接要是异常结束，调用方照常再设"关闭时发 RST"（[`close`]
/// 就会设）；那时对端的 FIN 已经到了的话设不上，对端看到的是正常关闭，它要发的那时已经发完了，只是不知道有没有送到。
/// 别的系统上随时都能改，这里只发 FIN，"关闭时发 RST"留到 [`graceful_close`] 再改：中途被取消、进程退出时对端照样看到重置。
///
/// # Errors
///
/// `shutdown` 的错误：连接已经被重置、断开。
pub fn shutdown_write(stream: &TcpStream) -> io::Result<()> {
    let socket = SockRef::from(stream);
    if !LINGER_SETTABLE_ONCE_SHUT && let Err(error) = socket.set_linger(None) {
        tracing::debug!(%error, "发 FIN 之前改不回正常关闭，这条连接关闭时可能发 RST");
    }
    socket.shutdown(Shutdown::Write)
}

/// 两个方向都正常结束了：关闭时正常关闭，不再发 RST。写的一半要已经用 [`shutdown_write`] 关掉。
///
/// macOS 上 [`shutdown_write`] 已经改过了，这时两个方向都关了，再改只会得到 EINVAL，所以那里什么都不做。
///
/// # Errors
///
/// 设不上 `SO_LINGER`。这条连接关闭时会发 RST。
pub fn graceful_close(stream: &TcpStream) -> io::Result<()> {
    if LINGER_SETTABLE_ONCE_SHUT { SockRef::from(stream).set_linger(None) } else { Ok(()) }
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

    /// 一对连好的 TCP：(拨出的, 接进来的)。
    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (dialled, accepted) = tokio::join!(TcpStream::connect(listener.local_addr().unwrap()), listener.accept());
        (dialled.unwrap(), accepted.unwrap().0)
    }

    #[tokio::test]
    async fn linger_can_be_changed_once_both_halves_are_shut_exactly_where_the_code_assumes_it() {
        // 对端先发 FIN，本端读到结束之后也发 FIN：两个方向都关了。这时还改不改得了 SO_LINGER，要和
        // `LINGER_SETTABLE_ONCE_SHUT` 说的一样（macOS 改不了，见模块文档）。哪天系统变了，这一句会失败，提醒去改。
        let (mut near, mut far) = pair().await;
        near.shutdown().await.unwrap();
        assert_eq!(far.read(&mut [0; 1]).await.unwrap(), 0, "对端的 FIN 到了");
        SockRef::from(&far).shutdown(Shutdown::Write).unwrap();
        let changed = SockRef::from(&far).set_linger(None);
        assert_eq!(changed.is_ok(), LINGER_SETTABLE_ONCE_SHUT, "两个方向都关了之后改 SO_LINGER：{changed:?}");
        drop(near);
    }

    #[tokio::test]
    async fn a_connection_set_to_reset_that_ends_normally_delivers_its_queued_tail_and_a_clean_end() {
        // 对端先发完（它的 FIN 先到），本端再写一大块、发 FIN、正常关闭。对端的接收缓冲小、本端的发送缓冲大，关闭时
        // 这一块的大半还在本端的内核里。正常关闭时内核接着把它们发完；以 RST 关闭时它们被丢掉，对端读到的是重置或者
        // 一直等下去：macOS 上发完 FIN 才改回正常关闭时就是这样（模块文档）。
        let (mut reader, mut writer) = pair().await;
        SockRef::from(&reader).set_recv_buffer_size(16 * 1024).unwrap();
        SockRef::from(&writer).set_send_buffer_size(1024 * 1024).unwrap();
        writer.set_zero_linger().unwrap();
        reader.shutdown().await.unwrap();
        assert_eq!(writer.read(&mut [0; 1]).await.unwrap(), 0, "对端的 FIN 到了");
        // 发送缓冲在 Linux 上有上限（`net.core.wmem_max`，常见的是 208 KB，内核给两倍）：这一块要放得进去，不然写不完。
        let block: Vec<u8> = (0..192 * 1024u32).map(|i| u8::try_from(i % 251).unwrap()).collect();
        writer.write_all(&block).await.unwrap();
        shutdown_write(&writer).unwrap();
        graceful_close(&writer).unwrap();
        drop(writer);
        let mut received = Vec::new();
        let end = tokio::time::timeout(std::time::Duration::from_secs(5), reader.read_to_end(&mut received)).await;
        let end = end.expect("对端应该及时读到结束");
        assert!(end.is_ok(), "读到 {} 字节之后结束的方式是 {end:?}，应该是正常结束", received.len());
        assert!(received == block, "读到 {} 字节，应该是 {}", received.len(), block.len());
    }

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
