"""在实验环境里启动 ice3tier 的程序，以及测试里反复用到的几样东西。"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
import time
from pathlib import Path

from lab import Host, wait_until

REPO = Path(__file__).resolve().parents[2]
BIN_DIR = Path(os.environ.get("I3T_BIN_DIR", REPO / "target" / "release"))
NETWORK = {"name": "lab", "domain": "lab.i3t", "cidr": "10.66.0.0/16"}

# 一个只回答"你是从哪个地址连过来的"的 HTTP 服务。用它确认流量是从哪个节点的出口出去的。
WHOAMI = r"""
import http.server, socketserver, sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = self.client_address[0].encode()
        if self.path.startswith("/bytes/"):
            body = b"x" * int(self.path.rsplit("/", 1)[1])
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
Server((sys.argv[1], int(sys.argv[2])), Handler).serve_forever()
"""


def binary(name: str) -> str:
    path = BIN_DIR / name
    if not path.exists():
        raise FileNotFoundError(f"找不到 {path}。先构建：cargo build --release，或者用环境变量 I3T_BIN_DIR 指到可执行文件所在的目录")
    return str(path)


def start_whoami(host: Host, port: int = 80, bind: str = "0.0.0.0") -> None:
    host.spawn(sys.executable, "-c", WHOAMI, bind, str(port))
    wait_until(lambda: f":{port} " in host.run("ss", "-lnt").stdout, what=f"{host.name} 上的 HTTP 服务启动")


class Node:
    """实验环境里的一个 ice3tier 节点。"""

    def __init__(self, host: Host, work: Path, node_id: int, *, declare: tuple[str, ...] = ()) -> None:
        self.host = host
        self.id = node_id
        self.name = host.name
        self.virtual_ip = f"10.66.0.{node_id}"
        self.state = work / host.name
        self.state.mkdir(parents=True, exist_ok=True)
        self.log = work / f"{host.name}.log"
        self.proc: subprocess.Popen | None = None
        declared = "\n".join(f"{name} = true" for name in declare)
        (self.state / "config.toml").write_text(f"port = 20330\n[declare]\n{declared}\n[log]\nlevel = \"debug\"\n")
        self.key = host.run(binary("i3t"), "--state-dir", str(self.state), "key").stdout.strip()

    @property
    def endpoint(self) -> str:
        return f"{self.host.ip}:20330"

    def as_peer(self, *, endpoints: bool = True, attached: list[tuple["Node", int]] = (), exit: bool = False,
                inbound: bool = True, outbound: bool = True) -> dict:
        """别的节点的 netmap 里，这个节点长什么样。"""
        peer = {"id": self.id, "key": self.key, "name": self.name, "virtual_ip": self.virtual_ip,
                "inbound": inbound, "outbound": outbound,
                "endpoints": [self.endpoint] if endpoints else [],
                "attached": [{"relay": relay.id, "rtt_ms": rtt} for relay, rtt in attached]}
        if exit:
            peer["exit"] = {}
        return peer

    def as_relay(self) -> dict:
        return {"id": self.id, "key": self.key, "name": self.name, "udp": [self.endpoint], "tcp": [self.endpoint]}

    def netmap(self, *, peers: list[dict] = (), relays: list[dict] = (), me: dict | None = None,
               relay_clients: list["Node"] = (), version: int = 1) -> dict:
        return {"version": version, "network": NETWORK,
                "me": {"id": self.id, "name": self.name, "virtual_ip": self.virtual_ip, **(me or {})},
                "peers": list(peers), "relays": list(relays),
                "relay_clients": [{"id": node.id, "key": node.key} for node in relay_clients]}

    def start_static(self, netmap: dict) -> None:
        path = self.state / "static-netmap.json"
        path.write_text(json.dumps(netmap, indent=1))
        self.proc = self.host.spawn(binary("i3t"), "--state-dir", str(self.state), "run", "--static", str(path), log=self.log)
        wait_until(lambda: "节点启动" in self.read_log() or self.proc.poll() is not None, what=f"{self.name} 启动")
        if self.proc.poll() is not None:
            raise RuntimeError(f"{self.name} 启动失败：\n{self.read_log()[-2000:]}")

    def stop(self) -> None:
        if self.proc is not None:
            self.host.stop(self.proc)
            self.proc = None

    def read_log(self) -> str:
        return self.log.read_text(errors="replace") if self.log.exists() else ""

    def curl(self, url: str, *, via: "Node | None" = None, proxy: str = "socks5", timeout: float = 20,
             extra: tuple[str, ...] = ()) -> subprocess.CompletedProcess:
        """从这个节点所在的机器上，经它的本机代理入口访问 `url`。`via` 是代理用户名里写的出口节点。"""
        user = f"{via.virtual_ip}@" if via else ""
        if proxy == "socks5":
            proxy_args = ["-x", f"socks5h://{user}127.0.0.1:1080"]
        else:
            # HTTP 入口只支持 CONNECT，-p 让 curl 对明文 HTTP 也走隧道。
            proxy_args = ["-p", "-x", f"http://{user.rstrip('@') + ':x@' if via else ''}127.0.0.1:1081"]
        return self.host.run("curl", "-sS", "--max-time", str(timeout), *proxy_args, *extra, url, check=False, timeout=timeout + 5)


class Checks:
    """收集一个测试脚本里各项检查的结果。"""

    def __init__(self) -> None:
        self.failures: list[str] = []

    def check(self, name: str, ok: bool, detail: str = "") -> bool:
        print(f"{'通过' if ok else '失败'}  {name}{'：' + detail if detail and not ok else ''}", flush=True)
        if not ok:
            self.failures.append(name)
        return ok

    def summary(self) -> int:
        print("全部通过" if not self.failures else f"{len(self.failures)} 项失败：" + "；".join(self.failures))
        return 1 if self.failures else 0


def tail(text: str, lines: int = 15) -> str:
    return textwrap.indent("\n".join(text.splitlines()[-lines:]), "    ")
