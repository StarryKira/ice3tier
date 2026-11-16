//! 管理 API 的测试：请求直接交给路由，不经过网络端口。

// clippy.toml 里的 allow-unwrap-in-tests 只认 `#[test]` 函数本身，认不出集成测试里的辅助函数。
#![allow(clippy::unwrap_used, reason = "整个文件都是测试代码，出错就该直接失败")]

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use i3t_controller::store::Store;
use i3t_controller::webui::{Asset, WebUi};
use i3t_controller::{Admitted, Controller, Timing, admin};
use i3t_crypto::NodeKey;
use i3t_model::admin as model;
use i3t_model::control::{Declared, EnrollToken, Hello, Report, Route, RouteEntry, SelfUpdate, ToController, ToNode};
use i3t_model::update::{FailureKind, UpdateStatus, UpdateTiming};
use i3t_proto::{NodeId, PublicKey};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tower::ServiceExt;

use crate::common::{Publisher, modern_hello};

const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

struct Api {
    router: Router,
    controller: Arc<Controller>,
    token: String,
    next_key: u8,
}

/// 一个假装在线的节点。收件箱要留着，丢了 controller 会当它断线。
struct Member {
    id: NodeId,
    key: PublicKey,
    session: Admitted,
    _inbox: mpsc::Receiver<ToNode>,
}

impl Api {
    fn new() -> Self {
        Self::with_webui(&WebUi::NotIncluded)
    }

    fn with_webui(webui: &WebUi) -> Self {
        let store = Store::open_in_memory().unwrap();
        let controller =
            Controller::new(store, NodeKey::generate().unwrap(), "ctl.example.com:20331".to_owned(), Timing::default()).unwrap();
        Self::around(controller, webui)
    }

    fn around(controller: Controller, webui: &WebUi) -> Self {
        let token = controller.issue_admin_token().unwrap();
        let controller = Arc::new(controller);
        Self { router: admin::router(Arc::clone(&controller), webui), controller, token, next_key: 0 }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (parts.status, parts.headers, bytes.to_vec())
    }

    fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Request<Body> {
        let builder =
            Request::builder().method(method).uri(format!("/api/v1{path}")).header(header::AUTHORIZATION, format!("Bearer {}", self.token));
        match body {
            Some(body) => builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        }
    }

    /// 带着管理令牌发一个请求，返回状态码和 JSON 应答体（没有应答体时是 `null`）。
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, _, bytes) = self.send(self.request(method, path, body.as_ref())).await;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| panic!("应答体不是 JSON：{}", String::from_utf8_lossy(&bytes)))
        };
        (status, value)
    }

    /// 期望成功，并把应答体读成 `T`。
    async fn ok<T: DeserializeOwned>(&self, method: Method, path: &str, body: Option<Value>) -> T {
        let (status, value) = self.call(method.clone(), path, body).await;
        assert!(status.is_success(), "{method} {path} 应该成功，实际是 {status}：{value}");
        serde_json::from_value(value.clone()).unwrap_or_else(|err| panic!("{method} {path} 的应答不符合模型：{err}\n{value}"))
    }

    /// 期望失败，返回错误码和说明。
    async fn fails(&self, method: Method, path: &str, body: Option<Value>, expected: StatusCode) -> model::ApiError {
        let (status, value) = self.call(method.clone(), path, body).await;
        assert_eq!(status, expected, "{method} {path}：{value}");
        let body: model::ErrorBody =
            serde_json::from_value(value.clone()).unwrap_or_else(|err| panic!("错误的应答不符合统一格式：{err}\n{value}"));
        assert!(!body.error.message.is_empty());
        body.error
    }

    async fn network(&self, name: &str) -> model::Network {
        self.ok(Method::POST, "/networks", Some(json!({ "name": name }))).await
    }

    /// 经 API 生成注册令牌，再让一个节点用它加入并保持在线。
    async fn join(&mut self, network: model::NetworkId, declared: Declared) -> Member {
        self.join_as(network, &hello(declared)).await
    }

    /// 同上，节点带着指定的自我介绍。懂更新的节点顺带做第一次上报：controller 等它到了才会给节点下发更新指令。
    async fn join_as(&mut self, network: model::NetworkId, hello: &Hello) -> Member {
        let created: model::CreatedEnrollToken = self.ok(Method::POST, &format!("/networks/{network}/tokens"), Some(json!({}))).await;
        let secret = created.token.parse::<EnrollToken>().unwrap().secret;
        self.next_key += 1;
        let key = PublicKey::from_bytes([self.next_key; PublicKey::LEN]);
        let (outbox, inbox) = mpsc::channel(64);
        let session = self.controller.enroll(key, &secret, hello, REMOTE, outbox).unwrap();
        if hello.protocol > 0 {
            let report = Report { update: Some(UpdateStatus::Idle), ..Report::default() };
            self.controller.handle(session, REMOTE, ToController::Report(report)).unwrap();
        }
        Member { id: session.node, key, session, _inbox: inbox }
    }
}

fn hello(declared: Declared) -> Hello {
    Hello { version: "0.1.0".to_owned(), platform: "linux-x86_64".to_owned(), declared, port: 20330, ..Hello::default() }
}

const CAN_DO_ALL: Declared = Declared { exit: true, relay: true, forward: true };

