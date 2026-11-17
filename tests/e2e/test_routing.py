#!/usr/bin/env python3
"""选路的端到端测试：延迟优先和多级中转、局域网发现、relay 下线通知。节点用静态的 netmap 运行，不需要 controller。

    python3 tests/e2e/test_routing.py [场景名...]
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import Checks, Node, current_route, flow_routes, route_changes, shape_between, start_whoami, tail  # noqa: E402
from lab import Lab, NatKind, Shape, wait_until  # noqa: E402


def reaches(node: Node, url: str, via: Node, expect: str, timeout: float = 30) -> bool:
    """反复尝试，直到经 `via` 访问 `url` 得到 `expect`，或者超时。建连和找路都要一点时间。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if node.curl(url, via=via, timeout=6).stdout == expect:
            return True
        time.sleep(0.5)
    return False


def start_download(node: Node, url: str, via: Node, megabytes: float, rate: str):
    """在后台开始一次限速下载，返回进程和应有的字节数。限速是为了让它持续一两分钟，中间好换路。"""
    size = int(megabytes * 1_000_000)
    cmd = node.host.cmd("curl", "-sS", "--max-time", "400", "--limit-rate", rate, "-o", "/dev/null", "-w", "%{size_download}",
                        "-x", f"socks5h://{via.virtual_ip}@127.0.0.1:1080", f"{url}bytes/{size}")
    return subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True), size


def finished_intact(download, size: int, timeout: float = 400) -> tuple[bool, str]:
    out, err = download.communicate(timeout=timeout)
    return out == str(size), f"下载了 {out!r}，应该是 {size}；{err.strip()}"


def waited(condition, timeout: float, what: str) -> bool:
    try:
        wait_until(condition, timeout=timeout, interval=0.5, what=what)
        return True
    except TimeoutError:
        return False


def lowest_cost_two_relays(lab: Lab, work: Path, t: Checks) -> None:
    """延迟优先：直连又慢又丢包时走两个 relay；条件对调之后换回直连。正在进行的下载不断。"""
    web = lab.host("web")
    start_whoami(web)
    r1 = Node(lab.host("r1"), work, 8, declare=("relay",))
    r2 = Node(lab.host("r2"), work, 9, declare=("relay",))
    a = Node(lab.host("a"), work, 1)
    d = Node(lab.host("d"), work, 2, declare=("exit",))

    # a—d 直连往返 120ms、每个方向丢 2%；a—r1、r2—d 各 10ms，骨干 r1—r2 是 20ms；a 自己到 r2 很慢（150ms）。
    # 所以 a 到 d：直连约 127，只经 r2 约 161，经 r1、r2 两级约 42。
    shape_between(a.host, d.host, Shape(rtt_ms=120, loss_pct=2))
    shape_between(a.host, r1.host, Shape(rtt_ms=10))
    shape_between(r1.host, r2.host, Shape(rtt_ms=20))
    shape_between(r2.host, d.host, Shape(rtt_ms=10))
    shape_between(a.host, r2.host, Shape(rtt_ms=150))

    relays = [r1.as_relay(), r2.as_relay()]
    lowest_cost = {"routing": "lowest_cost"}
    for relay in (r1, r2):
        relay.start_static(relay.netmap(me={"relay": {}}, relays=relays, relay_clients=[a, d, r1, r2]))
    a.start_static(a.netmap(peers=[d.as_peer(attached=[(r2, 10)], exit=True)], relays=relays, me=lowest_cost))
    d.start_static(d.netmap(peers=[a.as_peer(attached=[(r1, 10)])], relays=relays, me={"exit": {}, **lowest_cost}))

    url = f"http://{web.ip}/"
    t.check("a 经 d 出去访问得到", reaches(a, url, d, d.host.ip), tail(a.read_log(), 8))
    two_relays = f"经 relay {r1.id} → {r2.id}"
    converged = waited(lambda: current_route(a, d) == two_relays, 90, "a 到 d 改走两个 relay")
    t.check("直连又慢又丢包时，延迟优先选了经两个 relay 的路径", converged, f"现在是 {current_route(a, d)}；经过 {route_changes(a, d)}")
    reasons = [reason for route, reason in route_changes(a, d) if route == two_relays]
    t.check("换路的原因里写着代价", bool(reasons) and "延迟优先" in reasons[-1] and "ms" in reasons[-1], str(reasons))
    fast = a.host.run("curl", "-sS", "--max-time", "10", "-o", "/dev/null", "-w", "%{time_total}", "-x",
                      f"socks5h://{d.virtual_ip}@127.0.0.1:1080", url, check=False)
    t.check("经两个 relay 的请求确实比直连的往返快", fast.returncode == 0 and float(fast.stdout or 9) < 0.110, f"用了 {fast.stdout!r} 秒")

    download, size = start_download(a, url, d, 30, rate="200k")
    time.sleep(3)
    # 对调：直连变好（不加任何限制），骨干变差（往返 200ms）。
    shape_between(a.host, d.host, None)
    shape_between(r1.host, r2.host, Shape(rtt_ms=200))
    swapped = time.monotonic()
    back = waited(lambda: current_route(a, d).startswith("直连 "), 120, "a 到 d 换回直连")
    took = time.monotonic() - swapped
    t.check("条件对调之后换回了直连", back, f"现在是 {current_route(a, d)}；经过 {route_changes(a, d)[-3:]}")
    # 估计值要先跟上新的条件，然后连续 3 个评估窗口（15 秒）都更优才换；不会一看到变化就跳。
    t.check("换回去之前经过了迟滞", took >= 12, f"只用了 {took:.1f} 秒")
    last_reason = route_changes(a, d)[-1][1] if route_changes(a, d) else ""
    t.check("这一次的原因是连续几个评估窗口代价更低", "连续 3 个评估窗口" in last_reason, last_reason)
    ok, detail = finished_intact(download, size)
    t.check("两次换路期间下载没有断，字节数对得上", ok, detail)
    print("  a 到 d 的每一次换路：", *[f"    {route}：{reason}" for route, reason in route_changes(a, d)], sep="\n")
    if t.failures:
        print(tail(a.read_log(), 30), tail(r1.read_log(), 15), sep="\n  ---\n")


