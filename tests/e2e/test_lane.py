#!/usr/bin/env python3
"""ice3proxy（lane）的端到端测试：一对入口—出口之间每条 TCP 流独占一条 TCP。

    python3 tests/e2e/test_lane.py [场景名...]

前几个场景的节点用静态的 netmap 运行：两边互相看到的对端条目里写着 `"lane"`，
启用了 relay 的那一边在 netmap 的 relays 里带着它的 TCP 地址，另一边拨它。计数从节点本机的 /metrics 读。

default_where_udp_is_missing、default_follows_udp 由 controller 管理，节点对都没有设置（默认的 udp_fallback，
存下的设置只用来对照），用 I3T_BIN_DIR 里的程序。managed_drain 也由 controller 管理（节点对设置、排空、更新），
要用 test_update.py 编的那一套程序（第一次会编译几分钟）。
"""

from __future__ import annotations

import json
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import harness  # noqa: E402
import lab as lablib  # noqa: E402
from harness import (Checks, Controller, Node, binary, blob_digest, blob_file, block_tcp_port, fetch, lane_counts,  # noqa: E402
                     lane_line, lane_peer, set_nat_timeouts, start_blob_server, tail)
from lab import Host, Lab, NatKind, wait_until  # noqa: E402

BIG = 20_000_000
# udp_fallback 下 UDP 回来之后拨号方还拨多少秒才放手，测试里调短（程序里是 10 分钟）。
HOLD_SWITCH = "I3T_TEST_LANE_HOLD"


def waited(condition, timeout: float, interval: float = 0.5) -> bool:
    try:
        wait_until(condition, timeout=timeout, interval=interval)
        return True
    except TimeoutError:
        return False


class Pair:
    """一对入口—出口，和"互联网"上的一台数据服务（明文 80、TLS 443）。"""

    def __init__(self, lab: Lab, work: Path, entry: Node, exit_node: Node, web: Host, cert: Path) -> None:
        self.lab, self.work, self.entry, self.exit, self.web, self.cert = lab, work, entry, exit_node, web, cert

    def url(self, path: str, *, tls: bool = False) -> str:
        return f"{'https' if tls else 'http'}://{self.web.ip}/{path.lstrip('/')}"

    def fetch(self, path: str, *, tls: bool = False, **options) -> tuple[int, str, str]:
        return fetch(self.entry, self.url(path, tls=tls), via=self.exit, cert=self.cert if tls else None, **options)

    def intact(self, size: int, *, tls: bool = False, **options) -> tuple[bool, str]:
        code, digest, err = self.fetch(f"blob/{size}", tls=tls, **options)
        return code == 0 and digest == blob_digest(size), f"curl 退出码 {code}，{err.strip()[-300:]}"

    def entry_lanes(self) -> dict[str, float]:
        return lane_counts(self.entry, self.exit)

    def exit_lanes(self) -> dict[str, float]:
        return lane_counts(self.exit, self.entry)

    def report(self) -> None:
        print(f"  {lane_line(self.entry, self.exit)}\n  {lane_line(self.exit, self.entry)}", flush=True)

    def logs(self, lines: int = 25) -> None:
        print(tail(self.entry.read_log(), lines), tail(self.exit.read_log(), lines), sep="\n  ---\n")


def public_entry(lab: Lab, work: Path, lane: str, *, tcp_blocked: bool = False, exit_view_of_entry: dict | None = None,
                 nat_timeouts: tuple[int, int] | None = None, hold: int | None = None) -> Pair:
    """入口在公网上并且兼做 relay；出口在家用 NAT 后面，拨入口的 TCP 端口。`hold` 是 udp_fallback 下 UDP 回来之后
    出口还拨多少秒（I3T_TEST_LANE_HOLD，不写是程序的 10 分钟）。"""
    web = lab.host("web")
    cert = start_blob_server(web, work)
    entry = Node(lab.host("entry"), work, 1, declare=("relay",))
    if tcp_blocked:
        block_tcp_port(entry.host, 20330)
    home = lab.nat("home", NatKind.CONE)
    if nat_timeouts:
        set_nat_timeouts(home, *nat_timeouts)
    exit_node = Node(home.host("exit"), work, 2, declare=("exit",))
    if hold is not None:
        exit_node.env = {HOLD_SWITCH: str(hold)}
    entry.start_static(entry.netmap(me={"relay": {}}, relays=[entry.as_relay()], relay_clients=[entry, exit_node],
                                    peers=[lane_peer(exit_node, lane=lane, endpoints=False, attached=[(entry, 5)], exit=True)],
                                    proxies=[entry.proxy_to(exit_node)]))
    entry_peer = {**lane_peer(entry, lane=lane), **(exit_view_of_entry or {})}
    exit_node.start_static(exit_node.netmap(me={"exit": {}}, relays=[entry.as_relay()], peers=[entry_peer]))
    pair = Pair(lab, work, entry, exit_node, web, cert)
    pair.home = home
    return pair


def public_exit(lab: Lab, work: Path, lane: str) -> Pair:
    """出口在公网上并且兼做 relay；入口在 NAT 后面（谁也连不进来），拨出口的 TCP 端口，用自己拨的 lane 开流。"""
    web = lab.host("web")
    cert = start_blob_server(web, work)
    exit_node = Node(lab.host("exit"), work, 2, declare=("exit", "relay"))
    office = lab.nat("office", NatKind.SYMMETRIC)
    entry = Node(office.host("entry"), work, 1)
    exit_node.start_static(exit_node.netmap(me={"exit": {}, "relay": {}}, relays=[exit_node.as_relay()], relay_clients=[entry, exit_node],
                                            peers=[lane_peer(entry, lane=lane, endpoints=False, attached=[(exit_node, 5)])]))
    entry.start_static(entry.netmap(relays=[exit_node.as_relay()], peers=[lane_peer(exit_node, lane=lane, exit=True)],
                                    proxies=[entry.proxy_to(exit_node)]))
    return Pair(lab, work, entry, exit_node, web, cert)


def holder_ready(pair: Pair, timeout: float = 30) -> bool:
    """入口那里有出口停过来的 lane 了。"""
    return waited(lambda: (c := pair.entry_lanes())["control_up"] == 1 and c["parked_here"] >= 1, timeout)