#[tokio::test]
async fn requests_without_the_admin_token_are_refused() {
    let api = Api::new();
    let paths = ["/api/v1/status", "/api/v1/networks", "/api/v1/networks/1/nodes", "/api/v1/no-such-endpoint"];
    let attempts: [Option<String>; 5] = [
        None,
        Some("Bearer wrong-token".to_owned()),
        Some(format!("Basic {}", api.token)),
        Some(api.token.clone()),
        Some("Bearer ".to_owned()),
    ];
    for path in paths {
        for attempt in &attempts {
            let mut builder = Request::builder().uri(path);
            if let Some(value) = attempt {
                builder = builder.header(header::AUTHORIZATION, value);
            }
            let (status, headers, bytes) = api.send(builder.body(Body::empty()).unwrap()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} {attempt:?}");
            assert_eq!(headers.get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");
            let body: model::ErrorBody = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body.error.code, model::ErrorCode::Unauthorized);
        }
    }
    // 改动类的请求同样被挡住，什么都没发生。
    let request = Request::builder().method(Method::POST).uri("/api/v1/networks").header(header::CONTENT_TYPE, "application/json");
    let (status, _, _) = api.send(request.body(Body::from(json!({ "name": "home" }).to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(api.controller.networks().unwrap().is_empty());

    let status: model::Status = api.ok(Method::GET, "/status", None).await;
    assert_eq!(
        (status.controller_key, status.public_addr.as_str(), status.networks),
        (api.controller.public_key(), "ctl.example.com:20331", 0)
    );

    // 换了令牌之后旧的立刻失效。
    let fresh = api.controller.issue_admin_token().unwrap();
    api.fails(Method::GET, "/status", None, StatusCode::UNAUTHORIZED).await;
    let renewed = Api { token: fresh, router: api.router.clone(), controller: Arc::clone(&api.controller), next_key: 0 };
    renewed.ok::<model::Status>(Method::GET, "/status", None).await;
}

#[tokio::test]
async fn networks_can_be_created_listed_changed_and_deleted() {
    let api = Api::new();
    let home = api.network("home").await;
    assert_eq!((home.name.as_str(), home.domain.as_str(), home.nodes), ("home", "home.i3t", 0));
    assert_eq!(home.cidr, "10.66.0.0/16".parse().unwrap());
    let explicit = json!({ "name": "lab", "cidr": "10.99.3.4/24", "routing": "lowest_cost", "max_relay_hops": 3, "reach": "direct_only" });
    let lab: model::Network = api.ok(Method::POST, "/networks", Some(explicit)).await;
    assert_eq!(lab.cidr, "10.99.3.0/24".parse().unwrap());
    assert_eq!((lab.max_relay_hops, serde_json::to_value(lab.routing).unwrap()), (3, json!("lowest_cost")));

    let listed: Vec<model::Network> = api.ok(Method::GET, "/networks", None).await;
    assert_eq!(listed, vec![home.clone(), lab.clone()]);
    let shown: model::Network = api.ok(Method::GET, &format!("/networks/{}", lab.id), None).await;
    assert_eq!(shown, lab);

    let changed: model::Network =
        api.ok(Method::PATCH, &format!("/networks/{}", lab.id), Some(json!({ "reach": "any", "cidr": "10.99.0.0/16" }))).await;
    assert_eq!((changed.cidr, serde_json::to_value(changed.reach).unwrap()), ("10.99.0.0/16".parse().unwrap(), json!("any")));
    assert_eq!(changed.max_relay_hops, 3, "没写的字段不变");

    let overlap = api.fails(Method::POST, "/networks", Some(json!({ "name": "x", "cidr": "10.66.1.0/24" })), StatusCode::CONFLICT).await;
    assert_eq!(overlap.code, model::ErrorCode::RangeOverlaps);
    let taken = api.fails(Method::POST, "/networks", Some(json!({ "name": "home" })), StatusCode::CONFLICT).await;
    assert_eq!(taken.code, model::ErrorCode::NameTaken);
    let invalid = api.fails(Method::POST, "/networks", Some(json!({ "name": "Not Valid" })), StatusCode::BAD_REQUEST).await;
    assert_eq!(invalid.code, model::ErrorCode::InvalidRequest);
    let hops =
        api.fails(Method::PATCH, &format!("/networks/{}", lab.id), Some(json!({ "max_relay_hops": 9 })), StatusCode::BAD_REQUEST).await;
    assert_eq!(hops.code, model::ErrorCode::InvalidRequest);

    let (status, body) = api.call(Method::DELETE, &format!("/networks/{}", lab.id), None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    api.fails(Method::GET, &format!("/networks/{}", lab.id), None, StatusCode::NOT_FOUND).await;
    api.fails(Method::DELETE, &format!("/networks/{}", lab.id), None, StatusCode::NOT_FOUND).await;
}

#[tokio::test]
async fn enrolment_tokens_show_their_text_only_once() {
    let api = Api::new();
    let network = api.network("home").await;
    let path = format!("/networks/{}/tokens", network.id);
    let request = json!({ "expires_in_secs": 600, "tags": ["Servers"], "virtual_ip": "10.66.0.9" });
    let (status, created) = api.call(Method::POST, &path, Some(request)).await;
    assert_eq!(status, StatusCode::CREATED);
    let text = created["token"].as_str().unwrap();
    let token: EnrollToken = text.parse().unwrap();
    assert_eq!((token.controller.as_str(), token.key), ("ctl.example.com:20331", api.controller.public_key()));
    assert_eq!(created["tags"], json!(["servers"]));
    assert_eq!(created["virtual_ip"], json!("10.66.0.9"));
    assert_eq!(created["expires_at"].as_u64().unwrap() - created["created_at"].as_u64().unwrap(), 600);

    let (_, listed) = api.call(Method::GET, &path, None).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], created["id"]);
    assert!(listed[0].get("token").is_none(), "令牌文本只在创建的应答里出现");
    assert!(!listed.to_string().contains(&token.secret));

    let unknown_node = api.fails(Method::POST, &path, Some(json!({ "successor_of": 99 })), StatusCode::NOT_FOUND).await;
    assert_eq!(unknown_node.code, model::ErrorCode::NotFound);
    let (status, _) = api.call(Method::DELETE, &format!("{path}/{}", created["id"]), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let listed: Vec<model::EnrollTokenInfo> = api.ok(Method::GET, &path, None).await;
    assert!(listed.is_empty());
}

#[tokio::test]
async fn a_node_can_be_inspected_changed_and_deleted() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let base = format!("/networks/{}/nodes", network.id);
    let node = api.join(network.id, Declared::default()).await;
    let relay = api.join(network.id, CAN_DO_ALL).await;
    let path = format!("{base}/{}", node.id);

    let listed: Vec<model::Node> = api.ok(Method::GET, &base, None).await;
    assert_eq!(listed.iter().map(|node| node.id).collect::<Vec<_>>(), vec![node.id, relay.id]);
    assert!(listed[0].online && listed[0].last_seen_at.is_some());
    assert_eq!((listed[0].key, listed[0].virtual_ip), (node.key, "10.66.0.1".parse::<Ipv4Addr>().unwrap()));
    assert_eq!(listed[0].fqdn, format!("{}.home.i3t", listed[0].name));
    assert_eq!((listed[0].netmap_version, listed[0].sent_version, listed[0].acked_version), (2, 2, 0));

    let report = Report {
        endpoints: vec!["192.168.1.5:20330".parse().unwrap()],
        flows: 2,
        tx_bytes: 10,
        rx_bytes: 20,
        routes: vec![RouteEntry { peer: relay.id, route: Route::Relay { relays: vec![relay.id] }, rtt_ms: Some(30) }],
        ..Report::default()
    };
    api.controller.handle(node.session, REMOTE, ToController::Report(report)).unwrap();
    let detail: model::NodeDetail = api.ok(Method::GET, &path, None).await;
    assert_eq!(detail.node.id, node.id);
    assert_eq!(detail.endpoints, vec!["192.168.1.5:20330".parse().unwrap()]);
    assert_eq!(detail.observed_endpoint, Some("198.51.100.7:20330".parse().unwrap()));
    assert_eq!((detail.flows, detail.tx_bytes, detail.rx_bytes), (2, 10, 20));
    let (_, raw) = api.call(Method::GET, &path, None).await;
    assert!(raw["name"].is_string() && raw["endpoints"].is_array(), "详情里列表的字段和上报的字段在同一层：{raw}");
    let table: model::RouteTable = api.ok(Method::GET, &format!("{path}/routes"), None).await;
    assert!(table.online);
    assert_eq!((table.routes[0].peer.id, table.routes[0].rtt_ms), (relay.id, Some(30)));
    assert_eq!(serde_json::to_value(&table.routes[0].route).unwrap(), json!({ "via": "relay", "relays": [relay.id] }));

    // 改名、打标签、开关；null 表示清掉覆盖值。
    let patch =
        json!({ "name": "Laptop", "tags": ["laptops", "all"], "dial_out_only": true, "routing": "lowest_cost", "reach": "relay_only" });
    let changed: model::Node = api.ok(Method::PATCH, &path, Some(patch)).await;
    assert_eq!((changed.name.as_str(), changed.fqdn.as_str()), ("laptop", "laptop.home.i3t"));
    assert_eq!(changed.tags, vec!["laptops".to_owned()]);
    assert!(changed.dial_out_only && changed.routing.is_some() && changed.reach.is_some());
    let cleared: model::Node = api.ok(Method::PATCH, &path, Some(json!({ "routing": null }))).await;
    assert!(cleared.routing.is_none() && cleared.reach.is_some() && cleared.name == "laptop");
    let moved: model::Node = api.ok(Method::PATCH, &path, Some(json!({ "virtual_ip": "10.66.9.9" }))).await;
    assert_eq!(moved.virtual_ip, "10.66.9.9".parse::<Ipv4Addr>().unwrap());

    let taken = api.fails(Method::PATCH, &path, Some(json!({ "virtual_ip": "10.66.0.2" })), StatusCode::CONFLICT).await;
    assert_eq!(taken.code, model::ErrorCode::AddressTaken);
    let outside = api.fails(Method::PATCH, &path, Some(json!({ "virtual_ip": "10.67.0.1" })), StatusCode::BAD_REQUEST).await;
    assert_eq!(outside.code, model::ErrorCode::AddressOutOfRange);

    // 没声明的能力启用不了，错误里写着要在节点上执行的命令。
    let refused = api.fails(Method::PUT, &format!("{path}/exit"), Some(json!({})), StatusCode::CONFLICT).await;
    assert_eq!(refused.code, model::ErrorCode::NotDeclared);
    assert!(refused.message.contains("i3t set can-exit on") && refused.message.contains("laptop"), "{}", refused.message);
    let refused =
        api.fails(Method::PUT, &format!("{path}/relay"), Some(json!({ "udp": ["203.0.113.7:20330"] })), StatusCode::CONFLICT).await;
    assert!(refused.message.contains("i3t set can-relay on"), "{}", refused.message);

    let relay_path = format!("{base}/{}", relay.id);
    let no_address = api.fails(Method::PUT, &format!("{relay_path}/relay"), Some(json!({})), StatusCode::BAD_REQUEST).await;
    assert_eq!(no_address.code, model::ErrorCode::InvalidRequest);
    let settings = json!({ "udp": ["203.0.113.7:20330"], "tcp": ["203.0.113.7:20330"], "region": "hk", "bandwidth_mbps": 200 });
    let enabled: model::Node = api.ok(Method::PUT, &format!("{relay_path}/relay"), Some(settings.clone())).await;
    assert_eq!(serde_json::to_value(enabled.relay.unwrap()).unwrap(), settings);
    let exit: model::Node = api.ok(Method::PUT, &format!("{relay_path}/exit"), Some(json!({ "lan": true }))).await;
    assert_eq!(
        serde_json::to_value(exit.exit.unwrap()).unwrap(),
        json!({ "ports": [80, 443], "lan": true, "bandwidth_mbps": null, "upstream_socks5": null })
    );
    let pinned: model::Node = api.ok(Method::PATCH, &path, Some(json!({ "pinned_relays": [relay.id] }))).await;
    assert_eq!(pinned.pinned_relays, vec![relay.id]);
    let disabled: model::Node = api.ok(Method::DELETE, &format!("{relay_path}/exit"), None).await;
    assert!(disabled.exit.is_none() && disabled.relay.is_some());
    let disabled: model::Node = api.ok(Method::DELETE, &format!("{relay_path}/relay"), None).await;
    assert!(disabled.relay.is_none());

    let access: model::NodeAccess = api.ok(Method::GET, &format!("{path}/access"), None).await;
    assert_eq!(access.can_reach.iter().map(|edge| edge.node.id).collect::<Vec<_>>(), vec![relay.id]);
    assert_eq!(access.reachable_by[0].rules.len(), 1);

    // 继任：在线的节点不能被继任。
    let online = api.fails(Method::POST, &format!("{path}/succeed"), Some(json!({ "predecessor": relay.id })), StatusCode::CONFLICT).await;
    assert_eq!(online.code, model::ErrorCode::PredecessorOnline);
    api.controller.disconnect(relay.session);
    let successor: model::Node = api.ok(Method::POST, &format!("{path}/succeed"), Some(json!({ "predecessor": relay.id }))).await;
    assert_eq!((successor.id, successor.virtual_ip), (node.id, "10.66.0.2".parse::<Ipv4Addr>().unwrap()));
    api.fails(Method::GET, &relay_path, None, StatusCode::NOT_FOUND).await;

    let (status, body) = api.call(Method::DELETE, &path, None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    api.fails(Method::GET, &path, None, StatusCode::NOT_FOUND).await;
    api.fails(Method::DELETE, &path, None, StatusCode::NOT_FOUND).await;
    let listed: Vec<model::Node> = api.ok(Method::GET, &base, None).await;
    assert!(listed.is_empty());
}

#[tokio::test]
async fn rules_and_the_access_views_follow_each_other() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let base = format!("/networks/{}", network.id);
    let laptop = api.join(network.id, Declared::default()).await;
    let server = api.join(network.id, Declared::default()).await;
    let loner = api.join(network.id, Declared::default()).await;
    for (member, tag) in [(&laptop, "laptops"), (&server, "servers")] {
        api.ok::<model::Node>(Method::PATCH, &format!("{base}/nodes/{}", member.id), Some(json!({ "tags": [tag] }))).await;
    }

    let rules: Vec<model::Rule> = api.ok(Method::GET, &format!("{base}/rules"), None).await;
    assert_eq!(rules.len(), 1);
    assert_eq!(
        serde_json::to_value(&rules[0]).unwrap(),
        json!({ "id": rules[0].id, "name": "default", "src": "all", "dst": "all", "direction": "both", "enabled": true })
    );
    let default_rule = format!("{base}/rules/{}", rules[0].id);
    let disabled: model::Rule = api.ok(Method::PATCH, &default_rule, Some(json!({ "enabled": false }))).await;
    assert!(!disabled.enabled && disabled.src == "all");

    let (status, created) = api
        .call(
            Method::POST,
            &format!("{base}/rules"),
            Some(json!({ "src": "Laptops", "dst": "servers", "direction": "one_way", "name": "ssh" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        (created["src"].clone(), created["direction"].clone(), created["enabled"].clone()),
        (json!("laptops"), json!("one_way"), json!(true))
    );
    let rule_id = created["id"].as_u64().unwrap();
    let bad_tag =
        api.fails(Method::POST, &format!("{base}/rules"), Some(json!({ "src": "a b", "dst": "x" })), StatusCode::BAD_REQUEST).await;
    assert_eq!(bad_tag.code, model::ErrorCode::InvalidRequest);

    let check = |from: NodeId, to: NodeId| format!("{base}/access/check?from={from}&to={to}");
    let allowed: model::AccessCheck = api.ok(Method::GET, &check(laptop.id, server.id), None).await;
    assert!(allowed.allowed);
    assert_eq!(allowed.rules.iter().map(|rule| u64::from(rule.id)).collect::<Vec<_>>(), vec![rule_id]);
    assert_eq!((allowed.from.id, allowed.to.id), (laptop.id, server.id));
    let denied: model::AccessCheck = api.ok(Method::GET, &check(server.id, laptop.id), None).await;
    assert!(!denied.allowed && denied.rules.is_empty());
    api.fails(Method::GET, &check(laptop.id, NodeId::new(99).unwrap()), None, StatusCode::NOT_FOUND).await;
    api.fails(Method::GET, &format!("{base}/access/check?from={}", laptop.id), None, StatusCode::BAD_REQUEST).await;

    let graph: model::AccessGraph = api.ok(Method::GET, &format!("{base}/access/graph"), None).await;
    let members = |tag: &str| {
        graph.tags.iter().find(|entry| entry.tag == tag).map(|entry| entry.nodes.iter().map(|node| node.id).collect::<Vec<_>>())
    };
    assert_eq!(graph.tags.iter().map(|entry| entry.tag.as_str()).collect::<Vec<_>>(), vec!["all", "laptops", "servers"]);
    assert_eq!(members("all"), Some(vec![laptop.id, server.id, loner.id]));
    assert_eq!(members("laptops"), Some(vec![laptop.id]));
    assert_eq!(graph.rules.len(), 2, "停用的规则也在图里（画成虚线）");
    assert_eq!(graph.unconnected.iter().map(|node| node.id).collect::<Vec<_>>(), vec![loner.id]);
    assert!(!graph.unacknowledged.is_empty(), "这些节点从来没有确认过 netmap");

    let view: model::NodeAccess = api.ok(Method::GET, &format!("{base}/nodes/{}/access", server.id), None).await;
    assert!(view.can_reach.is_empty() && !view.acknowledged);
    assert_eq!(view.reachable_by.len(), 1);
    assert_eq!((view.reachable_by[0].node.id, view.reachable_by[0].rules.clone()), (laptop.id, vec![u32::try_from(rule_id).unwrap()]));

    let widened: model::Rule = api.ok(Method::PATCH, &format!("{base}/rules/{rule_id}"), Some(json!({ "direction": "both" }))).await;
    assert_eq!(serde_json::to_value(widened.direction).unwrap(), json!("both"));
    let now_allowed: model::AccessCheck = api.ok(Method::GET, &check(server.id, laptop.id), None).await;
    assert!(now_allowed.allowed);
    let (status, _) = api.call(Method::DELETE, &format!("{base}/rules/{rule_id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    api.fails(Method::PATCH, &format!("{base}/rules/{rule_id}"), Some(json!({})), StatusCode::NOT_FOUND).await;
    let gone: model::AccessCheck = api.ok(Method::GET, &check(laptop.id, server.id), None).await;
    assert!(!gone.allowed);
}

#[tokio::test]
async fn credentials_listeners_forwards_and_pairs_round_trip() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let base = format!("/networks/{}", network.id);
    let gateway = api.join(network.id, Declared::default()).await;
    let target = api.join(network.id, CAN_DO_ALL).await;

    let (status, created) =
        api.call(Method::POST, &format!("{base}/credentials"), Some(json!({ "name": "phone", "max_concurrent": 8 }))).await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().unwrap().to_owned();
    assert_eq!((created["name"].clone(), created["max_concurrent"].clone()), (json!("phone"), json!(8)));
    let (_, listed) = api.call(Method::GET, &format!("{base}/credentials"), None).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0].get("token").is_none() && !listed.to_string().contains(&secret), "凭据的令牌查不回来");
    let duplicate = api.fails(Method::POST, &format!("{base}/credentials"), Some(json!({ "name": "phone" })), StatusCode::CONFLICT).await;
    assert_eq!(duplicate.code, model::ErrorCode::NameTaken);

    let listener = json!({
        "node": gateway.id, "name": "lan", "bind": "192.168.1.5:1080", "protocol": "socks5", "allow_from": ["192.168.1.0/24"],
        "require_credential": true, "default_action": { "action": "exit", "node": target.id }, "fixed_exit": null
    });
    let (status, created_listener) = api.call(Method::POST, &format!("{base}/listeners"), Some(listener.clone())).await;
    assert_eq!(status, StatusCode::CREATED);
    let listener_id = created_listener["id"].as_u64().unwrap();
    let mut expected = listener.clone();
    expected["id"] = json!(listener_id);
    assert_eq!(created_listener, expected, "监听器的应答就是请求的字段加上 id");
    let open = json!({ "node": gateway.id, "name": "open", "bind": "0.0.0.0:8080", "protocol": "http" });
    let refused = api.fails(Method::POST, &format!("{base}/listeners"), Some(open), StatusCode::BAD_REQUEST).await;
    assert!(refused.message.contains("require_credential"), "{}", refused.message);
    let local = json!({ "node": target.id, "name": "local", "bind": "127.0.0.1:1080", "protocol": "http" });
    api.ok::<model::Listener>(Method::POST, &format!("{base}/listeners"), Some(local)).await;
    let all: Vec<model::Listener> = api.ok(Method::GET, &format!("{base}/listeners"), None).await;
    assert_eq!(all.len(), 2);
    let on_gateway: Vec<model::Listener> = api.ok(Method::GET, &format!("{base}/listeners?node={}", gateway.id), None).await;
    assert_eq!(on_gateway.iter().map(|listener| u64::from(listener.id)).collect::<Vec<_>>(), vec![listener_id]);
    let mut replacement = listener.clone();
    replacement["default_action"] = json!({ "action": "reject" });
    replacement["bind"] = json!("192.168.1.5:1081");
    let replaced: model::Listener = api.ok(Method::PUT, &format!("{base}/listeners/{listener_id}"), Some(replacement)).await;
    assert_eq!(replaced.config.bind, "192.168.1.5:1081".parse().unwrap());
    assert_eq!(api.controller.netmap_of(gateway.id).unwrap().unwrap().credentials.len(), 1, "有监听器要求凭据的节点拿到凭据");
    assert!(api.controller.netmap_of(target.id).unwrap().unwrap().credentials.is_empty());

    let forward = json!({
        "name": "web", "protocol": "tcp", "listen_node": gateway.id, "bind": "127.0.0.1:8080",
        "target_node": target.id, "target_addr": "192.168.1.10:80", "allow_from": [], "enabled": true
    });
    let (status, created_forward) = api.call(Method::POST, &format!("{base}/forwards"), Some(forward.clone())).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created_forward["status"], json!("active"));
    let forward_id = created_forward["id"].as_u64().unwrap();
    let undeclared = json!({ "protocol": "udp", "listen_node": target.id, "bind": "127.0.0.1:5353", "target_node": gateway.id, "target_addr": "127.0.0.1:53" });
    let refused = api.fails(Method::POST, &format!("{base}/forwards"), Some(undeclared), StatusCode::CONFLICT).await;
    assert_eq!(refused.code, model::ErrorCode::NotDeclared);
    assert!(refused.message.contains("i3t set can-forward on"), "{}", refused.message);
    let mut paused = forward.clone();
    paused["enabled"] = json!(false);
    let replaced: model::Forward = api.ok(Method::PUT, &format!("{base}/forwards/{forward_id}"), Some(paused)).await;
    assert_eq!(serde_json::to_value(replaced.status).unwrap(), json!("disabled"));
    api.controller.disconnect(target.session);
    api.ok::<model::Forward>(Method::PUT, &format!("{base}/forwards/{forward_id}"), Some(forward)).await;
    let listed: Vec<model::Forward> = api.ok(Method::GET, &format!("{base}/forwards"), None).await;
    assert_eq!(serde_json::to_value(listed[0].status).unwrap(), json!("target_offline"));

    let pair_path = format!("{base}/pairs/{}/{}", target.id, gateway.id);
    let pair: model::PairSetting = api.ok(Method::PUT, &pair_path, Some(json!({ "keep_alive": true }))).await;
    assert_eq!((pair.a, pair.b, pair.keep_alive), (gateway.id, target.id, true), "编号小的在前");
    let pairs: Vec<model::PairSetting> = api.ok(Method::GET, &format!("{base}/pairs"), None).await;
    assert_eq!(pairs, vec![pair]);
    let same = api
        .fails(Method::PUT, &format!("{base}/pairs/{0}/{0}", gateway.id), Some(json!({ "keep_alive": true })), StatusCode::BAD_REQUEST)
        .await;
    assert_eq!(same.code, model::ErrorCode::InvalidRequest);

    for path in [
        pair_path,
        format!("{base}/forwards/{forward_id}"),
        format!("{base}/listeners/{listener_id}"),
        format!("{base}/credentials/{}", created["id"]),
    ] {
        let (status, body) = api.call(Method::DELETE, &path, None).await;
        assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null), "{path}");
        api.fails(Method::DELETE, &path, None, StatusCode::NOT_FOUND).await;
    }
}

#[tokio::test]
async fn the_overview_counts_things_and_the_log_records_changes() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let base = format!("/networks/{}", network.id);
    let a = api.join(network.id, CAN_DO_ALL).await;
    let b = api.join(network.id, Declared::default()).await;
    api.ok::<model::Node>(Method::PUT, &format!("{base}/nodes/{}/relay", a.id), Some(json!({ "udp": ["203.0.113.7:20330"] }))).await;
    api.call(Method::POST, &format!("{base}/tokens"), Some(json!({}))).await;
    let report = Report { routes: vec![RouteEntry { peer: a.id, route: Route::None, rtt_ms: None }], ..Report::default() };
    api.controller.handle(b.session, REMOTE, ToController::Report(report)).unwrap();

    let overview: model::Overview = api.ok(Method::GET, &format!("{base}/overview"), None).await;
    assert_eq!(
        (overview.nodes, overview.online, overview.relays, overview.exits, overview.rules, overview.pending_tokens),
        (2, 2, 1, 0, 1, 1)
    );
    assert_eq!(overview.online_nodes.iter().map(|node| node.id).collect::<Vec<_>>(), vec![a.id, b.id]);
    assert_eq!(overview.unreachable_pairs.iter().map(|pair| (pair.a.id, pair.b.id)).collect::<Vec<_>>(), vec![(a.id, b.id)]);
    api.fails(Method::GET, "/networks/99/overview", None, StatusCode::NOT_FOUND).await;

    let log: Vec<model::LogEntry> = api.ok(Method::GET, "/log", None).await;
    let actions: Vec<&str> = log.iter().map(|entry| entry.action.as_str()).collect();
    assert_eq!(actions[..2], ["token.create", "node.relay.enable"], "新的在前：{actions:?}");
    assert!(actions.contains(&"network.create") && actions.contains(&"node.enroll") && actions.contains(&"admin.token"));
    let limited: Vec<model::LogEntry> = api.ok(Method::GET, "/log?limit=2", None).await;
    assert_eq!(limited.len(), 2);
    assert!(limited[0].id > limited[1].id);
}

