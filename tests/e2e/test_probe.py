#!/usr/bin/env python3
"""主动测试的端到端测试：controller 让两个节点量直连能力、直连和经 relay 的延迟、丢包和吞吐，两种选路模式各会选哪条路，
以及全网矩阵和取消。

    python3 tests/e2e/test_probe.py [场景名...]
"""

from __future__ import annotations

import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import (  # noqa: E402
    Checks, Controller, Node, active_test, add_node_config, binary, ctl_wait, forward_route, shape_between, tail,
)
from lab import Lab, NatKind, Shape, wait_until  # noqa: E402


def attached(controller: Controller, *nodes: Node, timeout: float = 30) -> None:
    wait_until(lambda: all(controller.ctl("node", "show", str(node.id))["attached"] for node in nodes), timeout=timeout,
               what="节点挂靠上 relay")


def between(value, low: float, high: float) -> bool:
    return value is not None and low <= value <= high


def ms(us) -> float | None:
    return None if us is None else us / 1000


def human(controller: Controller, *args: str) -> str:
    done = controller.host.run(binary("i3tctl"), "--token", controller.token, *args, check=False, timeout=300)
    return done.stdout + done.stderr


def relay_path(view: dict, relays: list[int]) -> dict:
    return next((path for path in view["relay_quality"] if path.get("route") == {"via": "relay", "relays": relays}), {})


