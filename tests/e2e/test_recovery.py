#!/usr/bin/env python3
"""断网之后节点之间的 TCP 多快恢复：lane 上的一条流，和经 TCP 中转的共用连接上的一条流，各遇到 20 秒和 40 秒完全不通。

    python3 tests/e2e/test_recovery.py [场景名...]

节点给 lane 和 TCP 中转的 socket 设了重传上限和放弃时限（`i3t_underlay::tcp_recovery`）。每个场景先这样跑，再用测试开关
`I3T_TEST_TCP_RECOVERY=off` 关掉这两项重启节点、照样跑一遍，两种都打印出来：路通之后多久又收到数据，流有没有断。
只检查开着的那一遍。内核没有重传上限这个选项（Linux 6.15 之前）时整个文件跳过。

流是经出口去连一个每 10 毫秒发一个小消息的服务；断开只加在那一段 TCP 经过的两台机器之间（netem 100% 丢包），
别的路不受影响。那一段平时的往返是 100 毫秒。

两种传输的期望不一样：

- lane 上的流就是那一条 TCP，断开 20 秒、40 秒都撑得住（放弃时限是 120 秒），路通之后最多一个重传上限加一点就恢复。
- 经 TCP 中转的共用连接：上面那一层在保活连续没有回答时（几秒）就换一条 TCP，路通之后等下一次重连（退避最长 4 秒）
  加一次握手。断开 40 秒时共用的连接 30 秒收不到任何东西，按它的空闲超时结束，上面的流以重置结束：这是共用连接的规矩
  （docs/internal/architecture.md §5.5），不是 TCP 的事，这里只检查它确实结束了、没有挂住。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import (  # noqa: E402
    TICKER_CLIENT, Checks, Node, binary, blackout_between, lane_counts, lane_line, lane_peer, rto_cap_supported, start_ticker, tail,
)
from lab import Host, Lab, NatKind, Shape, wait_until  # noqa: E402

# 断开那一段平时的网络条件。
BASE = Shape(rtt_ms=100)
BLACKOUTS = (20, 40)
# 断开之前、之后各收多久。
BEFORE, AFTER = 6, 20
# lane 上，路通之后最多多久又收到数据才算恢复得快：重传上限（2 秒）加上一秒的余量，再加一个往返。
QUICK = 3.2
# 经 TCP 中转时：等重连（断开 20 秒时退避到 4 秒）、连上、握手，再加一秒的余量。
QUICK_LEG = 5.5
TEST_SWITCH = "I3T_TEST_TCP_RECOVERY"


class Run:
    """一次断开的结果。"""

    def __init__(self, blackout: int, recovered_after: float | None, ended: str | None, on_path: bool) -> None:
        self.blackout, self.recovered_after, self.ended, self.on_path = blackout, recovered_after, ended, on_path

    @property
    def survived(self) -> bool:
        return self.ended is None and self.recovered_after is not None

    def __str__(self) -> str:
        if self.ended is not None:
            return f"流断了（{self.ended}）"
        if self.recovered_after is None:
            return "一直没有恢复"
        return f"路通之后 {self.recovered_after:.1f} 秒恢复"


def one_blackout(entry: Node, exit_node: Node, web: Host, a: Host, b: Host, blackout: int, on_path) -> Run:
    """在 `a` 和 `b` 之间断开 `blackout` 秒，同时经出口收着 ticker 的消息。`on_path()` 在断开之前问一次：流走的是不是要测的那一段。"""
    client = subprocess.Popen(entry.host.cmd(sys.executable, "-c", TICKER_CLIENT, str(entry.entry(exit_node).port), web.ip, "80",
                                             str(BEFORE + blackout + AFTER)), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(BEFORE)
    path_ok = on_path()
    blackout_between(a, b, BASE, True)
    began = time.monotonic()
    time.sleep(blackout)
    blackout_between(a, b, BASE, False)
    ended_at = time.monotonic()
    out, err = client.communicate(timeout=BEFORE + blackout + AFTER + 60)
    try:
        result = json.loads(out)
    except json.JSONDecodeError:
        return Run(blackout, None, f"客户端出错：{err.strip()[-200:]}", path_ok)
    # 跨过断开的那一段停顿：它的结尾就是恢复的时刻。
    gap = next((gap for gap in result["gaps"] if gap[0] <= began + 1 and gap[1] >= ended_at - 1), None)
    recovered = gap[1] - ended_at if gap else None
    if gap is None and result["ended"] is None and result["last"] >= ended_at:
        # 没有超过半秒的停顿跨过断开：数据一直在走（不该发生，断开是完全不通）。
        recovered = 0.0
    return Run(blackout, recovered, result["ended"], path_ok)


def report(t: Checks, what: str, on: list[Run], off: list[Run]) -> None:
    for run_on, run_off in zip(on, off):
        print(f"  {what}，断开 {run_on.blackout} 秒：设了上限 {run_on}；没设 {run_off}", flush=True)
    for run in on + off:
        t.check(f"{what}：断开 {run.blackout} 秒的那条流走的是要测的那一段", run.on_path)


def survives(t: Checks, what: str, run: Run, within: float) -> None:
    t.check(f"{what}：断开 {run.blackout} 秒之后流还在，路通之后 {within:g} 秒内又收到数据", run.survived and run.recovered_after <= within,
            str(run))


def with_switch(off: bool):
    """之后启动的节点带不带测试开关。"""
    if off:
        os.environ[TEST_SWITCH] = "off"
    else:
        os.environ.pop(TEST_SWITCH, None)


def lane(lab: Lab, work: Path, t: Checks) -> None:
    """lane（总是用）上的一条流：出口在家用 NAT 后面拨公网上的入口，断开加在入口和出口的 NAT 之间。"""
    web = lab.host("web")
    start_ticker(web)
    entry = Node(lab.host("entry"), work, 1, declare=("relay",))
    home = lab.nat("home", NatKind.CONE)
    exit_node = Node(home.host("exit"), work, 2, declare=("exit",))
    entry_netmap = entry.netmap(me={"relay": {}}, relays=[entry.as_relay()], relay_clients=[entry, exit_node],
                                peers=[lane_peer(exit_node, lane="always", endpoints=False, attached=[(entry, 5)], exit=True)],
                                proxies=[entry.proxy_to(exit_node)])
    exit_netmap = exit_node.netmap(me={"exit": {}}, relays=[entry.as_relay()], peers=[lane_peer(entry, lane="always")])
    blackout_between(entry.host, home, BASE, False)
    results = {}
    for off in (False, True):
        with_switch(off)
        entry.start_static(entry_netmap)
        exit_node.start_static(exit_netmap)
        with_switch(False)
        ready = wait_until_ok(lambda: (c := lane_counts(entry, exit_node))["control_up"] == 1 and c["parked_here"] >= 1, 40)
        if not t.check(f"lane 拨好（{'没设上限' if off else '设了上限'}）", ready, lane_line(entry, exit_node)):
            print(tail(entry.read_log()), tail(exit_node.read_log()), sep="\n  ---\n")
            return
        fallbacks = lane_counts(entry, exit_node)["fallbacks"]

        def on_lane() -> bool:
            counts = lane_counts(entry, exit_node)
            return counts["in_use"] >= 1 and counts["fallbacks"] == fallbacks

        results[off] = [one_blackout(entry, exit_node, web, entry.host, home, blackout, on_lane) for blackout in BLACKOUTS]
        entry.stop()
        exit_node.stop()
    report(t, "lane", results[False], results[True])
    for run in results[False]:
        survives(t, "lane", run, QUICK)


def relay_leg(lab: Lab, work: Path, t: Checks) -> None:
    """共用连接上的一条流，入口的 UDP 不通、经 TCP 中转连 relay；断开加在入口和 relay 之间。"""
    web = lab.host("web")
    start_ticker(web)
    relay = Node(lab.host("relay"), work, 9, declare=("relay",))
    entry_host = lab.host("entry")
    entry_host.block_udp()
    entry = Node(entry_host, work, 1)
    exit_node = Node(lab.host("exit"), work, 2, declare=("exit",))
    relays = [relay.as_relay()]
    relay_netmap = relay.netmap(me={"relay": {}}, relays=relays, relay_clients=[entry, exit_node, relay])
    entry_netmap = entry.netmap(relays=relays, peers=[exit_node.as_peer(exit=True, endpoints=False, attached=[(relay, 1)])],
                                proxies=[entry.proxy_to(exit_node)])
    exit_netmap = exit_node.netmap(me={"exit": {}}, relays=relays, peers=[entry.as_peer(endpoints=False, attached=[(relay, 1)])])
    blackout_between(entry_host, relay.host, BASE, False)
    results = {}
    for off in (False, True):
        with_switch(off)
        for node, netmap in ((relay, relay_netmap), (entry, entry_netmap), (exit_node, exit_netmap)):
            node.start_static(netmap)
        with_switch(False)
        leg = wait_until_ok(lambda: "到 relay 的中转会话建立（TCP）" in entry.read_log(), 40)
        if not t.check(f"入口经 TCP 连上 relay（{'没设上限' if off else '设了上限'}）", leg, tail(entry.read_log(), 10)):
            return
        def on_leg() -> bool:
            # 入口的 UDP 全被挡着，经 relay 是唯一的路，那一段只能是 TCP。
            return "到对端的路变了 peer=2 path=经 relay 9" in entry.read_log()

        results[off] = [one_blackout(entry, exit_node, web, entry_host, relay.host, blackout, on_leg) for blackout in BLACKOUTS]
        for node in (entry, exit_node, relay):
            node.stop()
        entry.log.rename(entry.log.with_name(f"entry-{'off' if off else 'on'}.log"))
    what = "经 TCP 中转的共用连接"
    report(t, what, results[False], results[True])
    short, long = results[False]
    survives(t, what, short, QUICK_LEG)
    t.check(f"{what}：断开 {long.blackout} 秒超过了连接的空闲超时（30 秒），流以重置结束，没有挂住", long.ended is not None, str(long))


def wait_until_ok(condition, timeout: float) -> bool:
    try:
        wait_until(condition, timeout=timeout, interval=0.5)
        return True
    except TimeoutError:
        return False


SCENARIOS = {"lane": lane, "relay_leg": relay_leg}


def main() -> int:
    names = sys.argv[1:] or list(SCENARIOS)
    unknown = [name for name in names if name not in SCENARIOS]
    if unknown:
        print(f"没有这些场景：{'、'.join(unknown)}；有 {'、'.join(SCENARIOS)}")
        return 2
    if not rto_cap_supported():
        print("跳过：这个内核不能给 TCP 的重传定时器设上限（Linux 6.15 起才有 TCP_RTO_MAX_MS），节点不设这两项，没有可比的")
        return 0
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
        time.sleep(0.3)
    return t.summary()


if __name__ == "__main__":
    sys.exit(main())
