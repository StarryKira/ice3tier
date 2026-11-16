//! 管理 API：HTTP + JSON，WebUI 和命令行都走它。
//!
//! 路径、请求体和应答体的说明在 `docs/admin-api.md`，数据结构在 `i3t_model::admin`。
//! 这里每个处理函数只做三件事：取出路径和请求体，调用 [`Controller`] 上对应的方法，把结果写成 JSON。
//!
//! 所有请求都要带管理令牌（`Authorization: Bearer <令牌>`）。只有一个管理员，没有角色。

use std::net::SocketAddr;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use i3t_model::admin::{
    API_PREFIX, ApiError, CreateCredential, CreateEnrollToken, CreateNetwork, CreateRule, ErrorBody, ErrorCode, ForwardInput,
    ListenerInput, NetworkId, PairInput, RelaySettings, Succeed, UpdateNetwork, UpdateNode, UpdateRule,
};
use i3t_model::netmap::ExitRole;
use i3t_proto::NodeId;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tower_http::services::{ServeDir, ServeFile};

use crate::controller::Controller;
use crate::error::{Error, error_chain};
use crate::server::blocking;

/// 操作记录一次最多取多少条，和不写 `limit` 时取多少条。
const MAX_LOG_LIMIT: u32 = 1000;
const DEFAULT_LOG_LIMIT: u32 = 100;

type App = Arc<Controller>;
type Reply = Result<Response, Failure>;

/// 管理 API 的全部路由。给了 `static_dir` 时，API 之外的路径从这个目录取文件（WebUI），找不到的路径回 `index.html`。
pub fn router(controller: Arc<Controller>, static_dir: Option<&FsPath>) -> Router {
    let network = "/networks/{network}";
    let api = Router::new()
        .route("/status", get(status))
        .route("/log", get(log))
        .route("/networks", get(list_networks).post(create_network))
        .route(network, get(show_network).patch(update_network).delete(delete_network))
        .route(&format!("{network}/overview"), get(overview))
        .route(&format!("{network}/nodes"), get(list_nodes))
        .route(&format!("{network}/nodes/{{node}}"), get(show_node).patch(update_node).delete(delete_node))
        .route(&format!("{network}/nodes/{{node}}/relay"), put(enable_relay).delete(disable_relay))
        .route(&format!("{network}/nodes/{{node}}/exit"), put(enable_exit).delete(disable_exit))
        .route(&format!("{network}/nodes/{{node}}/succeed"), post(succeed))
        .route(&format!("{network}/nodes/{{node}}/routes"), get(routes))
        .route(&format!("{network}/nodes/{{node}}/access"), get(node_access))
        .route(&format!("{network}/tokens"), get(list_tokens).post(create_token))
        .route(&format!("{network}/tokens/{{id}}"), axum::routing::delete(delete_token))
        .route(&format!("{network}/rules"), get(list_rules).post(create_rule))
        .route(&format!("{network}/rules/{{id}}"), axum::routing::patch(update_rule).delete(delete_rule))
        .route(&format!("{network}/credentials"), get(list_credentials).post(create_credential))
        .route(&format!("{network}/credentials/{{id}}"), axum::routing::delete(delete_credential))
        .route(&format!("{network}/listeners"), get(list_listeners).post(create_listener))
        .route(&format!("{network}/listeners/{{id}}"), put(replace_listener).delete(delete_listener))
        .route(&format!("{network}/forwards"), get(list_forwards).post(create_forward))
        .route(&format!("{network}/forwards/{{id}}"), put(replace_forward).delete(delete_forward))
        .route(&format!("{network}/pairs"), get(list_pairs))
        .route(&format!("{network}/pairs/{{a}}/{{b}}"), put(set_pair).delete(delete_pair))
        .route(&format!("{network}/access/graph"), get(access_graph))
        .route(&format!("{network}/access/check"), get(access_check))
        .fallback(no_such_endpoint)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(Arc::clone(&controller), require_admin))
        .with_state(controller);

    let router = Router::new().nest(API_PREFIX, api);
    match static_dir {
        // 单页应用的前端路由：不是文件的路径都回 index.html。
        Some(dir) => router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")))),
        None => router.fallback(no_such_endpoint),
    }
}