def punch_and_udp_blocked(lab: Lab, work: Path, t: Checks) -> None:
    """两个普通 NAT 后面的节点打洞直连，报出类型和用时；UDP 被挡住的节点直连不成，说得出是它的 UDP 不通。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.CONE).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.CONE).host("b"), work)
    c_host = lab.host("c")
    c_host.block_udp()
    c = Node(c_host, work)
    for node in (r, a, b, c):
        node.join(controller)
    controller.ctl("relay", "enable", str(r.id), "--udp", r.endpoint, "--tcp", r.endpoint)
    attached(controller, a, b, c, timeout=40)

    view = active_test(controller, a, b, "--only", "direct")
    direct = view.get("direct") or {}
    t.check("测试做完了", view["state"] == "done", str(view))
    t.check("直连能力：打洞打通了", direct.get("ok") is True and direct.get("kind") == "punched", str(direct))
    t.check("报了建立直连用了多久（几秒之内）", between(direct.get("elapsed_ms"), 0, 8000), str(direct))
    t.check("两端各自看到的都在", [side["node"] for side in direct.get("sides", [])] == [a.id, b.id], str(direct.get("sides")))
    t.check("之后 a 到 b 走直连", wait_route(controller, a, b, "direct"), str(forward_route(controller, a, b)))

    view = active_test(controller, a, c, "--only", "direct")
    direct = view.get("direct") or {}
    problem = direct.get("problem") or {}
    t.check("UDP 不通的节点直连不成", direct.get("state") == "done" and direct.get("ok") is False, str(direct))
    t.check("原因是 c 的 UDP 不通", problem.get("code") == "udp_blocked" and problem.get("node") == c.id, str(problem))
    c_side = next((side for side in direct.get("sides", []) if side["node"] == c.id), {})
    t.check("c 自己报 UDP 不通", c_side.get("udp") is False and c_side.get("failure") == "udp_blocked", str(c_side))
    shown = human(controller, "tests", "show", str(view["id"]))
    t.check("i3tctl 打印出原因", "udp_blocked（c）" in shown, shown)
    if t.failures:
        print(tail(a.read_log(), 30), tail(c.read_log(), 30), tail(controller.log.read_text(errors="replace"), 20), sep="\n  ---\n")


def wait_route(controller: Controller, a: Node, b: Node, via: str, timeout: float = 30, relays: list[int] | None = None) -> bool:
    def matches() -> bool:
        route = forward_route(controller, a, b)
        return route.get("via") == via and (relays is None or route.get("relays") == relays)
    try:
        wait_until(matches, timeout=timeout, interval=1, what=f"{a.name} 到 {b.name} 走 {via}")
        return True
    except TimeoutError:
        return False


def quality_and_throughput(lab: Lab, work: Path, t: Checks) -> None:
    """直连和经 relay 的延迟、丢包和测出来的网络条件对得上；限速的直连上吞吐接近限速、在时长上限处停，没限速的 relay 路径在字节上限处停。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a, b = Node(lab.host("a"), work), Node(lab.host("b"), work)
    # 网络条件在节点启动之前就加上：延迟的估计是平滑的，中途改的话要很多个样本才跟得上。
    shape_between(a.host, b.host, Shape(rtt_ms=60, loss_pct=3))
    shape_between(a.host, r.host, Shape(rtt_ms=30))
    shape_between(r.host, b.host, Shape(rtt_ms=20))
    for node in (r, a, b):
        node.join(controller)
    controller.ctl("relay", "enable", str(r.id), "--udp", r.endpoint, "--tcp", r.endpoint)
    attached(controller, a, b)

    view = active_test(controller, a, b, "--only", "direct,direct-quality,relay-quality")
    direct = (view.get("direct_quality") or {}).get("latency") or {}
    t.check("直连：100 个探测", direct.get("probes") == 100, str(direct))
    t.check("直连的平均延迟接近 60ms", between(ms(direct.get("avg_us")), 55, 85), str(direct))
    t.check("最小 ≤ 平均 ≤ p95", (direct.get("min_us") or 0) <= (direct.get("avg_us") or 0) <= (direct.get("p95_us") or 0), str(direct))
    # 一百个探测，期望丢 6 个左右：范围放宽到不会因为运气误报。
    t.check("直连的往返丢包接近 6%（每个方向 3%）", between(direct.get("loss_permille"), 5, 140), str(direct))
    t.check("丢包分得出去程和回程", between(direct.get("loss_up_permille"), 0, 100) and between(direct.get("loss_down_permille"), 0, 100),
            str(direct))
    relayed = relay_path(view, [r.id])
    latency = relayed.get("latency") or {}
    t.check("经 relay 的那条是两种模式默认都会选的", sorted(relayed.get("picked_by", [])) == ["direct_first", "lowest_cost"], str(relayed))
    t.check("经 relay 的平均延迟接近 30 + 20ms", between(ms(latency.get("avg_us")), 45, 80), str(latency))
    t.check("经 relay 几乎不丢", between(latency.get("loss_permille"), 0, 30), str(latency))
    hops = relayed.get("hops", [])
    t.check("每一段：a 到 relay 是 a 测的，约 30ms", len(hops) == 2 and hops[0]["source"] == "measured" and between(ms(hops[0]["rtt_us"]), 20, 45),
            str(hops))
    t.check("每一段：relay 到 b 是 b 报的，约 20ms", len(hops) == 2 and hops[1]["source"] == "reported" and between(ms(hops[1]["rtt_us"]), 12, 35),
            str(hops))

    # 直连改成限速 20Mbit/s、不丢包，再量吞吐：直连上在时长上限处停，接近限速；relay 路径没有限速，在 20MB 处停。
    shape_between(a.host, b.host, Shape(rtt_ms=20, rate_mbit=20))
    time.sleep(10)
    view = active_test(controller, a, b, "--only", "direct-quality,relay-quality", "--throughput", "--via", r.name)
    throughput = (view.get("direct_quality") or {}).get("throughput") or {}
    up, down = throughput.get("up") or {}, throughput.get("down") or {}
    mbit = lambda transfer: (transfer.get("bits_per_second") or 0) / 1e6  # noqa: E731
    t.check("直连上量了吞吐", throughput.get("state") == "done", str(throughput))
    t.check("上行接近 20Mbit/s", between(mbit(up), 10, 23), str(up))
    t.check("下行接近 20Mbit/s", between(mbit(down), 10, 23), str(down))
    t.check("两个方向都在 5 秒的时长上限处停", up.get("stopped_by") == "time" and down.get("stopped_by") == "time"
            and between(up.get("ms"), 3500, 5600) and between(down.get("ms"), 3500, 5600), f"{up} {down}")
    t.check("结果写明了传输、拥塞控制和上限", throughput.get("transport") == "quic" and throughput.get("congestion") in ("bbr", "cubic")
            and throughput.get("limit_bytes") == 20_000_000 and throughput.get("limit_ms") == 5000, str(throughput))
    t.check("测量期间连接固定在直连上，和别的流量共用连接", throughput.get("pinned") is True and throughput.get("shared_connection") is True,
            str(throughput))
    relayed = relay_path(view, [r.id]).get("throughput") or {}
    t.check("指定的 relay 路径上两个方向都在 20MB 处停", all((relayed.get(way) or {}).get("bytes") == 20_000_000
                                                    and (relayed.get(way) or {}).get("stopped_by") == "bytes" for way in ("up", "down")),
            str(relayed))
    t.check("relay 路径上也固定住了", relayed.get("pinned") is True, str(relayed))
    t.check("测完之后 a 到 b 回到直连", wait_route(controller, a, b, "direct", timeout=15), str(forward_route(controller, a, b)))
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log(), 20), sep="\n  ---\n")