#[tokio::test]
async fn every_error_has_the_same_shape() {
    let api = Api::new();
    let network = api.network("home").await;

    let not_json =
        Request::builder().method(Method::POST).uri("/api/v1/networks").header(header::AUTHORIZATION, format!("Bearer {}", api.token));
    let (status, _, bytes) =
        api.send(not_json.header(header::CONTENT_TYPE, "application/json").body(Body::from("{ not json")).unwrap()).await;
    let body: model::ErrorBody = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((status, body.error.code), (StatusCode::BAD_REQUEST, model::ErrorCode::InvalidRequest));

    let no_content_type =
        Request::builder().method(Method::POST).uri("/api/v1/networks").header(header::AUTHORIZATION, format!("Bearer {}", api.token));
    let (status, _, bytes) = api.send(no_content_type.body(Body::from(json!({ "name": "x" }).to_string())).unwrap()).await;
    let body: model::ErrorBody = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((status, body.error.code), (StatusCode::BAD_REQUEST, model::ErrorCode::InvalidRequest));

    let wrong_type = api.fails(Method::POST, "/networks", Some(json!({ "name": 5 })), StatusCode::BAD_REQUEST).await;
    assert_eq!(wrong_type.code, model::ErrorCode::InvalidRequest);
    let missing_field =
        api.fails(Method::POST, &format!("/networks/{}/rules", network.id), Some(json!({ "src": "a" })), StatusCode::BAD_REQUEST).await;
    assert_eq!(missing_field.code, model::ErrorCode::InvalidRequest);
    let bad_id = api.fails(Method::GET, "/networks/home", None, StatusCode::BAD_REQUEST).await;
    assert_eq!(bad_id.code, model::ErrorCode::InvalidRequest);
    let zero_node = api.fails(Method::GET, &format!("/networks/{}/nodes/0", network.id), None, StatusCode::BAD_REQUEST).await;
    assert_eq!(zero_node.code, model::ErrorCode::InvalidRequest);
    let bad_query = api.fails(Method::GET, "/log?limit=many", None, StatusCode::BAD_REQUEST).await;
    assert_eq!(bad_query.code, model::ErrorCode::InvalidRequest);

    let unknown = api.fails(Method::GET, "/no-such-endpoint", None, StatusCode::NOT_FOUND).await;
    assert_eq!(unknown.code, model::ErrorCode::NotFound);
    let wrong_method = api.fails(Method::DELETE, "/networks", None, StatusCode::METHOD_NOT_ALLOWED).await;
    assert_eq!(wrong_method.code, model::ErrorCode::InvalidRequest);
    let missing = api.fails(Method::GET, &format!("/networks/{}/nodes/42", network.id), None, StatusCode::NOT_FOUND).await;
    assert_eq!(missing.code, model::ErrorCode::NotFound);
}

