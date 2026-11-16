//! `i3t connect`：把标准输入输出接到一条经本机节点出去的 TCP 连接上。
//!
//! 给自己不会用代理的工具用，典型是 ssh：
//!
//! ```text
//! ssh -o ProxyCommand='i3t connect %h %p' nas
//! ```
//!
//! 连接经本机节点的 SOCKS5 入口出去（配置文件里的 `proxy.socks5`），所以节点要在运行。目标写节点的名字、
//! 全名或者虚拟 IP；加了 `--via <出口>` 时，目标是从那个出口节点的网络里看到的地址。
//!
//! 标准输出是数据通道，这里除了数据什么都不往它上面写；出错的说明都在标准错误上。

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, bail, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

const SOCKS_VERSION: u8 = 5;
/// 用户名密码认证这个子协议自己的版本号（RFC 1929）。
const AUTH_VERSION: u8 = 1;
const METHOD_NONE: u8 = 0x00;
const METHOD_PASSWORD: u8 = 0x02;
const METHOD_REFUSED: u8 = 0xff;
const COMMAND_CONNECT: u8 = 1;
const ADDR_V4: u8 = 1;
const ADDR_NAME: u8 = 3;
const ADDR_V6: u8 = 4;

/// 要连到哪里。
#[derive(Debug)]
pub(crate) struct Destination {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// 从哪个出口节点的网络出去。`None` 表示目标本身就是网络里的节点。
    pub(crate) via: Option<String>,
}

/// 经 `proxy`（本机节点的 SOCKS5 入口）连到 `destination`，然后在它和标准输入输出之间搬数据，直到对面关闭。
pub(crate) async fn connect(proxy: SocketAddr, destination: &Destination) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(proxy)
        .await
        .with_context(|| format!("连不上本机节点的 SOCKS5 入口 {proxy}。节点在运行吗？用 i3t status 看一下"))?;
    stream.set_nodelay(true).context("设置连接参数")?;
    negotiate(&mut stream, destination).await?;
    pipe(stream).await
}

/// 走完 SOCKS5 的握手：协商认证方式，（有出口时）报上用户名，发 CONNECT，读回复。
async fn negotiate(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), destination: &Destination) -> anyhow::Result<()> {
    // 出口节点写在用户名里，所以有出口时只提用户名密码这一种方式。
    let method = if destination.via.is_some() { METHOD_PASSWORD } else { METHOD_NONE };
    stream.write_all(&[SOCKS_VERSION, 1, method]).await.context("向本机节点发握手")?;
    let mut chosen = [0u8; 2];
    stream.read_exact(&mut chosen).await.context("读本机节点的握手应答")?;
    ensure!(chosen[0] == SOCKS_VERSION, "配置里的 SOCKS5 入口上答话的不是 SOCKS5 服务");
    match (chosen[1], &destination.via) {
        (METHOD_NONE, None) => {}
        (METHOD_PASSWORD, Some(exit)) => authenticate(stream, exit).await?,
        (METHOD_REFUSED, None) => {
            bail!("这个入口要求凭据（它没有绑在回环地址上）。i3t connect 只用本机回环上的入口，请把 proxy.socks5 设成 127.0.0.1 上的地址")
        }
        (METHOD_REFUSED, Some(_)) => bail!("本机节点的入口不接受用用户名指定出口"),
        (other, _) => bail!("本机节点选了一种没有提过的认证方式（{other:#04x}）"),
    }

    stream.write_all(&request(destination)?).await.context("向本机节点发连接请求")?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.context("读本机节点对连接请求的回复")?;
    ensure!(head[0] == SOCKS_VERSION, "本机节点的回复不是 SOCKS5 的格式");
    // 回复后面跟着一个绑定地址，用不上，但要读走，不然它会混进数据里。
    let bound_len = match head[3] {
        ADDR_V4 => 4,
        ADDR_V6 => 16,
        ADDR_NAME => usize::from(stream.read_u8().await.context("读回复里的地址")?),
        other => bail!("本机节点的回复里有不认识的地址类型（{other:#04x}）"),
    };
    let mut bound = vec![0u8; bound_len + 2];
    stream.read_exact(&mut bound).await.context("读回复里的地址")?;
    match head[1] {
        0 => Ok(()),
        code => bail!("连不上 {}：{}", destination.describe(), refusal(code)),
    }
}

