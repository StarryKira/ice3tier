#!/usr/bin/env python3
"""限额、用量和状态的端到端测试：出口的配额、代理的用量、自动挑端口的端口转发、端口被占用。

    python3 tests/e2e/test_limits.py [场景名...]
"""

from __future__ import annotations

import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import (  # noqa: E402
    Checks,
    Controller,
    Node,
    eventually_value,
    hold_port,
    socks5_udp,
    start_udp_echo,
    start_whoami,
    tail,
    timed_download,
)
from lab import Lab, wait_until  # noqa: E402


def exit_view(controller: Controller, exit_node: Node) -> dict | None:
    return next((view for view in controller.ctl("exit", "list") if view["node"]["id"] == exit_node.id), None)


def ready_exit(lab: Lab, work: Path, *settings: str) -> tuple[Controller, Node, Node, str]:
    """一个 controller，一个客户端节点 a，一个出口节点 ex（按 `settings` 启用），一台公网上的 HTTP 服务。返回服务的地址。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, ex = Node(lab.host("a"), work), Node(lab.host("ex"), work, declare=("exit",))
    for node in (a, ex):
        node.join(controller)
    controller.ctl("exit", "enable", str(ex.id), *settings)
    controller.add_proxy(a, ex)
    url = f"http://{web.ip}/bytes/"
    # 等出口的设置到了两边：能经它下载一点东西就是到了。
    eventually_value(lambda: timed_download(a, url + "1000", via=ex, timeout=6), lambda result: result[0] == 0 and result[1] == 1000)
    return controller, a, ex, url


def quota_runs_out(lab: Lab, work: Path, t: Checks) -> None:
    """出口的配额：用完时正在下载的被断开，之后的请求被拒绝；controller 看得到用量。"""
    controller, a, ex, url = ready_exit(lab, work, "--quota", "1M")
    code, received, _ = timed_download(a, url + "3000000", via=ex)
    t.check("超过配额的下载被断开", code != 0 and received <= 1_000_000, f"退出码 {code}，收到 {received}")
    code, received, _ = timed_download(a, url + "1000", via=ex, timeout=8)
    t.check("配额用完之后新的请求被拒绝", code != 0 and received == 0, f"退出码 {code}，收到 {received}")

    view = eventually_value(lambda: exit_view(controller, ex),
                            lambda view: view and view["usage"] and view["usage"]["quota"] and view["usage"]["quota"]["used_bytes"] >= 1_000_000)
    quota = (view or {}).get("usage", {}) and view["usage"].get("quota")
    t.check("controller 显示出口的配额和用量", bool(quota) and quota["limit_bytes"] == 1_000_000 and quota["used_bytes"] >= 1_000_000, f"{quota}")
    t.check("被拒绝的不算出口不健康", view is not None and view["usage"]["failed_by_exit"] == 0 and view["usage"]["refused"] >= 1,
            f"{view and view['usage']}")
    seen = [entry for entry in (view or {}).get("observed", []) if entry["by"]["id"] == a.id]
    t.check("发起的节点报的经这个出口的计数也在", bool(seen) and seen[0]["flows"] >= 2, f"{seen}")

    # 重启出口：用量从存下来的那一份接着记，配额还是用完的。
    ex.stop()
    wait_until(lambda: not controller.node(ex)["online"], timeout=40, what="controller 看到出口下线")
    ex.start()
    wait_until(lambda: controller.node(ex)["online"], timeout=40, what="出口重新连上")
    code, received, _ = eventually_value(lambda: timed_download(a, url + "1000", via=ex, timeout=6), lambda result: result[0] != 0, timeout=15)
    t.check("出口重启之后配额还是用完的", code != 0 and received == 0, f"退出码 {code}，收到 {received}")
    if t.failures:
        print(tail(ex.read_log()), tail(a.read_log()), sep="\n  ---\n")


def proxy_usage(lab: Lab, work: Path, t: Checks) -> None:
    """代理的用量：经对外开放的代理下载之后，管理 API 里这个代理有连接数和流量，节点的监听视图和指标里也有；删掉它端口就关了。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    gw, ex = Node(lab.host("gw"), work), Node(lab.host("ex"), work, declare=("exit",))
    for node in (gw, ex):
        node.join(controller)
    controller.ctl("exit", "enable", str(ex.id))
    created = controller.add_proxy(gw, ex, "--protocol", "http", bind="0.0.0.0", port=3128)
    outsider = lab.host("outsider")
    login = f"{created['username']}:{created['password']}"

    def fetch(size: int):
        return outsider.run("curl", "-sS", "--max-time", "10", "-p", "-x", f"http://{login}@{gw.host.ip}:3128",
                            "-o", "/dev/null", "-w", "%{http_connect} %{http_code} %{size_download}", f"http://{web.ip}/bytes/{size}",
                            check=False)

    first = eventually_value(lambda: fetch(200_000), lambda result: result.stdout == "200 200 200000")
    t.check("经对外开放的代理下载", first.stdout == "200 200 200000", f"{first.stdout!r} {first.stderr.strip()}")
    seen = eventually_value(lambda: controller.proxy(created["id"]), lambda proxy: proxy["bytes_down"] >= 200_000)
    t.check("管理 API 里看得到这个代理的流量", seen["bytes_down"] >= 200_000 and seen["bytes_up"] > 0, str(seen))
    t.check("状态是在监听，监听的是 0.0.0.0:3128", seen["status"] == {"code": "listening", "local": "0.0.0.0:3128"}, str(seen["status"]))
    on_node = controller.ctl("node", "listeners", str(gw.id))["listeners"]
    listed = [entry for entry in on_node if entry["id"] == created["id"]]
    t.check("节点的监听视图里有它，带着编号，在监听", bool(listed) and not listed[0]["builtin"] and listed[0]["state"] == "listening"
            and listed[0]["bytes_down"] >= 200_000, str(on_node))
    local = [entry["name"] for entry in on_node if entry["builtin"]]
    t.check("节点的监听视图里也有本机配置的组网入口", sorted(local) == ["local-http", "local-socks5"], str(on_node))
    metrics = gw.host.run("curl", "-sS", "--max-time", "5", "http://127.0.0.1:20333/metrics", check=False).stdout
    t.check("节点的指标里有每个代理的流量", f'i3t_proxy_bytes_total{{proxy="{created["name"]}",direction="down"}}' in metrics,
            "\n".join(line for line in metrics.splitlines() if "proxy" in line)[:500])

    controller.ctl("proxy", "delete", str(created["id"]))
    wait_until(lambda: ":3128 " not in gw.host.run("ss", "-lnt").stdout, timeout=20, what="删掉的代理不再监听")
    t.check("删掉之后端口关了", ":3128 " not in gw.host.run("ss", "-lnt").stdout)
    if t.failures:
        print(tail(gw.read_log()), tail(controller.log.read_text(errors="replace")), sep="\n  ---\n")