def nat_exit_dials_public_entry(lab: Lab, work: Path, t: Checks) -> None:
    """always：NAT 后面的出口拨公网上的入口。HTTP 在 lane 的 TLS 里，HTTPS 不再加密、由内核搬；字节一个不差。"""
    p = public_entry(lab, work, "always")
    if not t.check("出口拨到入口：控制 lane 建好，入口那里停着出口拨来的 lane", holder_ready(p), lane_line(p.entry, p.exit)):
        p.logs()
        return
    before = p.entry_lanes()
    ok, detail = p.intact(BIG)
    t.check("明文 HTTP 下载 20MB，摘要对得上", ok, detail)
    after = p.entry_lanes()
    t.check("它走的是 lane：取用了一条，lane 的 TLS 里搬了 20MB", after["claims"] > before["claims"] and
            after["sealed_bytes"] - before["sealed_bytes"] >= BIG, f"{before} → {after}")

    before, exit_before = p.entry_lanes(), p.exit_lanes()
    ok, detail = p.intact(BIG, tls=True)
    t.check("HTTPS 下载 20MB，摘要对得上", ok, detail)
    after, exit_after = p.entry_lanes(), p.exit_lanes()
    raw = lambda c: c["passthrough_bytes"] + c["spliced_bytes"]  # noqa: E731
    t.check("HTTPS 不再加密：入口和出口两边都在 lane 之外直接搬了 20MB", raw(after) - raw(before) >= BIG and
            raw(exit_after) - raw(exit_before) >= BIG, f"入口 {raw(before)}→{raw(after)}，出口 {raw(exit_before)}→{raw(exit_after)}")
    t.check("Linux 上由内核搬（splice）", after["spliced_bytes"] > before["spliced_bytes"] and exit_after["spliced_bytes"] >
            exit_before["spliced_bytes"], f"入口 {after['spliced_bytes']}，出口 {exit_after['spliced_bytes']}")

    ok, detail = p.intact(5_000_000, tls=True, proxy="http")
    t.check("经 HTTP CONNECT 入口的 HTTPS 也走 lane，摘要对得上", ok, detail)
    for tls in (False, True):
        code, answer, err = p.fetch("digest", tls=tls, upload=blob_file(p.entry, 8_000_000))
        t.check(f"上传 8MB（{'HTTPS' if tls else 'HTTP'}），服务端收到的字节的摘要对得上", code == 0 and answer == blob_digest(8_000_000),
                f"curl 退出码 {code}，{err.strip()[-200:]}")

    for tls in (False, True):
        code, _, err = p.fetch("reset/200000", tls=tls, timeout=20)
        t.check(f"上游回答到一半 RST（{'HTTPS' if tls else 'HTTP'}）：客户端看到的是重置，不是正常结束", code == 56,
                f"curl 退出码 {code}（56 是收的时候连接被重置，18 是对方正常关闭但没收全）：{err.strip()[-200:]}")
    t.check("没有一条流改走共用的连接", p.entry_lanes()["fallbacks"] == 0 and p.exit_lanes()["fallbacks"] == 0)
    p.report()
    if t.failures:
        p.logs()


def firewalled_entry_dials_public_exit(lab: Lab, work: Path, t: Checks) -> None:
    """always：入口在 NAT 后面，拨公网上兼做 relay 的出口，用自己拨的 lane 开流，不需要出口停 lane 过来。"""
    p = public_exit(lab, work, "always")
    ready = waited(lambda: p.entry_lanes()["control_up"] == 1, 30)
    if not t.check("入口拨到出口：控制 lane 建好", ready, lane_line(p.entry, p.exit)):
        p.logs()
        return
    before = p.entry_lanes()
    for tls in (False, True):
        ok, detail = p.intact(BIG, tls=tls)
        t.check(f"{'HTTPS' if tls else '明文 HTTP'} 下载 20MB，摘要对得上", ok, detail)
    after = p.entry_lanes()
    t.check("流走的是入口自己拨的 lane（不从出口停过来的里取）", after["carried_bytes"] - before["carried_bytes"] >= 2 * BIG and
            after["claims"] == 0, f"{before} → {after}")
    t.check("HTTPS 由内核搬", after["spliced_bytes"] - before["spliced_bytes"] >= BIG, f"{after['spliced_bytes']}")
    t.check("开过流之后入口备着自己拨好的热 lane", waited(lambda: p.entry_lanes()["ready"] >= 1, 10), lane_line(p.entry, p.exit))
    t.check("没有一条流改走共用的连接", after["fallbacks"] == 0)
    p.report()
    if t.failures:
        p.logs()


def no_lanes(node: Node, peer: Node) -> bool:
    """`node` 和 `peer` 之间什么 lane 都没有：没有控制 lane，没有空闲的、在用的、在拨的。"""
    c = lane_counts(node, peer)
    return c["control_up"] == 0 and c["dialling"] == 0 and c["in_use"] == 0 and c["parked_here"] + c["ready"] + c["parked_at_peer"] == 0


def udp_fallback(lab: Lab, work: Path, t: Checks) -> None:
    """udp_fallback：UDP 通的时候什么都不拨，流走共用的连接；出口的 UDP 被挡住、只能经 TCP 中转时出口拨起来，新的流走 lane；
    UDP 恢复后新的流回到共用的连接，过了保持时间出口放手。"""
    hold = 15
    p = public_entry(lab, work, "udp_fallback", hold=hold)
    wait_until(lambda: p.fetch("blob/10", timeout=5)[0] == 0, timeout=40, what="经出口访问到数据服务")
    time.sleep(5)
    t.check("UDP 通：出口不拨 lane，两边什么都不连着", no_lanes(p.entry, p.exit) and no_lanes(p.exit, p.entry),
            f"{lane_line(p.entry, p.exit)}；{lane_line(p.exit, p.entry)}")
    before = p.entry_lanes()
    ok, detail = p.intact(2_000_000)
    t.check("UDP 通：下载完整", ok, detail)
    t.check("UDP 通：流走共用的连接，没有取 lane，也不算改走", p.entry_lanes()["claims"] == before["claims"] and
            p.entry_lanes()["fallbacks"] == 0, str(p.entry_lanes()))

    p.exit.host.block_udp()
    blocked = time.monotonic()
    switched = waited(lambda: "到 relay 的中转会话建立（TCP）" in p.exit.read_log(), 60)
    t.check("出口的 UDP 被挡住之后经 TCP 连上入口", switched, tail(p.exit.read_log(), 10))
    print(f"  UDP 挡住之后 {time.monotonic() - blocked:.1f} 秒出口经 TCP 连上了入口", flush=True)
    # 出口这一段要连续 40 秒没有 UDP 才拨（到 relay 的会话 30 秒之后会再试一次 UDP）。
    t.check("没有 UDP 的路一阵之后出口拨起来，入口那里有了它停过来的 lane", holder_ready(p, 90), lane_line(p.entry, p.exit))
    print(f"  UDP 挡住之后 {time.monotonic() - blocked:.1f} 秒入口那里有了出口停过来的 lane", flush=True)

    def on_lane() -> bool:
        claims = p.entry_lanes()["claims"]
        good, _ = p.intact(2_000_000)
        return good and p.entry_lanes()["claims"] > claims

    moved = waited(on_lane, 30, interval=1)
    t.check("没有 UDP 的路时新的流走 lane，下载完整", moved, lane_line(p.entry, p.exit))
    print(f"  UDP 挡住之后 {time.monotonic() - blocked:.1f} 秒新的流开始走 lane", flush=True)

    p.exit.host.block_udp(False)
    opened = time.monotonic()

    def off_lane() -> bool:
        claims = p.entry_lanes()["claims"]
        good, _ = p.intact(1_000_000)
        return good and p.entry_lanes()["claims"] == claims

    back = waited(off_lane, 120, interval=2)
    t.check("UDP 恢复之后新的流回到共用的连接", back, lane_line(p.entry, p.exit))
    print(f"  UDP 放开之后 {time.monotonic() - opened:.1f} 秒新的流回到共用的连接", flush=True)
    let_go = waited(lambda: no_lanes(p.entry, p.exit) and no_lanes(p.exit, p.entry), 120 + hold, interval=1)
    t.check(f"UDP 恢复之后过了保持时间（{hold} 秒）出口放手，两边什么都不连着", let_go,
            f"{lane_line(p.entry, p.exit)}；{lane_line(p.exit, p.entry)}")
    print(f"  UDP 放开之后 {time.monotonic() - opened:.1f} 秒出口放手", flush=True)
    t.check("始终没有流改走（取不到 lane），也没有拨号失败", p.entry_lanes()["fallbacks"] == 0 and p.exit_lanes()["dial_failures"] == 0,
            f"{p.entry_lanes()}；{p.exit_lanes()}")
    if t.failures:
        p.logs()


