#!/usr/bin/env python3
"""LLM 网关负载的端到端测试：网关程序在入口节点旁边，经入口节点上经出口出去的代理把每个上游 API 请求送到出口节点，从出口的网络出去。

上游是 bench/llmgw 里模拟的 LLM API（HTTPS），放在实验环境"互联网"上的一台主机上；负载端 i3t-llmgw load 在入口那台机器上扮演网关。
每个场景先做功能上的检查（通过 / 失败），再打一张测量表，留着和以后的运行对比。

    python3 tests/e2e/test_llm.py [--long] [--keep] [--pair '{"字段": 值}'] [场景名...]

--long   sse_long_thinking 的静默从 75 秒改成 10 分钟。
--pair   按节点对的设置，原样合进入口和出口互相看到的对端条目（静态 netmap 的 peers）。见 apply_pair_setting。
         例如 '{"lane": "always"}' 让入口和出口之间备着 ice3proxy，入口上的代理也选上它（ice3proxy 是代理的选项，
         只有选了它的代理的流走它），代理的 TCP 流就走 ice3proxy；这时每次负载之后打出两边的 lane 计数。
--keep   留下每个场景的工作目录：节点日志、场景文件、每次负载的结果 JSON 和表。

需要 I3T_BIN_DIR 下有 i3t 和 i3t-llmgw（cargo build --release -p i3t -p i3t-llmgw）。
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import Checks, Node, binary, current_route, lane_line, route_changes, shape_between, tail  # noqa: E402
from lab import Host, Lab, Nat, NatKind, Shape, wait_until  # noqa: E402

# 家用路由器回收 UDP 映射比连接的保活（15 秒）慢一点、比连接的空闲超时（30 秒）快；TCP 的映射也设得短，
# 出口到上游的连接每 25 秒的内核保活要能把它续住。
NAT_UDP_TIMEOUT_S = 20
NAT_TCP_TIMEOUT_S = 40

LONG = False
PAIR_SETTING: dict = {}


def apply_pair_setting(entry_sees_exit: dict, exit_sees_entry: dict) -> None:
    """入口和出口这一对节点的设置。所有场景的入口—出口对都经过这里，要换这一对的传输方式只改这一处。

    参数是两边静态 netmap 里互相看到的对端条目（入口眼里的出口、出口眼里的入口）。现在什么都不做：
    `PAIR_SETTING` 默认是空的；命令行 `--pair '{"字段": 值}'` 给出的 JSON 原样合进两边的条目。
    """
    entry_sees_exit.update(PAIR_SETTING)
    exit_sees_entry.update(PAIR_SETTING)


def proxy_lane() -> bool:
    """入口上的代理选不选 ice3proxy：`--pair` 让这一对备着 lane 时选上，不然备着也没有流走它。"""
    return PAIR_SETTING.get("lane", "off") != "off"


def pair_label() -> str:
    return json.dumps(PAIR_SETTING, ensure_ascii=False) if PAIR_SETTING else "无"


def toml_value(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{key} = {toml_value(item)}" for key, item in value.items()) + " }"
    raise TypeError(f"写不成 TOML：{value!r}")


def scenario_toml(name: str, duration_s: float, groups: list[dict], stall_ms: int = 250) -> str:
    lines = [f"name = {toml_value(name)}", f"duration_s = {toml_value(duration_s)}", f"stall_ms = {stall_ms}", ""]
    for group in groups:
        lines.append("[[group]]")
        lines += [f"{key} = {toml_value(value)}" for key, value in group.items()]
        lines.append("")
    return "\n".join(lines)


def sse(name: str, count: int, **params) -> dict:
    return {"name": name, "kind": "sse", "count": count, **params}


def ws(name: str, count: int, **params) -> dict:
    return {"name": name, "kind": "ws", "count": count, **params}


def describe(groups: list[dict]) -> str:
    """场景里各组的参数，失败时打出来，说明发了什么。"""
    return "\n".join("    " + ", ".join(f"{key}={value}" for key, value in group.items()) for group in groups)


class Load:
    """一次负载运行：可以在前台跑完，也可以放到后台、过一会儿再等它的结果。"""

    def __init__(self, gw: "Deployment", name: str, duration_s: float, groups: list[dict], *, proxy: str = "socks5",
                 label: str = "", stall_ms: int = 250) -> None:
        self.gw, self.name, self.groups = gw, name, groups
        work = gw.work
        self.scenario = work / f"{name}.toml"
        self.scenario.write_text(scenario_toml(name, duration_s, groups, stall_ms))
        self.out = work / f"{name}.json"
        self.table = work / f"{name}.txt"
        # 入口上经出口出去的代理是 mixed 的：SOCKS5 和 HTTP 都连同一个端口。
        through = gw.entry.entry(gw.exit)
        proxy_url = {"socks5": through.url("socks5"), "http": through.url("http"), "direct": "direct"}[proxy]
        label = label or f"经入口和出口，节点对设置{pair_label()}"
        cmd = [binary("i3t-llmgw"), "load", "--scenario", str(self.scenario), "--target", f"https://{gw.api.ip}",
               "--ca", str(gw.ca), "--proxy", proxy_url, "--out", str(self.out), "--quiet", "--label", label]
        for node in gw.watched():
            cmd += ["--watch", f"{node.name}=state:{node.state}"]
        self.proc = subprocess.Popen(gw.entry.host.cmd(*cmd), stdout=self.table.open("w"), stderr=subprocess.STDOUT)
        gw.entry.host._procs.append(self.proc)
        self.timeout = duration_s + 60

    def wait(self) -> dict:
        try:
            self.proc.wait(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        text = self.table.read_text(errors="replace")
        print("\n".join("  | " + line for line in text.rstrip().splitlines()), flush=True)
        if not self.out.exists():
            raise RuntimeError(f"负载端没有写出结果（退出码 {self.proc.returncode}）：{text[-1500:]}")
        if "lane" in PAIR_SETTING:
            # 节点对打开了 ice3proxy：这次负载之后两边的 lane 计数（从启动累计），看流是不是真的走了 lane、有没有改走共用的连接。
            print(f"  | ice3proxy {lane_line(self.gw.entry, self.gw.exit)}\n  | ice3proxy {lane_line(self.gw.exit, self.gw.entry)}", flush=True)
        return json.loads(self.out.read_text())


def group(report: dict, name: str) -> dict:
    return next(entry for entry in report["groups"] if entry["name"] == name)


def outcomes(entry: dict) -> str:
    return "，".join(f"{key} {value}" for key, value in entry["outcomes"].items())


def all_as_expected(t: Checks, report: dict, what: str) -> bool:
    """每一组都按期望结束了；不是的话说清楚哪一组、怎么结束的。"""
    ok = True
    for entry in report["groups"]:
        detail = f"期望 {entry['expected']}，实际 {outcomes(entry)}"
        if entry["failures"]:
            first = entry["failures"][0]
            detail += f"；例如 {first['stream']}：{first['outcome']}，{first['detail']}"
        ok &= t.check(f"{what}：组 {entry['name']} 的 {entry['count']} 条都是 {entry['expected']}", entry["failed"] == 0, detail)
    return ok


def fmt(summary: dict, *fields: str) -> str:
    return "/".join(f"{summary[field]:.1f}" for field in fields) if summary["count"] else "-"


def key_numbers(report: dict) -> list[str]:
    """一次运行里拿来对比的数：每组一行。"""
    rows = []
    for entry in report["groups"]:
        rows.append(
            f"    {report['label'] or report['scenario']:<18} {entry['name']:<12} 结果[{outcomes(entry)}]  "
            f"首字节 p50/p99 {fmt(entry['first_byte_ms'], 'p50', 'p99')}  "
            f"上传 p50/p99 {fmt(entry['upload_ms'], 'p50', 'p99')}  "
            f"首事件 p50/p99 {fmt(entry['ttft_ms'], 'p50', 'p99')}  "
            f"首事件迟到 p99/max {fmt(entry['first_token_lag_ms'], 'p99', 'max')}  "
            f"超出节拍 p50/p99/p99.9/max {fmt(entry['gap_excess_ms'], 'p50', 'p99', 'p999', 'max')}  "
            f"卡顿 {entry['stalls']}")
    episodes = report["stall_episodes"]
    procs = "  ".join(f"{proc['name']} CPU {proc['cpu_s']:.1f}s 内存峰值 {proc['rss_peak_mb']:.0f}MB" for proc in report["processes"])
    rows.append(f"    {'':<18} 合计：{report['elapsed_s']:.1f}s，在线峰值 {report['peak_active']}，事件峰值 {report['peak_events_per_s']}/s，"
                f"停顿 {episodes['count']} 次（多流 {episodes['shared']} 次，每次最多 {episodes['streams_max']} 条、最长 "
                f"{episodes['length_ms_max']:.0f}ms）；{procs}；机器负载 {report['loadavg_start']}→{report['loadavg_end']}")
    return rows


def log_time(line: str) -> float | None:
    """节点日志一行开头的时间戳（Unix 毫秒）。"""
    match = re.match(r"(?:\x1b\[[0-9;]*m)*(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z", line)
    if not match:
        return None
    stamp = datetime.strptime(match.group(1)[:26], "%Y-%m-%dT%H:%M:%S.%f").replace(tzinfo=timezone.utc)
    return stamp.timestamp() * 1000


def earlier_log(node: Node) -> Path:
    return node.log.with_name(node.log.name + ".earlier")


def keep_log(node: Node) -> None:
    """节点重启时日志文件会从头写，先把已有的接到旁边的文件上，出了问题还查得到重启之前的。"""
    with earlier_log(node).open("a") as out:
        out.write(node.read_log())


def logs_around(node: Node, unix_ms: float, before_s: float = 3, after_s: float = 1, limit: int = 12) -> str:
    """`node` 的日志里在这个时刻前后的内容（包括重启之前的）：一条流出问题的时候节点在做什么。

    同样的话（去掉时间、数字之后相同）只留第一次，后面注明重复了几次，免得几百条一样的拒绝把别的淹掉。
    """
    earlier = earlier_log(node).read_text(errors="replace") if earlier_log(node).exists() else ""
    lines = [line for line in (earlier + node.read_log()).splitlines()
             if (at := log_time(line)) is not None and unix_ms - before_s * 1000 <= at <= unix_ms + after_s * 1000]
    if not lines:
        return "    （这段时间里没有日志）"
    distinct: dict[str, list] = {}
    for line in lines:
        key = re.sub(r"\d+", "#", line.split(" ", 1)[-1])
        distinct.setdefault(key, [line, 0])[1] += 1
    shown = [f"{line}{f'  （同样的共 {count} 行）' if count > 1 else ''}" for line, count in distinct.values()]
    more = f"\n    ……另外 {len(shown) - limit} 种" if len(shown) > limit else ""
    return tail("\n".join(shown[:limit]), limit) + more


def evidence(report: dict, nodes: list[Node]) -> None:
    """有流没有按期望结束时：打出发了什么、收到了什么，以及节点在那个时刻前后的日志。只看前两组，再多也是重复。"""
    failing = [entry for entry in report["groups"] if entry["failures"]]
    for entry in failing[:2]:
        first = entry["failures"][0]
        print(f"  证据：组 {entry['name']} 第一条不符合的流 {first['stream']}，期望 {first['expected']}，实际 {first['outcome']}，"
              f"收到 {first['events']} 个事件，{first['started_s']:.1f}s→{first['ended_s']:.1f}s：{first['detail']}")
        for node in nodes:
            print(f"  {node.name} 在那之前 3 秒到之后 1 秒的日志：\n{logs_around(node, first['ended_unix_ms'])}")


def set_nat_timeouts(nat: Nat, udp_s: int, tcp_s: int) -> str:
    """把 NAT 回收映射的时间调短，像家用路由器那样。返回读回来的值。"""
    nat.run("sysctl", "-qw", f"net.netfilter.nf_conntrack_udp_timeout={udp_s}",
            f"net.netfilter.nf_conntrack_udp_timeout_stream={udp_s}",
            f"net.netfilter.nf_conntrack_tcp_timeout_established={tcp_s}")
    shown = nat.run("sysctl", "net.netfilter.nf_conntrack_udp_timeout_stream", "net.netfilter.nf_conntrack_tcp_timeout_established").stdout
    return "，".join(line.strip() for line in shown.splitlines())


def start_upstream(api: Host, work: Path) -> Path:
    """在 `api` 上起模拟的上游，HTTPS 在 443、明文在 80。返回 CA 证书的路径。"""
    log = work / "upstream.log"
    api.spawn(binary("i3t-llmgw"), "upstream", "--https", "0.0.0.0:443", "--http", "0.0.0.0:80", "--cert-dir", str(work / "ca"),
              "--name", api.ip, log=log)
    wait_until(lambda: "上游已就绪" in log.read_text(errors="replace"), what="上游启动")
    return work / "ca" / "ca.pem"


class Deployment:
    """一套已经起好的节点和上游：负载端在入口那台机器上，经入口的代理入口、用出口的名字发流。"""

    def __init__(self, lab: Lab, work: Path, api: Host, ca: Path, entry: Node, exit_node: Node, others: tuple[Node, ...] = ()) -> None:
        self.lab, self.work, self.api, self.ca, self.entry, self.exit, self.others = lab, work, api, ca, entry, exit_node, others

    def watched(self) -> list[Node]:
        return [self.entry, self.exit, *self.others]

    def probe(self, timeout: float = 5) -> tuple[bool, str]:
        """经入口和出口请求一次上游的 /health。返回是否成功，以及失败时 curl 怎么说。"""
        done = self.entry.host.run("curl", "-sS", "--max-time", str(timeout), "--cacert", str(self.ca), "-x",
                                   self.entry.entry(self.exit).url("socks5h"), f"https://{self.api.ip}/health",
                                   check=False, timeout=timeout + 5)
        return done.stdout == "ok", (done.stderr.strip() or done.stdout.strip())

    def health(self, timeout: float = 5) -> bool:
        return self.probe(timeout)[0]

    def wait_ready(self, timeout: float = 40) -> None:
        wait_until(lambda: self.health(), timeout=timeout, interval=0.5, what="经入口和出口访问到上游")

    def load(self, name: str, duration_s: float, groups: list[dict], **options) -> Load:
        return Load(self, name, duration_s, groups, **options)


def peers_of(entry: Node, exit_node: Node, *, entry_view: dict, exit_view: dict) -> tuple[dict, dict]:
    """入口和出口互相看到的对端条目：连接常驻（网关场景要求），再套上按节点对的设置。"""
    exit_peer, entry_peer = {**entry_view, "keep_alive": True}, {**exit_view, "keep_alive": True}
    apply_pair_setting(exit_peer, entry_peer)
    return exit_peer, entry_peer


class Gateway(Deployment):
    """入口在公网上并且兼做 relay；出口在家用 NAT 后面，挂靠在入口上；上游在"互联网"上。

    这是网关部署的典型形态：出口主动连入口，两者之间一条常驻的连接，每个请求是它上面的一条流。
    """

    def __init__(self, lab: Lab, work: Path, *, nat_kind: NatKind = NatKind.CONE) -> None:
        api = lab.host("api")
        ca = start_upstream(api, work)
        entry = Node(lab.host("entry"), work, 1, declare=("relay",))
        self.home = lab.nat("home", nat_kind)
        self.nat_timeouts = set_nat_timeouts(self.home, NAT_UDP_TIMEOUT_S, NAT_TCP_TIMEOUT_S)
        exit_node = Node(self.home.host("exit"), work, 2, declare=("exit",))
        super().__init__(lab, work, api, ca, entry, exit_node)
        self.exit_peer, self.entry_peer = peers_of(
            entry, exit_node,
            entry_view=exit_node.as_peer(endpoints=False, attached=[(entry, 5)], exit=True),
            exit_view=entry.as_peer())
        entry.start_static(entry.netmap(me={"relay": {}}, relays=[entry.as_relay()], relay_clients=[entry, exit_node], peers=[self.exit_peer],
                                        proxies=[entry.proxy_to(exit_node, lane=proxy_lane())]))
        self.start_exit()
        self.wait_ready()

    def start_exit(self) -> None:
        self.exit.start_static(self.exit.netmap(me={"exit": {}}, relays=[self.entry.as_relay()], peers=[self.entry_peer]))


def header(gw: Gateway) -> None:
    print(f"  拓扑：入口 {gw.entry.host.ip}（兼做 relay）← 出口 {gw.exit.host.ip}（NAT {gw.home.ip} 后面：{gw.nat_timeouts}）"
          f"→ 上游 https://{gw.api.ip}；节点对设置：{pair_label()}", flush=True)


def sse_long_thinking(lab: Lab, work: Path, t: Checks) -> None:
    """模型思考时流完全静默，比系统里所有的空闲计时器都长；之后的事件一个不少，第一个事件一发出就到。"""
    gw = Gateway(lab, work)
    header(gw)
    think_ms = 600_000 if LONG else 75_000
    groups = [
        sse("silent", 100, arrival={"rate": 50}, prompt_bytes=[20_000, 200_000], think_ms=think_ms, tokens=200, rate=50),
        sse("keepalive", 40, arrival={"rate": 50}, think_ms=think_ms, keepalive_ms=15_000, tokens=200, rate=50),
        ws("ws_silent", 20, arrival={"rate": 20}, think_ms=think_ms, tokens=100, rate=50),
    ]
    print(f"  上游思考 {think_ms / 1000:.0f} 秒；连接保活 15 秒、空闲超时 30 秒，NAT 回收 UDP 映射 {NAT_UDP_TIMEOUT_S} 秒、TCP {NAT_TCP_TIMEOUT_S} 秒",
          flush=True)
    report = gw.load("sse_long_thinking", think_ms / 1000 + 60, groups).wait()
    print(*key_numbers(report), sep="\n")
    ok = all_as_expected(t, report, "长时间静默之后")
    for entry in report["groups"]:
        lag = entry["first_token_lag_ms"]
        t.check(f"组 {entry['name']}：静默之后第一个事件从上游写出到收到不超过 1 秒", lag["count"] > 0 and lag["max"] < 1000,
                f"首事件迟到 p50 {lag['p50']:.1f} p99 {lag['p99']:.1f} max {lag['max']:.1f} ms")
    if not ok:
        evidence(report, gw.watched())
        print(describe(groups))


def sse_fast_tokens(lab: Lab, work: Path, t: Checks) -> None:
    """几百条流同时以每秒几百个事件的速度输出：全部完整到达；先直连跑一遍作为对照。"""
    gw = Gateway(lab, work)
    header(gw)
    groups = [sse("fast", 300, arrival={"ramp_s": 2}, prompt_bytes=[5_000, 50_000], think_ms=500, tokens=1500, rate=300, event_bytes=30)]
    control = gw.load("sse_fast_tokens_direct", 60, groups, proxy="direct", label="直连对照").wait()
    t.check("直连对照：测量工具自己跑得动这个负载", control["passed"], outcomes(control["groups"][0]))
    report = gw.load("sse_fast_tokens", 90, groups).wait()
    print(*key_numbers(control), *key_numbers(report), sep="\n")
    if not all_as_expected(t, report, "经入口和出口"):
        evidence(report, gw.watched())
        print(describe(groups))


def websocket_session(lab: Lab, work: Path, t: Checks) -> None:
    """WebSocket 会话：两轮之间长时间空闲，回答中插进客户端消息，两边互相 ping，两个方向按固定节拍的二进制帧。"""
    gw = Gateway(lab, work)
    header(gw)
    groups = [
        # 两轮之间 35 秒什么都不发（没有 ping），比连接的空闲超时还长。
        ws("chat_idle", 40, arrival={"rate": 20}, prompt_bytes=20_000, turns=2, think_ms=2000, tokens=200, rate=100,
           client_msgs=4, idle_ms=35_000),
        ws("chat_ping", 20, arrival={"rate": 20}, turns=3, think_ms=1000, tokens=100, rate=50, client_msgs=2, idle_ms=10_000,
           client_ping_ms=5000, server_ping_ms=7000),
        # 像实时语音：每 20 毫秒一帧 640 字节，两个方向同时。
        ws("voice", 20, arrival={"rate": 20}, turns=2, think_ms=300, tokens=250, rate=50, binary=True, event_bytes=640,
           client_binary_ms=20, client_binary_bytes=640, idle_ms=5000, client_ping_ms=5000, server_ping_ms=5000),
    ]
    report = gw.load("websocket_session", 120, groups, proxy="http").wait()
    print(*key_numbers(report), sep="\n")
    if not all_as_expected(t, report, "经 HTTP CONNECT 入口"):
        evidence(report, gw.watched())
        print(describe(groups))
    pings = group(report, "chat_ping")["ping_rtt_ms"]
    t.check("客户端的 ping 都有回音", pings["count"] > 0, f"{pings}")


def burst_arrival(lab: Lab, work: Path, t: Checks) -> None:
    """几百条流在同一时刻打开，各带一份 20KB 到 1MB 不等的提示词。"""
    gw = Gateway(lab, work)
    header(gw)
    groups = [sse("burst", 400, arrival="all", prompt_bytes=[20_000, 1_000_000], think_ms=1000, tokens=100, rate=50)]
    control = gw.load("burst_arrival_direct", 150, groups, proxy="direct", label="直连对照").wait()
    t.check("直连对照：同时打开 400 条", control["passed"], outcomes(control["groups"][0]))
    report = gw.load("burst_arrival", 150, groups).wait()
    print(*key_numbers(control), *key_numbers(report), sep="\n")
    if not all_as_expected(t, report, "同时打开"):
        evidence(report, gw.watched())
        print(describe(groups))
    entry = report["groups"][0]
    print(f"  上行一共 {entry['bytes_up'] / 1e6:.1f} MB；连接 p99 {entry['connect_ms']['p99']:.1f} ms，TLS p99 {entry['tls_ms']['p99']:.1f} ms，"
          f"上传 p50/p99/max {fmt(entry['upload_ms'], 'p50', 'p99', 'max')} ms")


def many_streams(lab: Lab, work: Path, t: Checks) -> None:
    """一对入口—出口上同时开着两千条流（大多在等模型思考），像一个忙的网关：全部完整。"""
    gw = Gateway(lab, work)
    header(gw)
    groups = [
        sse("waiting", 1600, arrival={"ramp_s": 4}, prompt_bytes=[2_000, 20_000], think_ms=12_000, tokens=50, rate=10),
        sse("streaming", 400, arrival={"ramp_s": 4}, prompt_bytes=[2_000, 20_000], think_ms=1000, tokens=400, rate=20),
    ]
    report = gw.load("many_streams", 90, groups).wait()
    print(*key_numbers(report), sep="\n")
    t.check("同时在线的流到了两千条左右", report["peak_active"] >= 1900, f"最多 {report['peak_active']} 条")
    if not all_as_expected(t, report, "两千条同时在线"):
        evidence(report, gw.watched())
        print(describe(groups))


def upstream_abort(lab: Lab, work: Path, t: Checks) -> None:
    """上游中途重置一部分流、停住另一部分：重置的在客户端一定是错误（RST），停住的一直开着，不影响别的流。"""
    gw = Gateway(lab, work)
    header(gw)
    groups = [
        sse("normal", 100, arrival={"rate": 50}, think_ms=500, tokens=500, rate=50),
        sse("abort", 50, arrival={"rate": 50}, think_ms=500, tokens=500, rate=50, end="abort", end_after=100),
        # 停住之后拿着 40 秒，比连接的空闲超时长。
        sse("stall", 50, arrival={"rate": 50}, think_ms=500, tokens=500, rate=50, end="stall", end_after=100, hold_s=40),
        ws("ws_abort", 10, think_ms=500, turns=2, tokens=100, rate=50, end="abort", end_after=150),
        ws("ws_stall", 10, think_ms=500, turns=2, tokens=100, rate=50, end="stall", end_after=150, hold_s=40),
        # 停住的流还开着的时候来的一批新流。
        sse("during_stall", 100, start_s=15, arrival={"rate": 50}, think_ms=500, tokens=500, rate=50),
    ]
    report = gw.load("upstream_abort", 90, groups).wait()
    print(*key_numbers(report), sep="\n")
    ok = all_as_expected(t, report, "上游出事")
    truncated = sum(entry["outcomes"].get("truncated", 0) for entry in report["groups"])
    t.check("没有一条被截断的回复以正常关闭（FIN）结束", truncated == 0, f"{truncated} 条")
    if not ok:
        evidence(report, gw.watched())
        print(describe(groups))


def mix(prefix: str = "") -> list[dict]:
    """一份混合负载：快速输出、长时间思考、大请求体、WebSocket 会话。几个场景用同一份，好对比。"""
    return [
        sse(f"{prefix}tokens", 100, arrival={"ramp_s": 5}, prompt_bytes=[10_000, 100_000], think_ms=1000, tokens=400, rate=100),
        sse(f"{prefix}think", 40, arrival={"rate": 20}, think_ms=20_000, tokens=100, rate=50),
        sse(f"{prefix}upload", 40, arrival={"rate": 10}, prompt_bytes=[200_000, 1_000_000], think_ms=500, tokens=50, rate=50),
        ws(f"{prefix}ws", 20, arrival={"rate": 10}, turns=2, think_ms=1000, tokens=100, rate=50, client_msgs=2, idle_ms=3000,
           client_ping_ms=5000, server_ping_ms=5000),
    ]


def lossy_long_path(lab: Lab, work: Path, t: Checks) -> None:
    """同一份混合负载先走干净的路，再走往返 150ms、丢包 1% 的路；最后不经节点、每个请求一条 TCP 走同样差的路作为对照。"""
    gw = Gateway(lab, work)
    header(gw)
    clean = gw.load("mix_clean", 120, mix(), label="干净的路").wait()
    all_as_expected(t, clean, "干净的路")
    shape = Shape(rtt_ms=150, loss_pct=1)
    shape_between(gw.entry.host, gw.home, shape)
    print(f"  入口和出口之间：往返 {shape.rtt_ms:g}ms，每个方向丢包 {shape.loss_pct:g}%", flush=True)
    lossy = gw.load("mix_lossy", 240, mix(), label="150ms 丢包 1%").wait()
    if not all_as_expected(t, lossy, "往返 150ms、丢包 1%"):
        evidence(lossy, gw.watched())
        print(describe(mix()))
    # 对照：入口那台机器直接连上游，入口和上游之间加上同样的条件。每个请求自己一条 TCP，丢包只拖累它自己。
    shape_between(gw.entry.host, gw.home, None)
    shape_between(gw.entry.host, gw.api, shape)
    direct = gw.load("mix_lossy_direct", 240, mix(), proxy="direct", label="直连对照 150ms 丢包 1%").wait()
    all_as_expected(t, direct, "直连对照，往返 150ms、丢包 1%")
    print(*key_numbers(clean), *key_numbers(lossy), *key_numbers(direct), sep="\n")


def steady(prefix: str) -> list[dict]:
    """匀速的流，用来看一次丢包让多少条不相干的流一起停。

    量取得小（下行约 200KB/s）：丢包 1% 的 TCP 腿上整条连接只有几百 KB/s，负载再大测到的就是"带宽不够、
    一直落后"，而不是一个丢包造成的停顿。
    """
    return [
        sse(f"{prefix}steady", 100, arrival={"ramp_s": 2}, think_ms=500, tokens=300, rate=10),
        ws(f"{prefix}voice", 2, arrival={"ramp_s": 2}, turns=1, think_ms=300, tokens=1500, rate=50, binary=True, event_bytes=640,
           client_binary_ms=20, client_binary_bytes=640),
    ]


def tcp_only_exit(lab: Lab, work: Path, t: Checks) -> None:
    """出口的 UDP 不通，只能经 TCP 连入口这个 relay，这一段还丢包：一个丢包让多少条不相干的流一起停、停多久。先在 UDP 上跑同样的负载对照。"""
    gw = Gateway(lab, work)
    header(gw)
    shape = Shape(rtt_ms=50, loss_pct=1)
    shape_between(gw.entry.host, gw.home, shape)
    print(f"  入口和出口之间：往返 {shape.rtt_ms:g}ms，每个方向丢包 {shape.loss_pct:g}%；卡顿的门槛 100ms", flush=True)
    udp = gw.load("steady_udp", 90, steady("udp_"), label="UDP，丢包 1%", stall_ms=100).wait()
    all_as_expected(t, udp, "UDP")

    gw.exit.host.block_udp()
    blocked = time.monotonic()
    switched = waited(lambda: "到 relay 的中转会话建立（TCP）" in gw.exit.read_log(), 60)
    t.check("出口的 UDP 断了之后改经 TCP 连上入口", switched, tail(gw.exit.read_log(), 10))
    print(f"  UDP 断开之后 {time.monotonic() - blocked:.1f} 秒出口经 TCP 连上了入口", flush=True)
    if not switched:
        return
    tcp = gw.load("steady_tcp", 90, steady("tcp_"), label="TCP 中转，丢包 1%", stall_ms=100).wait()
    print(*key_numbers(udp), *key_numbers(tcp), sep="\n")
    if not all_as_expected(t, tcp, "TCP 中转"):
        evidence(tcp, gw.watched())
    for name, report in (("UDP", udp), ("TCP 中转", tcp)):
        episodes = report["stall_episodes"]
        print(f"  {name}：超出节拍 100ms 以上的停顿 {episodes['count']} 次，其中牵连多条流的 {episodes['shared']} 次；"
              f"一次停顿牵连的流数 p50 {episodes['streams_p50']}、最多 {episodes['streams_max']}（同时在线约 100 条）；"
              f"停顿长度 p50 {episodes['length_ms_p50']:.0f}ms、最长 {episodes['length_ms_max']:.0f}ms")


def tcp_only_exit_heavy(lab: Lab, work: Path, t: Checks) -> None:
    """出口只能经 TCP 连入口、这一段丢包 1%，负载比一条 TCP 带得动的大得多：约 220 条流、下行每秒 2MB 多。只在点名时运行。

    共用的连接在这里带不动是已知的（一条 TCP 在这样的链路上只有每秒几百 KB），所以节点对没有开 ice3proxy 时只记录结果，
    不算失败；开了之后每条流都要完成。
    """
    gw = Gateway(lab, work)
    header(gw)
    shape = Shape(rtt_ms=50, loss_pct=1)
    shape_between(gw.entry.host, gw.home, shape)
    gw.exit.host.block_udp()
    if not waited(lambda: "到 relay 的中转会话建立（TCP）" in gw.exit.read_log(), 60):
        t.check("出口的 UDP 断了之后改经 TCP 连上入口", False, tail(gw.exit.read_log(), 10))
        return
    print(f"  入口和出口之间：只有 TCP，往返 {shape.rtt_ms:g}ms，每个方向丢包 {shape.loss_pct:g}%", flush=True)
    groups = [
        sse("heavy_tokens", 200, arrival={"ramp_s": 5}, think_ms=500, tokens=3000, rate=100),
        ws("heavy_voice", 20, arrival={"ramp_s": 5}, turns=1, think_ms=300, tokens=1500, rate=50, binary=True, event_bytes=640,
           client_binary_ms=20, client_binary_bytes=640),
    ]
    heavy = gw.load("heavy_tcp", 120, groups, label="只有 TCP，丢包 1%，重负载", stall_ms=250).wait()
    print(*key_numbers(heavy), sep="\n")
    lanes_on = proxy_lane()
    if lanes_on:
        if not all_as_expected(t, heavy, "重负载"):
            evidence(heavy, gw.watched())
    else:
        done = sum(entry["count"] - entry["failed"] for entry in heavy["groups"])
        total = sum(entry["count"] for entry in heavy["groups"])
        print(f"  节点对没有开 ice3proxy：{total} 条流里 {done} 条按期望结束（只记录，不算失败）", flush=True)


def waited(condition, timeout: float, interval: float = 0.5) -> bool:
    try:
        wait_until(condition, timeout=timeout, interval=interval)
        return True
    except TimeoutError:
        return False


def block_between(core_host: Host, a: str, b: str, blocked: bool) -> None:
    """在"互联网"的路由器上丢掉 `a` 和 `b` 两个地址之间的 UDP，别的流量不受影响。"""
    core_host.run("nft", "delete", "table", "inet", "llmblock", check=False)
    if blocked:
        rules = f"""