/// 用户名密码认证：用户名是出口节点，密码留空（回环上的入口不查密码）。
async fn authenticate(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), exit: &str) -> anyhow::Result<()> {
    let name_len =
        u8::try_from(exit.len()).ok().filter(|len| *len > 0).with_context(|| format!("出口的名字要在 1 到 255 字节之间：{exit:?}"))?;
    let mut message = vec![AUTH_VERSION, name_len];
    message.extend_from_slice(exit.as_bytes());
    message.push(0);
    stream.write_all(&message).await.context("向本机节点报出口")?;
    let mut answer = [0u8; 2];
    stream.read_exact(&mut answer).await.context("读本机节点对出口的应答")?;
    ensure!(answer[1] == 0, "本机节点不接受这个出口：{exit:?}");
    Ok(())
}

/// CONNECT 请求。目标是地址时按地址发，否则按名字发，由节点去解析。
fn request(destination: &Destination) -> anyhow::Result<Vec<u8>> {
    let mut request = vec![SOCKS_VERSION, COMMAND_CONNECT, 0];
    match destination.host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.push(ADDR_V4);
            request.extend_from_slice(&ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            request.push(ADDR_V6);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let name = destination.host.as_bytes();
            let len = u8::try_from(name.len())
                .ok()
                .filter(|len| *len > 0)
                .with_context(|| format!("目标的名字要在 1 到 255 字节之间：{:?}", destination.host))?;
            request.push(ADDR_NAME);
            request.push(len);
            request.extend_from_slice(name);
        }
    }
    request.extend_from_slice(&destination.port.to_be_bytes());
    Ok(request)
}

/// SOCKS5 回复里的失败码是什么意思（RFC 1928 第 6 节）。节点把建流失败的原因映射到这几个码上。
const fn refusal(code: u8) -> &'static str {
    match code {
        1 => "出口或者目标节点现在接不了这条连接（不可用、正在排空、连接太多或者被限速），细节见节点的日志",
        2 => "访问规则不允许，或者达到了限额",
        3 => "到目标的网络不通",
        4 => "目标到不了，或者连接超时",
        5 => "目标拒绝了连接（端口上没有服务在听）",
        6 => "连接超时",
        7 => "节点不支持这种请求",
        8 => "节点不支持这种地址",
        _ => "节点回了一个不认识的失败码",
    }
}

impl Destination {
    fn describe(&self) -> String {
        match &self.via {
            Some(exit) => format!("{}:{}（经出口 {exit}）", self.host, self.port),
            None => format!("{}:{}", self.host, self.port),
        }
    }
}