BURST = r"""
import hashlib, json, random, socket, struct, sys, threading, time
proxy_port, target, size, count = int(sys.argv[1]), sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
expected = hashlib.sha256(random.Random(size).randbytes(size)).hexdigest()
start = threading.Barrier(count)
results = []
def one():
    try:
        start.wait()
        began = time.monotonic()
        s = socket.create_connection(("127.0.0.1", proxy_port), timeout=60)
        s.sendall(b"\x05\x01\x00")
        assert s.recv(2) == b"\x05\x00"
        s.sendall(b"\x05\x01\x00\x01" + socket.inet_aton(target) + struct.pack("!H", 80))
        reply = b""
        while len(reply) < 10:
            reply += s.recv(10 - len(reply))
        assert reply[1] == 0, reply
        s.sendall(f"GET /blob/{size} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n".encode())
        data = b""
        while True:
            chunk = s.recv(1 << 16)
            if not chunk:
                break
            data += chunk
        body = data.split(b"\r\n\r\n", 1)[1]
        results.append((hashlib.sha256(body).hexdigest() == expected, time.monotonic() - began))
    except Exception as error:
        results.append((False, repr(error)))
threads = [threading.Thread(target=one) for _ in range(count)]
for thread in threads:
    thread.start()
for thread in threads:
    thread.join()
ok = [took for good, took in results if good]
ok.sort()
print(json.dumps({"ok": len(ok), "errors": [took for good, took in results if not good][:3],
                  "p50": ok[len(ok) // 2] if ok else None, "max": ok[-1] if ok else None}))
"""


def burst_drains_pool(lab: Lab, work: Path, t: Checks) -> None:
    """always：几百条流在同一时刻打开，远多于停着的 lane：池被抽干，补拨跟上，全部完整。"""
    p = public_entry(lab, work, "always")
    if not t.check("lane 拨好", holder_ready(p), lane_line(p.entry, p.exit)):
        return
    idle = p.entry_lanes()["parked_here"]
    count, size = 300, 200_000
    before = p.entry_lanes()
    began = time.monotonic()
    done = p.entry.host.run(sys.executable, "-c", BURST, str(p.entry.entry(p.exit).port), p.web.ip, str(size), str(count), check=False,
                            timeout=180)
    took = time.monotonic() - began
    result = json.loads(done.stdout) if done.stdout.strip() else {"ok": 0, "errors": [done.stderr[-300:]]}
    after = p.entry_lanes()
    t.check(f"同时打开 {count} 条（入口那里本来停着 {idle:.0f} 条），全部完整", result["ok"] == count, str(result))
    claims, fallbacks = after["claims"] - before["claims"], after["fallbacks"] - before["fallbacks"]
    print(f"  用了 {took:.1f} 秒，单条 p50 {result.get('p50') or 0:.2f}s、最长 {result.get('max') or 0:.2f}s；取用 {claims:.0f} 次，"
          f"其中等补拨 {after['claims_waited'] - before['claims_waited']:.0f} 次、共 {after['claim_wait_ms'] - before['claim_wait_ms']:.0f}ms，"
          f"改走共用连接 {fallbacks:.0f} 条", flush=True)
    t.check("池抽干时流等补拨，而不是一下子改走共用的连接", claims + fallbacks >= count and fallbacks <= count // 10,
            f"取用 {claims}，改走 {fallbacks}")
    p.report()


def tcp_blocked_falls_back(lab: Lab, work: Path, t: Checks) -> None:
    """always，但入口的 TCP 端口被防火墙挡住：lane 建不起来，流悄悄走共用的连接、记作改走；放开之后 lane 建起来，新的流走 lane。"""
    p = public_entry(lab, work, "always", tcp_blocked=True)
    wait_until(lambda: p.fetch("blob/10", timeout=5)[0] == 0, timeout=40, what="经出口访问到数据服务")
    ok, detail = p.intact(2_000_000)
    t.check("lane 建不起来时下载照样完整", ok, detail)
    c = p.entry_lanes()
    t.check("入口记下了改走共用连接的流", c["fallbacks"] >= 1 and c["control_up"] == 0, str(c))
    # SYN 被丢掉：拨号一直挂着，到 10 秒的时限才算失败。
    c = p.exit_lanes()
    t.check("出口拨不通：挂在拨号上，或者已经记下了失败", c["dialling"] >= 1 or c["dial_failures"] >= 1, str(c))
    block_tcp_port(p.entry.host, 20330, False)
    opened = time.monotonic()
    t.check("放开 TCP 端口之后 lane 建起来", holder_ready(p, 60), lane_line(p.entry, p.exit))
    print(f"  放开之后 {time.monotonic() - opened:.1f} 秒入口那里有了停着的 lane（出口拨号失败之后按退避重试，最长 30 秒）", flush=True)
    before = p.entry_lanes()
    ok, detail = p.intact(2_000_000)
    after = p.entry_lanes()
    t.check("之后新的流走 lane，不再改走", ok and after["claims"] > before["claims"] and after["fallbacks"] == before["fallbacks"],
            f"{detail}；{before} → {after}")
    p.report()


