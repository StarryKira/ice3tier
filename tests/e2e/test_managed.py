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
from harness import Checks, Controller, Node, binary, create_groups, start_whoami, tail  # noqa: E402
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
    create_groups(controller, "laptop", "server")
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
    t.check("删掉 All <-> All 之后互相不可达", blocked(a, url, b))
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
    controller.ctl("relay", "enable", str(r.id))
    t.check("启用 relay 之后，不用重启节点就能经它到达", reaches(a, url, b, expect, timeout=30))
    def route_to_b() -> dict:
        routes = controller.ctl("node", "routes", str(a.id))["routes"]
        return next((route["route"] for route in routes if route["peer"]["id"] == b.id), {})

    try:
        wait_until(lambda: route_to_b().get("via") == "relay", timeout=15, what="a 上报经 relay 的路由")
    except TimeoutError:
        pass
    t.check("a 上报的路由显示经 relay", route_to_b() == {"via": "relay", "relays": [r.id]}, str(route_to_b()))
    pinged = a.host.run(binary("i3t"), "--state-dir", str(a.state), "ping", b.name, check=False, timeout=20)
    t.check("i3t ping 写 relay 的名字，不写编号", pinged.returncode == 0 and f"经 {r.name}" in pinged.stdout and f"#{r.id}" not in pinged.stdout,
            f"{pinged.stdout!r} {pinged.stderr.strip()}")
    if t.failures:
        print(tail(a.read_log(), 25), tail(b.read_log(), 25), tail(r.read_log(), 25), sep="\n  ---\n")


def route_of(controller: Controller, node: Node, peer: Node) -> dict:
    """`node` 最近上报的、到 `peer` 的路。"""
    routes = controller.ctl("node", "routes", str(node.id))["routes"]
    return next((route["route"] for route in routes if route["peer"]["id"] == peer.id), {})


def wait_route(controller: Controller, node: Node, peer: Node, via: str, timeout: float = 30) -> dict:
    try:
        wait_until(lambda: route_of(controller, node, peer).get("via") == via, timeout=timeout, what=f"{node.name} 到 {peer.name} 的路变成 {via}")
    except TimeoutError:
        pass
    return route_of(controller, node, peer)


def slow_download(node: Node, url: str, via: Node, megabytes: int, rate: str = "1M"):
    """在后台开始一次限速下载，返回进程。限速是为了让它持续几十秒，中间好换路。"""
    import subprocess
    size = megabytes * 1_000_000
    cmd = node.host.cmd("curl", "-sS", "--max-time", "120", "--limit-rate", rate, "-o", "/dev/null", "-w", "%{size_download}",
                        "-x", f"socks5h://{via.virtual_ip}@127.0.0.1:1080", f"{url}bytes/{size}")
    return subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True), size


def finished_intact(download, size: int, timeout: float = 120) -> tuple[bool, str]:
    out, err = download.communicate(timeout=timeout)
    return out == str(size), f"下载了 {out!r}，应该是 {size}；{err.strip()}"


