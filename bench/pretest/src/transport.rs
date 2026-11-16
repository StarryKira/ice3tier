//! 三种传输各自怎么开一条流、怎么当服务端。上层的负载只认 [`Duplex`]。

use std::io;
use std::net::SocketAddr;

use anyhow::Context;
use clap::ValueEnum;
use quinn::{Connection, Endpoint};
use serde::Serialize;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio_kcp::{KcpConfig, KcpListener, KcpNoDelayConfig, KcpStream};

use crate::quic::QuicStats;
use crate::workload::{self, Duplex};

/// 客户端一侧"开一条新流"的办法。
pub enum Connector {
    /// 在一条已经建好的 QUIC 连接上开流。
    Quic(Connection),
    /// 每条流一条新的 TCP 连接。
    Tcp(SocketAddr),
    /// 每条流一个新的 KCP 会话。
    Kcp { config: KcpConfig, addr: SocketAddr },
}

impl Connector {
    pub async fn open(&self) -> anyhow::Result<Duplex> {
        match self {
            Self::Quic(connection) => {
                let (send, recv) = connection.open_bi().await.context("开 QUIC 流")?;
                Ok(Duplex { rd: Box::pin(recv), wr: Box::pin(send) })
            }
            Self::Tcp(addr) => {
                let stream = TcpStream::connect(addr).await.with_context(|| format!("连接 TCP {addr}"))?;
                Ok(tcp_duplex(stream)?)
            }
            Self::Kcp { config, addr } => {
                let stream = KcpStream::connect(config, *addr).await.with_context(|| format!("连接 KCP {addr}"))?;
                Ok(kcp_duplex(stream))
            }
        }
    }
}

fn tcp_duplex(stream: TcpStream) -> std::io::Result<Duplex> {
    stream.set_nodelay(true)?;
    let (rd, wr) = stream.into_split();
    Ok(Duplex { rd: Box::pin(rd), wr: Box::pin(wr) })
}

fn kcp_duplex(stream: KcpStream) -> Duplex {
    let (rd, wr) = tokio::io::split(stream);
    Duplex { rd: Box::pin(rd), wr: Box::pin(wr) }
}

fn spawn_serve(stream: Duplex) {
    tokio::spawn(async move {
        // 客户端测完直接断开是常态，流上的错误不值得打印。
        workload::serve(stream).await.ok();
    });
}

/// 接受 QUIC 连接，每条流交给负载处理。每条连接结束时在标准输出打一行它的统计。
pub async fn serve_quic(endpoint: Endpoint, on_first_connection: impl FnOnce() + Send) {
    let mut on_first_connection = Some(on_first_connection);
    while let Some(incoming) = endpoint.accept().await {
        if let Some(notify) = on_first_connection.take() {
            notify();
        }
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else { return };
            while let Ok((send, recv)) = connection.accept_bi().await {
                spawn_serve(Duplex { rd: Box::pin(recv), wr: Box::pin(send) });
            }
            crate::print_json(&ServerConnectionReport { role: "server-connection", quic: connection.stats().into() });
        });
    }
}

#[derive(Serialize)]
struct ServerConnectionReport {
    role: &'static str,
    quic: QuicStats,
}

/// 监听队列的长度，系统会再把它压到自己的上限（Linux 上是 `net.core.somaxconn`）。
///
/// tokio 默认给 1024。"每个请求一条 TCP"的负载会在同一瞬间拨出几千个连接，队列一满内核就丢握手包：
/// 客户端要等一秒以上重传，个别连接会被重置。那测到的是监听队列，不是 TCP。
const TCP_BACKLOG: u32 = 16384;

fn listen_tcp(listen: SocketAddr) -> io::Result<TcpListener> {
    let socket = if listen.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
    // 和 `TcpListener::bind` 保持一致：Unix 上允许立刻重用刚关掉的端口，Windows 上这个选项含义不同，不设。
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    socket.bind(listen)?;
    socket.listen(TCP_BACKLOG)
}

pub async fn serve_tcp(listen: SocketAddr) -> anyhow::Result<()> {
    let listener = listen_tcp(listen).with_context(|| format!("绑定 TCP {listen}"))?;
    loop {
        let (stream, _peer) = listener.accept().await?;
        spawn_serve(tcp_duplex(stream)?);
    }
}

pub async fn serve_kcp(listen: SocketAddr, config: KcpConfig) -> anyhow::Result<()> {
    let mut listener = KcpListener::bind(config, listen).await.with_context(|| format!("绑定 KCP {listen}"))?;
    loop {
        let (stream, _peer) = listener.accept().await?;
        spawn_serve(kcp_duplex(stream));
    }
}

/// 在两条 TCP 之间搬数据的中间跳：每一段各自终结、各自重传。
/// 这是"端到端一条连接经 relay 转发"的对照组。
pub async fn run_tcp_hop(listen: SocketAddr, upstream: SocketAddr) -> anyhow::Result<()> {
    let listener = listen_tcp(listen).with_context(|| format!("绑定 TCP {listen}"))?;
    loop {
        let (mut inbound, _peer) = listener.accept().await?;
        tokio::spawn(async move {
            let Ok(mut outbound) = TcpStream::connect(upstream).await else { return };
            if inbound.set_nodelay(true).is_err() || outbound.set_nodelay(true).is_err() {
                return;
            }
            // 任何一边断开都算这条流结束。
            tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await.ok();
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KcpMode {
    /// nodelay、10ms 刷新、2 次跳过即快速重传、不做拥塞控制。
    Fast,
    /// 库的普通档：40ms 刷新，保留拥塞控制。
    Normal,
}

#[derive(Clone, Copy, Debug)]
pub struct KcpOpts {
    pub mode: KcpMode,
    /// 发送窗口和接收窗口，单位是包。
    pub window: u16,
}

impl KcpOpts {
    pub fn config(self) -> KcpConfig {
        KcpConfig {
            mtu: 1400,
            nodelay: match self.mode {
                KcpMode::Fast => KcpNoDelayConfig::fastest(),
                KcpMode::Normal => KcpNoDelayConfig::normal(),
            },
            wnd_size: (self.window, self.window),
            stream: true,
            ..KcpConfig::default()
        }
    }
}