#[tokio::test]
async fn a_build_without_the_web_ui_explains_itself_instead_of_serving_404s() {
    let api = Api::new();
    let get = |path: &str| Request::builder().uri(path).body(Body::empty()).unwrap();
    for path in ["/", "/networks/1/nodes"] {
        let (status, headers, bytes) = api.send(get(path)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("text/html"));
        let page = String::from_utf8(bytes).unwrap();
        assert!(page.contains("没有带 WebUI") && page.contains("--static-dir"), "{page}");
    }
    // API 不受影响：照样要令牌，不存在的接口照样是统一格式的 404。
    let (status, _, _) = api.send(get("/api/v1/status")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let unknown = api.fails(Method::GET, "/no-such-endpoint", None, StatusCode::NOT_FOUND).await;
    assert_eq!(unknown.code, model::ErrorCode::NotFound);
}

#[tokio::test]
async fn the_embedded_web_ui_is_served_from_memory() {
    static ASSETS: &[Asset] = &[
        Asset { path: "index.html", bytes: b"<html>embedded</html>" },
        Asset { path: "assets/index-3f2a.js", bytes: b"console.log('ui')" },
        Asset { path: "favicon.svg", bytes: b"<svg/>" },
    ];
    let api = Api::with_webui(&WebUi::Embedded(ASSETS));
    let get = |path: &str| Request::builder().uri(path).body(Body::empty()).unwrap();
    let header_of = |headers: &axum::http::HeaderMap, name| headers.get(name).unwrap().to_str().unwrap().to_owned();

    let (status, headers, bytes) = api.send(get("/")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"<html>embedded</html>".as_slice()));
    assert!(header_of(&headers, header::CONTENT_TYPE).starts_with("text/html"));
    assert_eq!(header_of(&headers, header::CACHE_CONTROL), "no-cache", "index.html 每次都要问，这样换了版本马上看得到");

    let (status, headers, bytes) = api.send(get("/assets/index-3f2a.js")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"console.log('ui')".as_slice()));
    assert!(header_of(&headers, header::CONTENT_TYPE).starts_with("text/javascript"));
    assert!(header_of(&headers, header::CACHE_CONTROL).contains("immutable"), "名字里带哈希的文件可以一直缓存");
    let (_, headers, _) = api.send(get("/favicon.svg")).await;
    assert_eq!(header_of(&headers, header::CONTENT_TYPE), "image/svg+xml");

    // 前端自己的路由：不是文件的路径回 index.html。
    let (status, _, bytes) = api.send(get("/networks/1/nodes")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"<html>embedded</html>".as_slice()));
    let (status, _, _) = api.send(get("/api/v1/status")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "/api/v1 不会落到静态文件上");

    // 打进去的文件里没有 index.html，等于没有带。
    static BROKEN: &[Asset] = &[Asset { path: "assets/x.js", bytes: b"" }];
    let (status, _, bytes) = Api::with_webui(&WebUi::Embedded(BROKEN)).send(get("/")).await;
    assert!(status == StatusCode::OK && String::from_utf8(bytes).unwrap().contains("没有带 WebUI"));
}

