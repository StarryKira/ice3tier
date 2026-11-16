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


class Controller:
    """实验环境里的 controller，以及操作它的命令行。"""

    def __init__(self, host: Host, work: Path) -> None:
        self.host = host
        self.log = work / "controller.log"
        self.out = work / "controller.out"
        self.address = f"{host.ip}:20331"
        data = work / "controller"
        # 管理令牌打在标准输出上，日志在标准错误上，分开收。
        self.proc = subprocess.Popen(
            host.cmd(binary("i3t-controller"), "--data-dir", str(data), "--public-addr", self.address),
            stdout=self.out.open("w"), stderr=self.log.open("w"))
        host._procs.append(self.proc)
        wait_until(lambda: "已启动" in self.log.read_text(errors="replace") or self.proc.poll() is not None, what="controller 启动")
        if self.proc.poll() is not None:
            raise RuntimeError(f"controller 启动失败：\n{self.log.read_text(errors='replace')[-2000:]}")
        tokens = [line.strip() for line in self.out.read_text().splitlines() if line.startswith("    ")]
        if len(tokens) != 1:
            raise RuntimeError(f"没有在 controller 的输出里找到管理令牌：{self.out.read_text()!r}")
        self.token = tokens[0]

    def ctl(self, *args: str, check: bool = True) -> object:
        """执行一条 i3tctl 命令，返回管理 API 的 JSON。`check=False` 时出错不抛异常，返回完成的进程。"""
        done = self.host.run(binary("i3tctl"), "--token", self.token, "--json", *args, check=False)
        if not check:
            return done
        if done.returncode != 0:
            raise RuntimeError(f"i3tctl {' '.join(args)} 失败：{done.stderr.strip() or done.stdout.strip()}")
        return json.loads(done.stdout) if done.stdout.strip() else None

    def enroll_token(self, *tags: str) -> str:
        created = self.ctl("token", "create", *[arg for tag in tags for arg in ("--tag", tag)])
        return created["token"]

    def node(self, node: "Node") -> dict:
        """controller 眼里这个节点现在的样子。"""
        return next(entry for entry in self.ctl("node", "list") if entry["key"] == node.key)


class Node:
    """实验环境里的一个 ice3tier 节点。"""

    def __init__(self, host: Host, work: Path, node_id: int = 0, *, declare: tuple[str, ...] = ()) -> None:
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

    def join(self, controller: Controller, *tags: str, name: bool = True) -> None:
        """用一个新的注册令牌加入网络，然后启动。加入之后把节点名改成主机名，方便看日志和写用户名。"""
        token = controller.enroll_token(*tags)
        done = self.host.run(binary("i3t"), "--state-dir", str(self.state), "join", input=token, check=False)
        if done.returncode != 0:
            raise RuntimeError(f"{self.name} 加入失败：{done.stderr.strip()}")
        seen = controller.node(self)
        self.id, self.virtual_ip = seen["id"], seen["virtual_ip"]
        if name:
            controller.ctl("node", "rename", str(self.id), self.name)
        self.start()

    def start(self) -> None:
        self.proc = self.host.spawn(binary("i3t"), "--state-dir", str(self.state), "run", log=self.log)
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


# 下面是选路场景（test_routing.py）用的：按"哪两台主机之间"加网络条件，从节点的日志里读它现在走哪条路。