def forward_auto_port(lab: Lab, work: Path, t: Checks) -> None:
    """端口写 0 的端口转发：节点挑一个端口，管理 API 报出来，节点重启之后还是这个端口。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, target = Node(lab.host("a"), work), Node(lab.host("t"), work)
    for node in (a, target):
        node.join(controller)
    controller.forward_target(target)
    start_whoami(target.host, 8081, bind="127.0.0.1")
    created = controller.ctl("forward", "add", str(a.id), "127.0.0.1:0", str(target.id), "127.0.0.1:8081")

    def listen() -> dict | None:
        forward = next(entry for entry in controller.ctl("forward", "list") if entry["id"] == created["id"])
        return forward["listen"]

    first = eventually_value(listen, lambda status: status is not None and status["state"] == "listening")
    port = int(first["local"].rsplit(":", 1)[1]) if first and first.get("local") else 0
    t.check("管理 API 报出节点挑定的端口", port != 0, f"{first}")
    reached = eventually_value(lambda: a.host.run("curl", "-sS", "--max-time", "6", f"http://127.0.0.1:{port}/", check=False),
                               lambda result: result.returncode == 0)
    t.check("挑定的端口通到目标节点回环上的服务", reached.stdout == "127.0.0.1", f"{reached.stdout!r} {reached.stderr.strip()}")

    a.stop()
    wait_until(lambda: not controller.node(a)["online"], timeout=40, what="controller 看到 a 下线")
    a.start()
    wait_until(lambda: controller.node(a)["online"], timeout=40, what="a 重新连上")
    after = eventually_value(listen, lambda status: status is not None and status["state"] == "listening", timeout=40)
    t.check("节点重启之后还是同一个端口", after is not None and after.get("local") == first.get("local"), f"{first} → {after}")
    reached = eventually_value(lambda: a.host.run("curl", "-sS", "--max-time", "6", f"http://127.0.0.1:{port}/", check=False),
                               lambda result: result.returncode == 0)
    t.check("重启之后这个端口照样能用", reached.stdout == "127.0.0.1", f"{reached.stdout!r}")
    listeners = controller.ctl("node", "listeners", str(a.id))
    builtin = [entry["name"] for entry in listeners["listeners"] if entry["builtin"]]
    t.check("节点自己开的回环入口也列出来了", "local-socks5" in builtin and "local-http" in builtin, f"{builtin}")
    if t.failures:
        print(tail(a.read_log()), sep="\n")


def port_in_use(lab: Lab, work: Path, t: Checks) -> None:
    """监听端口被别的程序占着：管理 API 里这条转发显示端口被占用。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, target = Node(lab.host("a"), work), Node(lab.host("t"), work)
    for node in (a, target):
        node.join(controller)
    controller.forward_target(target)
    hold_port(a.host, 9555)
    created = controller.ctl("forward", "add", str(a.id), "127.0.0.1:9555", str(target.id), "127.0.0.1:8081")

    def listen() -> dict | None:
        return next(entry for entry in controller.ctl("forward", "list") if entry["id"] == created["id"])["listen"]

    status = eventually_value(listen, lambda status: status is not None and status["state"] == "port_in_use")
    t.check("端口被占用显示在管理 API 里", status is not None and status["state"] == "port_in_use" and status.get("detail"), f"{status}")
    if t.failures:
        print(tail(a.read_log()), sep="\n")