def same_lan(lab: Lab, work: Path, t: Checks) -> None:
    """两个节点在同一个 NAT 后面，谁也不知道谁的地址，也没有 relay 和 controller：靠局域网信标找到对方，直连。"""
    web = lab.host("web")
    start_whoami(web)
    nat = lab.nat("nat-x", NatKind.SYMMETRIC)
    a = Node(nat.host("a"), work, 1)
    b = Node(nat.host("b"), work, 2, declare=("exit",))
    a.start_static(a.netmap(peers=[b.as_peer(endpoints=False, exit=True)]))
    b.start_static(b.netmap(peers=[a.as_peer(endpoints=False)], me={"exit": {}}))

    url = f"http://{web.ip}/"
    t.check("同一个局域网里的两个节点互相找到了：a 经 b 出去，目标看到的是 NAT 的公网地址", reaches(a, url, b, nat.ip, timeout=40),
            tail(a.read_log(), 10))
    lan_direct = f"直连 {b.host.ip}:20330"
    t.check("a 到 b 走的是局域网里的直连地址", current_route(a, b) == lan_direct, f"现在是 {current_route(a, b)}")
    t.check("a 的日志里记着是从信标里看到 b 的", "在局域网里看到了对端" in a.read_log())
    big = a.curl(f"{url}bytes/5000000", via=b, extra=("-o", "/dev/null", "-w", "%{size_download}"))
    t.check("局域网直连上下载 5MB", big.stdout == "5000000", f"下载了 {big.stdout!r} {big.stderr.strip()}")
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log(), 30), sep="\n  ---\n")