def two_relays_and_route_decision(lab: Lab, work: Path, t: Checks) -> None:
    """指定经两个 relay 的路径：每一段的分解，吞吐期间连接固定在它上面，测完回到原来的路；选路的结论和节点之后真的走的一致。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    controller.ctl("network", "set", "--max-relay-hops", "2")
    r1, r2 = Node(lab.host("r1"), work, declare=("relay",)), Node(lab.host("r2"), work, declare=("relay",))
    a, b = Node(lab.host("a"), work), Node(lab.host("b"), work)
    for node in (r1, r2, a, b):
        node.join(controller)
    for relay in (r1, r2):
        controller.ctl("relay", "enable", str(relay.id), "--udp", relay.endpoint, "--tcp", relay.endpoint)
    controller.ctl("node", "set", a.name, "--pin", r1.name)
    controller.ctl("node", "set", b.name, "--pin", r2.name)
    attached(controller, a, b)
    def backbone_measured() -> bool:
        relays = controller.ctl("backbone")["relays"]
        return len(relays) == 2 and all(any(link.get("rtt_ms") is not None for link in relay["links"]) for relay in relays)
    wait_until(backbone_measured, timeout=40, interval=1, what="两个 relay 之间的骨干有了测量值")

    view = active_test(controller, a, b, "--only", "relay-quality,route", "--throughput", "--via", f"{r1.name},{r2.name}")
    path = relay_path(view, [r1.id, r2.id])
    hops = path.get("hops", [])
    t.check("经两个 relay 的路径量出了延迟", (path.get("latency") or {}).get("answered", 0) > 90, str(path))
    t.check("每一段：a→r1 实测、r1→r2 骨干、r2→b 对端报的",
            [(hop["from"], hop["to"], hop["source"]) for hop in hops] == [(a.id, r1.id, "measured"), (r1.id, r2.id, "backbone"), (r2.id, b.id, "reported")],
            str(hops))
    t.check("每一段都有延迟", all(hop.get("rtt_us") for hop in hops), str(hops))
    throughput = path.get("throughput") or {}
    t.check("吞吐期间连接固定在这条路径上", throughput.get("pinned") is True and (throughput.get("up") or {}).get("bytes", 0) > 0, str(throughput))
    switched = [line for line in a.read_log().splitlines() if "主动测试暂时把路固定在经 relay" in line]
    t.check("a 的日志里有换到这条路径的记录", bool(switched), tail(a.read_log(), 10))
    t.check("测完之后 a 到 b 回到直连", wait_route(controller, a, b, "direct", timeout=15), str(forward_route(controller, a, b)))
    decision = ((view.get("route") or {}).get("forward")) or {}
    t.check("默认模式的选择是直连，和现在走的一样", (decision.get("direct_first") or {}).get("route", {}).get("via") == "direct"
            and forward_route(controller, a, b).get("via") == "direct", str(decision))

    # 直连变差，a 到 r2 也变差：延迟优先模式应该选经 r1、r2 的那条。测试说的和节点之后真的走的要一致。
    shape_between(a.host, b.host, Shape(rtt_ms=160))
    shape_between(a.host, r2.host, Shape(rtt_ms=120))
    controller.ctl("node", "set", a.name, "--routing", "lowest-cost")
    time.sleep(15)
    view = active_test(controller, a, b, "--only", "route")
    decision = ((view.get("route") or {}).get("forward")) or {}
    chosen = (decision.get("lowest_cost") or {}).get("route") or {}
    t.check("延迟优先模式会选经 r1、r2 的路径", chosen == {"via": "relay", "relays": [r1.id, r2.id]}, str(decision))
    t.check("节点之后真的走它", wait_route(controller, a, b, "relay", timeout=60, relays=chosen.get("relays")),
            str(forward_route(controller, a, b)))
    t.check("默认模式的选择仍然是直连", (decision.get("direct_first") or {}).get("route", {}).get("via") == "direct", str(decision))
    shown = human(controller, "tests", "show", str(view["id"]))
    t.check("i3tctl 打印出两种模式的选择", "DIRECT-FIRST" in shown and "lowest_cost" in shown, shown)
    if t.failures:
        print(tail(a.read_log(), 40), tail(b.read_log(), 20), sep="\n  ---\n")


def matrix_five_nodes(lab: Lab, work: Path, t: Checks) -> None:
    """五个节点的全网矩阵：每一对都有结果；打不了洞的一对说得出原因，延迟改测经 relay 的。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a = Node(lab.host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.CONE).host("b"), work)
    c = Node(lab.nat("nat-c", NatKind.CONE).host("c"), work)
    d = Node(lab.nat("nat-d", NatKind.SYMMETRIC).host("d"), work)
    nodes = (r, a, b, c, d)
    for node in nodes:
        node.join(controller)
    controller.ctl("relay", "enable", str(r.id), "--udp", r.endpoint, "--tcp", r.endpoint)
    attached(controller, a, b, c, d, timeout=40)

    run = ctl_wait(controller, "tests", "matrix", timeout=400)
    cells = {(cell["a"], cell["b"]): cell for cell in run["cells"]}
    t.check("矩阵做完了，十对都有结果", run["state"] == "done" and run["total"] == 10 and run["finished"] == 10, str(run))
    t.check("同时最多 4 个", run["concurrency"] == 4)
    t.check("每一对都量到了延迟", all((cell.get("latency") or {}).get("answered", 0) > 0 for cell in cells.values()),
            str([(key, cell["state"], cell.get("problem")) for key, cell in cells.items()]))
    pair = lambda x, y: cells.get((min(x.id, y.id), max(x.id, y.id))) or {}  # noqa: E731
    t.check("两个普通 NAT 后面的节点打洞直连", pair(b, c).get("direct_ok") is True and pair(b, c).get("direct_kind") == "punched", str(pair(b, c)))
    blocked = pair(c, d)
    t.check("对称型 NAT 后面的节点和普通 NAT 后面的打不了洞", blocked.get("direct_ok") is False, str(blocked))
    t.check("这一对的延迟测在 relay 上", (blocked.get("route") or {}).get("via") == "relay", str(blocked))
    t.check("原因说得出是谁", (blocked.get("problem") or {}).get("code") in ("mapping_varies", "timeout"), str(blocked))
    tests = controller.ctl("tests", "list", "--limit", "50")
    t.check("每一对是一个测试，历史里看得到", len([view for view in tests if view.get("matrix") == run["id"]]) == 10, str(len(tests)))
    t.check("矩阵不量吞吐", all(not view["request"]["throughput"] for view in tests))
    shown = human(controller, "tests", "matrix", "--show")
    t.check("i3tctl 打印出矩阵", shown.startswith(f"全网矩阵 #{run['id']}  done"), shown)
    if t.failures:
        print(tail(d.read_log(), 30), tail(controller.log.read_text(errors="replace"), 30), sep="\n  ---\n")


