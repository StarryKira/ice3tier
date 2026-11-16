#!/usr/bin/env python3
"""拥塞控制的对比测试：在本机的实验环境里起真实的 i3t 节点，经出口下载，比较 Cubic 和自己的 BBR。

    python3 bench/congestion/bench.py [场景名...] [--out 结果目录]

场景：matrix（单条下载，各种往返时间 / 丢包 / 带宽）、share（两条连接共用一个瓶颈）、
switch（下载途中换路）、ratedrop（下载途中带宽从 100 降到 20 Mbit/s）、tcprelay（一端只能用 TCP 连 relay）。
不写场景名就全跑。

拓扑：web 是下载源，b 是出口节点（发送方，拥塞控制用环境变量 I3T_CONGESTION 指定），a 是入口节点（接收方）。
网络条件加在 a 的上联线上，web 和 b 之间不加限制，所以瓶颈就是 b → a 这条 QUIC 连接经过的那条线。

要先构建可执行文件，并用环境变量 I3T_BIN_DIR 指到它们所在的目录（默认 target/release）。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tests" / "e2e"))
import lab as lablib  # noqa: E402
from harness import Node  # noqa: E402
from lab import Host, Lab, Shape, sh, wait_until  # noqa: E402

ALGORITHMS = ("cubic", "bbr")
STREAM_WINDOW = 2 * 1024 * 1024

# 下载源：/bytes/N 一块一块地发 N 字节，不把整个响应放进内存；其他路径回一个很短的响应。
SOURCE = r"""
import http.server, socketserver, sys
CHUNK = b"x" * 65536
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        size = int(self.path.rsplit("/", 1)[1]) if self.path.startswith("/bytes/") else 2
        self.send_response(200)
        self.send_header("Content-Length", str(size))
        self.end_headers()
        try:
            while size > 0:
                size -= self.wfile.write(CHUNK[:min(size, len(CHUNK))])
        except OSError:
            pass
    def log_message(self, *args):
        pass
class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 1024
Server(("0.0.0.0", 80), Handler).serve_forever()
"""

# 经本机的 SOCKS5 入口做一件事，结果在标准输出打一行 JSON。
#   download <出口的虚拟 IP> <目标 IP> <秒数> [条数]：同时开几条下载（默认一条），记下每 100ms 一共收到的字节数；
#   echo <出口的虚拟 IP> <目标 IP> <秒数> <间隔秒>：反复发小请求，记下每次从发起到收完用了多久。
CLIENT = r"""
import json, select, socket, sys, time

def connect(user, target):
    sock = socket.create_connection(("127.0.0.1", 1080), timeout=30)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.sendall(b"\x05\x01\x02")
    assert sock.recv(2) == b"\x05\x02", "入口不接受用户名认证"
    sock.sendall(bytes([1, len(user)]) + user.encode() + b"\x00")
    assert sock.recv(2) == b"\x01\x00", "入口拒绝了用户名"
    sock.sendall(b"\x05\x01\x00\x01" + socket.inet_aton(target) + (80).to_bytes(2, "big"))
    reply = sock.recv(10)
    assert len(reply) >= 2 and reply[1] == 0, f"建流失败：{reply!r}"
    return sock

mode, user, target, seconds = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
if mode == "download":
    socks = [connect(user, target) for _ in range(int(sys.argv[5]) if len(sys.argv) > 5 else 1)]
    for sock in socks:
        sock.sendall(b"GET /bytes/200000000000 HTTP/1.1\r\nHost: x\r\n\r\n")
    buckets, error = [0] * int(seconds * 10), None
    buffer = bytearray(1 << 20)
    start = time.monotonic()
    try:
        while error is None:
            index = int((time.monotonic() - start) * 10)
            if index >= len(buckets):
                break
            for sock in select.select(socks, [], [], 0.05)[0]:
                got = sock.recv_into(buffer)
                if got == 0:
                    error = "连接被对端关闭"
                buckets[index] += got
    except OSError as err:
        error = repr(err)
    print(json.dumps({"buckets": buckets, "error": error}))
