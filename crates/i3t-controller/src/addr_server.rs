//! 地址查询："你看到我的地址是什么"。
//!
//! 网络里还没有 relay 时，节点只能问 controller 自己在 NAT 外面的地址，所以 controller 在节点协议的端口号上
//! 另开一个 UDP 端口回答。回答的一方不记任何状态，应答不比查询大，伪造来源地址的人拿它放大不了流量。

use std::time::Duration;

use i3t_proto::addr::{Query, Reply};
use tokio::net::UdpSocket;
use tokio::sync::watch;

/// 查询帧固定 32 字节。更长的数据报只看开头，回答照样不比它大。
const BUFFER_LEN: usize = 64;

/// 在 `socket` 上回答地址查询，直到 `shutdown` 变化。不是查询帧的数据报不回应。
pub async fn serve(socket: UdpSocket, mut shutdown: watch::Receiver<bool>) {
    let mut buffer = [0; BUFFER_LEN];
    loop {
        let received = tokio::select! {
            received = socket.recv_from(&mut buffer) => received,
            _ = shutdown.changed() => return,
        };
        match received {
            Ok((len, from)) => {
                let Some(query) = Query::parse(&buffer[..len]) else { continue };
                let reply = Reply { nonce: query.nonce, observed: from }.to_bytes();
                if let Err(err) = socket.send_to(&reply, from).await {
                    tracing::debug!(%from, "地址查询的应答没发出去：{err}");
                }
            }
            Err(err) => {
                // 有的平台会把上一次发送引起的 ICMP 错误报在这里，和这个 socket 能不能用无关。
                tracing::debug!("地址查询的端口收包出错：{err}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}