def switched_off_and_cancel(lab: Lab, work: Path, t: Checks) -> None:
    """在本机关掉吞吐测试的节点不量吞吐（延迟照样量）；取消一个进行中的测试，固定的路随之解除。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a, b, c = Node(lab.host("a"), work), Node(lab.host("b"), work), Node(lab.host("c"), work)
    switched = b.host.run(binary("i3t"), "--state-dir", str(b.state), "set", "throughput-tests", "off", check=False)
    t.check("i3t set throughput-tests off", switched.returncode == 0 and "throughput-tests" in switched.stdout, switched.stdout + switched.stderr)
    for node in (r, a, b, c):
        node.join(controller)
    controller.ctl("relay", "enable", str(r.id), "--udp", r.endpoint, "--tcp", r.endpoint)
    attached(controller, a, b, c)

    view = active_test(controller, a, b, "--only", "direct-quality", "--throughput")
    quality = view.get("direct_quality") or {}
    throughput = quality.get("throughput") or {}
    problem = throughput.get("problem") or {}
    t.check("延迟照样量了", (quality.get("latency") or {}).get("answered", 0) > 90, str(quality))
    t.check("吞吐没有量，原因是 b 关掉了吞吐测试", throughput.get("state") == "skipped" and problem.get("code") == "throughput_off"
            and problem.get("node") == b.id, str(throughput))
    t.check("b 没有收到过测量流", "测量流结束" not in b.read_log())

    started = controller.ctl("test", a.name, c.name, "--only", "relay-quality", "--throughput", "--via", r.name, "--no-wait")
    wait_until(lambda: "主动测试暂时把路固定在经 relay" in a.read_log(), timeout=30, interval=0.5, what="a 开始在 relay 路径上量吞吐")
    cancelled = controller.ctl("tests", "cancel", str(started["id"]))
    t.check("取消之后状态是 cancelled", cancelled["state"] == "cancelled", str(cancelled))
    t.check("取消之后 a 到 c 回到直连", wait_route(controller, a, c, "direct", timeout=15), str(forward_route(controller, a, c)))
    t.check("a 的日志里有解除固定", "主动测试结束，回到选路规则选的" in a.read_log() or "主动测试解除固定的路" in a.read_log(), tail(a.read_log(), 15))
    again = controller.ctl("tests", "cancel", str(started["id"]), check=False)
    t.check("已经结束的测试不能再取消", again.returncode != 0 and "busy" in again.stderr, again.stderr)
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log(), 20), sep="\n  ---\n")


SCENARIOS = {
    "punch_and_udp_blocked": punch_and_udp_blocked,
    "quality_and_throughput": quality_and_throughput,
    "two_relays_and_route_decision": two_relays_and_route_decision,
    "matrix_five_nodes": matrix_five_nodes,
    "switched_off_and_cancel": switched_off_and_cancel,
}


def main() -> int:
    lablib.enter()
    names = sys.argv[1:] or list(SCENARIOS)
    t = Checks()
    for name in names:
        print(f"== {name}：{(SCENARIOS[name].__doc__ or '').strip()}", flush=True)
        with tempfile.TemporaryDirectory(prefix=f"i3t-e2e-{name}-") as work, Lab() as lab:
            try:
                SCENARIOS[name](lab, Path(work), t)
            except Exception as err:  # noqa: BLE001 - 一个场景出错不影响后面的场景
                t.check(f"{name} 没有跑完", False, repr(err))
        time.sleep(0.3)
    return t.summary()


if __name__ == "__main__":
    sys.exit(main())