def hole_punch(lab: Lab, work: Path, t: Checks) -> None:
    """两个节点各在一个普通 NAT 后面，没有 relay：经 controller 交换地址，打洞直连。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a = Node(lab.nat("nat-a", NatKind.CONE).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.CONE).host("b"), work, declare=("exit",))
    for node in (a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip

    t.check("没有 relay，靠打洞也能到达", reaches(a, url, b, expect, timeout=40))
    route = wait_route(controller, a, b, "direct")
    t.check("a 上报的路由是直连，地址是 b 所在 NAT 的公网地址", route.get("via") == "direct" and route.get("addr", "").startswith(expect + ":"), str(route))
    download, size = slow_download(a, url, b, 5, rate="5M")
    ok, detail = finished_intact(download, size)
    t.check("打洞出来的直连上下载 5MB", ok, detail)
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log(), 30), sep="\n  ---\n")


def relay_then_direct(lab: Lab, work: Path, t: Checks) -> None:
    """有 relay 时第一次访问不用等打洞；直连打通之后换过去，正在进行的下载不断。"""
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
    controller.ctl("relay", "enable", str(r.id))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip
    wait_until(lambda: all(controller.ctl("node", "show", str(node.id))["attached"] for node in (a, b)), timeout=20, what="两个节点挂靠上 relay")

    started = time.monotonic()
    download, size = slow_download(a, url, b, 30)
    wait_until(lambda: "和对端的连接建立" in a.read_log() or download.poll() is not None, timeout=15, what="a 和 b 建立连接")
    t.check("第一次访问在 3 秒内就连上了（先经 relay，不等打洞）", time.monotonic() - started < 3 and download.poll() is None,
            f"用了 {time.monotonic() - started:.1f} 秒")
    route = wait_route(controller, a, b, "direct", timeout=75)
    switched_during_download = download.poll() is None
    t.check("之后换到了直连", route.get("via") == "direct", str(route))
    t.check("换路发生在下载进行当中", switched_during_download)
    ok, detail = finished_intact(download, size)
    t.check("换路没有打断下载，字节数对得上", ok, detail)
    if t.failures:
        print(tail(a.read_log(), 40), tail(b.read_log(), 25), sep="\n  ---\n")


def tcp_relay(lab: Lab, work: Path, t: Checks) -> None:
    """a 所在的网络用不了 UDP：它用 TCP 连 relay，b 照常用 UDP。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.CONE).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.CONE).host("b"), work, declare=("exit",))
    a.host.block_udp()
    for node in (r, a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    controller.ctl("relay", "enable", str(r.id))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip

    t.check("UDP 用不了的节点经 TCP 中转也能到达", reaches(a, url, b, expect, timeout=40))
    t.check("a 用的是 TCP 中转", "中转会话建立（TCP）" in a.read_log(), tail(a.read_log(), 8))
    t.check("b 用的是 UDP 中转", "中转会话建立（UDP）" in b.read_log(), tail(b.read_log(), 8))
    download, size = slow_download(a, url, b, 10, rate="5M")
    ok, detail = finished_intact(download, size)
    t.check("经 TCP 中转下载 10MB", ok, detail)

    a.host.block_udp(False)
    route = wait_route(controller, a, b, "direct", timeout=90)
    t.check("UDP 恢复之后改走直连", route.get("via") == "direct", str(route))
    if t.failures:
        print(tail(a.read_log(), 40), tail(r.read_log(), 20), sep="\n  ---\n")


def relay_failover(lab: Lab, work: Path, t: Checks) -> None:
    """正在用的 relay 下线：连接换到另一个 relay，正在进行的下载不断。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r1, r2 = Node(lab.host("r1"), work, declare=("relay",)), Node(lab.host("r2"), work, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, declare=("exit",))
    for node in (r1, r2, a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    for relay in (r1, r2):
        controller.ctl("relay", "enable", str(relay.id))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip
    t.check("经 relay 到达", reaches(a, url, b, expect, timeout=40))

    download, size = slow_download(a, url, b, 30)
    time.sleep(5)
    used = wait_route(controller, a, b, "relay", timeout=15).get("relays", [None])[0]
    victim = r1 if used == r1.id else r2
    victim.stop()
    survivor = r2 if victim is r1 else r1
    ok, detail = finished_intact(download, size)
    t.check(f"下载途中停掉正在用的 relay（{victim.name}），下载没有断", ok, detail)
    route = wait_route(controller, a, b, "relay", timeout=20)
    t.check(f"之后的路改经 {survivor.name}", route.get("relays") == [survivor.id], f"{route}，原先经 {used}")
    if t.failures:
        print(tail(a.read_log(), 40), tail(b.read_log(), 25), sep="\n  ---\n")


def relay_record(controller: Controller, relay: Node) -> dict:
    """controller 眼里这个 relay 的 `relay` 一项：它的地址（找出来的）和设置。"""
    return controller.ctl("node", "show", str(relay.id))["relay"] or {}


def attached_to(controller: Controller, node: Node) -> list[int]:
    return [entry["relay"] for entry in controller.ctl("node", "show", str(node.id))["attached"]]


def relay_without_udp(lab: Lab, work: Path, t: Checks) -> None:
    """relay 所在的机器收不到 UDP：没有人说它只能用 TCP，节点自己改用 TCP 连它，照常经它到达。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    r = Node(lab.host("r"), work, declare=("relay",))
    lablib.drop_inbound_udp(r.host)
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, declare=("exit",))
    for node in (r, a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    enabled = controller.ctl("relay", "enable", str(r.id))
    addresses = [entry["address"] for entry in enabled["relay"]["addresses"]]
    t.check("启用时不填地址，controller 找出了 relay 的地址", addresses[:1] == [r.endpoint], str(enabled["relay"]))
    udp = [check for check in enabled["reachability"] if check["protocol"] == "udp"]
    t.check("controller 试连 UDP 不通也照样启用", bool(udp) and not udp[0]["reachable"] and enabled["relay"] is not None, str(enabled["reachability"]))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip
    t.check("经这个 relay 到达", reaches(a, url, b, expect, timeout=40))
    t.check("a 自己改用了 TCP 连它", "中转会话建立（TCP）" in a.read_log(), tail(a.read_log(), 8))
    if t.failures:
        print(tail(a.read_log(), 30), tail(r.read_log(), 20), sep="\n  ---\n")


def relay_on_declared_address(lab: Lab, work: Path, t: Checks) -> None:
    """relay 在 NAT 后面，路由器只在另一个端口上映射进来：它看到的地址和网卡上的地址都连不上，节点上声明的那个连得上。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    nat_r = lab.nat("nat-r", NatKind.CONE)
    r = Node(nat_r.host("r"), work, declare=("relay",))
    lablib.forward_port_as(nat_r, 40330, r.host, 20330)
    declared = f"{nat_r.ip}:40330"
    said = r.host.run(binary("i3t"), "--state-dir", str(r.state), "set", "relay-address", declared, check=False)
    t.check("i3t set relay-address 写进配置", said.returncode == 0 and declared in said.stdout, said.stderr.strip())
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, declare=("exit",))
    for node in (r, a, b):
        node.join(controller)
    controller.ctl("exit", "enable", str(b.id))
    enabled_at = time.monotonic()
    controller.ctl("relay", "enable", str(r.id))
    relay = relay_record(controller, r)
    listed = [(entry["address"], entry["source"]) for entry in relay.get("addresses", [])]
    t.check("声明的地址排在最前，后面是 controller 看到的和网卡上的",
            listed[:3] == [(declared, "declared"), (f"{nat_r.ip}:20330", "observed"), (f"{r.host.ip}:20330", "local")], str(listed))
    try:
        wait_until(lambda: r.id in attached_to(controller, a), timeout=20, what="a 挂靠上 relay")
    except TimeoutError:
        pass
    took = time.monotonic() - enabled_at
    t.check("前两个地址连不上，也很快挂靠上（不挨个等）", r.id in attached_to(controller, a) and took < 15, f"用了 {took:.1f} 秒")
    t.check("用的是声明的地址", f"via={declared}" in a.read_log(), tail(a.read_log(), 8))
    url, expect = f"http://{web.ip}/", lab.hosts["nat-b"].ip
    t.check("经它到达", reaches(a, url, b, expect, timeout=30))
    if t.failures:
        print(tail(a.read_log(), 30), tail(r.read_log(), 20), sep="\n  ---\n")


def relay_in_a_lan(lab: Lab, work: Path, t: Checks) -> None:
    """relay 在一个局域网里，外面连不进来：同一个局域网里的节点靠它网卡上的地址用它，外面的节点用不了，也没有谁说它坏了。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    lan = lab.nat("nat-l", NatKind.CONE)
    r = Node(lan.host("r"), work, declare=("relay",))
    inside = Node(lan.host("inside"), work)
    outside = Node(lab.nat("nat-o", NatKind.SYMMETRIC).host("outside"), work)
    for node in (r, inside, outside):
        node.join(controller)
    enabled_at = time.monotonic()
    controller.ctl("relay", "enable", str(r.id))
    listed = [(entry["address"], entry["source"]) for entry in relay_record(controller, r).get("addresses", [])]
    t.check("relay 的地址：controller 看到的是局域网的出口，网卡上的是内网地址",
            listed == [(f"{lan.ip}:20330", "observed"), (f"{r.host.ip}:20330", "local")], str(listed))
    try:
        wait_until(lambda: r.id in attached_to(controller, inside), timeout=20, what="局域网里的节点挂靠上 relay")
    except TimeoutError:
        pass
    took = time.monotonic() - enabled_at
    t.check("同一个局域网里的节点经内网地址用上了它", r.id in attached_to(controller, inside) and f"via={r.host.ip}:20330" in inside.read_log(),
            tail(inside.read_log(), 8))
    t.check("排在前面的地址连不上，也很快挂靠上（不挨个等）", took < 15, f"用了 {took:.1f} 秒")
    time.sleep(8)
    t.check("外面的节点连不上它，也就不用它", r.id not in attached_to(controller, outside), str(attached_to(controller, outside)))
    shown = controller.ctl("node", "show", str(r.id))
    t.check("它照样是启用着的 relay，没有提醒", shown["relay"] is not None and shown["warnings"] == [], str(shown["warnings"]))
    if t.failures:
        print(tail(inside.read_log(), 30), tail(outside.read_log(), 20), sep="\n  ---\n")


def range_overlap_warning(lab: Lab, work: Path, t: Checks) -> None:
    """节点所在的一个真实网络落在网络的地址段里：节点上和总览里有一条提醒，什么都不拦。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab", "--cidr", "10.66.0.0/16")
    a = Node(lab.host("a"), work)
    lablib.private_link(lab, a.host, "a-lan", "10.66.200.0/24")
    a.join(controller)
    warning = {"code": "range_overlap", "address": "10.66.200.1"}
    try:
        wait_until(lambda: controller.node(a)["warnings"] == [warning], timeout=30, what="节点上的提醒")
    except TimeoutError:
        pass
    t.check("节点上有地址段重叠的提醒", controller.node(a)["warnings"] == [warning], str(controller.node(a)["warnings"]))
    overview = controller.ctl("overview")
    t.check("总览里也有", [(entry["node"]["id"], entry["code"]) for entry in overview["warnings"]] == [(a.id, "range_overlap")],
            str(overview["warnings"]))
    shown = controller.host.run(binary("i3tctl"), "--token", controller.token, "node", "show", str(a.id), check=False).stdout
    t.check("i3tctl node show 写出了这条提醒", "网卡地址 10.66.200.1 在网络的地址段里" in shown, shown)
    t.check("节点照常在线", controller.node(a)["online"])


SCENARIOS = {
    "join_and_exit": join_and_exit,
    "enable_relay": enable_relay,
    "hole_punch": hole_punch,
    "relay_then_direct": relay_then_direct,
    "tcp_relay": tcp_relay,
    "relay_failover": relay_failover,
    "relay_without_udp": relay_without_udp,
    "relay_on_declared_address": relay_on_declared_address,
    "relay_in_a_lan": relay_in_a_lan,
    "range_overlap_warning": range_overlap_warning,
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
