//! 冒烟测试：在测试进程里跑一个 controller，再把编出来的 `i3tctl` 当成外部程序去操作它。

// clippy.toml 里的 allow-unwrap-in-tests 只认 `#[test]` 函数本身，认不出集成测试里的辅助函数。
#![allow(clippy::unwrap_used, reason = "整个文件都是测试代码，出错就该直接失败")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Output;
use std::sync::Arc;

use i3t_controller::store::Store;
use i3t_controller::{Admitted, Controller, ServeOptions, Server, Timing};
use i3t_crypto::NodeKey;
use i3t_model::control::{Declared, EnrollToken, Hello, ToNode};
use i3t_proto::PublicKey;
use serde_json::Value;
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
    _session: Admitted,
    _inbox: mpsc::Receiver<ToNode>,
}

impl Fixture {
    async fn start() -> Self {
        let store = Store::open_in_memory().unwrap();
        let controller =
            Controller::new(store, NodeKey::generate().unwrap(), "ctl.example.com:20331".to_owned(), Timing::default()).unwrap();
        let token = controller.issue_admin_token().unwrap();
        let controller = Arc::new(controller);
        let options =
            ServeOptions { node_listen: "127.0.0.1:0".parse().unwrap(), admin_listen: "127.0.0.1:0".parse().unwrap(), static_dir: None };
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

    /// 用 `i3tctl token create` 生成的令牌让一个节点加入。
    async fn join(&self, seed: u8, declared: Declared) -> Member {
        let created = self.json(&["token", "create", "--expires", "10m"]).await;
        let token: EnrollToken = created["token"].as_str().unwrap().parse().unwrap();
        let (outbox, inbox) = mpsc::channel(64);
        let hello = Hello { version: "0.1.0".to_owned(), platform: "linux-x86_64".to_owned(), declared, port: 20330 };
        let remote = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));
        let session = self.controller.enroll(PublicKey::from_bytes([seed; PublicKey::LEN]), &token.secret, &hello, remote, outbox).unwrap();
        Member { _session: session, _inbox: inbox }
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
    assert_eq!(fixture.json(&["pair", "list"]).await, serde_json::json!([{ "a": 1, "b": 2, "keep_alive": true }]));

    // 地址、令牌和网络也可以从环境变量来。
    let mut from_env = fixture.command();
    from_env.env("I3T_ADMIN_URL", &url).env("I3T_ADMIN_TOKEN", &fixture.token).env("I3T_NETWORK", "home");
    let output = from_env.args(["node", "routes", "laptop"]).output().await.unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(fixture.fails(&["--network", "nowhere", "node", "list"]).await.contains("nowhere"));

    // 删除。--json 时没有应答体的操作什么都不输出。
    fixture.ok(&["pair", "delete", "laptop", "nas-01"]).await;
    fixture.ok(&["forward", "delete", &forward_id]).await;
    assert_eq!(fixture.ok(&["--json", "credential", "delete", &credential["id"].to_string()]).await, "");
    assert!(fixture.ok(&["node", "delete", "laptop"]).await.contains("冷却期"));
    assert_eq!(fixture.json(&["node", "list"]).await.as_array().unwrap().len(), 1);
    assert!(fixture.fails(&["network", "delete", "home"]).await.contains("network_not_empty"));
    let log = fixture.ok(&["log", "--limit", "5"]).await;
    assert!(log.contains("node.delete") && log.lines().count() == 6, "{log}");
}
