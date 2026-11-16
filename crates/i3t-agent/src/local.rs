//! 节点在本机开的两个小接口，都只绑回环地址：
//!
//! - **指标**：`http://<配置里的地址>/metrics`，Prometheus 的文本格式，不带认证。里面只有计数和对端的名字，
//!   没有密钥、令牌和目标地址。地址在 `config.toml` 的 `[metrics]` 里，写 `"off"` 就不开。
//! - **本机状态**：`i3t status` 和 `i3t routes` 向正在运行的节点问现在的状态。它在一个系统挑的回环端口上，
//!   每个请求都要带令牌。
//!
//! 本机状态接口为什么是"回环端口加令牌"：回环地址本机的任何用户都连得上，只靠它挡不住别的用户。
//! 所以节点每次启动生成一个随机令牌，和端口一起写进状态目录里的 `local.json`。状态目录只有属主进得去
//! （节点的私钥也在里面），于是能问状态的就是能读私钥的那些人：运行节点的用户，和 root。
//! 没有用 Unix 域套接字，是因为这个办法在 Linux、Windows、macOS 上是同一份代码。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, anyhow};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::http::{self, Request, Response};
use crate::metrics;
use crate::runtime::Runtime;
use crate::state::{LocalEndpoint, StateDir};
use crate::status::LiveStatus;

const STATUS_PATH: &str = "/status";
const METRICS_PATH: &str = "/metrics";
/// Prometheus 的文本格式。
const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// 开着的本机接口。丢弃它就关掉，并删掉 `local.json`。
pub(crate) struct LocalServers {
    state: StateDir,
    tasks: Vec<JoinHandle<()>>,
}

impl LocalServers {
    /// 开出本机状态接口，和（配置里没有关掉的话）指标接口。
    ///
    /// 两个接口都不是节点工作所必需的：开不出来只记一条日志，节点照常运行。
    pub(crate) async fn start(runtime: &Arc<Runtime>, state: &StateDir, metrics_at: Option<SocketAddr>) -> Self {
        let mut tasks = Vec::new();
        match start_status(runtime, state).await {
            Ok(task) => tasks.push(task),
            Err(error) => tracing::warn!("本机状态接口没有开出来，i3t status 和 i3t routes 看不到运行中的状态：{error:#}"),
        }
        if let Some(addr) = metrics_at {
            match TcpListener::bind(addr).await {
                Ok(listener) => {
                    tracing::info!(%addr, "指标接口开在 http://{addr}{METRICS_PATH}");
                    let runtime = Arc::clone(runtime);
                    tasks.push(tokio::spawn(http::serve(listener, move |request| {
                        let response = respond_metrics(&runtime, &request);
                        async move { response }
                    })));
                }
                Err(error) => {
                    tracing::warn!(%addr, %error, "指标接口没有开出来（端口被占用的话，在 config.toml 的 [metrics] 里换一个，或者写 \"off\"）")
                }
            }
        }
        Self { state: state.clone(), tasks }
    }
}

impl Drop for LocalServers {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Err(error) = self.state.remove_local() {
            tracing::debug!("没能删掉本机状态接口的记录：{error:#}");
        }
    }
}

async fn start_status(runtime: &Arc<Runtime>, state: &StateDir) -> anyhow::Result<JoinHandle<()>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.context("在回环地址上绑定一个端口")?;
    let addr = listener.local_addr().context("查询绑定到的端口")?;
    let token: String = rand::random::<[u8; 24]>().iter().map(|byte| format!("{byte:02x}")).collect();
    state.save_local(&LocalEndpoint { addr, token: token.clone() })?;
    let runtime = Arc::clone(runtime);
    Ok(tokio::spawn(http::serve(listener, move |request| {
        let response = respond_status(&runtime, &token, &request);
        async move { response }
    })))
}

fn respond_status(runtime: &Runtime, token: &str, request: &Request) -> Response {
    if !request.bearer.as_deref().is_some_and(|presented| same(presented, token)) {
        tracing::warn!(path = %request.path, "拒绝了本机状态接口上的一个请求：没有带令牌，或者令牌不对");
        return Response::error(401, "要带状态目录里 local.json 记的令牌");
    }
    match request.path.as_str() {
        STATUS_PATH => match serde_json::to_string(&LiveStatus::of(runtime)) {
            Ok(body) => Response::ok("application/json", body),
            Err(error) => Response::error(500, &format!("状态写不成 JSON：{error}")),
        },
        _ => Response::error(404, "没有这个路径"),
    }
}

fn respond_metrics(runtime: &Runtime, request: &Request) -> Response {
    match request.path.as_str() {
        METRICS_PATH => Response::ok(METRICS_CONTENT_TYPE, metrics::render(runtime)),
        _ => Response::error(404, "指标在 /metrics"),
    }
}

/// 比较两个令牌，花的时间不随第一个不同的位置变化。
fn same(presented: &str, expected: &str) -> bool {
    presented.len() == expected.len() && presented.bytes().zip(expected.bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

/// 向正在运行的节点问它现在的状态。没有节点在运行时返回 `None`。
///
/// # Errors
///
/// `local.json` 读不了，或者节点在运行却没有给出看得懂的回答。
pub async fn query(state: &StateDir) -> anyhow::Result<Option<LiveStatus>> {
    let Some(local) = state.local()? else { return Ok(None) };
    let body = match http::get(local.addr, STATUS_PATH, Some(&local.token)).await {
        Ok(body) => body,
        Err(error) => {
            // 记录还在、端口却连不上：节点是被杀掉的，没来得及删记录。
            let refused = error
                .chain()
                .any(|cause| cause.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused));
            return if refused { Ok(None) } else { Err(anyhow!("问不到正在运行的节点：{error:#}")) };
        }
    };
    serde_json::from_str(&body).map(Some).context("正在运行的节点给的状态看不懂")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_must_match_exactly() {
        assert!(same("abc123", "abc123"));
        assert!(!same("abc124", "abc123") && !same("abc12", "abc123") && !same("", "abc123"));
    }

    #[tokio::test]
    async fn no_record_or_a_stale_record_means_no_node_is_running() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        assert_eq!(query(&state).await.unwrap(), None);
        // 被杀掉的节点留下的记录：端口上已经没有人了。
        let gone = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        state.save_local(&LocalEndpoint { addr: gone, token: "t".to_owned() }).unwrap();
        assert_eq!(query(&state).await.unwrap(), None);
    }
}
