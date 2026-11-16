//! 管理 API 的客户端：发请求，把出错的应答变成一句给人看的话。

use anyhow::{Context, anyhow, bail};
use i3t_model::admin::{API_PREFIX, ErrorBody};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    /// `url` 是管理 API 的地址，例如 `http://127.0.0.1:20332`。
    pub fn new(url: &str, token: String) -> anyhow::Result<Self> {
        if !url.starts_with("http://") {
            bail!("管理 API 的地址要以 http:// 开头，现在是 {url:?}。它是明文 HTTP，远程管理请用 SSH 端口转发");
        }
        let http = reqwest::Client::builder().build().context("初始化 HTTP 客户端")?;
        Ok(Self { http, base: format!("{}{API_PREFIX}", url.trim_end_matches('/')), token })
    }

    /// 发一个请求，返回应答的 JSON。没有应答体（删除成功）时是 `null`。
    pub async fn request<B: Serialize + Sync>(&self, method: Method, path: &str, body: Option<&B>) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut request = self.http.request(method, &url).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.with_context(|| format!("连不上 controller 的管理 API（{}）", self.base))?;
        let status = response.status();
        let bytes = response.bytes().await.context("读 controller 的应答")?;
        if status.is_success() {
            return if bytes.is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_slice(&bytes).context("controller 的应答不是 JSON")
            };
        }
        match serde_json::from_slice::<ErrorBody>(&bytes) {
            Ok(body) => {
                let code = serde_json::to_value(body.error.code).ok().and_then(|code| code.as_str().map(str::to_owned)).unwrap_or_default();
                let hint = if status == StatusCode::UNAUTHORIZED {
                    "。用 --token 或环境变量 I3T_ADMIN_TOKEN 给出管理令牌"
                } else {
                    ""
                };
                Err(anyhow!("{}（{code}）{hint}", body.error.message))
            }
            Err(_) => Err(anyhow!("controller 回了 {status}，应答不是预期的格式。--url 指的是管理 API 吗？")),
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        decode(self.request(Method::GET, path, None::<&()>).await?)
    }
}

/// 把应答的 JSON 读成模型里的类型。
pub fn decode<T: DeserializeOwned>(value: Value) -> anyhow::Result<T> {
    serde_json::from_value(value).context("controller 的应答和这个版本的 i3tctl 对不上")
}
