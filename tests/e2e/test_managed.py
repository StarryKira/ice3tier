#!/usr/bin/env python3
"""由 controller 管理的端到端测试：节点用注册令牌加入，配置全部由 controller 下发。

    python3 tests/e2e/test_managed.py [场景名...]
"""

from __future__ import annotations

import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import Checks, Controller, Node, start_whoami, tail  # noqa: E402
from lab import Lab, NatKind, wait_until  # noqa: E402


def reaches(node: Node, url: str, via: Node, expect: str, timeout: float = 20) -> bool:
    """反复尝试，直到经 `via` 访问 `url` 得到 `expect`，或者超时。配置下发和建连都要一点时间。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if node.curl(url, via=via, timeout=6).stdout == expect:
            return True
        time.sleep(0.5)
    return False


def blocked(node: Node, url: str, via: Node, timeout: float = 15) -> bool:
    """反复尝试，直到经 `via` 访问 `url` 失败，或者超时。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if node.curl(url, via=via, timeout=6).returncode != 0:
            return True
        time.sleep(0.5)
    return False


def join_and_exit(lab: Lab, work: Path, t: Checks) -> None:
    """两个公网节点加入网络；启用出口；访问规则收紧和放开；删除节点。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab", "--cidr", "10.66.0.0/16")
    a, b = Node(lab.host("a"), work), Node(lab.host("b"), work, declare=("exit",))
    a.join(controller, "laptop")
    b.join(controller, "server")
    t.check("加入之后得到虚拟 IP 和名字", (a.virtual_ip, b.virtual_ip) == ("10.66.0.1", "10.66.0.2"), f"{a.virtual_ip} {b.virtual_ip}")
    wait_until(lambda: all(controller.node(node)["online"] for node in (a, b)), what="两个节点上线")

    url = f"http://{web.ip}/"
    t.check("出口没启用时不能把 b 当出口", a.curl(url, via=b, timeout=6).returncode != 0)
    refused = controller.ctl("exit", "enable", str(a.id), check=False)
    t.check("没有声明的节点启用不了出口，并且提示要执行的命令", refused.returncode != 0 and "i3t set can-exit on" in refused.stderr,
            refused.stderr.strip())
    controller.ctl("exit", "enable", str(b.id))
    t.check("启用出口之后，不用重启节点就能用", reaches(a, url, b, b.host.ip))
    by_name = a.host.run("curl", "-sS", "--max-time", "8", "-x", "socks5h://b@127.0.0.1:1080", url, check=False)
    t.check("用户名可以写节点名", by_name.stdout == b.host.ip, by_name.stderr.strip())

    rules = controller.ctl("rule", "list")
    controller.ctl("rule", "delete", str(rules[0]["id"]))
    t.check("删掉 all <-> all 之后互相不可达", blocked(a, url, b))
    controller.ctl("rule", "add", "laptop", "server", "--one-way")
    t.check("加上 laptop -> server 之后 a 又能用 b", reaches(a, url, b, b.host.ip))

    controller.ctl("node", "delete", str(a.id))
    wait_until(lambda: a.proc.poll() is not None, timeout=20, what="被删除的节点自己退出")
    t.check("被删除的节点说明了原因", "吊销" in a.read_log(), tail(a.read_log(), 5))
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), tail(controller.log.read_text(errors="replace")), sep="\n  ---\n")


def enable_relay(lab: Lab, work: Path, t: Checks) -> None:
    """两个节点都在对称型 NAT 后面，没有 relay 时不可达；在 controller 上启用 relay 之后自动经它转发。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, declare=("exit",))
    for node in (r, a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip

    t.check("没有 relay 时这对节点不可达", a.curl(url, via=b, timeout=10).returncode != 0)
    controller.ctl("relay", "enable", str(r.id), "--udp", r.endpoint, "--tcp", r.endpoint)
    t.check("启用 relay 之后，不用重启节点就能经它到达", reaches(a, url, b, expect, timeout=30))
    def route_to_b() -> dict:
        routes = controller.ctl("node", "routes", str(a.id))["routes"]
        return next((route["route"] for route in routes if route["peer"]["id"] == b.id), {})

    try:
        wait_until(lambda: route_to_b().get("via") == "relay", timeout=15, what="a 上报经 relay 的路由")
    except TimeoutError:
        pass
    t.check("a 上报的路由显示经 relay", route_to_b() == {"via": "relay", "relays": [r.id]}, str(route_to_b()))
    if t.failures:
        print(tail(a.read_log(), 25), tail(b.read_log(), 25), tail(r.read_log(), 25), sep="\n  ---\n")


SCENARIOS = {"join_and_exit": join_and_exit, "enable_relay": enable_relay}


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
