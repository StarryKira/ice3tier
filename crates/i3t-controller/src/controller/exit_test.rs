//! 出口测试（白皮书 §13 的 `i3tctl exit test`）：让一个出口按它的出站策略试着连一个目标，看解析、连接和 TLS 握手各要多久、
//! 卡在哪一步、经什么出去。
//!
//! controller 只转达：给出口节点发一条查询（和路由查询同一套编号和等待），节点按自己的规矩做（一次只做一个、每 2 秒最多一个、
//! 每一步有时限），回答原样交给管理 API。节点不在线时是 `node_offline`；节点版本太旧不懂、或者时限之内没有回答时 `answered` 是 `false`。

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use i3t_model::admin::{ExitTestInput, ExitTestView, NetworkId};
use i3t_model::control::{ExitTestResult, ToNode};
use i3t_proto::NodeId;
use i3t_proto::flow::Host;
use tokio::sync::oneshot;

use super::Controller;
use super::routes::Reply;
use crate::error::{Entity, Error};
use crate::server::blocking;

/// 等出口节点回答多久。节点自己的时限是解析、连接、握手各 5 秒，加上路上的时间。
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// 不写端口时测的端口：顺带测 TLS 握手。
const DEFAULT_PORT: u16 = 443;

/// 一个发出去、还在等回答的出口测试。
pub struct AskedExitTest {
    target: Target,
    answer: Option<oneshot::Receiver<ExitTestResult>>,
}

/// 要测的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
}

impl Target {
    /// `主机[:端口]`。IPv6 地址可以带方括号（要写端口时必须带），不带时整个就是地址。
    fn parse(text: &str) -> Result<Self, Error> {
        let malformed = || Error::invalid_field("target", "malformed", format!("目标 {text:?} 不是「主机[:端口]」"));
        let text = text.trim();
        let (host, port) = if text.parse::<IpAddr>().is_ok() {
            (text, None)
        } else if let Some(rest) = text.strip_prefix('[') {
            let (address, after) = rest.split_once(']').ok_or_else(malformed)?;
            (address, if after.is_empty() { None } else { Some(after.strip_prefix(':').ok_or_else(malformed)?) })
        } else {
            match text.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (text, None),
            }
        };
        let port = match port {
            Some(port) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
                port.parse::<u16>().ok().filter(|&port| port != 0).ok_or_else(malformed)?
            }
            Some(_) => return Err(malformed()),
            None => DEFAULT_PORT,
        };
        host.parse::<Host>().map_err(|_| malformed())?;
        Ok(Self { host: host.to_owned(), port })
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') { write!(f, "[{}]:{}", self.host, self.port) } else { write!(f, "{}:{}", self.host, self.port) }
    }
}

impl Controller {
    /// 让出口 `node` 试着连 `input.target`，等它回答。最多等 20 秒左右。
    ///
    /// # Errors
    ///
    /// 节点不在这个网络里或者不是出口（`not_found`），目标写得不对（`invalid_request`），节点不在线（`node_offline`）。
    pub async fn exit_test(self: &Arc<Self>, network: NetworkId, node: NodeId, input: ExitTestInput) -> Result<ExitTestView, Error> {
        let asked = blocking(self, move |controller| controller.ask_exit_test(network, node, &input)).await?;
        let result = match asked.answer {
            Some(answer) => tokio::time::timeout(ANSWER_TIMEOUT, answer).await.ok().and_then(Result::ok),
            None => None,
        };
        let target = asked.target.to_string();
        blocking(self, move |controller| controller.exit_test_view(network, node, target, result)).await
    }

    /// [`Controller::exit_test`] 的前一半：检查，然后给出口节点发查询。节点的版本太旧不懂这条消息时不发。
    ///
    /// # Errors
    ///
    /// 同 [`Controller::exit_test`]。
    pub fn ask_exit_test(&self, network: NetworkId, node: NodeId, input: &ExitTestInput) -> Result<AskedExitTest, Error> {
        let mut inner = self.lock();
        let record = inner.require_node(network, node)?;
        if record.exit.is_none() {
            return Err(Error::NotFound(Entity::Exit));
        }
        let target = Target::parse(&input.target)?;
        let Some(live) = inner.live.get(&node) else { return Err(Error::NodeOffline(record.name)) };
        if !live.understands_updates() {
            tracing::info!(%node, "不给这个出口发出口测试：它的版本太旧，收到不认识的消息会断开");
            return Ok(AskedExitTest { target, answer: None });
        }
        let (reply, answer) = oneshot::channel();
        let message = |request| ToNode::ExitTest { request, host: target.host.clone(), port: target.port };
        if inner.query(node, message, Reply::ExitTest(reply)).is_none() {
            return Err(Error::NodeOffline(record.name));
        }
        tracing::info!(%node, %target, "让出口试着连一个目标");
        Ok(AskedExitTest { target, answer: Some(answer) })
    }

    /// [`Controller::exit_test`] 的后一半。
    fn exit_test_view(
        &self,
        network: NetworkId,
        node: NodeId,
        target: String,
        result: Option<ExitTestResult>,
    ) -> Result<ExitTestView, Error> {
        let inner = self.lock();
        let record = inner.require_node(network, node)?;
        Ok(ExitTestView { node: record.to_ref(), target, answered: result.is_some(), result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Option<(String, u16)> {
        Target::parse(text).ok().map(|target| (target.host, target.port))
    }

    #[test]
    fn a_target_is_a_host_with_an_optional_port_that_defaults_to_443() {
        assert_eq!(parse("example.com"), Some(("example.com".into(), 443)));
        assert_eq!(parse(" example.com:80 "), Some(("example.com".into(), 80)));
        assert_eq!(parse("93.184.216.34:8443"), Some(("93.184.216.34".into(), 8443)));
        assert_eq!(parse("2001:db8::1"), Some(("2001:db8::1".into(), 443)));
        assert_eq!(parse("[2001:db8::1]:80"), Some(("2001:db8::1".into(), 80)));
        assert_eq!(parse("[2001:db8::1]"), Some(("2001:db8::1".into(), 443)));
        for bad in ["", "example.com:", "example.com:0", "example.com:+1", "example.com:65536", "exa mple.com", "[::1", "[::1]x", "a:b:c"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert_eq!(Target::parse("[2001:db8::1]:80").unwrap().to_string(), "[2001:db8::1]:80");
        assert_eq!(Target::parse("example.com").unwrap().to_string(), "example.com:443");
    }
}
