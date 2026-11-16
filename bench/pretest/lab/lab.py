#!/usr/bin/env python3
"""本机 netns + netem 实验环境，以及在它上面跑的测试矩阵。

不需要 root：脚本会用 `unshare -Urnm` 把自己放进一个新的用户命名空间，
在里面建网络命名空间、veth 和 netem。

拓扑（每条线是一对 veth，各自可以设往返延迟、丢包率、带宽）：

    a ───────────────── b        ab    直连
    a ── r1 ─────────── b        ar1、r1b   经一个 relay
    a ── r1 ── r2 ───── b        ar1、r1r2、r2b   经两个 relay

a 是客户端，b 是服务端，r1 / r2 上跑 relay 或 TCP 中间跳。

用法：
    lab.py list                      列出所有测试组
    lab.py run <组名>... [--out 目录] [--secs N] [--only 子串]
    lab.py shell                     建好拓扑后进一个 shell，手工试
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import resource
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, Iterable, Iterator

REPO = Path(__file__).resolve().parents[3]
BIN = REPO / "target" / "release" / "i3t-pretest"
PORT = 20330
IN_LAB_ENV = "I3T_PRETEST_LAB"

# 各进程固定在哪些核上。本机 4～11 号是同一种核，避免结果随调度到哪种核上而漂移。
CORES = {"b": "4,5", "a": "6,7", "r1": "8,9", "r2": "10,11"}

# 链路名 -> (命名空间, 地址, 命名空间, 地址)
LINKS = {
    "ab": ("a", "10.1.0.1", "b", "10.1.0.2"),
    "ar1": ("a", "10.1.1.1", "r1", "10.1.1.2"),
    "r1b": ("r1", "10.1.2.1", "b", "10.1.2.2"),
    "r1r2": ("r1", "10.1.3.1", "r2", "10.1.3.2"),
    "r2b": ("r2", "10.1.4.1", "b", "10.1.4.2"),
}
NAMESPACES = ("a", "b", "r1", "r2")


def addr(link: str, ns: str) -> str:
    """某个命名空间在某条链路上的地址，带端口。"""
    ns_a, ip_a, ns_b, ip_b = LINKS[link]
    return f"{ip_a if ns == ns_a else ip_b}:{PORT}"


def sh(*cmd: str, check: bool = True) -> None:
    subprocess.run(cmd, check=check, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


@dataclasses.dataclass(frozen=True)
class Shape:
    """一条链路的网络条件。rtt_ms 是往返延迟，两个方向各占一半；丢包率和带宽两个方向相同。"""

    rtt_ms: float = 0
    loss_pct: float = 0
    rate_mbit: float = 0  # 0 表示不限速

    def label(self) -> str:
        rate = f"{self.rate_mbit:g}M" if self.rate_mbit else "nolimit"
        return f"rtt{self.rtt_ms:g}-loss{self.loss_pct:g}-{rate}"


class Lab:
    def setup(self) -> None:
        # /run/netns 归 root 所有，所以在自己的挂载命名空间里换一个空的 /run。
        sh("mount", "-t", "tmpfs", "tmpfs", "/run")
        os.mkdir("/run/netns")
        for ns in NAMESPACES:
            sh("ip", "netns", "add", ns)
            sh("ip", "-n", ns, "link", "set", "lo", "up")
            # 默认的 TCP 缓冲上限只有几 MB，1 Gbit × 150ms 的链路要十几 MB 的窗口。
            for key in ("tcp_rmem", "tcp_wmem"):
                sh("ip", "netns", "exec", ns, "sysctl", "-qw", f"net.ipv4.{key}=4096 262144 67108864")
        for link, (ns_a, ip_a, ns_b, ip_b) in LINKS.items():
            dev_a, dev_b = f"{link}-{ns_a}", f"{link}-{ns_b}"
            sh("ip", "link", "add", dev_a, "netns", ns_a, "type", "veth", "peer", "name", dev_b, "netns", ns_b)
            for ns, dev, ip in ((ns_a, dev_a, ip_a), (ns_b, dev_b, ip_b)):
                sh("ip", "-n", ns, "addr", "add", f"{ip}/30", "dev", dev)
                sh("ip", "-n", ns, "link", "set", dev, "up")

    def devices(self) -> Iterator[tuple[str, str, str]]:
        for link, (ns_a, _, ns_b, _) in LINKS.items():
            yield link, ns_a, f"{link}-{ns_a}"
            yield link, ns_b, f"{link}-{ns_b}"

    def set_offloads(self, enabled: bool) -> None:
        """分段与合并卸载。开着的时候 netem 丢的是一整批包，所以带丢包的测试要关掉。"""
        state = "on" if enabled else "off"
        for _, ns, dev in self.devices():
            sh("ip", "netns", "exec", ns, "ethtool", "-K", dev, "tso", state, "gso", state, "gro", state, check=False)

    def wire_counters(self) -> dict[str, tuple[int, int]]:
        """每个网卡到现在为止发出去的 (字节数, 包数)。"""
        counters = {}
        for ns in NAMESPACES:
            listing = subprocess.run(["ip", "-n", ns, "-s", "-j", "link", "show"], check=True, capture_output=True, text=True)
            for dev in json.loads(listing.stdout):
                tx = dev.get("stats64", {}).get("tx", {})
                counters[dev["ifname"]] = (tx.get("bytes", 0), tx.get("packets", 0))
        return counters

    @staticmethod
    def set_sysctls(settings: dict[str, dict[str, str]]) -> dict[str, dict[str, str]]:
        """按命名空间改内核参数，返回改之前的值，用例结束后拿它改回去。"""
        previous: dict[str, dict[str, str]] = {}
        for ns, values in settings.items():
            previous[ns] = {}
            for key, value in values.items():
                current = subprocess.run(["ip", "netns", "exec", ns, "sysctl", "-n", key], check=True, capture_output=True, text=True)
                previous[ns][key] = current.stdout.strip()
                sh("ip", "netns", "exec", ns, "sysctl", "-qw", f"{key}={value}")
        return previous

    def shape(self, shapes: dict[str, Shape]) -> None:
        for link, ns, dev in self.devices():
            sh("tc", "-n", ns, "qdisc", "del", "dev", dev, "root", check=False)
            shape = shapes.get(link)
            if shape is None or shape == Shape():
                continue
            delay_ms = shape.rtt_ms / 2
            args = ["delay", f"{delay_ms:g}ms"]
            if shape.loss_pct:
                args += ["loss", f"{shape.loss_pct:g}%"]
            if shape.rate_mbit:
                args += ["rate", f"{shape.rate_mbit:g}mbit"]
                # 队列里同时装着"还在延迟中的包"和"等带宽的包"。给后者留 100ms 的量。
                packets_per_sec = shape.rate_mbit * 1e6 / (1500 * 8)
                limit = int(packets_per_sec * (delay_ms / 1000 + 0.1)) + 50
            else:
                limit = 200_000
            sh("tc", "-n", ns, "qdisc", "add", "dev", dev, "root", "netem", *args, "limit", str(limit))


@dataclasses.dataclass
class Proc:
    """要在某个命名空间里启动的一个进程。"""

    ns: str
    args: list[str]


@dataclasses.dataclass
class Case:
    suite: str
    name: str
    shapes: dict[str, Shape]
    server: Proc
    client: Proc
    helpers: list[Proc] = dataclasses.field(default_factory=list)  # relay 或 TCP 中间跳
    offloads: bool = False
    params: dict = dataclasses.field(default_factory=dict)
    timeout_s: float = 60
    # 命名空间 -> {内核参数: 值}，只在这个用例运行期间生效。
    sysctls: dict[str, dict[str, str]] = dataclasses.field(default_factory=dict)

    @property
    def key(self) -> str:
        return f"{self.suite}/{self.name}"


class Runner:
    def __init__(self, lab: Lab, out_dir: Path) -> None:
        self.lab = lab
        self.out_dir = out_dir
        self.log_dir = out_dir / "logs"
        self.log_dir.mkdir(parents=True, exist_ok=True)

    def _start(self, proc: Proc, tag: str) -> tuple[subprocess.Popen, Path]:
        out = self.log_dir / f"{tag}.out"
        cmd = ["taskset", "-c", CORES[proc.ns], "ip", "netns", "exec", proc.ns, str(BIN), *proc.args]
        handle = subprocess.Popen(cmd, stdout=out.open("w"), stderr=subprocess.STDOUT)
        return handle, out

    @staticmethod
    def _stop(handle: subprocess.Popen) -> None:
        handle.send_signal(signal.SIGTERM)
        try:
            handle.wait(timeout=5)
        except subprocess.TimeoutExpired:
            handle.kill()
            handle.wait()

    @staticmethod
    def _json_lines(path: Path) -> tuple[list[dict], list[str]]:
        records, other = [], []
        for line in path.read_text().splitlines():
            try:
                records.append(json.loads(line))
            except json.JSONDecodeError:
                if line.strip():
                    other.append(line)
        return records, other

    def run(self, case: Case) -> dict:
        self.lab.set_offloads(case.offloads)
        self.lab.shape(case.shapes)
        previous_sysctls = self.lab.set_sysctls(case.sysctls)
        tag = case.key.replace("/", "__")
        background = [self._start(proc, f"{tag}.{proc.ns}.helper{i}") for i, proc in enumerate(case.helpers)]
        background.append(self._start(case.server, f"{tag}.server"))
        time.sleep(0.8)  # 等监听就绪、服务端在 relay 上登记好

        wire_before = self.lab.wire_counters()
        started = time.time()
        client_out = self.log_dir / f"{tag}.client.out"
        cmd = ["taskset", "-c", CORES[case.client.ns], "ip", "netns", "exec", case.client.ns, str(BIN), *case.client.args]
        error = None
        try:
            done = subprocess.run(cmd, stdout=client_out.open("w"), stderr=subprocess.STDOUT, timeout=case.timeout_s)
            if done.returncode != 0:
                error = f"客户端退出码 {done.returncode}"
        except subprocess.TimeoutExpired:
            error = f"客户端超过 {case.timeout_s:g} 秒没有结束"
        elapsed = time.time() - started
        wire_after = self.lab.wire_counters()
        time.sleep(0.3)  # 让服务端看到连接关闭并打出它的统计
        for handle, _ in background:
            self._stop(handle)
        self.lab.set_sysctls(previous_sysctls)

        client_records, client_noise = self._json_lines(client_out)
        result = {
            "case": case.key,
            "suite": case.suite,
            "params": case.params,
            "shapes": {link: dataclasses.asdict(shape) for link, shape in case.shapes.items()},
            "elapsed_s": round(elapsed, 2),
            # 客户端运行期间各网卡发出的字节数和包数，含 IP / UDP / TCP 头。设备名是 "链路-命名空间"。
            "wire": {dev: {"tx_bytes": after[0] - wire_before[dev][0], "tx_packets": after[1] - wire_before[dev][1]}
                     for dev, after in wire_after.items()
                     if dev != "lo" and after[1] > wire_before.get(dev, after)[1]},
            "client": client_records[-1] if client_records else None,
            "others": [],
        }
        for _, path in background:
            records, noise = self._json_lines(path)
            result["others"].extend(records)
            client_noise.extend(noise)
        if error or not client_records:
            result["error"] = error or "客户端没有输出结果"
            result["stderr"] = client_noise[-5:]
        return result


# ---- 命令行参数的拼法 -------------------------------------------------------------

def quic_opts(cc: str = "cubic", gso: bool = False, extra: Iterable[str] = ()) -> list[str]:
    return ["--cc", cc, *([] if gso else ["--no-gso"]), *extra]


def quic_server(path: list[str], opts: list[str]) -> Proc:
    return Proc("b", ["serve", "quic", "--listen", f"0.0.0.0:{PORT}", "--id", "2", "--peer", "1", *path, *opts])


def quic_client(path: list[str], opts: list[str], load: list[str]) -> Proc:
    return Proc("a", ["run", "quic", "--id", "1", "--peer", "2", *path, *opts, *load])


def relay(ns: str, *extra: str) -> Proc:
    return Proc(ns, ["relay", "--udp", f"0.0.0.0:{PORT}", "--tcp", f"0.0.0.0:{PORT}", *extra])


def bulk(secs: int, streams: int = 1, direction: str = "down", series: bool = False) -> list[str]:
    warmup = max(1, secs // 3)
    return ["--workload", "bulk", "--secs", str(secs), "--warmup", str(warmup), "--streams", str(streams),
            "--dir", direction, *(["--series"] if series else [])]


@dataclasses.dataclass(frozen=True)
class Transport:
    """一种"怎么把数据从 a 送到 b"的办法，知道自己要起哪些进程。"""

    label: str
    build: Callable[[list[str]], tuple[Proc, Proc, list[Proc]]]  # load -> (server, client, helpers)


def quic_direct(cc: str, gso: bool = False, extra: Iterable[str] = ()) -> Transport:
    opts = quic_opts(cc, gso, extra)
    def build(load: list[str]):
        return quic_server([], opts), quic_client(["--connect", addr("ab", "b")], opts, load), []
    return Transport(f"quic-{cc}", build)


def quic_relay1(cc: str, gso: bool = False) -> Transport:
    opts = quic_opts(cc, gso)
    def build(load: list[str]):
        server = quic_server(["--relay", addr("r1b", "r1"), "--route", "relay"], opts)
        client = quic_client(["--relay", addr("ar1", "r1"), "--route", "relay"], opts, load)
        return server, client, [relay("r1")]
    return Transport(f"quic-{cc}-relay", build)


def quic_relay2(cc: str) -> Transport:
    opts = quic_opts(cc)
    def build(load: list[str]):
        server = quic_server(["--relay", addr("r2b", "r2"), "--route", "relay"], opts)
        client = quic_client(["--relay", addr("ar1", "r1"), "--route", "relay"], opts, load)
        helpers = [relay("r1", "--route", f"2={addr('r1r2', 'r2')}"), relay("r2", "--route", f"1={addr('r1r2', 'r1')}")]
        return server, client, helpers
    return Transport(f"quic-{cc}-relay2", build)


def quic_tcp_relay(cc: str, a_leg: str, b_leg: str, queue: int = 256) -> Transport:
    """经一个 relay，两段各自用 UDP 还是 TCP。"""
    opts = quic_opts(cc)
    def leg(link: str, kind: str) -> list[str]:
        if kind == "tcp":
            return ["--relay-tcp", addr(link, "r1"), "--route", "relay-tcp", "--tcp-queue", str(queue)]
        return ["--relay", addr(link, "r1"), "--route", "relay"]
    def build(load: list[str]):
        return quic_server(leg("r1b", b_leg), opts), quic_client(leg("ar1", a_leg), opts, load), [relay("r1", "--tcp-queue", str(queue))]
    return Transport(f"quic-{cc}-{a_leg}+{b_leg}", build)


def tcp_direct() -> Transport:
    def build(load: list[str]):
        server = Proc("b", ["serve", "tcp", "--listen", f"0.0.0.0:{PORT}"])
        return server, Proc("a", ["run", "tcp", "--connect", addr("ab", "b"), *load]), []
    return Transport("tcp", build)


def tcp_hops(count: int) -> Transport:
    """每一段各自终结的 TCP：对照"端到端一条连接、中间只转发"。"""
    def build(load: list[str]):
        server = Proc("b", ["serve", "tcp", "--listen", f"0.0.0.0:{PORT}"])
        if count == 1:
            helpers = [Proc("r1", ["hop", "--listen", f"0.0.0.0:{PORT}", "--upstream", addr("r1b", "b")])]
        else:
            helpers = [Proc("r1", ["hop", "--listen", f"0.0.0.0:{PORT}", "--upstream", addr("r1r2", "r2")]),
                       Proc("r2", ["hop", "--listen", f"0.0.0.0:{PORT}", "--upstream", addr("r2b", "b")])]
        return server, Proc("a", ["run", "tcp", "--connect", addr("ar1", "r1"), *load]), helpers
    return Transport(f"tcp-hop{count}", build)


def kcp_direct(mode: str = "fast") -> Transport:
    opts = ["--kcp-mode", mode]
    def build(load: list[str]):
        server = Proc("b", ["serve", "kcp", "--listen", f"0.0.0.0:{PORT}", *opts])
        return server, Proc("a", ["run", "kcp", "--connect", addr("ab", "b"), *opts, *load]), []
    return Transport(f"kcp-{mode}", build)


def make_case(suite: str, transport: Transport, shapes: dict[str, Shape], load: list[str], *, name: str,
              params: dict, timeout_s: float, offloads: bool = False, sysctls: dict[str, dict[str, str]] | None = None) -> Case:
    server, client, helpers = transport.build(load)
    return Case(suite, f"{transport.label}/{name}", shapes, server, client, helpers, offloads,
                {"transport": transport.label, **params}, timeout_s, sysctls or {})


# ---- 测试组 -----------------------------------------------------------------------

def suite_t1(secs: int) -> Iterator[Case]:
    """T1 同一局域网：不加 netem，卸载打开。看各传输的上限和每 GB 的 CPU。"""
    none: dict[str, Shape] = {}
    for transport in (quic_direct("cubic", gso=True), quic_direct("bbr", gso=True), tcp_direct(), kcp_direct("fast")):
        for streams in (1, 8, 64):
            for direction in ("down", "up"):
                if transport.label.startswith("kcp") and streams == 64:
                    continue
                yield make_case("t1", transport, none, bulk(secs, streams, direction), offloads=True,
                                name=f"s{streams}-{direction}", timeout_s=secs + 30,
                                params={"streams": streams, "dir": direction, "gso": True})
    # 同样的直连，关掉分段卸载：每个包一次系统调用。
    no_gso = dataclasses.replace(quic_direct("cubic", gso=False), label="quic-cubic-nogso")
    yield make_case("t1", no_gso, none, bulk(secs), offloads=True, name="s1-down", timeout_s=secs + 30,
                    params={"streams": 1, "dir": "down", "gso": False})
    # 加密套件对 CPU 的影响。
    for cipher in ("aes128", "aes256", "chacha20"):
        transport = dataclasses.replace(quic_direct("cubic", gso=True, extra=["--cipher", cipher]), label=f"quic-{cipher}")
        yield make_case("t1", transport, none, bulk(secs), offloads=True, name="s1-down", timeout_s=secs + 30,
                        params={"streams": 1, "dir": "down", "gso": True, "cipher": cipher})
    # 经 relay：relay 自己每 GB 的 CPU，分"能合并发送"和"逐包发送"两种。
    for gso in (True, False):
        transport = dataclasses.replace(quic_relay1("cubic", gso=gso), label=f"quic-relay-{'gso' if gso else 'nogso'}")
        yield make_case("t1", transport, none, bulk(secs), offloads=True, name="s1-down", timeout_s=secs + 30,
                        params={"streams": 1, "dir": "down", "gso": gso, "via": "relay"})


def wan_shapes() -> Iterator[Shape]:
    for rtt in (0, 50, 150):
        for loss in (0, 0.5, 1, 3):
            for rate in (20, 100, 1000):
                yield Shape(rtt, loss, rate)


def suite_t2(secs: int) -> Iterator[Case]:
    """T2 直连，各种网络条件下四种传输的单流下行吞吐。"""
    for shape in wan_shapes():
        for transport in (quic_direct("cubic"), quic_direct("bbr"), tcp_direct(), kcp_direct("fast")):
            yield make_case("t2", transport, {"ab": shape}, bulk(secs), name=shape.label(), timeout_s=secs + 40,
                            params={"streams": 1, "dir": "down", **dataclasses.asdict(shape)})
        if shape.rate_mbit >= 1000:
            # 默认每条流 2MB 的接收窗口会把单流吞吐限制在 窗口 / 往返延迟。放大到 16MB 看上限，
            # 也看重度丢包时会不会触发 QUIC 库"空洞太多"的内部错误。
            wide = dataclasses.replace(quic_direct("cubic", extra=["--window-mb", "16"]), label="quic-cubic-w16")
            yield make_case("t2", wide, {"ab": shape}, bulk(secs), name=shape.label(), timeout_s=secs + 40,
                            params={"streams": 1, "dir": "down", "window_mb": 16, **dataclasses.asdict(shape)})


def suite_t2_streams(secs: int) -> Iterator[Case]:
    """T2 补充：多条并发流，以及上行。"""
    for shape in (Shape(50, 0, 100), Shape(50, 1, 100), Shape(150, 0, 100), Shape(150, 1, 100)):
        for transport in (quic_direct("cubic"), quic_direct("bbr"), tcp_direct()):
            for streams in (8, 64):
                yield make_case("t2s", transport, {"ab": shape}, bulk(secs, streams), name=f"{shape.label()}-s{streams}",
                                timeout_s=secs + 40, params={"streams": streams, "dir": "down", **dataclasses.asdict(shape)})
        for transport in (quic_direct("cubic"), tcp_direct()):
            yield make_case("t2s", transport, {"ab": shape}, bulk(secs, 1, "up"), name=f"{shape.label()}-s1-up",
                            timeout_s=secs + 40, params={"streams": 1, "dir": "up", **dataclasses.asdict(shape)})


def echo_load(secs: int) -> list[str]:
    return ["--workload", "echo", "--secs", str(secs), "--interval-ms", "20", "--msg-size", "100"]


def suite_latency(secs: int) -> Iterator[Case]:
    """小消息的往返延迟分布，以及第一次访问的建立耗时。"""
    for rtt in (50, 150):
        for loss in (0, 1, 3):
            shape = Shape(rtt, loss, 100)
            for transport in (quic_direct("cubic"), quic_direct("bbr"), tcp_direct(), kcp_direct("fast"), kcp_direct("normal")):
                yield make_case("lat", transport, {"ab": shape}, echo_load(secs * 2), name=f"echo-{shape.label()}",
                                timeout_s=secs * 2 + 40, params={"workload": "echo", **dataclasses.asdict(shape)})
        for loss in (0, 1):
            shape = Shape(rtt, loss, 100)
            for transport in (quic_direct("cubic"), tcp_direct()):
                yield make_case("lat", transport, {"ab": shape}, ["--workload", "setup", "--rounds", "30"],
                                name=f"setup-{shape.label()}", timeout_s=120,
                                params={"workload": "setup", **dataclasses.asdict(shape)})


def relay_conditions() -> Iterator[tuple[str, float, float, float, float]]:
    """(名字, 端到端往返延迟, 接入段丢包, 另一段丢包, 带宽)。接入段指 a 一侧。"""
    for rtt in (50, 150):
        for rate in (20, 100):
            for label, access_loss, far_loss in (("clean", 0, 0), ("access1", 1, 0), ("access3", 3, 0), ("both1", 1, 1)):
                yield f"rtt{rtt}-{label}-{rate}M", rtt, access_loss, far_loss, rate


def suite_t4(secs: int) -> Iterator[Case]:
    """T4 经一个 relay，和 T8 的对照组（每一段各自终结的 TCP）放在一起跑，条件完全相同。"""
    for name, rtt, access_loss, far_loss, rate in relay_conditions():
        shapes = {"ar1": Shape(rtt / 2, access_loss, rate), "r1b": Shape(rtt / 2, far_loss, rate)}
        params = {"rtt_ms": rtt, "access_loss": access_loss, "far_loss": far_loss, "rate_mbit": rate, "streams": 1, "dir": "down"}
        for transport in (quic_relay1("cubic"), quic_relay1("bbr"), tcp_hops(1)):
            yield make_case("t4", transport, shapes, bulk(secs), name=name, timeout_s=secs + 40, params=params)
        # 同样的总延迟和总丢包放在一条直连链路上，看 relay 这一跳本身有没有代价。
        combined = 100 * (1 - (1 - access_loss / 100) * (1 - far_loss / 100))
        direct = dataclasses.replace(quic_direct("cubic"), label="quic-cubic-direct-equiv")
        yield make_case("t4", direct, {"ab": Shape(rtt, combined, rate)}, bulk(secs), name=name, timeout_s=secs + 40, params=params)


def suite_t5(secs: int) -> Iterator[Case]:
    """T5 经两个 relay。"""
    for rtt in (150,):
        for label, losses in (("clean", (0, 0, 0)), ("access1", (1, 0, 0)), ("all1", (1, 1, 1))):
            shapes = {link: Shape(rtt / 3, loss, 100) for link, loss in zip(("ar1", "r1r2", "r2b"), losses)}
            params = {"rtt_ms": rtt, "losses": list(losses), "rate_mbit": 100, "streams": 1, "dir": "down"}
            for transport in (quic_relay2("cubic"), quic_relay2("bbr"), tcp_hops(2)):
                yield make_case("t5", transport, shapes, bulk(secs), name=f"rtt{rtt}-{label}", timeout_s=secs + 40, params=params)


def suite_t6(secs: int) -> Iterator[Case]:
    """T6 一条连接上大流量和交互流量同时跑，对比把交互流量分到另一条连接。"""
    secs = max(secs, 15)
    for shape in (Shape(50, 0, 20), Shape(50, 0, 100), Shape(50, 1, 100), Shape(150, 0, 100), Shape(150, 1, 100)):
        for cc in ("cubic", "bbr"):
            for split in (False, True):
                load = ["--workload", "mixed", "--secs", str(secs), "--warmup", "5", "--interval-ms", "20", "--msg-size", "100",
                        *(["--split"] if split else [])]
                transport = dataclasses.replace(quic_direct(cc), label=f"quic-{cc}-{'split' if split else 'shared'}")
                yield make_case("t6", transport, {"ab": shape}, load, name=shape.label(), timeout_s=secs + 40,
                                params={"cc": cc, "split": split, **dataclasses.asdict(shape)})
        # 只有交互流量、没有大流量时的延迟，作为基准。
        yield make_case("t6", dataclasses.replace(quic_direct("cubic"), label="quic-cubic-echo-only"), {"ab": shape},
                        echo_load(secs), name=shape.label(), timeout_s=secs + 40,
                        params={"cc": "cubic", "split": None, **dataclasses.asdict(shape)})


def suite_t7(secs: int) -> Iterator[Case]:
    """T7 换路：先经 relay，第 6 秒改走直连，第 12 秒改回 relay。两端按同一张时间表各自切换。"""
    schedule = "6:direct,12:relay"
    client_direct = f"10.1.0.1:{PORT + 1}"
    for label, relay_rtt, direct_rtt in (("to-faster", 100, 30), ("same", 60, 60), ("to-slower", 30, 100)):
        shapes = {"ar1": Shape(relay_rtt / 2, 0, 100), "r1b": Shape(relay_rtt / 2, 0, 100), "ab": Shape(direct_rtt, 0, 100)}
        for cc in ("cubic", "bbr"):
            opts = quic_opts(cc)
            for workload in ("bulk", "mixed"):
                load = ["--workload", workload, "--secs", "18", "--warmup", "2", "--series", "--interval-ms", "20"]
                server = quic_server(["--relay", addr("r1b", "r1"), "--direct", client_direct, "--route", "relay", "--switch", schedule], opts)
                client = quic_client(["--bind", f"0.0.0.0:{PORT + 1}", "--connect", addr("ab", "b"), "--relay", addr("ar1", "r1"),
                                      "--route", "relay", "--switch", schedule], opts, load)
                yield Case("t7", f"quic-{cc}/{label}-{workload}", shapes, server, client, [relay("r1")], False,
                           {"transport": f"quic-{cc}", "relay_rtt": relay_rtt, "direct_rtt": direct_rtt, "workload": workload,
                            "schedule": schedule}, 70)
                # 不换路的对照：全程走 relay。
            load = ["--workload", "mixed", "--secs", "18", "--warmup", "2", "--series", "--interval-ms", "20"]
            server = quic_server(["--relay", addr("r1b", "r1"), "--route", "relay"], opts)
            client = quic_client(["--relay", addr("ar1", "r1"), "--route", "relay"], opts, load)
            yield Case("t7", f"quic-{cc}/{label}-noswitch", shapes, server, client, [relay("r1")], False,
                       {"transport": f"quic-{cc}", "relay_rtt": relay_rtt, "direct_rtt": direct_rtt, "workload": "mixed",
                        "schedule": None}, 70)


def suite_t9(secs: int) -> Iterator[Case]:
    """T9 TCP 中转：两段都 UDP、a 段 TCP、两段都 TCP。大流量加小消息一起跑，看 TCP 那段丢包时的停顿。"""
    secs = max(secs, 15)
    load = ["--workload", "mixed", "--secs", str(secs), "--warmup", "5", "--interval-ms", "20", "--msg-size", "100"]
    for rtt in (50, 150):
        for loss in (0, 1, 3):
            shapes = {"ar1": Shape(rtt / 2, loss, 100), "r1b": Shape(rtt / 2, 0, 100)}
            for a_leg, b_leg in (("udp", "udp"), ("tcp", "udp"), ("tcp", "tcp")):
                for cc in ("cubic", "bbr"):
                    yield make_case("t9", quic_tcp_relay(cc, a_leg, b_leg), shapes, load, name=f"rtt{rtt}-loss{loss:g}",
                                    timeout_s=secs + 40, params={"cc": cc, "legs": f"{a_leg}+{b_leg}", "rtt_ms": rtt,
                                                                 "access_loss": loss, "tcp_queue": 256})
    # 发送队列的上限设多大合适：固定一种条件，扫队列长度。
    shapes = {"ar1": Shape(25, 1, 100), "r1b": Shape(25, 0, 100)}
    for queue in (32, 128, 1024, 4096):
        for cc in ("cubic", "bbr"):
            transport = dataclasses.replace(quic_tcp_relay(cc, "tcp", "udp", queue), label=f"quic-{cc}-tcp+udp-q{queue}")
            yield make_case("t9", transport, shapes, load, name="rtt50-loss1", timeout_s=secs + 40,
                            params={"cc": cc, "legs": "tcp+udp", "rtt_ms": 50, "access_loss": 1, "tcp_queue": queue})
    # 上面那个队列之后还有内核的发送缓冲。不设上限时，内核里"还没发出去的数据"能积到几百 KB 以上，
    # 队列设多短都不起作用。这里把它压到 16KB：产品里是每条 TCP 上的 TCP_NOTSENT_LOWAT，测试里用命名空间级的内核参数代替。
    lowat = {ns: {"net.ipv4.tcp_notsent_lowat": "16384"} for ns in ("a", "r1", "b")}
    for rtt, loss, cc in ((50, 1, "cubic"), (50, 1, "bbr"), (50, 3, "cubic"), (150, 1, "cubic")):
        shapes = {"ar1": Shape(rtt / 2, loss, 100), "r1b": Shape(rtt / 2, 0, 100)}
        transport = dataclasses.replace(quic_tcp_relay(cc, "tcp", "udp", 32), label=f"quic-{cc}-tcp+udp-q32-lowat16k")
        yield make_case("t9", transport, shapes, load, name=f"rtt{rtt}-loss{loss:g}", timeout_s=secs + 40, sysctls=lowat,
                        params={"cc": cc, "legs": "tcp+udp", "rtt_ms": rtt, "access_loss": loss, "tcp_queue": 32,
                                "notsent_lowat": 16384})


def llm_load(concurrency: int, requests: int, silent_ms: int) -> list[str]:
    return ["--workload", "llm", "--concurrency", str(concurrency), "--requests", str(requests),
            "--body-min", "30000", "--body-max", "1000000", "--ttft-ms", "800", "--tokens", "200",
            "--token-interval-ms", "30", "--token-size", "80", "--silent-every", "20", "--silent-after", "20",
            "--silent-ms", str(silent_ms)]


def suite_t10(_secs: int) -> Iterator[Case]:
    """T10 LLM 网关的负载：一条常驻连接上几千条流式请求，部分请求中途静默两分钟。

    a 是入口，b 是出口。"出口没有 UDP"的情形里，relay 紧挨着入口（a—r1 这一段不加延迟），
    出口用 TCP 连 relay；对照是每个请求一条 TCP，每次现拨，所以多一次握手。
    """
    load = llm_load(concurrency=2000, requests=4000, silent_ms=120_000)
    timeout = 600
    for loss in (0, 1):
        wan = Shape(50, loss, 1000)
        name = f"rtt50-loss{loss:g}"
        params = {"rtt_ms": 50, "loss": loss, "rate_mbit": 1000, "concurrency": 2000, "requests": 4000}
        idle = ["--idle-secs", "0"]
        yield make_case("t10", dataclasses.replace(quic_direct("cubic", extra=idle), label="quic-direct"), {"ab": wan}, load,
                        name=name, timeout_s=timeout, params={**params, "path": "direct"})
        yield make_case("t10", dataclasses.replace(quic_direct("bbr", extra=idle), label="quic-bbr-direct"), {"ab": wan}, load,
                        name=name, timeout_s=timeout, params={**params, "path": "direct"})
        near_entry = {"ar1": Shape(0, 0, 0), "r1b": wan}
        for a_leg, b_leg, label in (("udp", "udp", "quic-relay-udp"), ("udp", "tcp", "quic-relay-exit-tcp")):
            opts = quic_opts("cubic", extra=idle)
            def leg(link: str, kind: str) -> list[str]:
                if kind == "tcp":
                    return ["--relay-tcp", addr(link, "r1"), "--route", "relay-tcp", "--tcp-queue", "1024"]
                return ["--relay", addr(link, "r1"), "--route", "relay"]
            server, client = quic_server(leg("r1b", b_leg), opts), quic_client(leg("ar1", a_leg), opts, load)
            yield Case("t10", f"{label}/{name}", near_entry, server, client, [relay("r1", "--tcp-queue", "1024")], False,
                       {**params, "transport": label, "path": f"relay {a_leg}+{b_leg}"}, timeout)
        yield make_case("t10", dataclasses.replace(tcp_direct(), label="tcp-per-request"), {"ab": wan}, load,
                        name=name, timeout_s=timeout, params={**params, "path": "one tcp per request"})


SUITES: dict[str, Callable[[int], Iterator[Case]]] = {
    "t1": suite_t1,
    "t2": suite_t2,
    "t2s": suite_t2_streams,
    "lat": suite_latency,
    "t4": suite_t4,
    "t5": suite_t5,
    "t6": suite_t6,
    "t7": suite_t7,
    "t9": suite_t9,
    "t10": suite_t10,
}


def enter_lab() -> None:
    """不在实验用的命名空间里时，换进去重新执行自己。"""
    if os.environ.get(IN_LAB_ENV) == "1":
        return
    env = {**os.environ, IN_LAB_ENV: "1"}
    os.execvpe("unshare", ["unshare", "-Urnm", sys.executable, *sys.argv], env)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list")
    sub.add_parser("shell")
    run = sub.add_parser("run")
    run.add_argument("suites", nargs="+", choices=sorted(SUITES))
    run.add_argument("--out", type=Path, default=REPO / "docs" / "internal" / "pretest" / "results")
    run.add_argument("--secs", type=int, default=10, help="每次大流量测试的时长")
    run.add_argument("--only", default="", help="只跑名字里包含这个子串的用例")
    run.add_argument("--redo", action="store_true", help="已经有结果的用例也重跑")
    args = parser.parse_args()

    if args.command == "list":
        for name, suite in SUITES.items():
            print(f"{name:5} {len(list(suite(10))):4} 个用例  {(suite.__doc__ or '').strip().splitlines()[0]}")
        return 0
    if not BIN.exists():
        print(f"找不到 {BIN}，先执行 cargo build --release -p i3t-pretest", file=sys.stderr)
        return 1

    # T10 里每个请求一条 TCP 的对照组要同时开几千个连接。
    _soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (hard, hard))

    enter_lab()
    lab = Lab()
    lab.setup()
    if args.command == "shell":
        return subprocess.call([os.environ.get("SHELL", "bash")])

    args.out.mkdir(parents=True, exist_ok=True)
    runner = Runner(lab, args.out)
    for suite_name in args.suites:
        out_file = args.out / f"{suite_name}.jsonl"
        existing = [json.loads(line) for line in out_file.read_text().splitlines() if line.strip()] if out_file.exists() else []
        cases = [case for case in SUITES[suite_name](args.secs) if args.only in case.key]
        if args.redo:
            # 重跑的用例先把旧记录去掉，文件里每个用例始终只有一行。
            rerun = {case.key for case in cases}
            existing = [record for record in existing if record["case"] not in rerun]
            out_file.write_text("".join(json.dumps(record, ensure_ascii=False) + "\n" for record in existing))
        else:
            done = {record["case"] for record in existing}
            cases = [case for case in cases if case.key not in done]
        print(f"[{suite_name}] 要跑 {len(cases)} 个用例，已有 {len(existing)} 个", flush=True)
        with out_file.open("a") as sink:
            for index, case in enumerate(cases, 1):
                result = runner.run(case)
                sink.write(json.dumps(result, ensure_ascii=False) + "\n")
                sink.flush()
                status = result.get("error") or summarize(result)
                print(f"[{suite_name} {index}/{len(cases)}] {case.name}: {status}", flush=True)
    return 0


def summarize(result: dict) -> str:
    client = result["client"] or {}
    parts = []
    if "bulk" in client:
        parts.append(f"{client['bulk']['mbps']:.1f} Mbit/s")
    if "echo" in client:
        rtt = client["echo"]["rtt_ms"]
        parts.append(f"往返 p50 {rtt['p50_ms']:.1f} p99 {rtt['p99_ms']:.1f} ms")
    if "llm" in client:
        llm = client["llm"]
        parts.append(f"成功 {llm['ok'] + llm['silent_ok']} 失败 {llm['failed'] + llm['silent_failed']}")
    if "setup" in client:
        parts.append(f"新连接 p50 {client['setup']['new_connection_ms']['p50_ms']:.1f} ms")
    return "，".join(parts) or "完成"


if __name__ == "__main__":
    sys.exit(main())