def shape_between(a: Host, b: Host, shape=None) -> None:
    """只给 `a` 和 `b` 之间的包加网络条件，两台主机的别的流量不受影响。`shape` 是 `lab.Shape`，`None` 表示去掉。

    `Host.shape` 管的是一台主机的整条上联线，做不出"直连差、绕 relay 好"的拓扑；这里按目的地址分流，
    两个方向各在发出的一侧加一半的延迟和各自的丢包。
    """
    for host, peer in ((a, b), (b, a)):
        shapes = host.__dict__.setdefault("_pair_shapes", {})
        if shape is None:
            shapes.pop(peer.ip, None)
        else:
            shapes[peer.ip] = shape
        host.run("tc", "qdisc", "del", "dev", "up0", "root", check=False)
        if not shapes:
            continue
        # 第一条带（1:1）走没有限制的流量，之后每个对端一条带，带下面挂一个 netem。
        host.run("tc", "qdisc", "add", "dev", "up0", "root", "handle", "1:", "prio", "bands", str(max(2, len(shapes) + 1)),
                 "priomap", *["0"] * 16)
        for band, (ip, pair_shape) in enumerate(shapes.items(), start=2):
            host.run("tc", "qdisc", "add", "dev", "up0", "parent", f"1:{band:x}", "handle", f"{band:x}0:", "netem",
                     *pair_shape.netem_args())
            host.run("tc", "filter", "add", "dev", "up0", "parent", "1:", "protocol", "ip", "prio", "1", "u32",
                     "match", "ip", "dst", f"{ip}/32", "flowid", f"1:{band:x}")


def route_changes(node: "Node", peer: "Node") -> list[tuple[str, str]]:
    """`node` 的日志里到 `peer` 的每一次换路：(换成了哪条路, 原因)，按发生的顺序。

    静态模式下没有 controller 可问，路径层每次换路都记一条带原因的日志，这里读的就是它。
    路写成"直连 1.2.3.4:20330"、"经 relay 8 → 9"或者"没有路"。
    """
    import re
    pattern = re.compile(rf"到对端的路变了 peer={peer.id} path=(.*?) reason=\"(.*?)\"\s*$")
    return [match.groups() for line in node.read_log().splitlines() if (match := pattern.search(line))]


def current_route(node: "Node", peer: "Node") -> str:
    """`node` 到 `peer` 现在走的路；还没有过路时是"没有路"。"""
    changes = route_changes(node, peer)
    return changes[-1][0] if changes else "没有路"


# 下面是 UDP 的场景（test_udp.py）用的。

# 一个 UDP 回显服务：把收到的报文发回去，前面加上"它看到的来源地址"和一个竖线。用它确认 UDP 是从哪个节点的出口出去的。
UDP_ECHO = r"""
import socket, sys
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind((sys.argv[1], int(sys.argv[2])))
while True:
    data, peer = sock.recvfrom(65535)
    sock.sendto(peer[0].encode() + b"|" + data, peer)
"""

# 一个最小的 SOCKS5 UDP 客户端：做 UDP ASSOCIATE，经中继端口发一个报文，打印回来的报文（JSON）；等不到就打印 null。
# 参数：代理地址、代理端口、用户名（可以为空）、目标、目标端口、载荷（十六进制）、最多发几次。
# 第一个报文要等两个节点之间建好连接才出得去，所以每隔一秒半重发一次，直到有回音。
SOCKS5_UDP = r"""
import json, socket, struct, sys
proxy_host, proxy_port, user, target, port, payload, tries = sys.argv[1:8]
control = socket.create_connection((proxy_host, int(proxy_port)), timeout=10)
if user:
    control.sendall(b"\x05\x01\x02")
    assert control.recv(2) == b"\x05\x02", "代理应该选中用户名密码认证"
    control.sendall(bytes([1, len(user)]) + user.encode() + b"\x00")
    assert control.recv(2) == b"\x01\x00", "认证应该通过"
else:
    control.sendall(b"\x05\x01\x00")
    assert control.recv(2) == b"\x05\x00", "代理应该选中不认证"
control.sendall(b"\x05\x03\x00\x01" + bytes(6))
reply = b""
while len(reply) < 10:
    reply += control.recv(10 - len(reply))
assert reply[:4] == b"\x05\x00\x00\x01", reply
relay = (socket.inet_ntoa(reply[4:8]), struct.unpack("!H", reply[8:10])[0])
try:
    address = b"\x01" + socket.inet_aton(target)
except OSError:
    address = b"\x03" + bytes([len(target)]) + target.encode()
packet = b"\x00\x00\x00" + address + struct.pack("!H", int(port)) + bytes.fromhex(payload)
udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
udp.settimeout(1.5)
answer = None
for _ in range(int(tries)):
    udp.sendto(packet, relay)
    try:
        data, sender = udp.recvfrom(65535)
    except socket.timeout:
        continue
    assert sender == relay and data[:4] == b"\x00\x00\x00\x01", (sender, data[:10])
    answer = {"source": socket.inet_ntoa(data[4:8]), "port": struct.unpack("!H", data[8:10])[0], "data": data[10:].hex()}
    break
print(json.dumps(answer))
"""

