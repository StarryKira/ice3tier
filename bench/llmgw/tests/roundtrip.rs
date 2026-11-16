//! 上游和负载端放在一起跑：每种回答方式（正常结束、中途重置、停住），SSE 和 WebSocket，HTTPS 和明文，
//! 直连时都应该按期望结束。这验证的是工具自己：直连时出错就是工具的错。

// 这个文件里全是测试。clippy.toml 的 allow-unwrap-in-tests 只管 #[test] 函数本身，管不到它们调用的辅助函数。
#![allow(clippy::unwrap_used)]

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SCENARIO: &str = r#"
name = "roundtrip"
duration_s = 30
stall_ms = 200

[[group]]
name = "done"
kind = "sse"
count = 6
prompt_bytes = [100, 300000]
think_ms = 200
keepalive_ms = 100
tokens = 100
rate = 200
pauses = [{ after = 50, ms = 300 }]

[[group]]
name = "fast"
kind = "sse"
count = 2
tokens = 2000
rate = 0

[[group]]
name = "abort"
kind = "sse"
count = 3
tokens = 100
rate = 200
end = "abort"
end_after = 40

[[group]]
name = "stall"
kind = "sse"
count = 3
tokens = 100
rate = 200
end = "stall"
end_after = 40
hold_s = 1

[[group]]
name = "chat"
kind = "ws"
count = 3
turns = 2
think_ms = 100
tokens = 40
rate = 200
client_msgs = 3
idle_ms = 200
client_ping_ms = 50
server_ping_ms = 70

# ping 的定时器几乎时时都到：关闭握手进行中也会到，这时不能再发 ping。
[[group]]
name = "pingy"
kind = "ws"
count = 2
turns = 2
tokens = 20
rate = 200
client_ping_ms = 1
server_ping_ms = 1

[[group]]
name = "voice"
kind = "ws"
count = 2
turns = 1
tokens = 50
rate = 100
binary = true
event_bytes = 640
client_binary_ms = 10
client_binary_bytes = 640

[[group]]
name = "ws_abort"
kind = "ws"
count = 2
turns = 2
tokens = 40
rate = 200
end = "abort"
end_after = 60

[[group]]
name = "ws_stall"
kind = "ws"
count = 2
turns = 2
tokens = 40
rate = 200
end = "stall"
end_after = 60
hold_s = 1
"#;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Upstream(Child);

impl Drop for Upstream {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_upstream(dir: &Path, https: u16, http: u16) -> Upstream {
    let child = Command::new(env!("CARGO_BIN_EXE_i3t-llmgw"))
        .args(["upstream", "--https", &format!("127.0.0.1:{https}"), "--http", &format!("127.0.0.1:{http}"), "--cert-dir"])
        .arg(dir.join("ca"))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", http)).is_err() {
        assert!(Instant::now() < deadline, "上游 10 秒内没有开始监听");
        std::thread::sleep(Duration::from_millis(50));
    }
    Upstream(child)
}

fn load(dir: &Path, target: &str) -> serde_json::Value {
    let out = dir.join(format!("{}.json", target.split(':').next().unwrap()));
    let done = Command::new(env!("CARGO_BIN_EXE_i3t-llmgw"))
        .args(["load", "--quiet", "--target", target, "--scenario"])
        .arg(dir.join("scenario.toml"))
        .arg("--ca")
        .arg(dir.join("ca").join("ca.pem"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    let table = String::from_utf8_lossy(&done.stdout);
    assert_eq!(done.status.code(), Some(0), "经 {target} 有流没有按期望结束：\n{table}\n{}", String::from_utf8_lossy(&done.stderr));
    serde_json::from_slice(&std::fs::read(out).unwrap()).unwrap()
}

#[test]
fn every_kind_of_ending_is_recognised_over_https_and_plain_http() {
    let dir = std::env::temp_dir().join(format!("i3t-llmgw-roundtrip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("scenario.toml"), SCENARIO).unwrap();
    let (https, http) = (free_port(), free_port());
    let _upstream = start_upstream(&dir, https, http);

    for target in [format!("https://127.0.0.1:{https}"), format!("http://127.0.0.1:{http}")] {
        let report = load(&dir, &target);
        let groups = report["groups"].as_array().unwrap();
        let outcome = |name: &str| groups.iter().find(|group| group["name"] == name).unwrap()["outcomes"].clone();
        assert_eq!(outcome("done"), serde_json::json!({ "complete": 6 }), "{target}");
        assert_eq!(outcome("abort"), serde_json::json!({ "reset": 3 }), "{target}");
        assert_eq!(outcome("stall"), serde_json::json!({ "stalled": 3 }), "{target}");
        assert_eq!(outcome("chat"), serde_json::json!({ "complete": 3 }), "{target}");
        assert_eq!(outcome("pingy"), serde_json::json!({ "complete": 2 }), "{target}");
        assert_eq!(outcome("ws_abort"), serde_json::json!({ "reset": 2 }), "{target}");
        let done = groups.iter().find(|group| group["name"] == "done").unwrap();
        assert_eq!(done["events"], 600, "{target}");
        assert!(done["keepalives"].as_u64().unwrap() >= 6, "思考期间的保活注释应该都收到了：{}", done["keepalives"]);
        let chat = groups.iter().find(|group| group["name"] == "chat").unwrap();
        assert!(chat["ping_rtt_ms"]["count"].as_u64().unwrap() > 0, "客户端的 ping 应该有回音");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