def graceful_relay_shutdown(lab: Lab, work: Path, t: Checks) -> None:
    """正在用的 relay 正常退出：它先通知挂靠的节点，节点立刻换到另一个 relay，下载不断。

    r1 收到的是 SIGTERM，走的是节点平常的退出流程。通知由 relay 角色在进程退出、它被丢弃时发出；
    节点的装配调用了 `Relay::announce_shutdown` 之后，通知会在退出流程的一开始就发出，这个场景照样成立。
    """
    web = lab.host("web")
    start_whoami(web)
    r1 = Node(lab.host("r1"), work, 8, declare=("relay",))
    r2 = Node(lab.host("r2"), work, 9, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work, 1)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, 2, declare=("exit",))
    relays = [r1.as_relay(), r2.as_relay()]
    for relay in (r1, r2):
        relay.start_static(relay.netmap(me={"relay": {}}, relays=relays, relay_clients=[a, b, r1, r2]))
    # 两个节点都在对称型 NAT 后面，只能经 relay。默认模式：用对端挂靠表里排第一的 r1。
    attached = [(r1, 5), (r2, 6)]
    a.start_static(a.netmap(peers=[b.as_peer(endpoints=False, attached=attached, exit=True)], relays=relays))
    b.start_static(b.netmap(peers=[a.as_peer(endpoints=False, attached=attached)], relays=relays, me={"exit": {}}))

    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip
    t.check("经 relay 到达", reaches(a, url, b, expect, timeout=40), tail(a.read_log(), 8))
    t.check("开始时经 r1", current_route(a, b) == f"经 relay {r1.id}", current_route(a, b))

    download, size = start_download(a, url, b, 12, rate="1M")
    time.sleep(4)
    r1.stop()
    stopped = time.monotonic()
    moved = waited(lambda: current_route(a, b) == f"经 relay {r2.id}", 20, "a 到 b 改经 r2")
    took = time.monotonic() - stopped
    t.check("r1 退出之后 a 改经 r2", moved, f"现在是 {current_route(a, b)}")
    reason = route_changes(a, b)[-1][1] if route_changes(a, b) else ""
    t.check("换路是因为收到了 r1 的下线通知，不是等保活超时才发现", "relay 通知即将下线" in reason and took < 3, f"{took:.1f} 秒；原因：{reason}")
    t.check("b 也收到了通知", "relay 通知即将下线" in b.read_log(), tail(b.read_log(), 8))
    t.check("通知是 r1 退出时自己发的", "relay 即将下线" in r1.read_log(), tail(r1.read_log(), 8))
    ok, detail = finished_intact(download, size)
    t.check("下载途中正常停掉正在用的 relay，下载没有断", ok, detail)
    summaries = flow_routes(a)
    t.check("这条流结束时的摘要写出开始时经 r1、结束时经 r2、中间换过路",
            (f"经 #{r1.id}", f"经 #{r2.id}", "true") in summaries, str(summaries[-3:]))
    exit_side = flow_routes(b)
    t.check("出口那一侧的摘要也写着它到 a 的路换过", any(changed == "true" for _, _, changed in exit_side), str(exit_side[-3:]))
    if t.failures:
        print(tail(a.read_log(), 40), tail(r1.read_log(), 20), sep="\n  ---\n")


SCENARIOS = {
    "lowest_cost_two_relays": lowest_cost_two_relays,
    "same_lan": same_lan,
    "graceful_relay_shutdown": graceful_relay_shutdown,
}


def main() -> int:
    lablib.enter()
    names = sys.argv[1:] or list(SCENARIOS)
    t = Checks()
    for name in names:
        print(f"== {name}：{(SCENARIOS[name].__doc__ or '').strip().splitlines()[0]}", flush=True)
        with tempfile.TemporaryDirectory(prefix=f"i3t-e2e-{name}-") as work, Lab() as lab:
            try:
                SCENARIOS[name](lab, Path(work), t)
            except Exception as err:  # noqa: BLE001 - 一个场景出错不影响后面的场景
                t.check(f"{name} 没有跑完", False, repr(err))
        # 每个场景用全新的命名空间；上一个场景的要等进程退干净才能重名。
        time.sleep(0.3)
    return t.summary()


if __name__ == "__main__":
    sys.exit(main())