table inet llmblock {{
    chain forward {{
        type filter hook forward priority filter;
        ip saddr {a} ip daddr {b} meta l4proto udp drop
        ip saddr {b} ip daddr {a} meta l4proto udp drop
    }}
}}
"""
        core_host.run("nft", "-f", "-", input=rules)


def route_change_mid_stream(lab: Lab, work: Path, t: Checks) -> None:
    """流正在跑的时候换路：经 relay → 打洞成功后直连 → 直连断了回到 relay → 这个 relay 挂了换到另一个。没有一条流断。"""
    api = lab.host("api")
    ca = start_upstream(api, work)
    r1 = Node(lab.host("r1"), work, 8, declare=("relay",))
    r2 = Node(lab.host("r2"), work, 9, declare=("relay",))
    office = lab.nat("office", NatKind.CONE)
    home = lab.nat("home", NatKind.CONE)
    set_nat_timeouts(home, NAT_UDP_TIMEOUT_S, NAT_TCP_TIMEOUT_S)
    entry = Node(office.host("entry"), work, 1)
    exit_node = Node(home.host("exit"), work, 2, declare=("exit",))
    core = Host(lab, "core", "")
    # 一开始两个 NAT 之间的 UDP 不通，只能经 relay。
    block_between(core, office.ip, home.ip, True)

    relays = [r1.as_relay(), r2.as_relay()]
    for relay in (r1, r2):
        relay.start_static(relay.netmap(me={"relay": {}}, relays=relays, relay_clients=[entry, exit_node, r1, r2]))
    attached = [(r1, 5), (r2, 6)]
    exit_peer, entry_peer = peers_of(
        entry, exit_node,
        entry_view={**exit_node.as_peer(endpoints=False, attached=attached, exit=True), "endpoints": [f"{home.ip}:20330"]},
        exit_view={**entry.as_peer(endpoints=False, attached=attached), "endpoints": [f"{office.ip}:20330"]})
    entry.start_static(entry.netmap(peers=[exit_peer], relays=relays, proxies=[entry.proxy_to(exit_node, lane=proxy_lane())]))
    exit_node.start_static(exit_node.netmap(peers=[entry_peer], relays=relays, me={"exit": {}}))
    gw = Deployment(lab, work, api, ca, entry, exit_node, (r1, r2))
    gw.wait_ready()
    print(f"  拓扑：入口在 NAT {office.ip} 后面，出口在 NAT {home.ip} 后面，relay r1 {r1.host.ip}、r2 {r2.host.ip}；节点对设置：{pair_label()}",
          flush=True)
    t.check("开始时经 relay r1", current_route(entry, exit_node) == f"经 relay {r1.id}", current_route(entry, exit_node))

    groups = [
        # 几次换路在实验环境里一共约 25 秒；流要比它长，留出余量。
        sse("steady", 100, arrival={"ramp_s": 3}, think_ms=500, tokens=1200, rate=20),
        ws("chat", 20, arrival={"ramp_s": 3}, turns=4, think_ms=1000, tokens=200, rate=20, client_msgs=2, idle_ms=4000,
           client_ping_ms=5000, server_ping_ms=5000),
        ws("voice", 10, arrival={"ramp_s": 3}, turns=1, think_ms=300, tokens=3000, rate=50, binary=True, event_bytes=640,
           client_binary_ms=20, client_binary_bytes=640),
    ]
    load = gw.load("route_change", 200, groups)
    began = time.monotonic()
    timeline: list[str] = []

    def mark(what: str) -> None:
        timeline.append(f"    {time.monotonic() - began:6.1f}s  {what}；现在 {current_route(entry, exit_node)}")
        print(timeline[-1], flush=True)

    time.sleep(8)
    mark("放开两个 NAT 之间的 UDP")
    block_between(core, office.ip, home.ip, False)
    direct = waited(lambda: current_route(entry, exit_node).startswith("直连 "), 60)
    mark("换到直连" if direct else "60 秒内没有换到直连")
    t.check("打洞成功之后从 relay 换到直连", direct, f"经过 {route_changes(entry, exit_node)[-4:]}")
    time.sleep(5)
    mark("挡住两个 NAT 之间的 UDP")
    block_between(core, office.ip, home.ip, True)
    back = waited(lambda: current_route(entry, exit_node).startswith("经 relay"), 40)
    mark("回到 relay" if back else "40 秒内没有回到 relay")
    t.check("直连断了之后回到 relay", back, f"经过 {route_changes(entry, exit_node)[-4:]}")
    time.sleep(5)
    now_via = current_route(entry, exit_node)
    victim, survivor = (r1, r2) if now_via == f"经 relay {r1.id}" else (r2, r1)
    mark(f"杀掉正在用的 relay {victim.name}（SIGKILL）")
    victim.host.stop(victim.proc, sig=signal.SIGKILL)
    victim.proc = None
    moved = waited(lambda: current_route(entry, exit_node) == f"经 relay {survivor.id}", 40)
    mark(f"换到 {survivor.name}" if moved else f"40 秒内没有换到 {survivor.name}")
    t.check(f"正在用的 relay 挂了之后换到 {survivor.name}", moved, f"经过 {route_changes(entry, exit_node)[-4:]}")

    last_change = time.monotonic() - began
    report = load.wait()
    print("  时间线（从负载开始算）：", *timeline, sep="\n")
    print("  入口到出口的每一次换路：", *[f"    {route}：{reason}" for route, reason in route_changes(entry, exit_node)], sep="\n")
    print(*key_numbers(report), sep="\n")
    ok = all_as_expected(t, report, "几次换路期间")
    t.check("几次换路都发生在流还在跑的时候", report["elapsed_s"] > last_change,
            f"负载 {report['elapsed_s']:.0f} 秒就结束了，最后一次换路在 {last_change:.0f} 秒")
    if not ok:
        evidence(report, [entry, exit_node])


def exit_restart(lab: Lab, work: Path, t: Checks) -> None:
    """流正在跑的时候重启出口：手上的流以错误结束（不是正常结束），出口回来之后新的流马上能用。先正常重启，再模拟崩溃（SIGKILL）。"""
    gw = Gateway(lab, work)
    header(gw)
    for how, sig in (("正常重启（SIGTERM）", signal.SIGTERM), ("崩溃后重启（SIGKILL）", signal.SIGKILL)):
        print(f"  -- {how}", flush=True)
        tag = "term" if sig == signal.SIGTERM else "kill"
        running = [
            sse("running", 100, arrival={"ramp_s": 2}, think_ms=500, tokens=1200, rate=20, expect="reset"),
            ws("ws_running", 10, arrival={"ramp_s": 2}, turns=1, think_ms=500, tokens=1200, rate=20, client_ping_ms=5000, expect="reset"),
        ]
        load = gw.load(f"exit_restart_{tag}", 100, running)
        time.sleep(8)
        stopped_ms = time.time() * 1000
        gw.exit.host.stop(gw.exit.proc, sig=sig)
        gw.exit.proc = None
        keep_log(gw.exit)
        gw.start_exit()
        up = time.monotonic()
        up_ms = time.time() * 1000
        failed_probes: dict[str, int] = {}

        def probe() -> bool:
            ok, detail = gw.probe(timeout=2)
            if not ok:
                key = re.sub(r"\d+", "#", detail)
                failed_probes[key] = failed_probes.get(key, 0) + 1
            return ok

        recovered = waited(probe, 60, interval=0.25)
        recovery = time.monotonic() - up
        t.check(f"{how}：出口回来之后经它的请求又能用了", recovered, tail(gw.entry.read_log(), 15))
        print(f"  出口进程停掉到新进程启动用了 {(up_ms - stopped_ms) / 1000:.1f} 秒，之后 {recovery:.1f} 秒经它的第一个请求成功", flush=True)
        if not t.check(f"{how}：出口回来之后 5 秒内新的请求就能用", recovered and recovery <= 5, f"用了 {recovery:.1f} 秒"):
            print("  这段时间里请求失败的样子：", *[f"    {count} 次：{detail}" for detail, count in failed_probes.items()], sep="\n")
            for node in (gw.entry, gw.exit):
                print(f"  {node.name} 在出口的新进程启动之后 8 秒内的日志：\n{logs_around(node, up_ms, before_s=0, after_s=8)}")
        fresh = [sse("fresh", 50, arrival={"rate": 50}, think_ms=500, tokens=100, rate=50)]
        after = gw.load(f"exit_restart_{tag}_after", 60, fresh).wait()
        report = load.wait()
        print(*key_numbers(report), *key_numbers(after), sep="\n")
        ok = all_as_expected(t, report, f"{how}：正在跑的流")
        for entry in report["groups"]:
            ended = entry["ended_at_ms"]
            since_stop = [report["started_unix_ms"] + value - stopped_ms for value in (ended["min"], ended["p50"], ended["max"])]
            print(f"  组 {entry['name']}：出口停掉之后最早 {since_stop[0]:.0f} ms、p50 {since_stop[1]:.0f} ms、最晚 {since_stop[2]:.0f} ms 流结束"
                  f"（直方图精度约千分之一）；结果 {outcomes(entry)}")
        clean_looking = sum(entry["outcomes"].get("truncated", 0) + entry["outcomes"].get("complete", 0) for entry in report["groups"])
        t.check(f"{how}：没有一条正在跑的流看起来像正常结束", clean_looking == 0, f"{clean_looking} 条")
        if not ok:
            evidence(report, gw.watched())
        if not all_as_expected(t, after, f"{how}：出口回来之后新开的流"):
            evidence(after, gw.watched())


SCENARIOS = {
    "sse_long_thinking": sse_long_thinking,
    "sse_fast_tokens": sse_fast_tokens,
    "websocket_session": websocket_session,
    "burst_arrival": burst_arrival,
    "many_streams": many_streams,
    "upstream_abort": upstream_abort,
    "lossy_long_path": lossy_long_path,
    "tcp_only_exit": tcp_only_exit,
    "route_change_mid_stream": route_change_mid_stream,
    "exit_restart": exit_restart,
    "tcp_only_exit_heavy": tcp_only_exit_heavy,
}
# 不点名就不跑的场景：耗时长，或者在默认的传输上本来就过不了。
ON_REQUEST = {"tcp_only_exit_heavy"}


def main() -> int:
    global LONG, PAIR_SETTING
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--long", action="store_true", help="sse_long_thinking 静默 10 分钟")
    parser.add_argument("--pair", default="{}", help="按节点对的设置（JSON），合进入口和出口互相看到的对端条目")
    parser.add_argument("--keep", action="store_true", help="留下每个场景的工作目录（节点日志、场景文件、结果 JSON）")
    parser.add_argument("names", nargs="*", metavar="场景名", help="、".join(SCENARIOS))
    args = parser.parse_args()
    unknown = [name for name in args.names if name not in SCENARIOS]
    if unknown:
        parser.error(f"没有这些场景：{'、'.join(unknown)}")
    LONG, PAIR_SETTING = args.long, json.loads(args.pair)
    lablib.enter()
    names = args.names or [name for name in SCENARIOS if name not in ON_REQUEST]
    t = Checks()
    began = time.monotonic()
    for name in names:
        print(f"== {name}：{(SCENARIOS[name].__doc__ or '').strip().splitlines()[0]}", flush=True)
        scenario_began = time.monotonic()
        work = Path(tempfile.mkdtemp(prefix=f"i3t-e2e-{name}-"))
        with Lab() as lab:
            try:
                SCENARIOS[name](lab, work, t)
            except Exception as err:  # noqa: BLE001 - 一个场景出错不影响后面的场景
                t.check(f"{name} 没有跑完", False, repr(err))
        if args.keep:
            print(f"   日志、场景文件和结果留在 {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)
        print(f"   （{name} 用了 {time.monotonic() - scenario_began:.0f} 秒）", flush=True)
        # 每个场景用全新的命名空间；上一个场景的要等进程退干净才能重名。
        time.sleep(0.3)
    print(f"一共用了 {time.monotonic() - began:.0f} 秒")
    return t.summary()


if __name__ == "__main__":
    sys.exit(main())