else:
    interval, samples, errors = float(sys.argv[5]), [], 0
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        began = time.monotonic()
        try:
            sock = connect(user, target)
            sock.sendall(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            while sock.recv(4096):
                pass
            sock.close()
            samples.append(round((time.monotonic() - began) * 1000, 1))
        except (OSError, AssertionError):
            errors += 1
        time.sleep(max(0.0, interval - (time.monotonic() - began)))
    print(json.dumps({"samples": samples, "errors": errors}))
"""

METER = re.compile(r"拥塞控制统计.*?conn=(\d+) algorithm=(\w+) sent=(\d+) acked=(\d+) lost=(\d+)(?: window=(\d+) rtt_us=(\d+) rtt_max_us=(\d+) min_rtt_us=(\d+))?")


def start_node(node: Node, netmap: dict, algorithm: str | None = None) -> None:
    """启动一个节点。`algorithm` 是它作为发送方时用的拥塞控制；日志只留拥塞控制的统计，免得影响吞吐。"""
    env = {"I3T_LOG": "info,i3t_conn::congestion=debug"}
    if algorithm:
        env["I3T_CONGESTION"] = algorithm
    saved = {name: os.environ.get(name) for name in env}
    os.environ.update(env)
    try:
        node.start_static(netmap)
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def start_source(host: Host) -> None:
    host.spawn(sys.executable, "-c", SOURCE)
    wait_until(lambda: ":80 " in host.run("ss", "-lnt").stdout, what="下载源启动")


def client(node: Node, *args: object) -> subprocess.Popen:
    cmd = node.host.cmd(sys.executable, "-c", CLIENT, *[str(arg) for arg in args])
    return subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def finish(proc: subprocess.Popen, timeout: float = 120) -> dict:
    out, err = proc.communicate(timeout=timeout)
    if proc.returncode != 0 or not out.strip():
        return {"buckets": [], "samples": [], "errors": 1, "error": (err.strip().splitlines() or ["没有输出"])[-1]}
    return json.loads(out)


def reshape(lab: Lab, host: Host, shape: Shape) -> None:
    """不清空队列地改一条线的网络条件（`Lab.shape` 是先删后加，排着队的包会丢）。"""
    for namespace, dev in ((host.name, "up0"), ("core", f"c-{host.name}"[:15])):
        sh("tc", "-n", namespace, "qdisc", "change", "dev", dev, "root", "netem", *shape.netem_args())


def bottleneck_drops(host: Host) -> int:
    """发往 `host` 的那个方向上，限速器一共丢了多少个包（随机丢包和排队溢出都算）。"""
    stats = sh("tc", "-n", "core", "-s", "qdisc", "show", "dev", f"c-{host.name}"[:15], check=False).stdout
    found = re.search(r"dropped (\d+)", stats)
    return int(found.group(1)) if found else 0


def meter(node: Node) -> list[dict]:
    """发送方日志里每条连接的统计：累计字节数，和每秒一次的窗口与往返时间。"""
    conns: dict[str, dict] = {}
    for line in node.read_log().splitlines():
        found = METER.search(line)
        if not found:
            continue
        conn, algorithm, sent, acked, lost, window, rtt, rtt_max, min_rtt = found.groups()
        entry = conns.setdefault(conn, {"algorithm": algorithm, "rtt_ms": [], "rtt_max_ms": [], "window": []})
        entry.update(sent=int(sent), acked=int(acked), lost=int(lost))
        if window:
            entry["window"].append(int(window))
            entry["rtt_ms"].append(int(rtt) / 1000)
            entry["rtt_max_ms"].append(int(rtt_max) / 1000)
            entry["min_rtt_ms"] = int(min_rtt) / 1000
    # 只要真正传过数据的连接。
    return [entry for entry in conns.values() if entry.get("sent", 0) > 1_000_000]


def mbit(buckets: list[int], start: float, end: float) -> float:
    chosen = buckets[int(start * 10):int(end * 10)]
    return sum(chosen) * 8 / 1e6 / (len(chosen) / 10) if chosen else 0.0


def series(buckets: list[int]) -> list[float]:
    """每秒的吞吐，Mbit/s。"""
    return [round(sum(buckets[i:i + 10]) * 8 / 1e6, 1) for i in range(0, len(buckets) - 9, 10)]


def summarize(download: dict, sender: Node, seconds: float, warmup: float) -> dict:
    stats = meter(sender)
    conn = stats[-1] if stats else {}
    sent, lost = conn.get("sent", 0), conn.get("lost", 0)
    rtts = conn.get("rtt_ms", [])[int(warmup):]
    return {
        "mbit": round(mbit(download["buckets"], warmup, seconds), 2),
        "mbit_all": round(mbit(download["buckets"], 0, seconds), 2),
        "loss_pct": round(100 * lost / sent, 2) if sent else None,
        "rtt_ms": round(statistics.mean(rtts), 1) if rtts else None,
        "rtt_max_ms": round(max(conn.get("rtt_max_ms", [0])[int(warmup):] or [0]), 1),
        "min_rtt_ms": conn.get("min_rtt_ms"),
        "error": download.get("error"),
        "series": series(download["buckets"]),
    }


def percentile(samples: list[float], fraction: float) -> float | None:
    if not samples:
        return None
    ordered = sorted(samples)
    return ordered[min(len(ordered) - 1, int(len(ordered) * fraction))]


def pair(lab: Lab, work: Path, algorithm: str) -> tuple[Node, Node, Host]:
    """web、入口 a、出口 b；a 和 b 直连。"""
    web = lab.host("web")
    start_source(web)
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2, declare=("exit",))
    start_node(a, a.netmap(peers=[b.as_peer(exit=True)]))
    start_node(b, b.netmap(peers=[a.as_peer()], me={"exit": {}}), algorithm)
    return a, b, web


CONDITIONS = [
    # (往返 ms, 丢包 %, 带宽 Mbit/s；0 表示不限, 同时几条下载)
    (0, 0, 0, 1),
    (50, 0, 20, 1),
    (50, 0, 100, 1),
    (50, 1, 100, 1),
    (150, 1, 100, 1),
    (150, 0, 1000, 1),
    (50, 3, 100, 1),
    # 单条流的吞吐受流的接收窗口限制（窗口 ÷ 往返时间；有丢包时还要等重传填上空洞）。
    # 下面几行在同一条连接上同时开 8 条下载，看的是连接本身（也就是拥塞控制）能到多少。
    (50, 1, 100, 8),
    (150, 1, 100, 8),
    (150, 0, 1000, 8),
    (50, 3, 100, 8),
]


def matrix(out: Path, seconds: float = 15, warmup: float = 3, conditions=CONDITIONS, echo: bool = True) -> list[dict]:
    """单条下载。每种条件、每种拥塞控制各用一套全新的节点。"""
    rows = []
    for rtt, loss, rate, streams in conditions:
        for algorithm in ALGORITHMS:
            with tempfile.TemporaryDirectory(prefix="i3t-cc-") as work, Lab() as lab:
                a, b, web = pair(lab, Path(work), algorithm)
                a.host.shape(Shape(rtt_ms=rtt, loss_pct=loss, rate_mbit=rate))
                bulk = client(a, "download", b.virtual_ip, web.ip, seconds, streams)
                pings = client(a, "echo", b.virtual_ip, web.ip, seconds, 0.2) if echo else None
                row = summarize(finish(bulk), b, seconds, warmup)
                latency = finish(pings)["samples"] if pings else []
                drops = bottleneck_drops(a.host)
                save_log(out, f"matrix-{rtt:g}-{loss:g}-{rate:g}-{streams}-{algorithm}", b)
            row.update(rtt=rtt, loss=loss, rate=rate, streams=streams, algorithm=algorithm, drops=drops,
                       echo_p50=percentile(latency, 0.5), echo_p99=percentile(latency, 0.99))
            print(json.dumps(row, ensure_ascii=False), flush=True)
            rows.append(row)
            time.sleep(0.3)
    return rows


def share(out: Path, seconds: float = 40, late: float = 8) -> list[dict]:
    """两条连接（两个出口 b1、b2 各自往 a 发）共用 a 的上联线：50ms、100 Mbit/s。第二条晚 `late` 秒开始。"""
    rows = []
    for first, second in (("bbr", "bbr"), ("bbr", "cubic"), ("cubic", "bbr"), ("cubic", "cubic")):
        with tempfile.TemporaryDirectory(prefix="i3t-cc-") as tmp, Lab() as lab:
            work = Path(tmp)
            web = lab.host("web")
            start_source(web)
            a = Node(lab.host("a"), work, 1)
            b1, b2 = Node(lab.host("b1"), work, 2, declare=("exit",)), Node(lab.host("b2"), work, 3, declare=("exit",))
            start_node(a, a.netmap(peers=[b1.as_peer(exit=True), b2.as_peer(exit=True)]))
            start_node(b1, b1.netmap(peers=[a.as_peer()], me={"exit": {}}), first)
            start_node(b2, b2.netmap(peers=[a.as_peer()], me={"exit": {}}), second)
            a.host.shape(Shape(rtt_ms=50, rate_mbit=100))
            one = client(a, "download", b1.virtual_ip, web.ip, seconds)
            time.sleep(late)
            two = client(a, "download", b2.virtual_ip, web.ip, seconds - late)
            got_one, got_two = finish(one), finish(two)
            flows = [summarize(got_one, b1, seconds, late + 5), summarize(got_two, b2, seconds - late, 5)]
            save_log(out, f"share-{first}-{second}-1", b1)
            save_log(out, f"share-{first}-{second}-2", b2)
        row = {"first": first, "second": second, "alone_mbit": round(mbit(got_one["buckets"], 3, late), 1),
               "first_mbit": flows[0]["mbit"], "second_mbit": flows[1]["mbit"],
               "first_loss_pct": flows[0]["loss_pct"], "second_loss_pct": flows[1]["loss_pct"],
               "first_rtt_ms": flows[0]["rtt_ms"], "second_rtt_ms": flows[1]["rtt_ms"],
               "first_series": flows[0]["series"], "second_series": flows[1]["series"],
               "errors": [flow["error"] for flow in flows if flow["error"]]}
        print(json.dumps(row, ensure_ascii=False), flush=True)
        rows.append(row)
        time.sleep(0.3)
    return rows


def block_direct(lab: Lab, a: Node, b: Node, blocked: bool) -> None:
    """在"互联网"上丢掉 a 和 b 之间直接来往的 UDP。经 relay 的包不受影响。"""
    core = Host(lab, "core", "")
    core.run("nft", "delete", "table", "inet", "nodirect", check=False)
    if blocked:
        core.run("nft", "-f", "-", input=f"""
table inet nodirect {{
    chain forward {{
        type filter hook forward priority filter;
        ip saddr {a.host.ip} ip daddr {b.host.ip} meta l4proto udp drop
        ip saddr {b.host.ip} ip daddr {a.host.ip} meta l4proto udp drop
    }}
}}
""")


def switch(out: Path, seconds: float = 30) -> list[dict]:
    """下载途中换路。

    relay-to-direct：直连被挡住，连接经 relay（往返 100ms）；第 8 秒放开，路径层探测通了之后换到直连（30ms）。
    这是产品里真实的换路。反方向在产品里只有直连断掉时才发生，中间有几秒谁也到不了谁，所以另做两个：
    direct-to-relay：第 8 秒挡住直连，路径层发现之后改走 relay；
    rtt-step：同一条路，第 8 秒把往返从 30ms 改成 100ms，第 18 秒改回去，相当于没有中断的换路。
    """
    rows = []
    for scenario in ("relay-to-direct", "direct-to-relay", "rtt-step"):
        for algorithm in ALGORITHMS:
            with tempfile.TemporaryDirectory(prefix="i3t-cc-") as tmp, Lab() as lab:
                work = Path(tmp)
                web = lab.host("web")
                start_source(web)
                r = Node(lab.host("r"), work, 9, declare=("relay",))
                a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2, declare=("exit",))
                start_node(r, r.netmap(me={"relay": {}}, relays=[r.as_relay()], relay_clients=[a, b, r]))
                start_node(a, a.netmap(peers=[b.as_peer(attached=[(r, 5)], exit=True)], relays=[r.as_relay()]))
                start_node(b, b.netmap(peers=[a.as_peer(attached=[(r, 5)])], relays=[r.as_relay()], me={"exit": {}}), algorithm)
                a.host.shape(Shape(rtt_ms=30, rate_mbit=100))
                # 经 relay 的包在 r 的线上一进一出，各 17.5ms：比直连多 70ms 往返。
                r.host.shape(Shape(rtt_ms=35))
                if scenario == "relay-to-direct":
                    block_direct(lab, a, b, True)
                    time.sleep(12)  # 等路径层认定直连不通
                bulk = client(a, "download", b.virtual_ip, web.ip, seconds)
                time.sleep(8)
                if scenario == "relay-to-direct":
                    block_direct(lab, a, b, False)
                elif scenario == "direct-to-relay":
                    block_direct(lab, a, b, True)
                else:
                    reshape(lab, a.host, Shape(rtt_ms=100, rate_mbit=100))
                    time.sleep(10)
                    reshape(lab, a.host, Shape(rtt_ms=30, rate_mbit=100))
                got = finish(bulk)
                row = summarize(got, b, seconds, 3)
                changes = [line.split()[0][11:23] + " " + line.split("reason=", 1)[-1] for line in b.read_log().splitlines() if "路变了" in line]
                save_log(out, f"switch-{scenario}-{algorithm}", b)
            row.update(scenario=scenario, algorithm=algorithm, route_changes=changes[:6])
            print(json.dumps(row, ensure_ascii=False), flush=True)
            rows.append(row)
            time.sleep(0.3)
    return rows


def ratedrop(out: Path, seconds: float = 30) -> list[dict]:
    """下载途中带宽从 100 降到 20 Mbit/s（第 10 秒），第 20 秒恢复。往返 50ms。"""
    rows = []
    for algorithm in ALGORITHMS:
        with tempfile.TemporaryDirectory(prefix="i3t-cc-") as work, Lab() as lab:
            a, b, web = pair(lab, Path(work), algorithm)
            a.host.shape(Shape(rtt_ms=50, rate_mbit=100))
            bulk = client(a, "download", b.virtual_ip, web.ip, seconds)
            time.sleep(10)
            lost_before = sum(conn["lost"] for conn in meter(b))
            reshape(lab, a.host, Shape(rtt_ms=50, rate_mbit=20))
            time.sleep(10)
            during = meter(b)
            reshape(lab, a.host, Shape(rtt_ms=50, rate_mbit=100))
            got = finish(bulk)
            row = summarize(got, b, seconds, 3)
            save_log(out, f"ratedrop-{algorithm}", b)
        conn = during[-1] if during else {}
        row.update(algorithm=algorithm, lost_bytes_during_low=sum(c["lost"] for c in during) - lost_before,
                   rtt_series=[round(value) for value in conn.get("rtt_ms", [])])
        print(json.dumps(row, ensure_ascii=False), flush=True)
        rows.append(row)
        time.sleep(0.3)
    return rows


def tcprelay(out: Path, seconds: float = 20) -> list[dict]:
    """a 用不了 UDP，用 TCP 连 relay；b 用 UDP。a 的线（也就是那一段 TCP）往返 50ms、丢包 1%，不限速。"""
    rows = []
    for loss in (0, 1):
        for algorithm in ALGORITHMS:
            with tempfile.TemporaryDirectory(prefix="i3t-cc-") as tmp, Lab() as lab:
                work = Path(tmp)
                web = lab.host("web")
                start_source(web)
                r = Node(lab.host("r"), work, 9, declare=("relay",))
                a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2, declare=("exit",))
                a.host.block_udp()
                start_node(r, r.netmap(me={"relay": {}}, relays=[r.as_relay()], relay_clients=[a, b, r]))
                start_node(a, a.netmap(peers=[b.as_peer(endpoints=False, attached=[(r, 5)], exit=True)], relays=[r.as_relay()]))
                start_node(b, b.netmap(peers=[a.as_peer(endpoints=False, attached=[(r, 5)])], relays=[r.as_relay()], me={"exit": {}}), algorithm)
                a.host.shape(Shape(rtt_ms=50, loss_pct=loss))
                wait_until(lambda: "中转会话建立（TCP）" in a.read_log(), timeout=30, what="a 用 TCP 挂上 relay")
                idle = finish(client(a, "echo", b.virtual_ip, web.ip, 4, 0.2))["samples"]
                bulk = client(a, "download", b.virtual_ip, web.ip, seconds)
                pings = client(a, "echo", b.virtual_ip, web.ip, seconds, 0.2)
                row = summarize(finish(bulk), b, seconds, 3)
                echo = finish(pings)
                save_log(out, f"tcprelay-{loss}-{algorithm}", b)
            row.update(loss=loss, algorithm=algorithm, echo_idle_p50=percentile(idle, 0.5),
                       echo_p50=percentile(echo["samples"], 0.5), echo_p99=percentile(echo["samples"], 0.99),
                       echo_max=max(echo["samples"], default=None), echo_errors=echo["errors"])
            print(json.dumps(row, ensure_ascii=False), flush=True)
            rows.append(row)
            time.sleep(0.3)
    return rows


def save_log(out: Path, name: str, node: Node) -> None:
    lines = [line for line in node.read_log().splitlines() if any(mark in line for mark in ("拥塞控制", "路变了", "ERROR", "WARN"))]
    (out / f"{name}.log").write_text("\n".join(lines) + "\n")


def cap_mbit(rtt: float) -> float | None:
    """单条流受接收窗口限制的吞吐上限。"""
    return STREAM_WINDOW * 8 / 1e6 / (rtt / 1000) if rtt else None


def report(results: dict) -> str:
    lines = []
    if rows := results.get("matrix"):
        lines += ["## 单条下载", "",
                  "| 往返 ms | 丢包 % | 带宽 Mbit/s | 下载条数 | 拥塞控制 | 吞吐 Mbit/s | 发送方丢包率 % | 平滑往返 ms（平均 / 最大） | 小请求往返 ms（p50 / p99） | 出错 |",
                  "|---|---|---|---|---|---|---|---|---|---|"]
        for row in rows:
            cap = cap_mbit(row["rtt"])
            note = f"（单条流的窗口上限 {cap:.0f}）" if cap and row["streams"] == 1 and (not row["rate"] or cap < row["rate"] * 1.2) else ""
            lines.append(f"| {row['rtt']:g} | {row['loss']:g} | {format(row['rate'], 'g') if row['rate'] else '不限'} | {row['streams']} | {row['algorithm']} | {row['mbit']}{note} | {row['loss_pct']} | "
                         f"{row['rtt_ms']} / {row['rtt_max_ms']} | {row['echo_p50']} / {row['echo_p99']} | {row['error'] or '无'} |")
        lines.append("")
    if rows := results.get("share"):
        lines += ["## 两条连接共用 100 Mbit/s、50ms 的瓶颈", "",
                  "| 先开始的 | 后开始的 | 先开始的独占时 | 共用时：先 / 后 Mbit/s | 丢包率 %：先 / 后 | 平滑往返 ms：先 / 后 | 出错 |", "|---|---|---|---|---|---|---|"]
        for row in rows:
            lines.append(f"| {row['first']} | {row['second']} | {row['alone_mbit']} | {row['first_mbit']} / {row['second_mbit']} | "
                         f"{row['first_loss_pct']} / {row['second_loss_pct']} | {row['first_rtt_ms']} / {row['second_rtt_ms']} | {row['errors'] or '无'} |")
        lines.append("")
    if rows := results.get("switch"):
        lines += ["## 下载途中换路（第 8 秒）", "", "| 场景 | 拥塞控制 | 每秒吞吐 Mbit/s | 发送方丢包率 % | 出错 |", "|---|---|---|---|---|"]
        for row in rows:
            lines.append(f"| {row['scenario']} | {row['algorithm']} | {' '.join(f'{value:.0f}' for value in row['series'])} | {row['loss_pct']} | {row['error'] or '无'} |")
        lines.append("")
    if rows := results.get("ratedrop"):
        lines += ["## 带宽 100 → 20（第 10 秒）→ 100（第 20 秒）Mbit/s，往返 50ms", "",
                  "| 拥塞控制 | 每秒吞吐 Mbit/s | 每秒的平滑往返 ms（前 20 秒） | 低带宽期间丢的字节 | 全程丢包率 % | 出错 |", "|---|---|---|---|---|---|"]
        for row in rows:
            lines.append(f"| {row['algorithm']} | {' '.join(f'{value:.0f}' for value in row['series'])} | {' '.join(str(value) for value in row['rtt_series'])} | "
                         f"{row['lost_bytes_during_low']} | {row['loss_pct']} | {row['error'] or '无'} |")
        lines.append("")
    if rows := results.get("tcprelay"):
        lines += ["## 一端经 TCP 连 relay（那一段往返 50ms）", "",
                  "| TCP 段丢包 % | 拥塞控制 | 吞吐 Mbit/s | 发送方丢包率 % | 小请求往返 ms：空闲 p50 | 下载时 p50 / p99 / 最大 | 出错 |", "|---|---|---|---|---|---|---|"]
        for row in rows:
            lines.append(f"| {row['loss']} | {row['algorithm']} | {row['mbit']} | {row['loss_pct']} | {row['echo_idle_p50']} | "
                         f"{row['echo_p50']} / {row['echo_p99']} / {row['echo_max']} | {row['error'] or ('小请求失败 %d 次' % row['echo_errors'] if row['echo_errors'] else '无')} |")
        lines.append("")
    return "\n".join(lines)


SCENARIOS = {"matrix": matrix, "share": share, "switch": switch, "ratedrop": ratedrop, "tcprelay": tcprelay}


def main() -> int:
    lablib.enter()
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("scenarios", nargs="*", help=f"要跑的场景，默认全部：{' '.join(SCENARIOS)}")
    parser.add_argument("--out", type=Path, default=Path(tempfile.gettempdir()) / "i3t-congestion-bench", help="结果和日志放在哪里")
    parser.add_argument("--conditions", help="matrix 只跑这些条件，写法是 往返/丢包/带宽[/同时几条下载]，逗号分隔，例如 50/1/100,150/0/1000/8")
    args = parser.parse_args()
    if unknown := [name for name in args.scenarios if name not in SCENARIOS]:
        parser.error(f"没有这些场景：{' '.join(unknown)}")
    args.out.mkdir(parents=True, exist_ok=True)
    results = {}
    for name in args.scenarios or list(SCENARIOS):
        print(f"== {name}", flush=True)
        if name == "matrix" and args.conditions:
            chosen = [(*parts[:3], int(parts[3]) if len(parts) > 3 else 1)
                      for parts in ([float(part) for part in item.split("/")] for item in args.conditions.split(","))]
            results[name] = matrix(args.out, conditions=chosen)
        else:
            results[name] = SCENARIOS[name](args.out)
        (args.out / f"{name}.json").write_text(json.dumps(results[name], ensure_ascii=False, indent=1))
    table = report(results)
    (args.out / "report.md").write_text(table)
    print(table)
    return 0


if __name__ == "__main__":
    sys.exit(main())