# 一个普通的 UDP 客户端：往一个地址发一个报文，打印回来的报文（十六进制）；等不到就什么都不打印。
# 参数：地址、端口、载荷（十六进制）、最多发几次。
UDP_CLIENT = r"""
import socket, sys
address, port, payload, tries = sys.argv[1:5]
udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
udp.settimeout(1.5)
for _ in range(int(tries)):
    udp.sendto(bytes.fromhex(payload), (address, int(port)))
    try:
        data, _ = udp.recvfrom(65535)
    except socket.timeout:
        continue
    print(data.hex())
    break
"""

# 没有 dnsmasq 的机器上顶替它的 DNS 服务：不管问什么名字，都用同一条 A 记录回答。参数：监听地址、端口、要回答的地址。
DNS_STUB = r"""
import socket, sys
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind((sys.argv[1], int(sys.argv[2])))
while True:
    query, peer = sock.recvfrom(2048)
    answer = b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\x3c\x00\x04" + socket.inet_aton(sys.argv[3])
    sock.sendto(query[:2] + b"\x81\x80\x00\x01\x00\x01\x00\x00\x00\x00" + query[12:] + answer, peer)
"""


def _wait_udp_listening(host: Host, port: int, what: str) -> None:
    wait_until(lambda: f":{port} " in host.run("ss", "-lnu").stdout, what=what)


def start_udp_echo(host: Host, port: int, bind: str = "0.0.0.0") -> None:
    host.spawn(sys.executable, "-c", UDP_ECHO, bind, str(port))
    _wait_udp_listening(host, port, f"{host.name} 上的 UDP 回显服务启动")


def start_dns(host: Host, bind: str, port: int, name: str, address: str) -> str:
    """在 `host` 上起一个 DNS 服务，`name` 解析成 `address`。有 dnsmasq 就用它，没有就用一个替身。返回用的是哪个。"""
    import shutil

    if shutil.which("dnsmasq"):
        # -d：留在前台，也不切换用户（测试用的用户命名空间里只有 root 这一个用户，切不了）。
        host.spawn("dnsmasq", "-d", "--conf-file=/dev/null", "--no-resolv", "--no-hosts", "--bind-interfaces",
                   f"--listen-address={bind}", f"--port={port}", f"--address=/{name}/{address}")
        server = "dnsmasq"
    else:
        host.spawn(sys.executable, "-c", DNS_STUB, bind, str(port), address)
        server = "替身"
    _wait_udp_listening(host, port, f"{host.name} 上的 DNS 服务启动")
    return server


def socks5_udp(host: Host, target: str, port: int, payload: bytes, *, user: str = "", tries: int = 10) -> dict | None:
    """从 `host` 上经它的本机 SOCKS5 入口发一个 UDP 报文给 `target:port`，返回回来的报文：来源地址、端口和数据。"""
    done = host.run(sys.executable, "-c", SOCKS5_UDP, "127.0.0.1", "1080", user, target, str(port), payload.hex(), str(tries),
                    check=False, timeout=tries * 2 + 15)
    if done.returncode != 0:
        raise RuntimeError(f"SOCKS5 UDP 客户端出错：{done.stderr.strip()[-500:]}")
    answer = json.loads(done.stdout)
    if answer is not None:
        answer["data"] = bytes.fromhex(answer["data"])
    return answer