#[tokio::test]
async fn metrics_need_no_token_and_name_no_node() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let _member = api.join(network.id, Declared::default()).await;
    let (status, headers, bytes) = api.send(Request::builder().uri("/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("text/plain; version=0.0.4"));
    let text = String::from_utf8(bytes).unwrap();
    for line in [
        "i3t_controller_nodes 1\n",
        "i3t_controller_nodes_online 1\n",
        "i3t_controller_networks 1\n",
        "# TYPE i3t_controller_snapshots_sent_total counter\n",
    ] {
        assert!(text.contains(line), "应该有 {line:?}：\n{text}");
    }
    assert!(text.contains("i3t_controller_snapshots_sent_total 1\n"), "{text}");
    assert!(!text.contains("node-1") && !text.contains("10.66."), "指标里不该有节点的名字和地址：\n{text}");
}

#[tokio::test]
async fn update_settings_checks_and_forced_updates_work_over_the_api() {
    let publisher = Publisher::new();
    let cache = tempfile::tempdir().unwrap();
    let mut api = Api::around(publisher.controller(Timing::default(), cache.path()), &WebUi::NotIncluded);
    let network = api.network("home").await;

    let overview: model::UpdateOverview = api.ok(Method::GET, "/update", None).await;
    assert!(overview.auto_update && !overview.source_is_default && overview.release.is_none() && overview.checked_at.is_none());
    assert_eq!(overview.source, publisher.source());
    let (_, raw) = api.call(Method::GET, "/update", None).await;
    for field in
        ["auto_update", "source", "source_is_default", "controller_version", "release", "checked_at", "check_error", "paused", "nodes"]
    {
        assert!(raw.get(field).is_some(), "应答里的字段总是全的，缺了 {field}：{raw}");
    }

    // 还没有发布：查不到是写在应答里的，不是请求失败。
    let checked: model::UpdateOverview = api.ok(Method::POST, "/update/check", Some(json!({}))).await;
    assert!(checked.release.is_none() && checked.check_error.is_some() && checked.checked_at.is_some());

    publisher.publish("0.2.0", &[("linux-x86_64", b"the new program")]);
    let checked: model::UpdateOverview = api.ok(Method::POST, "/update/check", Some(json!({}))).await;
    let release = checked.release.unwrap();
    assert_eq!((release.version.to_string().as_str(), release.packages.len(), checked.check_error), ("0.2.0", 1, None));
    assert_eq!(release.packages[0].state, model::PackageState::NotNeeded);
    let status: model::Status = api.ok(Method::GET, "/status", None).await;
    assert_eq!(status.newer_version.map(|version| version.to_string()), Some("0.2.0".to_owned()), "controller 自己也有新版本");

    let off: model::UpdateOverview = api.ok(Method::PATCH, "/update", Some(json!({ "auto_update": false }))).await;
    assert!(!off.auto_update && off.source == publisher.source(), "没写的字段不动");
    let bad = api.fails(Method::PATCH, "/update", Some(json!({ "source": "relative/dir" })), StatusCode::BAD_REQUEST).await;
    assert_eq!(bad.code, model::ErrorCode::InvalidRequest);

    // 节点的更新情况在节点列表、节点详情和单独的接口里都有。
    let member = api.join_as(network.id, &modern_hello("0.1.0", Declared::default())).await;
    let ancient = api.join(network.id, Declared::default()).await;
    let update_path = |node: NodeId| format!("/networks/{}/nodes/{node}/update", network.id);
    let nodes: Vec<model::Node> = api.ok(Method::GET, &format!("/networks/{}/nodes", network.id), None).await;
    assert!(nodes.iter().all(|node| node.update.available.as_ref().is_some_and(|version| version.to_string() == "0.2.0")));
    let update: model::NodeUpdate = api.ok(Method::GET, &update_path(member.id), None).await;
    assert_eq!((update.self_update, update.ordered, update.status), (Some(SelfUpdate::ABLE), None, UpdateStatus::Idle));
    let (_, raw) = api.call(Method::GET, &format!("/networks/{}/nodes/{}", network.id, member.id), None).await;
    assert_eq!(raw["update"]["self_update"], json!({ "able": true, "blocker": null }));
    assert_eq!(raw["update"]["status"], json!({ "state": "idle" }));
    assert_eq!(raw["hostname"], json!(""));

    // 强制更新：时机只能是 now 或 after_drain；不能自己更新的、不在线的各有各的错误码。
    let idle = api.fails(Method::POST, &update_path(member.id), Some(json!({ "timing": "idle" })), StatusCode::BAD_REQUEST).await;
    assert_eq!(idle.code, model::ErrorCode::InvalidRequest);
    let unable = api.fails(Method::POST, &update_path(ancient.id), Some(json!({ "timing": "now" })), StatusCode::CONFLICT).await;
    assert_eq!(unable.code, model::ErrorCode::CannotSelfUpdate);
    let forced: model::NodeUpdate = api.ok(Method::POST, &update_path(member.id), Some(json!({ "timing": "after_drain" }))).await;
    assert_eq!(forced.waiting, Some(model::UpdateWait::Package), "包还没有取回来：先排着");
    let controller = Arc::clone(&api.controller);
    tokio::task::spawn_blocking(move || controller.fetch_packages().unwrap()).await.unwrap();
    let update: model::NodeUpdate = api.ok(Method::GET, &update_path(member.id), None).await;
    assert_eq!(update.ordered.map(|order| order.timing), Some(UpdateTiming::AfterDrain));
    let all: model::ForcedUpdates = api.ok(Method::POST, "/update/force", Some(json!({ "timing": "now" }))).await;
    assert_eq!((all.ordered.len(), all.queued.len(), all.skipped.len()), (1, 0, 1));
    assert!(all.skipped[0].node.id == ancient.id && all.skipped[0].reason.contains("不能自己更新"));

    // 节点说这个版本不能用：暂停出现在总览里，继续之后消失；没有暂停的版本继续不了。
    let failed =
        UpdateStatus::Failed {
            version: Some("0.2.0".parse().unwrap()), kind: FailureKind::VersionRejected, reason: "自检没有通过".into()
        };
    api.controller.handle(member.session, REMOTE, ToController::Report(Report { update: Some(failed), ..Report::default() })).unwrap();
    let paused: model::UpdateOverview = api.ok(Method::GET, "/update", None).await;
    assert_eq!((paused.paused.len(), paused.nodes.failed, paused.nodes.manual), (1, 1, 1));
    let (_, raw) = api.call(Method::GET, "/update", None).await;
    assert_eq!(raw["paused"][0]["version"], json!("0.2.0"));
    assert_eq!(raw["paused"][0]["node"]["id"], json!(member.id.get()));
    let resumed: model::UpdateOverview = api.ok(Method::POST, "/update/resume", Some(json!({ "version": "0.2.0" }))).await;
    assert!(resumed.paused.is_empty());
    let nothing = api.fails(Method::POST, "/update/resume", Some(json!({ "version": "0.2.0" })), StatusCode::NOT_FOUND).await;
    assert_eq!(nothing.code, model::ErrorCode::NotFound);

    api.controller.disconnect(member.session);
    let offline = api.fails(Method::POST, &update_path(member.id), Some(json!({ "timing": "now" })), StatusCode::CONFLICT).await;
    assert_eq!(offline.code, model::ErrorCode::NodeOffline);
    let current = api.join_as(network.id, &modern_hello("0.2.0", Declared::default())).await;
    let none = api.fails(Method::POST, &update_path(current.id), Some(json!({ "timing": "now" })), StatusCode::CONFLICT).await;
    assert_eq!(none.code, model::ErrorCode::NoUpdate);

    let reset: model::UpdateOverview = api.ok(Method::PATCH, "/update", Some(json!({ "source": null }))).await;
    assert!(reset.source_is_default && reset.source.starts_with("https://github.com/"));
    let log: Vec<model::LogEntry> = api.ok(Method::GET, "/log", None).await;
    for action in ["update.settings", "update.release", "update.force", "update.force_all", "update.pause", "update.resume"] {
        assert!(log.iter().any(|entry| entry.action == action), "操作记录里应该有 {action}");
    }
}

#[tokio::test]
async fn the_route_matrix_and_the_path_between_two_nodes_are_served() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let a = api.join(network.id, Declared::default()).await;
    let b = api.join(network.id, Declared::default()).await;
    let routes = vec![RouteEntry { peer: b.id, route: Route::Relay { relays: vec![b.id] }, rtt_ms: Some(25) }];
    api.controller.handle(a.session, REMOTE, ToController::Report(Report { routes, routes_full: true, ..Report::default() })).unwrap();

    let (_, raw) = api.call(Method::GET, &format!("/networks/{}/routes", network.id), None).await;
    assert_eq!(raw["nodes"][0], json!({ "id": a.id.get(), "name": "node-1", "virtual_ip": "10.66.0.1", "online": true }));
    let entry = &raw["entries"][0];
    assert_eq!(
        (&entry["from"], &entry["to"], &entry["rtt_ms"], &entry["stale"]),
        (&json!(a.id.get()), &json!(b.id.get()), &json!(25), &json!(false))
    );
    assert_eq!(entry["route"], json!({ "via": "relay", "relays": [b.id.get()] }));
    assert!(entry["age_secs"].as_u64().is_some_and(|age| age <= 2));
    let matrix: model::RouteMatrix = api.ok(Method::GET, &format!("/networks/{}/routes", network.id), None).await;
    assert_eq!((matrix.nodes.len(), matrix.entries.len()), (2, 1));

    // 这两个节点是最初那一版的，问不了，两个方向都退回到上报的数据。
    let path = format!("/networks/{}/path?from={}&to={}", network.id, a.id, b.id);
    let view: model::PathView = api.ok(Method::GET, &path, None).await;
    assert_eq!((view.forward.from.id, view.forward.to.id, view.forward.source), (a.id, b.id, model::PathSource::Report));
    assert_eq!((view.forward.rtt_ms, view.forward.route.clone()), (Some(25), Some(Route::Relay { relays: vec![b.id] })));
    assert_eq!((view.reverse.from.id, view.reverse.source, view.reverse.route.clone()), (b.id, model::PathSource::None, None));
    let (_, raw) = api.call(Method::GET, &path, None).await;
    assert_eq!(
        raw["reverse"],
        json!({
            "from": { "id": b.id.get(), "name": "node-2", "virtual_ip": "10.66.0.2" },
            "to": { "id": a.id.get(), "name": "node-1", "virtual_ip": "10.66.0.1" },
            "source": "none", "route": null, "rtt_ms": null, "age_secs": null, "since": null
        })
    );

    let same =
        api.fails(Method::GET, &format!("/networks/{}/path?from={}&to={}", network.id, a.id, a.id), None, StatusCode::BAD_REQUEST).await;
    assert_eq!(same.code, model::ErrorCode::InvalidRequest);
    let missing = api.fails(Method::GET, &format!("/networks/{}/path?from={}&to=99", network.id, a.id), None, StatusCode::NOT_FOUND).await;
    assert_eq!(missing.code, model::ErrorCode::NotFound);
    let no_query = api.fails(Method::GET, &format!("/networks/{}/path", network.id), None, StatusCode::BAD_REQUEST).await;
    assert_eq!(no_query.code, model::ErrorCode::InvalidRequest);
    api.fails(Method::GET, "/networks/99/routes", None, StatusCode::NOT_FOUND).await;
}