/// 出错时的应答：HTTP 状态码加上统一格式的 JSON。
struct Failure {
    status: StatusCode,
    code: ErrorCode,
    message: String,
}

impl Failure {
    fn bad_request(message: String) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: ErrorCode::InvalidRequest, message }
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        let code = error.code();
        let status = match code {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::InvalidRequest | ErrorCode::AddressOutOfRange => StatusCode::BAD_REQUEST,
            ErrorCode::NameTaken
            | ErrorCode::NameCoolingDown
            | ErrorCode::AddressTaken
            | ErrorCode::AddressCoolingDown
            | ErrorCode::RangeOverlaps
            | ErrorCode::RangeExhausted
            | ErrorCode::RangeNotSuperset
            | ErrorCode::NetworkNotEmpty
            | ErrorCode::NotDeclared
            | ErrorCode::PredecessorOnline
            | ErrorCode::Conflict => StatusCode::CONFLICT,
            // `ErrorCode` 以后可能加新的值；这里没有专门对应的一律按内部错误报。
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!("管理请求失败：{}", error_chain(&error));
            // 内部错误的细节只进日志，不发给调用方。
            return Self { status, code: ErrorCode::Internal, message: "controller 内部出错，详情见它的日志".to_owned() };
        }
        tracing::info!(?code, reason = %error, "拒绝管理请求");
        Self { status, code, message: error.to_string() }
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: ApiError { code: self.code, message: self.message } };
        let mut response = (self.status, Json(body)).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, header::HeaderValue::from_static("Bearer"));
        }
        response
    }
}

/// 每个请求都要带对的管理令牌。
async fn require_admin(State(controller): State<App>, request: Request, next: Next) -> Response {
    let presented =
        request.headers().get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer "));
    let reason = match presented {
        Some(token) if controller.is_admin_token(token.trim()) => return next.run(request).await,
        Some(_) => "管理令牌不对",
        None => "请求没有带管理令牌（Authorization: Bearer <令牌>）",
    };
    let remote = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    tracing::warn!(?remote, path = %request.uri().path(), reason, "拒绝管理请求");
    Failure { status: StatusCode::UNAUTHORIZED, code: ErrorCode::Unauthorized, message: reason.to_owned() }.into_response()
}

async fn no_such_endpoint() -> Failure {
    Failure { status: StatusCode::NOT_FOUND, code: ErrorCode::NotFound, message: "没有这个接口".to_owned() }
}

async fn method_not_allowed() -> Failure {
    Failure {
        status: StatusCode::METHOD_NOT_ALLOWED, code: ErrorCode::InvalidRequest, message: "这个接口不支持这种请求方法".to_owned()
    }
}

/// 路径里的参数。解析不了时按统一的格式报错。
struct Path<T>(T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Path<T> {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection @ PathRejection::FailedToDeserializePathParams(_)) => {
                Err(Failure::bad_request(format!("路径里的编号不对：{}", rejection.body_text())))
            }
            Err(rejection) => Err(Failure::bad_request(rejection.body_text())),
        }
    }
}

/// 查询串里的参数。
struct Query<T>(T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Query<T> {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection @ QueryRejection::FailedToDeserializeQueryString(_)) => {
                Err(Failure::bad_request(format!("查询参数不对：{}", rejection.body_text())))
            }
            Err(rejection) => Err(Failure::bad_request(rejection.body_text())),
        }
    }
}