/// 在连接和标准输入输出之间搬数据。对面关闭就结束；标准输入先结束时只关掉发送的方向，接着收对面的。
async fn pipe(stream: TcpStream) -> anyhow::Result<()> {
    let (mut from_peer, mut to_peer) = stream.into_split();
    let upload = async {
        tokio::io::copy(&mut tokio::io::stdin(), &mut to_peer).await?;
        to_peer.shutdown().await
    };
    let download = async {
        let mut stdout = tokio::io::stdout();
        tokio::io::copy(&mut from_peer, &mut stdout).await?;
        stdout.flush().await
    };
    tokio::pin!(upload, download);
    let mut uploading = true;
    loop {
        tokio::select! {
            finished = &mut download => return finished.context("从对端收数据"),
            finished = &mut upload, if uploading => {
                finished.context("把标准输入发给对端")?;
                uploading = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::DuplexStream;

    use super::*;

    fn to(host: &str, port: u16, via: Option<&str>) -> Destination {
        Destination { host: host.to_owned(), port, via: via.map(str::to_owned) }
    }

    /// 让 `negotiate` 和一个照剧本答话的假入口对话：`replies` 是它依次回的内容，返回握手的结果和它收到的全部字节。
    async fn talk(destination: &Destination, replies: &[&[u8]]) -> (anyhow::Result<()>, Vec<u8>) {
        let (mut client, mut server): (DuplexStream, DuplexStream) = tokio::io::duplex(1024);
        for reply in replies {
            server.write_all(reply).await.unwrap();
        }
        let result = negotiate(&mut client, destination).await;
        drop(client);
        let mut received = Vec::new();
        server.read_to_end(&mut received).await.unwrap();
        (result, received)
    }

    const ACCEPTED_V4: &[u8] = &[5, 0, 0, 1, 0, 0, 0, 0, 0, 0];

    #[tokio::test]
    async fn a_node_name_is_sent_as_a_name_without_authentication() {
        let (result, sent) = talk(&to("nas", 22, None), &[&[5, 0], ACCEPTED_V4]).await;
        result.unwrap();
        assert_eq!(sent, [&[5, 1, 0][..], &[5, 1, 0, 3, 3], b"nas", &[0, 22]].concat());
    }

    #[tokio::test]
    async fn addresses_are_sent_as_addresses() {
        let (result, sent) = talk(&to("10.77.0.3", 8080, None), &[&[5, 0], ACCEPTED_V4]).await;
        result.unwrap();
        assert_eq!(sent[3..], [5, 1, 0, 1, 10, 77, 0, 3, 0x1f, 0x90]);

        let (result, sent) = talk(&to("fd00::3", 443, None), &[&[5, 0], ACCEPTED_V4]).await;
        result.unwrap();
        assert_eq!(sent[3..7], [5, 1, 0, 4]);
        assert_eq!(sent.len(), 3 + 4 + 16 + 2);
    }

    #[tokio::test]
    async fn an_exit_goes_into_the_user_name_with_an_empty_password() {
        let (result, sent) = talk(&to("192.168.1.10", 22, Some("home")), &[&[5, 2], &[1, 0], ACCEPTED_V4]).await;
        result.unwrap();
        let expected = [&[5, 1, 2][..], &[1, 4], b"home", &[0], &[5, 1, 0, 1, 192, 168, 1, 10, 0, 22]].concat();
        assert_eq!(sent, expected);
    }

    #[tokio::test]
    async fn the_bound_address_in_the_reply_is_consumed_whatever_its_kind() {
        let named = [&[5, 0, 0, 3, 4][..], b"host", &[0, 80]].concat();
        let v6 = [&[5, 0, 0, 4][..], &[0; 16], &[0, 80]].concat();
        for reply in [named, v6] {
            let (mut client, mut server) = tokio::io::duplex(1024);
            server.write_all(&[5, 0]).await.unwrap();
            server.write_all(&reply).await.unwrap();
            server.write_all(b"first bytes of the session").await.unwrap();
            negotiate(&mut client, &to("nas", 22, None)).await.unwrap();
            let mut rest = [0u8; 26];
            client.read_exact(&mut rest).await.unwrap();
            assert_eq!(&rest, b"first bytes of the session", "握手之后读到的第一个字节就是数据");
        }
    }

    #[tokio::test]
    async fn a_refusal_names_the_destination_and_the_reason() {
        let refused = [5, 2, 0, 1, 0, 0, 0, 0, 0, 0];
        let (result, _) = talk(&to("nas", 22, None), &[&[5, 0], &refused]).await;
        let message = result.unwrap_err().to_string();
        assert!(message.contains("nas:22") && message.contains("访问规则不允许"), "{message}");

        let unreachable = [5, 3, 0, 1, 0, 0, 0, 0, 0, 0];
        let (result, _) = talk(&to("10.0.0.9", 22, Some("home")), &[&[5, 2], &[1, 0], &unreachable]).await;
        let message = result.unwrap_err().to_string();
        assert!(message.contains("经出口 home") && message.contains("网络不通"), "{message}");
    }

    #[tokio::test]
    async fn an_entry_that_demands_credentials_is_explained() {
        let (result, _) = talk(&to("nas", 22, None), &[&[5, 0xff]]).await;
        assert!(result.unwrap_err().to_string().contains("要求凭据"));
        let (result, _) = talk(&to("nas", 22, Some("home")), &[&[5, 2], &[1, 1]]).await;
        assert!(result.unwrap_err().to_string().contains("不接受这个出口"));
    }

    #[tokio::test]
    async fn something_that_is_not_socks5_is_recognised() {
        let (result, _) = talk(&to("nas", 22, None), &[b"HTTP/1.1 400 Bad Request\r\n"]).await;
        assert!(result.unwrap_err().to_string().contains("不是 SOCKS5"));
    }

    #[test]
    fn names_that_do_not_fit_are_refused_before_anything_is_sent() {
        assert!(request(&to("", 22, None)).is_err());
        assert!(request(&to(&"a".repeat(256), 22, None)).is_err());
        assert!(request(&to(&"a".repeat(255), 22, None)).is_ok());
    }
}