def udp_exchange(host: Host, address: str, port: int, payload: bytes, *, tries: int = 10) -> bytes | None:
    """从 `host` 上往 `address:port` 发一个 UDP 报文，返回回来的报文；等不到返回 `None`。"""
    done = host.run(sys.executable, "-c", UDP_CLIENT, address, str(port), payload.hex(), str(tries), check=False, timeout=tries * 2 + 15)
    return bytes.fromhex(done.stdout.strip()) if done.stdout.strip() else None


def dns_query(name: str, query_id: int = 0x1d3f) -> bytes:
    """一个查 `name` 的 A 记录的 DNS 请求。"""
    labels = b"".join(bytes([len(label)]) + label.encode() for label in name.rstrip(".").split("."))
    return query_id.to_bytes(2, "big") + b"\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00" + labels + b"\x00\x00\x01\x00\x01"


def dns_a_records(response: bytes, query: bytes) -> list[str]:
    """从 DNS 应答里取出 A 记录的地址。应答对不上 `query`（编号、问题）时抛出异常。"""
    import socket

    if response[:2] != query[:2] or not response[2] & 0x80 or response[12:len(query)] != query[12:]:
        raise ValueError(f"这不是对那个请求的应答：{response[:len(query)].hex()}")
    count, at, addresses = int.from_bytes(response[6:8], "big"), len(query), []
    for _ in range(count):
        # 名字要么是一个 2 字节的指针，要么是一串标签；这里的应答里都是指针。
        at += 2 if response[at] & 0xc0 == 0xc0 else response.index(b"\x00", at) - at + 1
        kind, length = int.from_bytes(response[at:at + 2], "big"), int.from_bytes(response[at + 8:at + 10], "big")
        if kind == 1 and length == 4:
            addresses.append(socket.inet_ntoa(response[at + 10:at + 14]))
        at += 10 + length
    return addresses


# 下面是限额与用量的场景（test_limits.py）用的。

def eventually_value(action, ok, timeout: float = 30, interval: float = 0.5):
    """反复执行 `action`，直到 `ok(结果)` 为真或者超时；返回最后一次的结果。节点每 10 秒上报一次，等上报的东西要用它。"""
    deadline = time.monotonic() + timeout
    while True:
        result = action()
        if ok(result) or time.monotonic() > deadline:
            return result
        time.sleep(interval)


def timed_download(node: Node, url: str, *, via: Node | None = None, user: str | None = None, timeout: float = 60):
    """从 `node` 所在的机器经它的本机 SOCKS5 入口下载 `url`，返回 (curl 的退出码, 下载的字节数, 用了多少秒)。

    `user` 是代理的用户名；不写时用 `via` 的虚拟 IP。
    """
    name = user if user is not None else (via.virtual_ip if via else "")
    auth = f"{name}@" if name else ""
    done = node.host.run("curl", "-sS", "--max-time", str(timeout), "-x", f"socks5h://{auth}127.0.0.1:1080", "-o", "/dev/null",
                         "-w", "%{size_download} %{time_total}", url, check=False, timeout=timeout + 5)
    size, seconds = (done.stdout.split() + ["0", "0"])[:2]
    return done.returncode, int(float(size)), float(seconds)


def hold_port(host: Host, port: int) -> None:
    """在 `host` 上占住一个 TCP 端口（别的程序在监听它），直到实验环境关闭。"""
    code = "import socket, time\ns = socket.socket()\ns.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n" \
           f"s.bind(('127.0.0.1', {port}))\ns.listen()\ntime.sleep(3600)\n"
    host.spawn(sys.executable, "-c", code)
    wait_until(lambda: f"127.0.0.1:{port} " in host.run("ss", "-lnt").stdout, what=f"{host.name} 上的 {port} 端口被占住")