/// JSON 请求体。
struct Body<T>(T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = Failure;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => {
                let what = match &rejection {
                    JsonRejection::MissingJsonContentType(_) => "请求体要是 JSON（Content-Type: application/json）",
                    JsonRejection::JsonSyntaxError(_) => "请求体不是合法的 JSON",
                    JsonRejection::JsonDataError(_) => "请求体里的字段不对",
                    _ => "请求体读不出来",
                };
                Err(Failure::bad_request(format!("{what}：{}", rejection.body_text())))
            }
        }
    }
}

/// 调用 controller，把结果写成 JSON 应答。
async fn respond<T: Serialize + Send + 'static>(
    controller: &App,
    status: StatusCode,
    call: impl FnOnce(&Controller) -> Result<T, Error> + Send + 'static,
) -> Reply {
    let value = blocking(controller, call).await?;
    Ok((status, Json(value)).into_response())
}

async fn ok<T: Serialize + Send + 'static>(controller: &App, call: impl FnOnce(&Controller) -> Result<T, Error> + Send + 'static) -> Reply {
    respond(controller, StatusCode::OK, call).await
}

async fn created<T: Serialize + Send + 'static>(
    controller: &App,
    call: impl FnOnce(&Controller) -> Result<T, Error> + Send + 'static,
) -> Reply {
    respond(controller, StatusCode::CREATED, call).await
}

/// 删除成功没有应答体。
async fn deleted(controller: &App, call: impl FnOnce(&Controller) -> Result<(), Error> + Send + 'static) -> Reply {
    blocking(controller, call).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct ListenerQuery {
    node: Option<NodeId>,
}

#[derive(Deserialize)]
struct CheckQuery {
    from: NodeId,
    to: NodeId,
}

async fn status(State(app): State<App>) -> Reply {
    ok(&app, Controller::status).await
}

async fn log(State(app): State<App>, Query(query): Query<LogQuery>) -> Reply {
    let limit = query.limit.unwrap_or(DEFAULT_LOG_LIMIT).min(MAX_LOG_LIMIT);
    ok(&app, move |controller| controller.oplog(limit)).await
}

async fn list_networks(State(app): State<App>) -> Reply {
    ok(&app, Controller::networks).await
}

async fn create_network(State(app): State<App>, Body(request): Body<CreateNetwork>) -> Reply {
    created(&app, move |controller| controller.create_network(&request)).await
}

async fn show_network(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.network(network)).await
}

async fn update_network(State(app): State<App>, Path(network): Path<NetworkId>, Body(request): Body<UpdateNetwork>) -> Reply {
    ok(&app, move |controller| controller.update_network(network, &request)).await
}

async fn delete_network(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    deleted(&app, move |controller| controller.delete_network(network)).await
}

async fn overview(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.overview(network)).await
}

async fn list_nodes(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.nodes(network)).await
}

async fn show_node(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>) -> Reply {
    ok(&app, move |controller| controller.node(network, node)).await
}

async fn update_node(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>, Body(request): Body<UpdateNode>) -> Reply {
    ok(&app, move |controller| controller.update_node(network, node, &request)).await
}

async fn delete_node(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>) -> Reply {
    deleted(&app, move |controller| controller.delete_node(network, node)).await
}

async fn enable_relay(
    State(app): State<App>,
    Path((network, node)): Path<(NetworkId, NodeId)>,
    Body(settings): Body<RelaySettings>,
) -> Reply {
    ok(&app, move |controller| controller.enable_relay(network, node, settings)).await
}

async fn disable_relay(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>) -> Reply {
    ok(&app, move |controller| controller.disable_relay(network, node)).await
}

async fn enable_exit(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>, Body(settings): Body<ExitRole>) -> Reply {
    ok(&app, move |controller| controller.enable_exit(network, node, settings)).await
}

async fn disable_exit(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>) -> Reply {
    ok(&app, move |controller| controller.disable_exit(network, node)).await
}

async fn succeed(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>, Body(request): Body<Succeed>) -> Reply {
    ok(&app, move |controller| controller.succeed(network, node, request.predecessor)).await
}

