#!/usr/bin/env python3
"""路径诊断的端到端测试：controller 现问节点，拿到候选路径和它们的代价，以及 relay 之间的延迟矩阵。

    python3 tests/e2e/test_paths.py [场景名...]
"""

from __future__ import annotations

import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import Checks, Controller, Node, binary, start_whoami, tail  # noqa: E402
from lab import Lab, NatKind, wait_until  # noqa: E402


def reaches(node: Node, url: str, via: Node, expect: str, timeout: float = 30) -> bool:
    """反复尝试，直到经 `via` 访问 `url` 得到 `expect`，或者超时。配置下发和建连都要一点时间。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if node.curl(url, via=via, timeout=6).stdout == expect:
            return True
        time.sleep(0.5)
    return False


def eventually(action, ok, timeout: float = 30, interval: float = 1.0):
    """反复执行 `action`，直到结果满足 `ok` 或者超时，返回最后一次的结果。"""
    deadline = time.monotonic() + timeout
    while True:
        value = action()
        if ok(value) or time.monotonic() >= deadline:
            return value
        time.sleep(interval)


def human(controller: Controller, *args: str) -> str:
    """i3tctl 给人看的输出（不带 --json）。"""
    done = controller.host.run(binary("i3tctl"), "--token", controller.token, *args, check=False)
    return done.stdout + done.stderr


def candidates_are_sane(leg: dict) -> tuple[bool, str]:
    """候选路径按代价从低到高排，每条都有代价和各段的测量值，现在走的那条恰好标了一条。"""
    candidates = leg.get("candidates") or []
    costs = [candidate["cost_us"] for candidate in candidates]
    current = [candidate for candidate in candidates if candidate["current"]]
    legs_measured = all(candidate["legs"] and all(segment["rtt_us"] > 0 for segment in candidate["legs"]) for candidate in candidates)
    ok = (bool(candidates) and costs == sorted(costs) and all(cost > 0 for cost in costs) and legs_measured
          and len(current) == 1 and current[0]["route"] == leg["route"])
    return ok, str(candidates)


def path_candidates_and_backbone(lab: Lab, work: Path, t: Checks) -> None:
    """一个 relay、两个在普通 NAT 后面的节点：路径接口给出两个方向的候选路径和代价，骨干接口给出 relay 的那一行。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.CONE).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.CONE).host("b"), work, declare=("exit",))
    for node in (r, a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    controller.ctl("relay", "enable", str(r.id), "--udp", r.endpoint, "--tcp", r.endpoint)
    wait_until(lambda: all(controller.ctl("node", "show", str(node.id))["attached"] for node in (a, b)), timeout=20,
               what="两个节点挂靠上 relay")
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip
    t.check("a 经 b 出得去", reaches(a, url, b, expect))

    path = eventually(lambda: controller.ctl("path", str(a.id), str(b.id)),
                      lambda view: candidates_are_sane(view["forward"])[0] and candidates_are_sane(view["reverse"])[0])
    for direction in ("forward", "reverse"):
        leg = path[direction]
        t.check(f"{direction}：现问到了发送方", leg["source"] == "query", str(leg))
        ok, detail = candidates_are_sane(leg)
        t.check(f"{direction}：候选路径带着代价和每一段的延迟、抖动、丢包，按代价排好，现在走的那条标出来了", ok, detail)
        t.check(f"{direction}：有选路模式和节点自己说的原因", leg["mode"] in ("direct_first", "lowest_cost") and bool(leg["reason"]),
                f"mode={leg['mode']!r} reason={leg['reason']!r}")
    relayed = [candidate for candidate in path["forward"]["candidates"] if candidate["route"] == {"via": "relay", "relays": [r.id]}]
    t.check("候选里有经 relay 的那条，它有两段", bool(relayed) and len(relayed[0]["legs"]) == 2, str(path["forward"]["candidates"]))
    shown = human(controller, "path", "a", "b")
    t.check("i3tctl path 打印出候选路径", "a → b 的候选路径" in shown and "代价 ms" in shown, shown)

    backbone = eventually(lambda: controller.ctl("backbone"), lambda view: all(relay["state"] for relay in view["relays"]))
    rows = {relay["relay"]["id"]: relay for relay in backbone["relays"]}
    row = rows.get(r.id) or {}
    t.check("骨干接口列出了这个网络里的 relay", list(rows) == [r.id], str(backbone))
    t.check("relay 的那一行有状态、负载和有多旧", row.get("state") == "serving" and row.get("load_percent") is not None
            and row.get("age_ms") is not None and row["age_ms"] < 120_000, str(row))
    t.check("relay 自己手上没有自己的行，这一行是挂靠它的节点给的", row.get("reported_by") in (a.id, b.id), str(row))
    t.check("回答了的节点列在 answered 里", bool(backbone["answered"]), str(backbone["answered"]))
    t.check("i3tctl backbone 打印出这一行", "正常" in human(controller, "backbone"), human(controller, "backbone"))

    # 打洞成功之后走直连：直连的类型出现在路径和路由表里，经 relay 的那条仍然在候选里。
    direct = eventually(lambda: controller.ctl("path", str(a.id), str(b.id))["forward"],
                        lambda leg: (leg.get("route") or {}).get("via") == "direct", timeout=75, interval=2)
    t.check("换到直连之后，路径接口说直连是打洞打通的", direct.get("direct") == "punched", str(direct))
    t.check("经 relay 的那条还在候选里，只是不是现在走的",
            any(candidate["route"].get("via") == "relay" and not candidate["current"] for candidate in direct.get("candidates") or []),
            str(direct.get("candidates")))
    routes = eventually(lambda: controller.ctl("node", "routes", str(a.id))["routes"],
                        lambda entries: any(entry["peer"]["id"] == b.id and entry.get("direct") == "punched" for entry in entries))
    t.check("节点的路由表里也有直连的类型", any(entry["peer"]["id"] == b.id and entry.get("direct") == "punched" for entry in routes),
            str(routes))
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log(), 20), tail(controller.log.read_text(errors="replace"), 20), sep="\n  ---\n")


SCENARIOS = {
    "path_candidates_and_backbone": path_candidates_and_backbone,
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