# 下面是独占 TCP（lane）的场景（test_lane.py）用的。

# 一个按路径决定怎么回答的 HTTP 服务，同时听明文和 TLS。数据是按长度做种子的伪随机字节，测试那边算得出它的摘要，
# 收到的字节一个不差才对得上。参数：监听地址、明文端口、TLS 端口、证书、私钥。
#   GET  /blob/<n>         n 字节
#   POST /digest           回答请求体的 SHA-256（十六进制）
#   GET  /reset/<n>        说有 10n 字节，发出 n 字节之后用 RST 断开（上游在回答中途出了事）
#   GET  /slow/<s>/<n>     n 字节：先发一半，静默 s 秒，再发另一半
BLOB_SERVER = r"""
import hashlib, http.server, random, socket, socketserver, ssl, struct, sys, threading, time
def blob(n):
    return random.Random(n).randbytes(n)
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def answer(self, length):
        self.send_response(200)
        self.send_header("Content-Length", str(length))
        self.end_headers()
    def do_GET(self):
        parts = self.path.strip("/").split("/")
        if parts[0] == "blob":
            body = blob(int(parts[1]))
            self.answer(len(body))
            self.wfile.write(body)
        elif parts[0] == "reset":
            n = int(parts[1])
            self.answer(10 * n)
            self.wfile.write(blob(n))
            self.wfile.flush()
            time.sleep(0.5)
            # 不发 TLS 的 close_notify，也不发 FIN：直接 RST。
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            self.connection.close()
            self.close_connection = True
        elif parts[0] == "slow":
            seconds, n = float(parts[1]), int(parts[2])
            body = blob(n)
            self.answer(n)
            self.wfile.write(body[:n // 2])
            self.wfile.flush()
            time.sleep(seconds)
            self.wfile.write(body[n // 2:])
        else:
            self.send_error(404)
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        digest = hashlib.sha256()
        while length > 0:
            chunk = self.rfile.read(min(length, 1 << 16))
            if not chunk:
                break
            digest.update(chunk)
            length -= len(chunk)
        body = digest.hexdigest().encode()
        self.answer(len(body))
        self.wfile.write(body)
    def log_message(self, *args):
        pass
class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 1024
bind, plain, secure, cert, key = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5]
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
tls = Server((bind, secure), Handler)
tls.socket = context.wrap_socket(tls.socket, server_side=True)
threading.Thread(target=tls.serve_forever, daemon=True).start()
Server((bind, plain), Handler).serve_forever()
"""


def blob_digest(size: int) -> str:
    """`BLOB_SERVER` 的 /blob/<size> 回答的字节的 SHA-256。"""
    import hashlib
    import random

    return hashlib.sha256(random.Random(size).randbytes(size)).hexdigest()


def start_blob_server(host: Host, work: Path) -> Path:
    """在 `host` 上起 `BLOB_SERVER`，明文在 80、TLS 在 443（出口默认只放行这两个端口）。返回给客户端用的证书。"""
    cert, key = work / f"{host.name}-cert.pem", work / f"{host.name}-key.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "2",
                    "-subj", f"/CN={host.ip}", "-addext", f"subjectAltName=IP:{host.ip}", "-keyout", str(key), "-out", str(cert)],
                   check=True, capture_output=True)
    host.spawn(sys.executable, "-c", BLOB_SERVER, "0.0.0.0", "80", "443", str(cert), str(key))
    wait_until(lambda: ":443 " in host.run("ss", "-lnt").stdout and ":80 " in host.run("ss", "-lnt").stdout,
               what=f"{host.name} 上的数据服务启动")
    return cert


