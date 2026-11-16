//! 冒烟测试：在测试进程里跑一个 controller，再把编出来的 `i3tctl` 当成外部程序去操作它。

// clippy.toml 里的 allow-unwrap-in-tests 只认 `#[test]` 函数本身，认不出集成测试里的辅助函数。
#![allow(clippy::unwrap_used, reason = "整个文件都是测试代码，出错就该直接失败")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Output;
use std::sync::Arc;

use i3t_controller::store::Store;
use i3t_controller::webui::WebUi;
use i3t_controller::{Admitted, Controller, ServeOptions, Server, Timing};
use i3t_crypto::NodeKey;
use i3t_model::control::{Declared, EnrollToken, Hello, PROTOCOL_REVISION, Report, Route, RouteEntry, SelfUpdate, ToController, ToNode};
use i3t_model::update::{Artifact, FailureKind, LATEST_DIR, MANIFEST_FILE, Manifest, SIGNATURE_FILE, Sha256, UpdateStatus};
use i3t_proto::PublicKey;
use i3t_update::{TrustedKeys, sign_manifest};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

struct Fixture {
    controller: Arc<Controller>,
    admin: SocketAddr,
    token: String,
    _stop: oneshot::Sender<()>,
}

/// 一个假装在线的节点。收件箱要留着，丢了 controller 会当它断线。
struct Member {
    session: Admitted,
    _inbox: mpsc::Receiver<ToNode>,
}

const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

impl Fixture {
    async fn start() -> Self {
        let store = Store::open_in_memory().unwrap();
        let controller =
            Controller::new(store, NodeKey::generate().unwrap(), "ctl.example.com:20331".to_owned(), Timing::default()).unwrap();
        Self::serve(controller).await
    }

    async fn serve(controller: Controller) -> Self {
        let token = controller.issue_admin_token().unwrap();
        let controller = Arc::new(controller);
        // 后台不查发布源：测试不该去连项目的发布源。命令行的"立即检查"照样可以用。
        let options = ServeOptions {
            node_listen: "127.0.0.1:0".parse().unwrap(),
            admin_listen: "127.0.0.1:0".parse().unwrap(),
            webui: WebUi::NotIncluded,
            follow_releases: false,
        };
        let server = Server::bind(Arc::clone(&controller), options).await.unwrap();
        let admin = server.admin_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        tokio::spawn(server.run(async {
            stopped.await.ok();
        }));
        Self { controller, admin, token, _stop: stop }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_i3tctl"));
        // 测试所在的环境里可能设着这些变量，不能让它们混进来。
        command.env_remove("I3T_ADMIN_URL").env_remove("I3T_ADMIN_TOKEN").env_remove("I3T_NETWORK");
        command
    }

    async fn run(&self, args: &[&str]) -> Output {
        let url = format!("http://{}", self.admin);
        self.command().args(["--url", &url, "--token", &self.token]).args(args).output().await.unwrap()
    }

