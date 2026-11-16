//! 启用 relay 时从 controller 这边试连它填的地址（白皮书 §10.4）。
//!
//! 只是参考，不挡着启用：controller 和别的节点不在同一个网络位置上，它连得上不等于别人连得上，它连不上也不等于别人连不上
//! （比如 controller 和 relay 在同一个局域网里，填的却是公网地址）。
//!
//! - UDP：向那个地址发一次地址查询（`i3t_proto::addr`），收到带着同一个随机数的应答就算通。节点的 UDP 端口总在回答地址查询，
//!   不用等它把 relay 开起来。
//! - TCP：连一次就关。relay 的 TCP 监听是节点收到新的 netmap 之后才开的，所以在时限之内隔一会儿重试。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use i3t_model::admin::{Reachability, Transport};
use i3t_proto::addr::{Query, Reply};
use ring::rand::{SecureRandom, SystemRandom};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time;

/// 每个地址最多试这么久。
pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(3);
/// UDP 的查询每隔这么久重发一次：一个包丢了不能就算不通。
const UDP_RESEND: Duration = Duration::from_millis(500);
/// TCP 连不上时隔这么久再试：节点可能还没开始监听。
const TCP_RETRY: Duration = Duration::from_millis(200);

/// 同时试 `udp` 和 `tcp` 里的每个地址，每个地址一项结果，顺序和填的一样（先 UDP 后 TCP）。最多等 [`CHECK_TIMEOUT`] 多一点。
pub(crate) async fn check(udp: &[SocketAddr], tcp: &[SocketAddr]) -> Vec<Reachability> {
    let udp = udp.iter().map(|&addr| (Transport::Udp, addr, tokio::spawn(check_udp(addr))));
    let tcp = tcp.iter().map(|&addr| (Transport::Tcp, addr, tokio::spawn(check_tcp(addr))));
    let checks: Vec<_> = udp.chain(tcp).collect();
    let mut results = Vec::with_capacity(checks.len());
    // 它们是同时在跑的，挨个等的总时间也就是最慢的那一个。
    for (protocol, addr, check) in checks {
        results.push(check.await.unwrap_or_else(|error| failed(protocol, addr, format!("检查没有做完：{error}"))));
    }
    results
}

async fn check_udp(addr: SocketAddr) -> Reachability {
    let started = Instant::now();
    let outcome = time::timeout(CHECK_TIMEOUT, ask_address(addr)).await;
    let rtt = started.elapsed();
    match outcome {
        Ok(Ok(())) => reached(Transport::Udp, addr, rtt),
        Ok(Err(error)) => failed(Transport::Udp, addr, error),
        Err(_) => failed(Transport::Udp, addr, format!("{} 秒之内没有回答地址查询", CHECK_TIMEOUT.as_secs())),
    }
}

/// 向 `addr` 发地址查询，直到收到对得上的应答。
async fn ask_address(addr: SocketAddr) -> Result<(), String> {
    let bind: SocketAddr = if addr.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { ([0u16; 8], 0).into() };
    let socket = UdpSocket::bind(bind).await.map_err(|error| format!("绑不出 UDP 端口：{error}"))?;
    let mut nonce = [0; 8];
    SystemRandom::new().fill(&mut nonce).map_err(|_| "系统的随机数源不可用".to_owned())?;
    let query = Query { nonce }.to_bytes();
    let mut buf = [0; 64];
    loop {
        socket.send_to(&query, addr).await.map_err(|error| format!("发不出去：{error}"))?;
        let deadline = time::sleep(UDP_RESEND);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => break,
                received = socket.recv_from(&mut buf) => {
                    // ICMP 的"端口不可达"在有的系统上报在这里：照常等，到时限再算不通。
                    let Ok((len, from)) = received else { continue };
                    if from == addr && Reply::parse(&buf[..len]).is_some_and(|reply| reply.nonce == nonce) {
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn check_tcp(addr: SocketAddr) -> Reachability {
    let started = Instant::now();
    let attempt = async {
        loop {
            let tried = Instant::now();
            match TcpStream::connect(addr).await {
                Ok(_stream) => return tried.elapsed(),
                Err(_) => time::sleep(TCP_RETRY).await,
            }
        }
    };
    match time::timeout(CHECK_TIMEOUT, attempt).await {
        Ok(rtt) => reached(Transport::Tcp, addr, rtt),
        Err(_) => {
            // 时限到了，再试一次拿到具体的原因。
            let reason = match time::timeout(Duration::from_millis(500), TcpStream::connect(addr)).await {
                Ok(Err(error)) => error.to_string(),
                Ok(Ok(_)) => return reached(Transport::Tcp, addr, started.elapsed()),
                Err(_) => "连接超时".to_owned(),
            };
            failed(Transport::Tcp, addr, reason)
        }
    }
}

fn reached(protocol: Transport, addr: SocketAddr, rtt: Duration) -> Reachability {
    let rtt_ms = u32::try_from(rtt.as_millis()).unwrap_or(u32::MAX);
    Reachability { protocol, addr, reachable: true, rtt_ms: Some(rtt_ms), error: None }
}

fn failed(protocol: Transport, addr: SocketAddr, error: String) -> Reachability {
    tracing::info!(?protocol, %addr, %error, "从 controller 这边连不上 relay 填的地址（只是参考，不挡着启用）");
    Reachability { protocol, addr, reachable: false, rtt_ms: None, error: Some(error) }
}