def virtual_forward_relay(lab: Lab, work: Path, t: Checks) -> None:
    """监听在 gw 虚拟 IP 上、目标是另一个节点的端口转发：gw 替发起节点接力，TCP 和 UDP 都行。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, gw, target = Node(lab.host("a"), work), Node(lab.host("gw"), work), Node(lab.host("t"), work)
    for node in (a, gw, target):
        node.join(controller)
    controller.forward_target(target)
    start_whoami(target.host, 8081, bind="127.0.0.1")
    start_udp_echo(target.host, 7000, bind="127.0.0.1")
    controller.ctl("forward", "add", str(gw.id), f"{gw.virtual_ip}:9200", str(target.id), "127.0.0.1:8081")
    controller.ctl("forward", "add", str(gw.id), f"{gw.virtual_ip}:9300", str(target.id), "127.0.0.1:7000", "--udp")

    seen = eventually_value(lambda: a.curl(f"http://{gw.virtual_ip}:9200/", timeout=6), lambda result: result.returncode == 0)
    t.check("TCP：经 gw 的虚拟 IP 到了 t 回环上的服务", seen.stdout == "127.0.0.1", f"{seen.stdout!r} {seen.stderr.strip()}")
    answer = eventually_value(lambda: socks5_udp(a, gw.virtual_ip, 9300, b"relayed", tries=3), lambda answer: answer is not None)
    source, data = (answer["data"].partition(b"|")[0].decode(), answer["data"].partition(b"|")[2]) if answer else ("", b"")
    t.check("UDP：经 gw 的虚拟 IP 到了 t 回环上的服务", data == b"relayed" and source.startswith("127.0.0.1"), f"{answer}")
    t.check("UDP：回来的报文报成从 gw 的那个端口来的", bool(answer) and answer["source"] == gw.virtual_ip and answer["port"] == 9300, f"{answer}")
    forwards = eventually_value(lambda: controller.ctl("node", "listeners", str(gw.id))["forwards"], lambda forwards: len(forwards) == 2)
    states = {entry["bind"]: entry["state"] for entry in forwards}
    # gw 的虚拟网卡装着（默认打开）：两条都在它的虚拟 IP 上真的监听，经网卡来的连接也到得了。
    t.check("两条转发在 gw 上都报 listening", set(states.values()) == {"listening"}, f"{states}")
    if t.failures:
        print(tail(gw.read_log()), tail(target.read_log()), sep="\n  ---\n")


SCENARIOS = {
    "quota_runs_out": quota_runs_out,
    "proxy_usage": proxy_usage,
    "forward_auto_port": forward_auto_port,
    "port_in_use": port_in_use,
    "virtual_forward_relay": virtual_forward_relay,
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
