#!/usr/bin/env python3
"""限额、用量和状态的端到端测试：出口的带宽上限和配额、接入凭据的用量和流量上限、自动挑端口的端口转发、端口被占用。

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
    url = f"http://{web.ip}/bytes/"
    # 等出口的设置到了两边：能经它下载一点东西就是到了。
    eventually_value(lambda: timed_download(a, url + "1000", via=ex, timeout=6), lambda result: result[0] == 0 and result[1] == 1000)
    return controller, a, ex, url


def bandwidth_cap(lab: Lab, work: Path, t: Checks) -> None:
    """出口的带宽上限：经它的下载被限在上限附近，去掉上限之后快得多。"""
    controller, a, ex, url = ready_exit(lab, work, "--bandwidth", "4")
    size = 2_000_000
    code, received, seconds = timed_download(a, url + str(size), via=ex)
    rate_mbit = received * 8 / seconds / 1e6 if seconds else 0
    t.check("限速之下下载完整", code == 0 and received == size, f"退出码 {code}，收到 {received}")
    t.check("4 Mbit/s 的上限把 2MB 的下载限在 5 Mbit/s 以内", 0 < rate_mbit <= 5, f"{rate_mbit:.2f} Mbit/s，用了 {seconds:.2f} 秒")

    controller.ctl("exit", "enable", str(ex.id))
    code, received, seconds = eventually_value(lambda: timed_download(a, url + str(size), via=ex), lambda result: result[2] and result[2] < 1.5,
                                               timeout=20)
    fast = received * 8 / seconds / 1e6 if seconds else 0
    t.check("去掉上限之后明显更快", code == 0 and fast > 3 * rate_mbit, f"{fast:.2f} Mbit/s 对比 {rate_mbit:.2f} Mbit/s")
    if t.failures:
        print(tail(ex.read_log()), sep="\n")


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


def credential_usage_and_limit(lab: Lab, work: Path, t: Checks) -> None:
    """接入凭据：经对外的入口用了它之后，管理 API 看得到用量；调低流量上限之后它被拦下。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    gw, ex = Node(lab.host("gw"), work), Node(lab.host("ex"), work, declare=("exit",))
    for node in (gw, ex):
        node.join(controller)
    controller.ctl("exit", "enable", str(ex.id))
    created = controller.ctl("credential", "create", "phone")
    token, credential_id = created["token"], created["id"]
    t.check("凭据带着认得出来的前缀", token.startswith(created["prefix"]) and len(created["prefix"]) == 6, created["prefix"])
    controller.ctl("listener", "add", str(gw.id), "public", "0.0.0.0:3128", "--http", "--require-credential")
    wait_until(lambda: ":3128 " in gw.host.run("ss", "-lnt").stdout, timeout=20, what="入口开始监听")
    outsider = lab.host("outsider")

    def fetch(size: int):
        return outsider.run("curl", "-sS", "--max-time", "10", "-p", "-x", f"http://{ex.virtual_ip}:{token}@{gw.host.ip}:3128",
                            "-o", "/dev/null", "-w", "%{http_connect} %{http_code} %{size_download}", f"http://{web.ip}/bytes/{size}",
                            check=False)

    first = eventually_value(lambda: fetch(200_000), lambda result: result.stdout == "200 200 200000")
    t.check("经对外的入口用凭据下载", first.stdout == "200 200 200000", f"{first.stdout!r} {first.stderr.strip()}")

    def credential() -> dict:
        return next(entry for entry in controller.ctl("credential", "list") if entry["id"] == credential_id)

    seen = eventually_value(credential, lambda entry: entry["usage"]["bytes"] >= 200_000)
    t.check("管理 API 里看得到凭据的用量", seen["usage"]["bytes"] >= 200_000 and seen["usage"]["last_used_at"] is not None, f"{seen}")

    controller.ctl("credential", "set", str(credential_id), "--max-bytes", "100K")
    refused = eventually_value(lambda: fetch(1000), lambda result: result.stdout.startswith("429"), timeout=20)
    t.check("调低流量上限之后它被拦下（429）", refused.stdout.startswith("429"), f"{refused.stdout!r}")
    t.check("凭据的状态是用完了", credential()["state"] == "exhausted", f"{credential()}")

    controller.ctl("credential", "set", str(credential_id), "--reset-usage")
    again = eventually_value(lambda: fetch(1000), lambda result: result.stdout == "200 200 1000", timeout=20)
    t.check("用量清零之后又能用，令牌不用换", again.stdout == "200 200 1000", f"{again.stdout!r}")

    controller.ctl("credential", "delete", str(credential_id))
    t.check("吊销之后凭据还在列表里，状态是吊销", credential()["state"] == "revoked")
    if t.failures:
        print(tail(gw.read_log()), tail(controller.log.read_text(errors="replace")), sep="\n  ---\n")


def forward_auto_port(lab: Lab, work: Path, t: Checks) -> None:
    """端口写 0 的端口转发：节点挑一个端口，管理 API 报出来，节点重启之后还是这个端口。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, target = Node(lab.host("a"), work), Node(lab.host("t"), work, declare=("forward",))
    for node in (a, target):
        node.join(controller)
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
    a, target = Node(lab.host("a"), work), Node(lab.host("t"), work, declare=("forward",))
    for node in (a, target):
        node.join(controller)
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
    a, gw, target = Node(lab.host("a"), work), Node(lab.host("gw"), work), Node(lab.host("t"), work, declare=("forward",))
    for node in (a, gw, target):
        node.join(controller)
    start_whoami(target.host, 8081, bind="127.0.0.1")
    start_udp_echo(target.host, 7000, bind="127.0.0.1")
    controller.ctl("forward", "add", str(gw.id), f"{gw.virtual_ip}:9200", str(target.id), "127.0.0.1:8081")
    controller.ctl("forward", "add", str(gw.id), f"{gw.virtual_ip}:9300", str(target.id), "127.0.0.1:7000", "--udp")

    seen = eventually_value(lambda: a.curl(f"http://{gw.virtual_ip}:9200/", timeout=6), lambda result: result.returncode == 0)
    t.check("TCP：经 gw 的虚拟 IP 到了 t 回环上的服务", seen.stdout == "127.0.0.1", f"{seen.stdout!r} {seen.stderr.strip()}")
    answer = eventually_value(lambda: socks5_udp(a.host, gw.virtual_ip, 9300, b"relayed", tries=3), lambda answer: answer is not None)
    source, data = (answer["data"].partition(b"|")[0].decode(), answer["data"].partition(b"|")[2]) if answer else ("", b"")
    t.check("UDP：经 gw 的虚拟 IP 到了 t 回环上的服务", data == b"relayed" and source.startswith("127.0.0.1"), f"{answer}")
    t.check("UDP：回来的报文报成从 gw 的那个端口来的", bool(answer) and answer["source"] == gw.virtual_ip and answer["port"] == 9300, f"{answer}")
    forwards = eventually_value(lambda: controller.ctl("node", "listeners", str(gw.id))["forwards"], lambda forwards: len(forwards) == 2)
    states = {entry["bind"]: entry["state"] for entry in forwards}
    t.check("两条转发在 gw 上都报 virtual", set(states.values()) == {"virtual"}, f"{states}")
    if t.failures:
        print(tail(gw.read_log()), tail(target.read_log()), sep="\n  ---\n")


SCENARIOS = {
    "bandwidth_cap": bandwidth_cap,
    "quota_runs_out": quota_runs_out,
    "credential_usage_and_limit": credential_usage_and_limit,
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