async fn routes(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>) -> Reply {
    ok(&app, move |controller| controller.routes(network, node)).await
}

async fn node_access(State(app): State<App>, Path((network, node)): Path<(NetworkId, NodeId)>) -> Reply {
    ok(&app, move |controller| controller.node_access(network, node)).await
}

async fn list_tokens(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.tokens(network)).await
}

async fn create_token(State(app): State<App>, Path(network): Path<NetworkId>, Body(request): Body<CreateEnrollToken>) -> Reply {
    created(&app, move |controller| controller.create_token(network, &request)).await
}

async fn delete_token(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>) -> Reply {
    deleted(&app, move |controller| controller.delete_token(network, id)).await
}

async fn list_rules(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.rules(network)).await
}

async fn create_rule(State(app): State<App>, Path(network): Path<NetworkId>, Body(request): Body<CreateRule>) -> Reply {
    created(&app, move |controller| controller.create_rule(network, &request)).await
}

async fn update_rule(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>, Body(request): Body<UpdateRule>) -> Reply {
    ok(&app, move |controller| controller.update_rule(network, id, &request)).await
}

async fn delete_rule(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>) -> Reply {
    deleted(&app, move |controller| controller.delete_rule(network, id)).await
}

async fn list_credentials(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.credentials(network)).await
}

async fn create_credential(State(app): State<App>, Path(network): Path<NetworkId>, Body(request): Body<CreateCredential>) -> Reply {
    created(&app, move |controller| controller.create_credential(network, &request)).await
}

async fn delete_credential(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>) -> Reply {
    deleted(&app, move |controller| controller.delete_credential(network, id)).await
}

async fn list_listeners(State(app): State<App>, Path(network): Path<NetworkId>, Query(query): Query<ListenerQuery>) -> Reply {
    ok(&app, move |controller| controller.listeners(network, query.node)).await
}

async fn create_listener(State(app): State<App>, Path(network): Path<NetworkId>, Body(input): Body<ListenerInput>) -> Reply {
    created(&app, move |controller| controller.create_listener(network, input)).await
}

async fn replace_listener(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>, Body(input): Body<ListenerInput>) -> Reply {
    ok(&app, move |controller| controller.replace_listener(network, id, input)).await
}

async fn delete_listener(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>) -> Reply {
    deleted(&app, move |controller| controller.delete_listener(network, id)).await
}

async fn list_forwards(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.forwards(network)).await
}

async fn create_forward(State(app): State<App>, Path(network): Path<NetworkId>, Body(input): Body<ForwardInput>) -> Reply {
    created(&app, move |controller| controller.create_forward(network, input)).await
}

async fn replace_forward(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>, Body(input): Body<ForwardInput>) -> Reply {
    ok(&app, move |controller| controller.replace_forward(network, id, input)).await
}

async fn delete_forward(State(app): State<App>, Path((network, id)): Path<(NetworkId, u32)>) -> Reply {
    deleted(&app, move |controller| controller.delete_forward(network, id)).await
}

async fn list_pairs(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.pairs(network)).await
}

async fn set_pair(State(app): State<App>, Path((network, a, b)): Path<(NetworkId, NodeId, NodeId)>, Body(input): Body<PairInput>) -> Reply {
    ok(&app, move |controller| controller.set_pair(network, a, b, input)).await
}

async fn delete_pair(State(app): State<App>, Path((network, a, b)): Path<(NetworkId, NodeId, NodeId)>) -> Reply {
    deleted(&app, move |controller| controller.delete_pair(network, a, b)).await
}

async fn access_graph(State(app): State<App>, Path(network): Path<NetworkId>) -> Reply {
    ok(&app, move |controller| controller.access_graph(network)).await
}

async fn access_check(State(app): State<App>, Path(network): Path<NetworkId>, Query(query): Query<CheckQuery>) -> Reply {
    ok(&app, move |controller| controller.access_check(network, query.from, query.to)).await
}