    /// 期望成功，返回标准输出。
    async fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args).await;
        let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
        assert!(output.status.success(), "i3tctl {args:?} 应该成功\nstdout: {stdout}\nstderr: {stderr}");
        stdout
    }

    /// 期望失败，返回标准错误。
    async fn fails(&self, args: &[&str]) -> String {
        let output = self.run(args).await;
        assert!(!output.status.success(), "i3tctl {args:?} 应该失败，输出：{}", text(&output.stdout));
        assert!(output.stdout.is_empty(), "失败时标准输出应该是空的：{}", text(&output.stdout));
        text(&output.stderr)
    }

    async fn json(&self, args: &[&str]) -> Value {
        let mut with_flag = vec!["--json"];
        with_flag.extend_from_slice(args);
        let stdout = self.ok(&with_flag).await;
        serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("i3tctl --json {args:?} 的输出不是 JSON：{err}\n{stdout}"))
    }

    /// 用 `i3tctl token create` 生成的令牌让一个节点加入。它是最初那一版的节点：不报主机名，不懂更新。
    async fn join(&self, seed: u8, declared: Declared) -> Member {
        let hello = Hello { version: "0.1.0".to_owned(), platform: "linux-x86_64".to_owned(), declared, port: 20330, ..Hello::default() };
        self.join_as(seed, &hello).await
    }

    async fn join_as(&self, seed: u8, hello: &Hello) -> Member {
        let created = self.json(&["token", "create", "--expires", "10m"]).await;
        let token: EnrollToken = created["token"].as_str().unwrap().parse().unwrap();
        let (outbox, inbox) = mpsc::channel(64);
        let session = self.controller.enroll(PublicKey::from_bytes([seed; PublicKey::LEN]), &token.secret, hello, REMOTE, outbox).unwrap();
        Member { session, _inbox: inbox }
    }

    fn report(&self, member: &Member, report: Report) {
        self.controller.handle(member.session, REMOTE, ToController::Report(report)).unwrap();
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn the_command_line_drives_a_running_controller() {
    let fixture = Fixture::start().await;

    // 连接与认证。
    let status = fixture.ok(&["status"]).await;
    assert!(status.contains("ctl.example.com:20331"), "{status}");
    let url = format!("http://{}", fixture.admin);
    let wrong = fixture.command().args(["--url", &url, "--token", "wrong", "status"]).output().await.unwrap();
    assert!(!wrong.status.success());
    assert!(text(&wrong.stderr).contains("unauthorized"), "{}", text(&wrong.stderr));
    let missing = fixture.command().args(["--url", &url, "status"]).output().await.unwrap();
    assert!(!missing.status.success() && text(&missing.stderr).contains("I3T_ADMIN_TOKEN"), "{}", text(&missing.stderr));
    let unreachable = fixture.command().args(["--url", "http://127.0.0.1:1", "--token", "x", "status"]).output().await.unwrap();
    assert!(!unreachable.status.success() && text(&unreachable.stderr).contains("连不上"), "{}", text(&unreachable.stderr));

    // 网络。
    assert!(fixture.fails(&["node", "list"]).await.contains("network create"), "还没有网络时要告诉人怎么办");
    let created = fixture.ok(&["network", "create", "home"]).await;
    assert!(created.contains("home.i3t") && created.contains("10.66.0.0/16"), "{created}");
    let listed = fixture.ok(&["network", "list"]).await;
    assert!(listed.lines().next().unwrap().starts_with("ID  NAME") && listed.contains("home"), "{listed}");
    let networks = fixture.json(&["network", "list"]).await;
    assert_eq!(networks.as_array().unwrap().len(), 1);
    assert_eq!(networks[0]["name"], "home");
    fixture.ok(&["network", "set", "--routing", "lowest-cost", "--max-relay-hops", "3"]).await;
    assert_eq!(fixture.json(&["network", "show"]).await["routing"], "lowest_cost");

    // 注册令牌和节点。
    let token = fixture.ok(&["token", "create", "--tag", "servers", "--ip", "10.66.0.50"]).await;
    assert!(token.lines().any(|line| line.trim().starts_with("i3t-")), "{token}");
    let tokens = fixture.json(&["token", "list"]).await;
    assert_eq!(tokens[0]["tags"], serde_json::json!(["servers"]));
    fixture.ok(&["token", "delete", &tokens[0]["id"].to_string()]).await;

    let _plain = fixture.join(1, Declared::default()).await;
    let _capable = fixture.join(2, Declared { exit: true, relay: true, forward: true }).await;
    let nodes = fixture.ok(&["node", "list"]).await;
    assert!(nodes.contains("10.66.0.1") && nodes.contains("10.66.0.2") && nodes.contains("online"), "{nodes}");
    // 节点可以用编号、虚拟 IP 或名字指代。
    fixture.ok(&["node", "rename", "1", "laptop"]).await;
    fixture.ok(&["node", "rename", "10.66.0.2", "nas-01"]).await;
    fixture.ok(&["node", "tag", "laptop", "laptops"]).await;
    fixture.ok(&["node", "tag", "nas-01", "servers", "home"]).await;
    let detail = fixture.ok(&["node", "show", "nas-01"]).await;
    assert!(detail.contains("nas-01.home.i3t") && detail.contains("home,servers"), "{detail}");
    assert_eq!(fixture.json(&["node", "show", "laptop"]).await["virtual_ip"], "10.66.0.1");
    assert!(fixture.fails(&["node", "show", "nobody"]).await.contains("nobody"));
    fixture.ok(&["node", "set", "laptop", "--dial-out-only", "on", "--reach", "direct-only"]).await;
    let laptop = fixture.json(&["node", "show", "laptop"]).await;
    assert_eq!((laptop["dial_out_only"].clone(), laptop["reach"].clone()), (Value::Bool(true), Value::from("direct_only")));
    fixture.ok(&["node", "set", "laptop", "--reach", "default"]).await;
    assert!(fixture.json(&["node", "show", "laptop"]).await["reach"].is_null());

    // 没声明的能力启用不了，错误里有要在节点上执行的命令。
    let refused = fixture.fails(&["relay", "enable", "laptop", "--udp", "203.0.113.7:20330"]).await;
    assert!(refused.contains("i3t set can-relay on") && refused.contains("not_declared"), "{refused}");
    assert!(fixture.fails(&["exit", "enable", "laptop"]).await.contains("i3t set can-exit on"));
    fixture.ok(&["relay", "enable", "nas-01", "--udp", "203.0.113.7:20330", "--region", "hk"]).await;
    fixture.ok(&["exit", "enable", "nas-01", "--ports", "any", "--lan"]).await;
    let nas = fixture.json(&["node", "show", "nas-01"]).await;
    assert_eq!(nas["relay"]["udp"], serde_json::json!(["203.0.113.7:20330"]));
    assert!(nas["exit"]["ports"].is_null() && nas["exit"]["lan"] == true);
    fixture.ok(&["node", "set", "laptop", "--pin", "nas-01"]).await;
    assert!(fixture.ok(&["node", "list"]).await.contains("relay,exit"));
    let overview = fixture.ok(&["overview"]).await;
    assert!(overview.contains("relay 1") && overview.contains("laptop"), "{overview}");
    fixture.ok(&["exit", "disable", "nas-01"]).await;

    // 规则和访问关系。
    let rules = fixture.json(&["rule", "list"]).await;
    fixture.ok(&["rule", "disable", &rules[0]["id"].to_string()]).await;
    let added = fixture.json(&["rule", "add", "laptops", "servers", "--one-way", "--name", "ssh"]).await;
    assert_eq!(added["direction"], "one_way");
    let listed = fixture.ok(&["rule", "list"]).await;
    assert!(listed.contains("laptops  ->   servers  yes      ssh") && listed.contains("all      <->  all      no"), "{listed}");
    assert!(fixture.ok(&["access", "check", "laptop", "nas-01"]).await.contains("可以访问"));
    assert!(fixture.ok(&["access", "check", "nas-01", "laptop"]).await.contains("不能访问"));
    assert_eq!(fixture.json(&["access", "check", "laptop", "nas-01"]).await["allowed"], true);
    assert!(fixture.ok(&["access", "graph"]).await.contains("laptops"));
    assert!(fixture.ok(&["node", "access", "nas-01"]).await.contains("reachable-by"));

    // 凭据、监听器、端口转发、节点对。
    let credential = fixture.json(&["credential", "create", "phone", "--max-concurrent", "4"]).await;
    assert_eq!(credential["token"].as_str().unwrap().len(), 43);
    assert!(fixture.ok(&["credential", "list"]).await.contains("phone"));
    assert!(fixture.fails(&["listener", "add", "laptop", "lan", "192.168.1.5:1080"]).await.contains("require_credential"));
    fixture
        .ok(&[
            "listener",
            "add",
            "laptop",
            "lan",
            "192.168.1.5:1080",
            "--require-credential",
            "--default",
            "exit:nas-01",
            "--allow-from",
            "192.168.1.0/24",
        ])
        .await;
    let listeners = fixture.ok(&["listener", "list", "--node", "laptop"]).await;
    assert!(listeners.contains("192.168.1.5:1080") && listeners.contains("exit:#2"), "{listeners}");
    assert!(
        fixture.fails(&["forward", "add", "nas-01", "127.0.0.1:8080", "laptop", "127.0.0.1:80"]).await.contains("i3t set can-forward on")
    );
    let forward = fixture.json(&["forward", "add", "laptop", "127.0.0.1:8080", "nas-01", "192.168.1.10:80", "--name", "web"]).await;
    assert_eq!(forward["status"], "active");
    let forward_id = forward["id"].to_string();
    assert_eq!(fixture.json(&["forward", "disable", &forward_id]).await["status"], "disabled");
    assert_eq!(fixture.json(&["forward", "enable", &forward_id]).await["enabled"], true);
    assert!(fixture.ok(&["forward", "list"]).await.contains("#2 192.168.1.10:80"));
    fixture.ok(&["pair", "set", "nas-01", "laptop", "--keep-alive", "on"]).await;
    let pairs = fixture.json(&["pair", "list"]).await;
    assert_eq!(
        (&pairs[0]["a"], &pairs[0]["b"], &pairs[0]["keep_alive"], &pairs[0]["lane"]),
        (&json!(1), &json!(2), &json!(true), &json!("off"))
    );
    // 只改独占 TCP，常驻不变；设置起不起作用要说出来。
    let pair = fixture.json(&["pair", "set", "nas-01", "laptop", "--lane", "udp_fallback"]).await;
    assert_eq!((&pair["keep_alive"], &pair["lane"]), (&json!(true), &json!("udp_fallback")));
    assert!(pair["lane_effect"]["effective"].is_boolean(), "{pair}");
    assert!(fixture.ok(&["pair", "list"]).await.contains("udp_fallback"));
    assert!(fixture.fails(&["pair", "set", "nas-01", "laptop", "--lane", "sometimes"]).await.contains("udp_fallback"));
    assert!(fixture.fails(&["pair", "set", "nas-01", "laptop"]).await.contains("--lane"), "至少要改一项");

    // 地址、令牌和网络也可以从环境变量来。
    let mut from_env = fixture.command();
    from_env.env("I3T_ADMIN_URL", &url).env("I3T_ADMIN_TOKEN", &fixture.token).env("I3T_NETWORK", "home");
    let output = from_env.args(["node", "routes", "laptop"]).output().await.unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(fixture.fails(&["--network", "nowhere", "node", "list"]).await.contains("nowhere"));

    // 删除。--json 时没有应答体的操作什么都不输出。
    fixture.ok(&["pair", "unset", "laptop", "nas-01"]).await;
    fixture.ok(&["forward", "delete", &forward_id]).await;
    assert_eq!(fixture.ok(&["--json", "credential", "delete", &credential["id"].to_string()]).await, "");
    assert!(fixture.ok(&["node", "delete", "laptop"]).await.contains("冷却期"));
    assert_eq!(fixture.json(&["node", "list"]).await.as_array().unwrap().len(), 1);
    assert!(fixture.fails(&["network", "delete", "home"]).await.contains("network_not_empty"));
    let log = fixture.ok(&["log", "--limit", "5"]).await;
    assert!(log.contains("node.delete") && log.lines().count() == 6, "{log}");
}

/// 在 `root` 里摆一个发布源（布局见 `docs/releasing.md` 第 6 节）：`version` 的 Linux 节点程序，清单用 `key` 签。
fn publish(root: &std::path::Path, key: &NodeKey, version: &str, program: &[u8]) {
    let version: i3t_model::update::Version = version.parse().unwrap();
    let file = "i3t-linux-x86_64".to_owned();
    let release = root.join(version.release_dir());
    std::fs::create_dir_all(&release).unwrap();
    std::fs::write(release.join(&file), program).unwrap();
    let hash: [u8; 32] = ring::digest::digest(&ring::digest::SHA256, program).as_ref().try_into().unwrap();
    let artifact = Artifact { file, size: u64::try_from(program.len()).unwrap(), sha256: Sha256::from_bytes(hash) };
    let manifest = Manifest {
        version,
        min_controller: "0.0.0".parse().unwrap(),
        published: 1_763_200_000,
        artifacts: [("linux-x86_64".to_owned(), artifact)].into(),
    };
    let signed = sign_manifest(key, serde_json::to_string(&manifest).unwrap());
    let latest = root.join(LATEST_DIR);
    std::fs::create_dir_all(&latest).unwrap();
    std::fs::write(latest.join(MANIFEST_FILE), &signed.manifest).unwrap();
    std::fs::write(latest.join(SIGNATURE_FILE), signed.signature.to_string()).unwrap();
}

#[tokio::test]
async fn updates_routes_and_paths_can_be_driven_from_the_command_line() {
    let (source, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let release_key = NodeKey::generate().unwrap();
    let controller = Controller::new(
        Store::open_in_memory().unwrap(),
        NodeKey::generate().unwrap(),
        "ctl.example.com:20331".to_owned(),
        Timing::default(),
    )
    .unwrap()
    .with_release_keys(TrustedKeys::from(release_key.public()))
    .unwrap()
    .with_release_cache(cache.path().to_owned())
    // 下面发布的版本号是写死的，controller 的版本也固定下来，不随这个包的版本号变。
    .with_version("0.1.0".parse().unwrap());
    let fixture = Fixture::serve(controller).await;
    fixture.ok(&["network", "create", "home"]).await;

    // 设置。
    let shown = fixture.ok(&["update", "show"]).await;
    assert!(
        shown.contains("自动更新    开") && shown.contains("github.com") && shown.contains("（默认）") && shown.contains("还没有查到"),
        "{shown}"
    );
    let source_dir = source.path().to_str().unwrap();
    let set = fixture.json(&["update", "set", "--auto", "off", "--source", source_dir]).await;
    assert_eq!(
        (set["auto_update"].clone(), set["source"].clone(), set["source_is_default"].clone()),
        (false.into(), source_dir.into(), false.into())
    );
    assert!(fixture.fails(&["update", "set", "--source", "not/absolute"]).await.contains("invalid_request"));

    // 查发布源：先是什么都没有，然后有了一个版本。
    let empty = fixture.ok(&["update", "check"]).await;
    assert!(empty.contains("检查失败") && empty.contains("还没有查到"), "{empty}");
    publish(source.path(), &release_key, "0.2.0", b"the new program");
    let checked = fixture.ok(&["update", "check"]).await;
    assert!(checked.contains("最新正式版  0.2.0") && checked.contains("linux-x86_64") && checked.contains("not_needed"), "{checked}");
    assert!(checked.contains("controller 自己也有新版本 0.2.0"), "{checked}");
    assert!(fixture.ok(&["status"]).await.contains("有新版本    0.2.0"));

    // 一个懂更新的节点（主机名变成了它的名字），一个最初那一版的。
    let modern = Hello {
        version: "0.1.0".to_owned(),
        platform: "linux-x86_64".to_owned(),
        port: 20330,
        hostname: "Laptop.lan".to_owned(),
        protocol: PROTOCOL_REVISION,
        self_update: Some(SelfUpdate::ABLE),
        ..Hello::default()
    };
    let laptop = fixture.join_as(1, &modern).await;
    let ancient = fixture.join(2, Declared::default()).await;
    fixture.report(&laptop, Report { update: Some(UpdateStatus::Idle), ..Report::default() });
    let nodes = fixture.ok(&["update", "nodes"]).await;
    let row = |name: &str| nodes.lines().find(|line| line.contains(name)).unwrap_or_else(|| panic!("{nodes}")).to_owned();
    assert!(row("laptop").contains("0.2.0") && row("laptop").contains("yes") && row("laptop").contains("有新版本 0.2.0"), "{nodes}");
    assert!(row("node-2").contains("unknown"), "{nodes}");
    let detail = fixture.ok(&["node", "show", "laptop"]).await;
    assert!(detail.contains("主机名        Laptop.lan") && detail.contains("更新          有新版本 0.2.0"), "{detail}");

    // 强制更新：包还没有取回来时先排着；取回来之后指令发出去。
    assert!(fixture.fails(&["update", "node", "node-2"]).await.contains("cannot_self_update"));
    let queued = fixture.ok(&["update", "node", "laptop", "--drain"]).await;
    assert!(queued.contains("在等 controller 把它的平台的包取回来"), "{queued}");
    let controller = Arc::clone(&fixture.controller);
    tokio::task::spawn_blocking(move || controller.fetch_packages().unwrap()).await.unwrap();
    let update = fixture.json(&["update", "node", "laptop", "--drain"]).await;
    assert_eq!(update["ordered"], serde_json::json!({ "version": "0.2.0", "timing": "after_drain" }));
    let all = fixture.ok(&["update", "all"]).await;
    assert!(all.contains("已下发指令  laptop") && all.contains("跳过        node-2"), "{all}");
    fixture.report(&laptop, Report { update: Some(UpdateStatus::Draining { version: "0.2.0".parse().unwrap() }), ..Report::default() });
    assert!(fixture.ok(&["update", "nodes"]).await.contains("不再接新的流"));

    // 节点说这个版本不能用：自动更新暂停，看过之后继续。
    let failed =
        UpdateStatus::Failed {
            version: Some("0.2.0".parse().unwrap()), kind: FailureKind::VersionRejected, reason: "自检没有通过".into()
        };
    fixture.report(&laptop, Report { update: Some(failed), ..Report::default() });
    let paused = fixture.ok(&["update", "show"]).await;
    assert!(paused.contains("版本 0.2.0 的自动更新已暂停：laptop") && paused.contains("i3tctl update resume 0.2.0"), "{paused}");
    assert!(fixture.ok(&["update", "nodes"]).await.contains("这个版本在这台机器上不能用"));
    assert!(fixture.json(&["update", "resume", "0.2.0"]).await["paused"].as_array().unwrap().is_empty());
    assert!(fixture.fails(&["update", "resume", "0.2.0"]).await.contains("not_found"));
    assert!(fixture.fails(&["update", "resume", "latest"]).await.contains("版本号"));

    // 路由矩阵和两个节点之间的路径。
    let route =
        RouteEntry { peer: ancient.session.node, route: Route::Direct { addr: "203.0.113.9:20330".parse().unwrap() }, rtt_ms: Some(7) };
    fixture.report(
        &ancient,
        Report { routes: vec![RouteEntry { peer: laptop.session.node, ..route.clone() }], routes_full: true, ..Report::default() },
    );
    let matrix = fixture.ok(&["routes"]).await;
    assert!(matrix.lines().next().unwrap().starts_with("FROM") && matrix.contains("node-2  laptop  direct  7"), "{matrix}");
    assert!(matrix.contains("没有上报过路由的节点：laptop"), "{matrix}");
    assert_eq!(fixture.json(&["routes"]).await["entries"][0]["stale"], false);
    // laptop 懂路由查询但不会回答（它只是个收件箱），等几秒之后退回到上报的数据；它没有上报过。
    let path = fixture.ok(&["path", "laptop", "node-2"]).await;
    assert!(path.contains("laptop  node-2  -       -       none") && path.contains("node-2  laptop  direct  7       report"), "{path}");
    assert!(path.contains("没有数据") && path.contains("这是它最近上报的"), "{path}");
    assert_eq!(fixture.json(&["path", "node-2", "laptop"]).await["forward"]["source"], "report");
    assert!(fixture.fails(&["path", "laptop", "laptop"]).await.contains("invalid_request"));
    assert!(fixture.ok(&["backbone"]).await.contains("这个网络里还没有 relay"));
    assert_eq!(fixture.json(&["backbone"]).await["relays"], serde_json::json!([]));
}