#[tokio::test]
async fn a_new_node_is_named_after_its_hostname_in_the_api() {
    let mut api = Api::new();
    let network = api.network("home").await;
    let member = api.join_as(network.id, &Hello { hostname: "Office-PC.corp.example".to_owned(), ..hello(Declared::default()) }).await;
    let node: model::NodeDetail = api.ok(Method::GET, &format!("/networks/{}/nodes/{}", network.id, member.id), None).await;
    assert_eq!(
        (node.node.name.as_str(), node.node.fqdn.as_str(), node.node.hostname.as_str()),
        ("office-pc", "office-pc.home.i3t", "Office-PC.corp.example")
    );
}

#[tokio::test]
async fn the_static_directory_is_served_at_the_root_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>ice3tier</html>").unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/app.js"), "console.log('ui')").unwrap();
    let api = Api::with_webui(&WebUi::Dir(dir.path().to_owned()));
    let get = |path: &str| Request::builder().uri(path).body(Body::empty()).unwrap();

    let (status, headers, bytes) = api.send(get("/")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"<html>ice3tier</html>".as_slice()));
    assert!(headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("text/html"));
    let (status, _, bytes) = api.send(get("/assets/app.js")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"console.log('ui')".as_slice()));
    // 前端自己的路由：不是文件的路径回 index.html。
    let (status, _, bytes) = api.send(get("/networks/1/nodes")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"<html>ice3tier</html>".as_slice()));

    // 静态文件不需要令牌，API 照样需要。
    let (status, _, _) = api.send(get("/api/v1/status")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    api.ok::<model::Status>(Method::GET, "/status", None).await;
    let unknown = api.fails(Method::GET, "/no-such-endpoint", None, StatusCode::NOT_FOUND).await;
    assert_eq!(unknown.code, model::ErrorCode::NotFound, "/api/v1 下面不存在的路径不会落到静态文件上");
}