def fetch(node: Node, url: str, *, via: Node | None, cert: Path | None = None, proxy: str = "socks5", timeout: float = 60,
          upload: Path | None = None) -> tuple[int, str, str]:
    """从 `node` 所在的机器经它的本机代理入口取 `url`，返回 (curl 的退出码, 应答体, curl 的错误输出)。

    应答体比 1KB 长、或者不是文本时换成它的 SHA-256（十六进制）。`upload` 是要 POST 上去的文件。
    """
    user = f"{via.virtual_ip}@" if via else ""
    if proxy == "socks5":
        proxy_args = f"-x socks5h://{user}127.0.0.1:1080"
    else:
        proxy_args = f"-p -x http://{user.rstrip('@') + ':x@' if via else ''}127.0.0.1:1081"
    tls = f"--cacert {cert}" if cert else ""
    body = f"--data-binary @{upload} -H 'Content-Type: application/octet-stream'" if upload else ""
    answer = node.state / "answer"
    script = (f"curl -sS --max-time {timeout} {proxy_args} {tls} {body} -o {answer} '{url}'; code=$?; "
              f"if [ $(stat -c %s {answer} 2>/dev/null || echo 0) -le 1024 ] && iconv -f utf-8 -t utf-8 {answer} >/dev/null 2>&1; "
              f"then cat {answer}; elif [ -e {answer} ]; then sha256sum {answer} | cut -d' ' -f1; fi; rm -f {answer}; echo $code >&2")
    done = node.host.run("bash", "-c", script, check=False, timeout=timeout + 10)
    lines = done.stderr.strip().splitlines()
    code = int(lines[-1]) if lines and lines[-1].isdigit() else -1
    return code, done.stdout.strip(), "\n".join(lines[:-1])


def blob_file(node: Node, size: int) -> Path:
    """`node` 的状态目录里一个内容和 /blob/<size> 一样的文件，上传用。"""
    import random

    path = node.state / f"blob-{size}"
    if not path.exists():
        path.write_bytes(random.Random(size).randbytes(size))
    return path


def node_metrics(node: Node) -> str:
    """节点本机的 /metrics。"""
    return node.host.run("curl", "-sS", "--max-time", "5", "http://127.0.0.1:20333/metrics", check=False).stdout


def metric(text: str, name: str, **labels: str) -> float:
    """`text`（Prometheus 文本）里名字是 `name`、标签都对得上的样本之和。"""
    import re

    total = 0.0
    pattern = re.compile(rf"^{re.escape(name)}(\{{(.*?)\}})? (\S+)$")
    for line in text.splitlines():
        match = pattern.match(line)
        if not match:
            continue
        found = dict(re.findall(r'(\w+)="([^"]*)"', match.group(2) or ""))
        if all(found.get(key) == value for key, value in labels.items()):
            total += float(match.group(3))
    return total


LANE_COUNTERS = ("claims", "claims_waited", "claims_failed", "fallbacks", "dial_failures", "refused")


def lane_counts(node: Node, peer: Node | None = None) -> dict[str, float]:
    """`node` 上和 `peer`（不写时是全部对端）之间的独占 TCP 的计数和水位，读自它的 /metrics。"""
    text = node_metrics(node)
    labels = {"peer": peer.name} if peer else {}
    counts = {name: metric(text, f"i3t_lane_{name}_total", **labels) for name in LANE_COUNTERS}
    counts["claim_wait_ms"] = metric(text, "i3t_lane_claim_wait_milliseconds_total", **labels)
    counts["in_use"] = metric(text, "i3t_lane_in_use", **labels)
    counts["dialling"] = metric(text, "i3t_lane_dialling", **labels)
    counts["control_up"] = metric(text, "i3t_lane_control_up", **labels)
    for pool in ("parked_here", "ready", "parked_at_peer"):
        counts[pool] = metric(text, "i3t_lane_idle", pool=pool, **labels)
    for mode in ("sealed", "passthrough", "spliced"):
        counts[f"{mode}_bytes"] = metric(text, "i3t_lane_bytes_total", mode=mode, **labels)
    counts["carried_bytes"] = counts["sealed_bytes"] + counts["passthrough_bytes"] + counts["spliced_bytes"]
    for reason in ("deadline", "unknown_key", "tls", "protocol", "busy"):
        counts[f"handshake_{reason}"] = metric(text, "i3t_lane_handshake_failures_total", reason=reason)
    return counts


