//! 就地出站：不经过任何对端，从本机直接连目标。

use std::future::Future;
use std::io;
use std::net::SocketAddr;

use i3t_proto::flow::{Host, Reason, Reply, Status};
use socket2::SockRef;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{ReadHalf, WriteHalf};

use crate::usage::IngressMeter;

/// 就地出站时目标受什么约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressPolicy {
    /// 回环监听器：调用者就是本机的程序，它本来就能自己去连，所以不加限制。
    Unrestricted,
    /// 非回环监听器：调用者是别人，本机这时就是在当他的出口，套用出口的出站策略（端口白名单、拒绝内网地址等）。
    Exit,
}

/// 就地出站的拨号器：解析域名、按策略检查、连接目标。
///
/// 入口自己不做这些事，它们是出口模块的本职。装配的时候把出口模块的实现交进来，这样两条路用的是同一份出站策略和同一个解析缓存。
pub trait LocalEgress: Send + Sync + 'static {
    /// 连到 `host:port`。
    ///
    /// 失败时返回的 [`Reply`] 和对端节点拒绝建流时给的是同一套状态，入口按同样的规则把它告诉客户端。
    fn connect(&self, host: &Host, port: u16, policy: EgressPolicy) -> impl Future<Output = Result<TcpStream, Reply>> + Send;

    /// 就地出站的 UDP：发往 `host:port` 的报文该发到哪个地址。解析域名、按策略检查，和 [`connect`](Self::connect) 是同一套规则。
    ///
    /// 入口自己拿着发报文的端口，每个报文发之前来问一次；实现应该让重复的询问很便宜（解析结果要缓存）。
    /// 不支持 UDP 的实现不用写它：默认一律拒绝。
    fn udp_destination(&self, host: &Host, port: u16, policy: EgressPolicy) -> impl Future<Output = Result<SocketAddr, Reply>> + Send {
        let _ = (host, port, policy);
        async { Err(Reply::new(Status::NotEnabled, "就地出站不支持 UDP")) }
    }
}

/// 两个方向各搬了多少字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Copied {
    pub from_client: u64,
    pub from_target: u64,
}

/// 每个方向一块这么大的缓冲区。
const BUFFER_LEN: usize = 16 * 1024;

/// 在客户端和目标这两条 TCP 连接之间双向搬运，直到两个方向都结束，或者 `cancel` 完成。
///
/// 半关闭照常传递。和经过流的搬运一样，任何一头异常结束（包括被取消）时另一头看到的是重置而不是正常结束：
/// 两条连接一开始就设成"关闭时发 RST"，两个方向都正常结束了才改回去。每一段数据都记到 `meter` 上。
pub(crate) async fn relay(
    mut client: TcpStream,
    mut target: TcpStream,
    meter: &IngressMeter<'_>,
    cancel: impl Future<Output = Reason>,
) -> io::Result<Copied> {
    client.set_zero_linger()?;
    target.set_zero_linger()?;
    let (mut from_client, mut from_target) = (0, 0);
    {
        let (client_read, client_write) = client.split();
        let (target_read, target_write) = target.split();
        let upward = copy(client_read, target_write, meter, true, &mut from_client);
        let downward = copy(target_read, client_write, meter, false, &mut from_target);
        tokio::select! {
            copied = async { tokio::try_join!(upward, downward) } => { copied?; }
            reason = cancel => return Err(io::Error::other(format!("搬运被叫停：{reason:?}"))),
        }
    }
    SockRef::from(&client).set_linger(None)?;
    SockRef::from(&target).set_linger(None)?;
    Ok(Copied { from_client, from_target })
}

/// 一个方向：读到 EOF 就把另一头的写关掉。
async fn copy(mut from: ReadHalf<'_>, mut to: WriteHalf<'_>, meter: &IngressMeter<'_>, upward: bool, count: &mut u64) -> io::Result<()> {
    let mut buf = vec![0; BUFFER_LEN];
    loop {
        let len = from.read(&mut buf).await?;
        if len == 0 {
            return to.shutdown().await;
        }
        meter.record(upward, len);
        to.write_all(&buf[..len]).await?;
        // usize 不会比 u64 宽，这里不会截断。
        *count += len as u64;
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;
    use crate::usage::Tally;

    /// 一对连好的 TCP 连接：`near` 是测试扮演的那一头，`far` 交给被测的搬运。
    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (far, _) = listener.accept().await.unwrap();
        (near, far)
    }

    #[tokio::test]
    async fn half_close_is_passed_on_and_bytes_are_counted() {
        let (mut client, client_far) = pair().await;
        let (mut target, target_far) = pair().await;
        let listen = std::sync::Arc::new(Tally::default());
        let counted = std::sync::Arc::clone(&listen);
        let relay = tokio::spawn(async move {
            let meter = IngressMeter { listen: &counted, credential: None, exit: None };
            relay(client_far, target_far, &meter, std::future::pending()).await
        });

        client.write_all(b"request").await.unwrap();
        client.shutdown().await.unwrap();
        let mut request = Vec::new();
        target.read_to_end(&mut request).await.unwrap();
        assert_eq!(request, b"request");

        target.write_all(b"a longer response").await.unwrap();
        drop(target);
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"a longer response");

        assert_eq!(relay.await.unwrap().unwrap(), Copied { from_client: 7, from_target: 17 });
        assert_eq!((listen.up(), listen.down()), (7, 17), "每一段都记了");
    }

    #[tokio::test]
    async fn a_reset_on_one_side_is_a_reset_on_the_other() {
        let (mut client, client_far) = pair().await;
        let (mut target, target_far) = pair().await;
        let relay = tokio::spawn(async move {
            let listen = Tally::default();
            let meter = IngressMeter { listen: &listen, credential: None, exit: None };
            relay(client_far, target_far, &meter, std::future::pending()).await
        });

        client.write_all(b"request").await.unwrap();
        target.read_exact(&mut [0; 7]).await.unwrap();
        target.write_all(b"half a resp").await.unwrap();
        target.set_zero_linger().unwrap();
        drop(target);

        let error = client.read_to_end(&mut Vec::new()).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert!(relay.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn cancelling_resets_both_sides() {
        let (mut client, client_far) = pair().await;
        let (mut target, target_far) = pair().await;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let relay = tokio::spawn(async move {
            let listen = Tally::default();
            let meter = IngressMeter { listen: &listen, credential: None, exit: None };
            relay(client_far, target_far, &meter, async move {
                let _ = stopped.await;
                Reason::LimitReached
            })
            .await
        });
        client.write_all(b"x").await.unwrap();
        target.read_exact(&mut [0; 1]).await.unwrap();
        stop.send(()).unwrap();
        assert!(relay.await.unwrap().is_err());
        assert_eq!(client.read_to_end(&mut Vec::new()).await.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(target.read_to_end(&mut Vec::new()).await.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
    }
}