def access_denied(lab: Lab, work: Path, t: Checks) -> None:
    """always，但出口不让入口访问它：lane 上开过来的流照样被出口按访问规则拒绝，不会因为换了传输就放进去。"""
    p = public_entry(lab, work, "always", exit_view_of_entry={"inbound": False})
    if not t.check("lane 拨好（拨 lane 的方向是出口到入口，不受影响）", holder_ready(p), lane_line(p.entry, p.exit)):
        return
    before = p.entry_lanes()
    refused_before = harness.metric(harness.node_metrics(p.exit), "i3t_exit_failures_total", attribution="refused")
    code, digest, _ = p.fetch("blob/100000", timeout=10)
    after = p.entry_lanes()
    refused = harness.metric(harness.node_metrics(p.exit), "i3t_exit_failures_total", attribution="refused") - refused_before
    t.check("请求失败，没有拿到数据", code != 0 and digest != blob_digest(100_000), f"curl 退出码 {code}")
    t.check("流是在 lane 上开过去、被出口拒绝的", after["claims"] > before["claims"] and refused >= 1 and after["fallbacks"] == 0,
            f"取用 {before['claims']}→{after['claims']}，出口拒绝 {refused}，改走 {after['fallbacks']}")
    p.report()


def exit_restart(lab: Lab, work: Path, t: Checks) -> None:
    """always：流正在跑的时候出口重启。手上的流以重置结束（不是正常结束）；出口回来之后 lane 很快重新拨好，新的流又走 lane。"""
    p = public_entry(lab, work, "always")
    if not t.check("lane 拨好", holder_ready(p), lane_line(p.entry, p.exit)):
        return
    for how, sig in (("正常重启（SIGTERM）", signal.SIGTERM), ("崩溃后重启（SIGKILL）", signal.SIGKILL)):
        running = subprocess.Popen(p.entry.host.cmd("bash", "-c", f"curl -sS --max-time 60 -x {p.entry.entry(p.exit).url('socks5h')} "
                                                    f"-o /dev/null '{p.url('slow/20/4000000')}'; echo \"exit=$?\""),
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        time.sleep(3)
        on_lane = p.entry_lanes()["in_use"] >= 1
        p.exit.host.stop(p.exit.proc, sig=sig)
        p.exit.proc = None
        out, _ = running.communicate(timeout=70)
        code = int(out.rsplit("exit=", 1)[-1].strip() or -1)
        t.check(f"{how}：跑在 lane 上的流以重置结束", on_lane and code == 56, f"在 lane 上 {on_lane}；curl：{out.strip()[-200:]}")
        claims = p.entry_lanes()["claims"]
        p.exit.start_static(json.loads((p.exit.state / "static-netmap.json").read_text()))
        up = time.monotonic()

        def fresh_on_lane() -> bool:
            good, _ = p.intact(100_000, timeout=5)
            return good and p.entry_lanes()["claims"] > claims

        recovered = waited(fresh_on_lane, 30, interval=0.5)
        t.check(f"{how}：出口回来之后 10 秒内新的流又走 lane", recovered and time.monotonic() - up <= 10,
                f"用了 {time.monotonic() - up:.1f} 秒；{lane_line(p.entry, p.exit)}")
        print(f"  {how}：出口新进程启动之后 {time.monotonic() - up:.1f} 秒，新的流在 lane 上完整地跑完", flush=True)
    p.report()
    if t.failures:
        p.logs()


def silent_stream_outlives_nat_mapping(lab: Lab, work: Path, t: Checks) -> None:
    """always：NAT 回收 TCP 映射只要 40 秒，一条 lane 上的流静默 70 秒：保活把映射续住，后半截一个字节不少；停着的 lane 也还能用。"""
    p = public_entry(lab, work, "always", nat_timeouts=(20, 40))
    if not t.check("lane 拨好", holder_ready(p), lane_line(p.entry, p.exit)):
        return
    size = 2_000_000
    expected = blob_digest(size)
    procs = []
    for tls in (False, True):
        tls_args = f"--cacert {p.cert}" if tls else ""
        script = (f"curl -sS --max-time 120 -x {p.entry.entry(p.exit).url('socks5h')} {tls_args} "
                  f"'{p.url(f'slow/70/{size}', tls=tls)}' | sha256sum | cut -d' ' -f1")
        procs.append((tls, subprocess.Popen(p.entry.host.cmd("bash", "-c", script), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)))
    for tls, proc in procs:
        out, err = proc.communicate(timeout=130)
        t.check(f"{'HTTPS' if tls else 'HTTP'}：静默 70 秒之后后半截完整到达", out.strip() == blob_digest(size) and out.strip() == expected,
                err.strip()[-200:])
    before = p.entry_lanes()
    ok, detail = p.intact(1_000_000)
    after = p.entry_lanes()
    t.check("之后新的流照样在 lane 上，没有改走", ok and after["claims"] > before["claims"] and after["fallbacks"] == 0, detail)
    p.report()


def established_to(host: Host, ip: str, port: int = 20330) -> list[str]:
    """`host` 上连到 `ip:port`、建立着的 TCP 连接，一条一行。"""
    done = host.run("ss", "-Htn", "state", "established", f"( dst {ip} and dport = :{port} )", check=False)
    return [line for line in done.stdout.splitlines() if line.strip()]


class Managed:
    """由 controller 管理的一个小网络：公网上的 relay（兼做出口），和场景自己加的节点。节点对都没有设置，除非场景自己设。"""

    def __init__(self, lab: Lab, work: Path) -> None:
        self.lab, self.work = lab, work
        self.web = lab.host("web")
        self.cert = start_blob_server(self.web, work)
        self.controller = Controller(lab.host("ctl"), work)
        self.controller.ctl("network", "create", "lab")
        self.relay = Node(lab.host("relay"), work, declare=("relay", "exit"))
        self.relay.join(self.controller)
        self.controller.ctl("relay", "enable", str(self.relay.id))
        self.controller.ctl("exit", "enable", str(self.relay.id))

    def node(self, host: Host, *, udp: bool = True, exit: bool = False, hold: int | None = None) -> Node:
        """加一个节点。`udp` 是假时它加入之前就用不了 UDP；`exit` 时启用它的出口；`hold` 见 `public_entry`。"""
        node = Node(host, self.work, declare=("exit",) if exit else ())
        if hold is not None:
            node.env = {HOLD_SWITCH: str(hold)}
        if not udp:
            host.block_udp()
        node.join(self.controller)
        if exit:
            self.controller.ctl("exit", "enable", str(node.id))
        return node

    def url(self, size: int) -> str:
        return f"http://{self.web.ip}/blob/{size}"

    def intact(self, node: Node, via: Node, size: int, timeout: float = 60) -> tuple[bool, str]:
        code, digest, err = fetch(node, self.url(size), via=via, timeout=timeout)
        return code == 0 and digest == blob_digest(size), f"curl 退出码 {code}，{err.strip()[-300:]}"

    def pair(self, a: Node, b: Node) -> dict:
        """`i3tctl pair show`：这一对的设置和状况，没有设置的也有。旧的 i3tctl 没有这个命令时是空的。"""
        done = self.controller.ctl("pair", "show", a.name, b.name, check=False)
        return json.loads(done.stdout) if done.returncode == 0 and done.stdout.strip() else {}

    def logs(self, *nodes: Node) -> None:
        print(*(tail(node.read_log(), 25) for node in nodes), sep="\n  ---\n")


def lanes_up(holder: Node, dialler: Node, timeout: float) -> bool:
    """`holder` 那里有 `dialler` 拨来的控制 lane 和停着的 lane。"""
    return waited(lambda: (c := lane_counts(holder, dialler))["control_up"] == 1 and c["parked_here"] >= 1, timeout)


def default_where_udp_is_missing(lab: Lab, work: Path, t: Checks) -> None:
    """没有节点对设置（默认的 udp_fallback）：没有 UDP 的节点和它拨得到的 relay 之间，TCP 流走 ice3proxy，头一条流不因为取 lane 耽误；
    有 UDP 的一对一分钟之后什么 lane 都没有（存下 udp_fallback 的也一样）；存下 off 的一对没有 UDP 也不拨。"""
    m = Managed(lab, work)
    relay, ctl = m.relay, m.controller
    started = time.monotonic()
    # s：用不了 UDP（像宿迁那台），兼做出口。n、q：公网上，UDP 好好的；q 和 relay 存下了 udp_fallback。
    # o：存下了 off，之后 UDP 也被挡住。
    s = m.node(lab.nat("home", NatKind.CONE).host("s"), udp=False, exit=True)
    n = m.node(lab.host("n"))
    q = m.node(lab.host("q"))
    o = m.node(lab.nat("office", NatKind.CONE).host("o"), exit=True)
    ctl.ctl("pair", "set", relay.name, q.name, "--lane", "udp_fallback")
    ctl.ctl("pair", "set", relay.name, o.name, "--lane", "off")
    o.host.block_udp()
    ctl.add_proxy(relay, s)
    ctl.add_proxy(s, relay)
    ctl.add_proxy(relay, o)
    ctl.add_proxy(n, relay)

    # 头一条流：这时 s 多半还没拨上来，流直接走共用的连接，不等 lane、不记失败。
    first = None
    deadline = time.monotonic() + 60
    while first is None and time.monotonic() < deadline:
        began = time.monotonic()
        if m.intact(relay, s, 100_000, timeout=20)[0]:
            first = time.monotonic() - began
        else:
            time.sleep(0.5)
    c = lane_counts(relay, s)
    t.check("经没有 UDP 的出口访问得到", first is not None)
    t.check("头一条流没有取 lane 失败、没有等补拨、没有记作改走", c["claims_failed"] == 0 and c["claim_wait_ms"] < 500 and c["fallbacks"] == 0,
            str(c))
    print(f"  头一条流用了 {first or 0:.2f} 秒", flush=True)

    # (a) s 这一段没有 UDP：连续 40 秒之后它拨 relay，之后的流走 ice3proxy。
    t.check("s 拨起来：relay 那里有了它的控制 lane 和停着的 lane", lanes_up(relay, s, 120), lane_line(relay, s))
    print(f"  加入之后 {time.monotonic() - started:.1f} 秒 relay 那里有了 s 停过来的 lane", flush=True)
    before, s_before = lane_counts(relay, s), lane_counts(s, relay)
    ok, detail = m.intact(relay, s, 2_000_000)
    after = lane_counts(relay, s)
    t.check("经 s 出去的下载走 ice3proxy（relay 取了 lane，lane 上搬了 2MB），完整", ok and after["claims"] > before["claims"] and
            after["carried_bytes"] - before["carried_bytes"] >= 2_000_000, f"{detail}；{before} → {after}")
    ok, detail = m.intact(s, relay, 2_000_000)
    s_after = lane_counts(s, relay)
    t.check("s 自己经 relay 出去的下载走它自己拨的 lane，完整", ok and s_after["carried_bytes"] - s_before["carried_bytes"] >= 2_000_000,
            f"{detail}；{s_before} → {s_after}")
    t.check("没有流改走、没有取 lane 失败", lane_counts(relay, s)["fallbacks"] == 0 and lane_counts(relay, s)["claims_failed"] == 0 and
            lane_counts(s, relay)["fallbacks"] == 0, f"{lane_line(relay, s)}；{lane_line(s, relay)}")

    def in_use_by_default() -> bool:
        view = m.pair(relay, s)
        opening = {side["node"]: side for side in (view.get("lane_effect") or {}).get("opening", [])}
        return view.get("stored") is False and view.get("lane") == "udp_fallback" and opening.get(relay.id, {}).get("lanes") is True
    t.check("节点对视图：没有设置、用默认的 udp_fallback，relay 现在在 ice3proxy 上开新的 TCP 流", waited(in_use_by_default, 30, 2),
            json.dumps(m.pair(relay, s))[:800])

    # (c) 存下 off 的一对：没有 UDP 也不拨，流照常走共用的连接。
    ok, detail = m.intact(relay, o, 1_000_000)
    t.check("存下 off、没有 UDP 的 o：下载完整", ok, detail)
    co, oc = lane_counts(relay, o), lane_counts(o, relay)
    t.check("存下 off 的一对什么 lane 都没有，也没有改走", no_lanes(relay, o) and no_lanes(o, relay) and co["claims"] + co["fallbacks"] +
            oc["dial_failures"] == 0, f"{co}；{oc}")

    # (b) 有 UDP 的一对：一分钟之后什么 lane 都没有，存下 udp_fallback 的也一样。
    ok, detail = m.intact(n, relay, 1_000_000)
    t.check("有 UDP 的 n 经 relay 出去：下载完整，不碰 lane", ok and lane_counts(n, relay)["carried_bytes"] == 0, detail)
    time.sleep(max(0.0, started + 60 - time.monotonic()))
    for node, how in ((n, "没有设置"), (q, "存下 udp_fallback")):
        t.check(f"有 UDP 的 {node.name}（{how}）一分钟之后：没有控制 lane、没有空闲的、没有在拨的，两边都是",
                no_lanes(node, relay) and no_lanes(relay, node) and lane_counts(node, relay)["dial_failures"] == 0,
                f"{lane_line(node, relay)}；{lane_line(relay, node)}")
        connections = established_to(node.host, relay.host.ip)
        t.check(f"{node.name} 到 relay 的节点端口没有一条 TCP 连接", connections == [], "\n".join(connections))
    if t.failures:
        m.logs(relay, s, q)


def default_follows_udp(lab: Lab, work: Path, t: Checks) -> None:
    """没有节点对设置：节点的 UDP 回来之后，过了保持时间它放开拨的 lane；UDP 又没了，一阵之后 lane 又回来。"""
    hold = 30
    m = Managed(lab, work)
    relay = m.relay
    s = m.node(lab.nat("home", NatKind.CONE).host("s"), udp=False, exit=True, hold=hold)
    m.controller.add_proxy(relay, s)
    wait_until(lambda: m.intact(relay, s, 10, timeout=5)[0], timeout=60, what="经 s 访问到数据服务")
    t.check("没有 UDP：s 拨起来", lanes_up(relay, s, 120), lane_line(relay, s))

    def on_lane() -> bool:
        claims = lane_counts(relay, s)["claims"]
        return m.intact(relay, s, 1_000_000)[0] and lane_counts(relay, s)["claims"] > claims

    t.check("没有 UDP：流走 ice3proxy", waited(on_lane, 30, 1), lane_line(relay, s))

    s.host.block_udp(False)
    opened = time.monotonic()

    def off_lane() -> bool:
        claims = lane_counts(relay, s)["claims"]
        return m.intact(relay, s, 1_000_000)[0] and lane_counts(relay, s)["claims"] == claims

    t.check("UDP 回来之后新的流回到共用的连接", waited(off_lane, 150, 2), lane_line(relay, s))
    print(f"  UDP 放开之后 {time.monotonic() - opened:.1f} 秒新的流回到共用的连接", flush=True)
    let_go = waited(lambda: no_lanes(relay, s) and no_lanes(s, relay), 150 + hold, 1)
    released = time.monotonic() - opened
    t.check(f"UDP 回来之后 s 放开拨的 lane，两边什么都不连着", let_go, f"{lane_line(relay, s)}；{lane_line(s, relay)}")
    t.check(f"放手不早于保持时间（{hold} 秒）", released >= hold - 2, f"UDP 放开之后 {released:.1f} 秒就放手了")
    print(f"  UDP 放开之后 {released:.1f} 秒 s 放手", flush=True)
    # 到 relay 的中转会话走着 TCP 时，最多 30 秒之后再试 UDP；那之后 s 到 relay 的节点端口上一条 TCP 都不该有。
    gone = waited(lambda: established_to(s.host, relay.host.ip) == [], 45, 1)
    t.check("放手之后 s 到 relay 的节点端口上没有 TCP 连接（lane 都关了，中转会话也回到 UDP）", gone,
            "\n".join(established_to(s.host, relay.host.ip)))

    s.host.block_udp()
    blocked = time.monotonic()
    t.check("UDP 又没了：一阵之后 s 又拨起来", lanes_up(relay, s, 150), lane_line(relay, s))
    print(f"  UDP 挡住之后 {time.monotonic() - blocked:.1f} 秒 s 又拨起来", flush=True)
    t.check("之后的流又走 ice3proxy", waited(on_lane, 30, 1), lane_line(relay, s))
    t.check("始终没有流改走、没有取 lane 失败、没有拨号失败", lane_counts(relay, s)["fallbacks"] + lane_counts(relay, s)["claims_failed"] +
            lane_counts(s, relay)["dial_failures"] == 0, f"{lane_line(relay, s)}；{lane_line(s, relay)}")
    if t.failures:
        m.logs(relay, s)


def route_to(controller: Controller, node: Node, peer: Node) -> dict:
    """`node` 最近上报的、到 `peer` 的路。"""
    routes = controller.ctl("node", "routes", str(node.id))["routes"]
    return next((route["route"] for route in routes if route["peer"]["id"] == peer.id), {})


def default_reaches_a_relay_without_udp(lab: Lab, work: Path, t: Checks) -> None:
    """没有节点对设置：普通节点 y（UDP 好好的，挂靠在离它近的两个 UDP 中继上）和没有 UDP 的中继 s（像宿迁那台）之间，
    闲了一分多钟之后两个方向的头一条 TCP 流都走 ice3proxy。y 到 s 的路是 y → 中继（UDP）→ s（TCP）：y 自己这一段有 UDP，
    要靠 controller 认定 s 用不了 UDP。"""
    m = Managed(lab, work)
    r, ctl = m.relay, m.controller
    started = time.monotonic()
    a = Node(lab.host("a"), work, declare=("relay",))
    s = Node(lab.host("s"), work, declare=("relay", "exit"))
    y = Node(lab.host("y"), work, declare=("exit",))
    # y 离 r、a 近，挂靠的是它们两个；s 离谁都是 100 ms 以上。s 的机房挡掉了 UDP。
    for one, other, rtt in ((s, a, 100), (s, r, 110), (s, y, 120), (a, r, 30), (a, y, 20), (r, y, 2)):
        harness.shape_between(one.host, other.host, lablib.Shape(rtt_ms=rtt))
    s.host.block_udp()
    for node in (a, s, y):
        node.join(ctl)
    for node in (a, s):
        ctl.ctl("relay", "enable", str(node.id))
    for node in (s, y):
        ctl.ctl("exit", "enable", str(node.id))
    ctl.add_proxy(y, s)
    ctl.add_proxy(s, y)

    flagged = waited(lambda: ctl.node(s).get("no_udp") is True, 240, 3)
    t.check("controller 认定 s 用不了 UDP，y 和中继都不算", flagged and not any(ctl.node(node).get("no_udp") for node in (y, r, a)),
            str({node.name: ctl.node(node).get("no_udp") for node in (s, y, r, a)}))
    up = lanes_up(s, y, 120)
    t.check("y 拨 s：s 那里有 y 的控制 lane 和停着的 lane", up, lane_line(s, y))
    print(f"  加入之后 {time.monotonic() - started:.1f} 秒 s 那里有了 y 停过来的 lane", flush=True)
    # 闲一分多钟：y 到 s 的临时会话回收，路回到经 UDP 中继的那一条。
    time.sleep(70)
    route = route_to(ctl, y, s)
    print(f"  闲了一分多钟之后 y 到 s 的路：{route}", flush=True)
    t.check("闲了一分多钟：y 到 s 的路的第一段是 UDP 中继，不是 s 自己", route.get("via") == "relay" and
            (route.get("relays") or [s.id])[0] != s.id, str(route))
    y_before, s_before = lane_counts(y, s), lane_counts(s, y)
    ok, detail = m.intact(y, s, 1_000_000)
    y_after = lane_counts(y, s)
    t.check("y 经 s 出去的头一条流走 ice3proxy（y 自己拨的 lane 上搬了 1MB），完整", ok and
            y_after["carried_bytes"] - y_before["carried_bytes"] >= 1_000_000, f"{detail}；{y_before} → {y_after}")
    ok, detail = m.intact(s, y, 1_000_000)
    s_after = lane_counts(s, y)
    t.check("s 经 y 出去的头一条流走 ice3proxy（s 取了 y 停过来的 lane），完整", ok and s_after["claims"] > s_before["claims"] and
            s_after["carried_bytes"] - s_before["carried_bytes"] >= 1_000_000, f"{detail}；{s_before} → {s_after}")
    t.check("没有流改走、没有取 lane 失败", y_after["fallbacks"] + s_after["fallbacks"] + s_after["claims_failed"] == 0,
            f"{lane_line(y, s)}；{lane_line(s, y)}")

    def both_open_on_lanes() -> bool:
        opening = {side["node"]: side for side in (m.pair(y, s).get("lane_effect") or {}).get("opening", [])}
        return all(opening.get(node.id, {}).get("lanes") is True for node in (y, s))
    t.check("节点对视图：两边开新的 TCP 流都走 ice3proxy（y 的 udp_path 因为 s 用不了 UDP 是 false）", waited(both_open_on_lanes, 30, 2),
            json.dumps(m.pair(y, s))[:800])
    if t.failures:
        m.logs(y, s)


def default_keeps_a_direct_path_to_a_node_without_udp(lab: Lab, work: Path, t: Checks) -> None:
    """没有节点对设置：没有 UDP 的中继 s 到各个中继都只有 TCP（controller 认定它用不了 UDP），和普通节点 y 之间的 UDP 却是通的。
    y 和 s 直连着的时候流走共用的连接（整条路都是 UDP），谁也不拨 ice3proxy；直连没了、路回到经中继，才用 ice3proxy。"""
    m = Managed(lab, work)
    r, ctl = m.relay, m.controller
    a = Node(lab.host("a"), work, declare=("relay",))
    s = Node(lab.host("s"), work, declare=("relay", "exit"))
    y = Node(lab.host("y"), work, declare=("exit",))
    for one, other, rtt in ((s, a, 100), (s, r, 110), (s, y, 120), (a, r, 30), (a, y, 20), (r, y, 2)):
        harness.shape_between(one.host, other.host, lablib.Shape(rtt_ms=rtt))
    s.host.block_udp(except_with=y.host)
    for node in (a, s, y):
        node.join(ctl)
    for node in (a, s):
        ctl.ctl("relay", "enable", str(node.id))
    for node in (s, y):
        ctl.ctl("exit", "enable", str(node.id))
    ctl.add_proxy(y, s)
    ctl.add_proxy(s, y)

    def direct(node: Node, peer: Node) -> bool:
        return route_to(ctl, node, peer).get("via") == "direct"

    flagged = waited(lambda: ctl.node(s).get("no_udp") is True, 240, 3)
    t.check("controller 认定 s 用不了 UDP", flagged, str({node.name: ctl.node(node).get("no_udp") for node in (s, y, r, a)}))
    ok, detail = m.intact(y, s, 1_000_000)
    t.check("y 经 s 出去的流完整", ok, detail)
    t.check("y 和 s 之间是直连", waited(lambda: direct(y, s) and direct(s, y), 60, 2), f"{route_to(ctl, y, s)}；{route_to(ctl, s, y)}")
    # 认定之后再等过按需拨号的观察时间：直连着就不该拨。
    time.sleep(40)
    before = lane_counts(y, s)
    ok, detail = m.intact(y, s, 1_000_000)
    after = lane_counts(y, s)
    t.check("直连着：y 经 s 出去的流走共用的连接，ice3proxy 上一个字节也没有，也没有拨", ok and after["carried_bytes"] == before["carried_bytes"] and
            after["control_up"] == 0 and lane_counts(s, y)["control_up"] == 0, f"{detail}；{lane_line(y, s)}；{lane_line(s, y)}")
    ok, detail = m.intact(s, y, 1_000_000)
    t.check("直连着：s 经 y 出去的流也完整，也不走 ice3proxy", ok and lane_counts(s, y)["carried_bytes"] == 0, f"{detail}；{lane_line(s, y)}")

    # 有 UDP 的路、什么都没连着的对端，节点不往上报：节点对视图里没有哪一边在 ice3proxy 上开流，报了的那一边路上也有 UDP。
    opening = (m.pair(y, s).get("lane_effect") or {}).get("opening", [])
    t.check("节点对视图：没有哪一边开新的 TCP 流走 ice3proxy",
            all(side.get("lanes") is False and side.get("udp_path") is True for side in opening), json.dumps(m.pair(y, s))[:800])

    # s 和 y 之间的 UDP 也没了：路回到经中继的那一条（y 这一段是 UDP，s 那一段是 TCP），这时才靠 controller 说的那一条去拨。
    s.host.block_udp()
    cut = time.monotonic()
    t.check("直连没了：y 拨 s，s 那里有 y 的控制 lane 和停着的 lane", lanes_up(s, y, 180), f"{lane_line(s, y)}；{route_to(ctl, y, s)}")
    print(f"  直连断开之后 {time.monotonic() - cut:.1f} 秒 s 那里有了 y 停过来的 lane", flush=True)
    before = lane_counts(y, s)
    ok, detail = m.intact(y, s, 1_000_000)
    after = lane_counts(y, s)
    t.check("直连没了：y 经 s 出去的流走 ice3proxy，完整", ok and after["carried_bytes"] - before["carried_bytes"] >= 1_000_000,
            f"{detail}；{before} → {after}")
    if t.failures:
        m.logs(y, s)


def managed_drain(lab: Lab, work: Path, t: Checks) -> None:
    """由 controller 管理：管理员打开一对节点的 ice3proxy，两边报上状况；出口排空后更新：lane 上跑着的流跑完，新的请求被拒绝，更新重启之后新的流又走 lane。"""
    import test_update as upd

    harness.BIN_DIR = upd.STAGE / "old"
    old, new = upd.versions()
    controller, release, web = upd.setup(lab, work, auto=False)
    entry = upd.SupervisedNode(lab.host("entry"), work, declare=("relay",))
    home = lab.nat("home", NatKind.CONE)
    exit_node = upd.SupervisedNode(home.host("exit"), work, declare=("exit",))
    for node in (entry, exit_node):
        node.join(controller)
    controller.ctl("relay", "enable", str(entry.id))
    controller.ctl("exit", "enable", str(exit_node.id))
    controller.add_proxy(entry, exit_node)
    view = controller.ctl("pair", "set", entry.name, exit_node.name, "--lane", "always")
    effect = view.get("lane_effect") or {}
    t.check("节点对视图说设置起作用，由出口拨入口", effect.get("effective") is True and effect.get("dialers") == [exit_node.id], json.dumps(view))
    url = f"http://{web.ip}/"
    t.check("经出口访问得到", upd.reaches(entry, url, exit_node, home.ip))
    ready = waited(lambda: lane_counts(entry, exit_node)["parked_here"] >= 1, 40)
    t.check("两个节点按 controller 下发的设置拨好了 lane", ready, lane_line(entry, exit_node))

    def reported() -> bool:
        pairs = controller.ctl("pair", "list")
        sides = {side["node"]: side["lanes"] for pair in pairs for side in pair.get("lanes", [])}
        return entry.id in sides and exit_node.id in sides and sides[entry.id]["control"]
    t.check("节点对视图里有两边报上来的 lane 状况", waited(reported, 30), json.dumps(controller.ctl("pair", "list"))[:600])
    detail = controller.ctl("node", "show", entry.name)
    t.check("节点详情里有它的 lane 状况", bool(detail.get("lanes")) and detail["lanes"]["peers"][0]["peer"] == exit_node.id,
            json.dumps(detail.get("lanes"))[:300])

    release.publish(new, upd.STAGE / "new" / "i3t")
    controller.ctl("update", "check")
    claims = lane_counts(entry, exit_node)["claims"]
    download, size = upd.slow_download(entry, url, exit_node, 20)
    time.sleep(2)
    t.check("排空之前开始的下载在 lane 上", lane_counts(entry, exit_node)["claims"] > claims and lane_counts(entry, exit_node)["in_use"] >= 1)
    controller.ctl("update", "node", str(exit_node.id), "--drain")
    draining = upd.waited(lambda: upd.state_of(controller, exit_node) == "draining", 30, "出口进入排空")
    t.check("出口准备好新程序之后进入排空", draining, str(upd.update_of(controller, exit_node)["status"]))
    gone = waited(lambda: lane_counts(entry, exit_node)["control_up"] == 0, 10)
    t.check("排空的出口告诉入口不要再在 lane 上开流", gone, lane_line(entry, exit_node))
    refused = entry.curl(url, via=exit_node, timeout=8)
    t.check("排空期间经出口的新请求被拒绝", refused.returncode != 0 and refused.stdout != home.ip, f"{refused.stdout!r} {refused.stderr.strip()}")
    t.check("出口记下了因为排空而拒绝", "节点正在排空" in exit_node.read_log(), tail(exit_node.read_log(), 6))
    ok, detail = upd.finished_intact(download, size)
    t.check("排空之前就在 lane 上跑的下载完整地结束", ok, detail)
    t.check("流跑完之后出口更新到新版本", upd.wait_version(controller, exit_node, new), controller.node(exit_node)["version"])
    claims = lane_counts(entry, exit_node)["claims"]
    back = waited(lambda: upd.reaches(entry, url, exit_node, home.ip, timeout=5) and lane_counts(entry, exit_node)["claims"] > claims, 40)
    t.check("更新重启之后新的流又走 lane", back, lane_line(entry, exit_node))
    print(f"  {lane_line(entry, exit_node)}\n  {lane_line(exit_node, entry)}", flush=True)
    controller.ctl("pair", "unset", entry.name, exit_node.name)
    off = waited(lambda: lane_counts(entry, exit_node)["control_up"] == 0 and lane_counts(entry, exit_node)["parked_here"] == 0, 30)
    t.check("去掉节点对设置回到默认（udp_fallback）：两边都有 UDP 的路，lane 关掉", off, lane_line(entry, exit_node))
    if t.failures:
        print(tail(entry.read_log(), 30), tail(exit_node.read_log(), 30), sep="\n  ---\n")
    for node in upd.RUNNING:
        node.stop()
    upd.RUNNING.clear()


SCENARIOS = {
    "nat_exit_dials_public_entry": nat_exit_dials_public_entry,
    "firewalled_entry_dials_public_exit": firewalled_entry_dials_public_exit,
    "udp_fallback": udp_fallback,
    "burst_drains_pool": burst_drains_pool,
    "tcp_blocked_falls_back": tcp_blocked_falls_back,
    "access_denied": access_denied,
    "exit_restart": exit_restart,
    "silent_stream_outlives_nat_mapping": silent_stream_outlives_nat_mapping,
    "default_where_udp_is_missing": default_where_udp_is_missing,
    "default_follows_udp": default_follows_udp,
    "default_reaches_a_relay_without_udp": default_reaches_a_relay_without_udp,
    "default_keeps_a_direct_path_to_a_node_without_udp": default_keeps_a_direct_path_to_a_node_without_udp,
    "managed_drain": managed_drain,
}


def main() -> int:
    names = sys.argv[1:] or list(SCENARIOS)
    unknown = [name for name in names if name not in SCENARIOS]
    if unknown:
        print(f"没有这些场景：{'、'.join(unknown)}；有 {'、'.join(SCENARIOS)}")
        return 2
    if "managed_drain" in names and not lablib.os.environ.get(lablib.IN_LAB_ENV):
        # 要在进入实验环境之前编：那里面的"root"不该去动 target 目录。
        import test_update

        test_update.build()
    lablib.enter()
    binary("i3t")
    t = Checks()
    for name in names:
        print(f"== {name}：{(SCENARIOS[name].__doc__ or '').strip()}", flush=True)
        began = time.monotonic()
        with tempfile.TemporaryDirectory(prefix=f"i3t-e2e-{name}-") as work, Lab() as lab:
            try:
                SCENARIOS[name](lab, Path(work), t)
            except Exception as err:  # noqa: BLE001 - 一个场景出错不影响后面的场景
                t.check(f"{name} 没有跑完", False, repr(err))
        print(f"   （{name} 用了 {time.monotonic() - began:.0f} 秒）", flush=True)
        # 每个场景用全新的命名空间；上一个场景的要等进程退干净才能重名。
        time.sleep(0.3)
    return t.summary()


if __name__ == "__main__":
    sys.exit(main())