def lane_line(node: Node, peer: Node | None = None) -> str:
    """一行给人看的独占 TCP 计数。"""
    c = lane_counts(node, peer)
    return (f"{node.name}：控制 {c['control_up']:.0f}，空闲 {c['parked_here']:.0f}/{c['ready']:.0f}/{c['parked_at_peer']:.0f}，"
            f"在用 {c['in_use']:.0f}，取用 {c['claims']:.0f}（等过 {c['claims_waited']:.0f}，共 {c['claim_wait_ms']:.0f}ms，"
            f"没取到 {c['claims_failed']:.0f}），改走共用 {c['fallbacks']:.0f}，拨号失败 {c['dial_failures']:.0f}，拒绝 {c['refused']:.0f}，"
            f"字节 加密 {c['sealed_bytes'] / 1e6:.1f}MB / 程序搬 {c['passthrough_bytes'] / 1e6:.1f}MB / 内核搬 {c['spliced_bytes'] / 1e6:.1f}MB")


def lane_peer(node: Node, *, lane: str, **options) -> dict:
    """别的节点的 netmap 里打开了独占 TCP 的 `node`。"""
    return {**node.as_peer(**options), "lane": lane}


def set_nat_timeouts(nat: Host, udp_s: int, tcp_s: int) -> None:
    """把 NAT 回收 UDP 和 TCP 映射的时间调短，像家用路由器那样。"""
    nat.run("sysctl", "-qw", f"net.netfilter.nf_conntrack_udp_timeout={udp_s}", f"net.netfilter.nf_conntrack_udp_timeout_stream={udp_s}",
            f"net.netfilter.nf_conntrack_tcp_timeout_established={tcp_s}")


def block_tcp_port(host: Host, port: int, blocked: bool = True) -> None:
    """`host` 不接受别人连它的 TCP 端口 `port`（直接丢掉，像防火墙那样），别的流量不受影响。"""
    host.run("nft", "delete", "table", "inet", "notcp", check=False)
    if blocked:
        rules = f"""
table inet notcp {{
    chain in {{
        type filter hook input priority filter;
        iifname != "lo" tcp dport {port} drop
    }}
}}
"""
        host.run("nft", "-f", "-", input=rules)


# 下面是主动测试（test_probe.py）用的。

def ctl_wait(controller: Controller, *args: str, timeout: float = 300) -> object:
    """执行一条要等一阵的 i3tctl 命令（主动测试会等到结果出来），返回管理 API 的 JSON。"""
    done = controller.host.run(binary("i3tctl"), "--token", controller.token, "--json", *args, check=False, timeout=timeout)
    if done.returncode != 0:
        raise RuntimeError(f"i3tctl {' '.join(args)} 失败：{done.stderr.strip() or done.stdout.strip()}")
    return json.loads(done.stdout)


def active_test(controller: Controller, a: Node, b: Node, *options: str, timeout: float = 300) -> dict:
    """在 `a` 和 `b` 之间做一次主动测试（`i3tctl test`），等它做完，返回结果。"""
    return ctl_wait(controller, "test", a.name, b.name, *options, timeout=timeout)


def forward_route(controller: Controller, a: Node, b: Node) -> dict:
    """controller 现问 `a`：到 `b` 现在走哪条路。"""
    return controller.ctl("path", str(a.id), str(b.id))["forward"].get("route") or {}


def add_node_config(node: Node, text: str) -> None:
    """在节点的配置文件末尾加几行。要在节点启动之前。"""
    with (node.state / "config.toml").open("a") as config:
        config.write(text)
