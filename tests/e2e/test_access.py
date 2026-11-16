#!/usr/bin/env python3
"""接入方式的端到端测试：节点上的服务、端口转发、出口背后的局域网、对外开放的代理入口，以及分组决定谁能访问谁。

    python3 tests/e2e/test_access.py [场景名...]
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import Checks, Controller, Node, binary, create_groups, group_names, start_whoami, tail  # noqa: E402
from lab import Lab, NatKind, wait_until  # noqa: E402


def socks(node: Node, url: str, user: str = "", timeout: float = 8):
    """从 `node` 所在的机器经它的本机 SOCKS5 入口访问 `url`；`user` 是代理的用户名。"""
    auth = f"{user}@" if user else ""
    return node.host.run("curl", "-sS", "--max-time", str(timeout), "-x", f"socks5h://{auth}127.0.0.1:1080", url, check=False, timeout=timeout + 5)


def connect(node: Node, host: str, port: int, *, via: str = "", timeout: float = 15):
    """用 `i3t connect` 从 `node` 连到 `host:port`，发一个 HTTP 请求，返回进程的结果（标准输出是对面的应答）。"""
    exit_args = ("--via", via) if via else ()
    return node.host.run(binary("i3t"), "--state-dir", str(node.state), "connect", host, str(port), *exit_args,
                         input="GET / HTTP/1.0\r\n\r\n", check=False, timeout=timeout)


def eventually(action, ok, timeout: float = 25):
    """反复执行 `action`，直到 `ok(结果)` 为真或者超时；返回最后一次的结果。配置下发和建连都要一点时间。"""
    deadline = time.monotonic() + timeout
    while True:
        result = action()
        if ok(result) or time.monotonic() > deadline:
            return result
        time.sleep(0.5)


def node_service(lab: Lab, work: Path, t: Checks) -> None:
    """访问另一个节点上的服务：目标写它的虚拟 IP 或名字，不需要它是出口；只监听回环地址的服务到不了。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, b = Node(lab.host("a"), work), Node(lab.host("b"), work)
    for node in (a, b):
        node.join(controller)
    start_whoami(b.host, 8080)
    start_whoami(b.host, 8081, bind="127.0.0.1")

    seen = eventually(lambda: socks(a, f"http://{b.virtual_ip}:8080/"), lambda r: r.returncode == 0)
    t.check("用虚拟 IP 访问 b 上的服务", seen.returncode == 0, seen.stderr.strip())
    t.check("服务看到的来源是 b 自己的非回环地址，不是 127.0.0.1", seen.stdout == b.host.ip, f"看到 {seen.stdout!r}")
    by_name = socks(a, "http://b.lab.i3t:8080/")
    t.check("用全名 b.lab.i3t 访问", by_name.stdout == b.host.ip, by_name.stderr.strip())
    loopback_only = socks(a, f"http://{b.virtual_ip}:8081/")
    t.check("只监听 127.0.0.1 的服务到不了", loopback_only.returncode != 0)
    as_exit = socks(a, "http://11.0.0.2/", user=b.virtual_ip)
    t.check("b 没有启用出口，不能经它上网", as_exit.returncode != 0)

    piped = connect(a, "b.lab.i3t", 8080)
    t.check("i3t connect 把标准输入输出接到 b 上的服务", piped.returncode == 0 and piped.stdout.endswith(b.host.ip),
            f"{piped.returncode} {piped.stdout!r} {piped.stderr.strip()}")
    short = connect(a, "b", 8080)
    t.check("i3t connect 只写节点的名字也行", short.returncode == 0 and short.stdout.endswith(b.host.ip),
            f"{short.returncode} {short.stdout!r} {short.stderr.strip()}")
    refused = connect(a, b.virtual_ip, 8081)
    t.check("i3t connect 连不上时以失败退出，标准输出上什么都没有", refused.returncode != 0 and refused.stdout == "",
            f"{refused.returncode} {refused.stdout!r} {refused.stderr.strip()}")
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), sep="\n  ---\n")


def port_forward(lab: Lab, work: Path, t: Checks) -> None:
    """端口转发：把目标节点回环上的服务转发到监听节点的本地端口，以及转发到目标节点的虚拟 IP 上。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    a, c, d = Node(lab.host("a"), work), Node(lab.host("c"), work, declare=("forward",)), Node(lab.host("d"), work)
    for node in (a, c, d):
        node.join(controller)
    for node in (c, d):
        start_whoami(node.host, 8081, bind="127.0.0.1")

    refused = controller.ctl("forward", "add", str(a.id), "127.0.0.1:9001", str(d.id), "127.0.0.1:8081", check=False)
    t.check("目标节点没有声明时配不了转发，并且提示要执行的命令", refused.returncode != 0 and "i3t set can-forward on" in refused.stderr,
            refused.stderr.strip())

    controller.ctl("forward", "add", str(a.id), "127.0.0.1:9000", str(c.id), "127.0.0.1:8081")
    local = lambda: a.host.run("curl", "-sS", "--max-time", "6", "http://127.0.0.1:9000/", check=False)  # noqa: E731
    seen = eventually(local, lambda r: r.returncode == 0)
    t.check("a 的本地端口通到了 c 回环上的服务", seen.stdout == "127.0.0.1", f"{seen.stdout!r} {seen.stderr.strip()}")

    controller.ctl("forward", "add", str(c.id), f"{c.virtual_ip}:9100", str(c.id), "127.0.0.1:8081")
    seen = eventually(lambda: socks(a, f"http://{c.virtual_ip}:9100/"), lambda r: r.returncode == 0)
    t.check("监听在 c 的虚拟 IP 上的转发，组网里的节点可以访问", seen.stdout == "127.0.0.1", f"{seen.stdout!r} {seen.stderr.strip()}")
    listening = c.host.run("ss", "-lnt").stdout
    t.check("它不占用 c 的操作系统端口", ":9100 " not in listening)
    other = socks(a, f"http://{c.virtual_ip}:8081/")
    t.check("没有配转发的回环端口仍然到不了", other.returncode != 0)

    forwards = controller.ctl("forward", "list")
    controller.ctl("forward", "delete", str(forwards[0]["id"]))
    gone = eventually(local, lambda r: r.returncode != 0)
    t.check("删掉转发之后本地端口不再可用", gone.returncode != 0)
    if t.failures:
        print(tail(a.read_log()), tail(c.read_log()), sep="\n  ---\n")


def lan_behind_exit(lab: Lab, work: Path, t: Checks) -> None:
    """经出口访问它所在的局域网：要出口打开"允许访问本机局域网"这个开关。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    home = lab.nat("home", NatKind.CONE)
    printer = home.host("printer")
    start_whoami(printer, 80)
    a, b = Node(lab.host("a"), work), Node(home.host("gw"), work, declare=("exit",))
    for node in (a, b):
        node.join(controller)
    url = f"http://{printer.ip}/"

    controller.ctl("exit", "enable", str(b.id))
    wait_until(lambda: controller.node(b)["exit"] is not None, what="出口启用")
    # 等出口的配置到了两个节点上，否则"到不了"可能只是因为配置还没到。
    wait_until(lambda: "出口" in b.read_log() or controller.node(b)["acked_version"] == controller.node(b)["netmap_version"], what="出口的配置下发")
    t.check("没开局域网开关时到不了出口背后的局域网", socks(a, url, user=b.virtual_ip).returncode != 0)
    controller.ctl("exit", "enable", str(b.id), "--lan")
    seen = eventually(lambda: socks(a, url, user=b.virtual_ip), lambda r: r.returncode == 0)
    t.check("打开开关之后可以访问，局域网里的机器看到的来源是出口在局域网里的地址", seen.stdout == b.host.ip, f"{seen.stdout!r} {seen.stderr.strip()}")
    piped = connect(a, printer.ip, 80, via=b.virtual_ip)
    t.check("i3t connect --via 经出口连到它局域网里的机器", piped.returncode == 0 and piped.stdout.endswith(b.host.ip),
            f"{piped.returncode} {piped.stdout!r} {piped.stderr.strip()}")
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), sep="\n  ---\n")


def public_listener(lab: Lab, work: Path, t: Checks) -> None:
    """对外开放的代理入口：必须带凭据；用户名仍然指定出口。"""
    web = lab.host("web")
    start_whoami(web)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    gw, ex = Node(lab.host("gw"), work), Node(lab.host("ex"), work, declare=("exit",))
    for node in (gw, ex):
        node.join(controller)
    controller.ctl("exit", "enable", str(ex.id))
    outsider = lab.host("outsider")
    credential = controller.ctl("credential", "create", "phone")["token"]
    controller.ctl("listener", "add", str(gw.id), "public", "0.0.0.0:3128", "--http", "--require-credential")
    url = f"http://{web.ip}/"

    def via(password: str | None):
        auth = f"{ex.virtual_ip}:{password}@" if password is not None else ""
        return outsider.run("curl", "-sS", "--max-time", "8", "-p", "-x", f"http://{auth}{gw.host.ip}:3128", "-o", "/dev/null",
                            "-w", "%{http_connect} %{http_code}", url, check=False)

    wait_until(lambda: ":3128 " in gw.host.run("ss", "-lnt").stdout, timeout=20, what="入口开始监听")
    t.check("不带凭据：407", via(None).stdout.startswith("407"), via(None).stdout)
    t.check("凭据不对：407", via("wrong").stdout.startswith("407"), via("wrong").stdout)
    good = eventually(lambda: via(credential), lambda r: r.stdout == "200 200")
    t.check("凭据正确：经指定的出口访问成功", good.stdout == "200 200", f"{good.stdout!r} {good.stderr.strip()}")
    seen = outsider.run("curl", "-sS", "--max-time", "8", "-p", "-x", f"http://{ex.virtual_ip}:{credential}@{gw.host.ip}:3128", url, check=False)
    t.check("目标看到的来源是出口的地址", seen.stdout == ex.host.ip, f"{seen.stdout!r}")
    if t.failures:
        print(tail(gw.read_log()), tail(ex.read_log()), sep="\n  ---\n")


def groups(lab: Lab, work: Path, t: Checks) -> None:
    """分组决定访问：加入分组就得到规则给它的访问，移出时正在进行的连接断开；删掉分组连同它的规则和访问；
    令牌预设分组；命令行上写了不存在的分组被拒绝，并且说出是哪一个。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    create_groups(controller, "admins", "servers")
    for rule in controller.ctl("rule", "list"):
        controller.ctl("rule", "delete", str(rule["id"]))
    controller.ctl("rule", "add", "admins", "servers", "--one-way", "--name", "admin")
    a, b, c = Node(lab.host("a"), work), Node(lab.host("b"), work), Node(lab.host("c"), work)
    a.join(controller)
    b.join(controller, "servers")
    c.join(controller)
    t.check("令牌预设的分组：b 一加入就在 servers 里", group_names(controller, b) == ["servers"], str(group_names(controller, b)))
    for node in (a, b, c):
        start_whoami(node.host, 8080)
    to_b = f"http://{b.virtual_ip}:8080/"
    t.check("a 不在 admins 里时到不了 b", socks(a, to_b).returncode != 0)

    unknown = controller.ctl("group", "add", "nope", str(a.id), check=False)
    t.check("命令行上写了不存在的分组被拒绝，说出是哪一个", unknown.returncode != 0 and "nope" in unknown.stderr, unknown.stderr.strip())
    unknown = controller.ctl("node", "groups", str(a.id), "admins", "ghosts", check=False)
    t.check("一串分组里有一个不存在也被拒绝，说出是哪一个", unknown.returncode != 0 and "ghosts" in unknown.stderr, unknown.stderr.strip())
    t.check("被拒绝的请求什么也没改", group_names(controller, a) == [], str(group_names(controller, a)))

    controller.ctl("group", "add", "admins", str(a.id))
    seen = eventually(lambda: socks(a, to_b), lambda r: r.returncode == 0)
    t.check("a 加入 admins 之后可以访问 b", seen.stdout == b.host.ip, f"{seen.stdout!r} {seen.stderr.strip()}")
    t.check("规则是单向的：b 还是访问不了 a", socks(b, f"http://{a.virtual_ip}:8080/").returncode != 0)
    t.check("c 不在 admins 里，照旧到不了 b", socks(c, to_b).returncode != 0)
    t.check("a 也没有因此能访问 c", socks(a, f"http://{c.virtual_ip}:8080/").returncode != 0)

    # 一次限速的下载，持续三十秒左右；下载途中把 a 移出 admins。
    size = 30_000_000
    download = subprocess.Popen(
        a.host.cmd("curl", "-sS", "--max-time", "90", "--limit-rate", "1M", "-o", "/dev/null", "-w", "%{size_download}",
                   "-x", "socks5h://127.0.0.1:1080", f"{to_b}bytes/{size}"),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(3)
    t.check("下载开始了", download.poll() is None)
    controller.ctl("group", "remove", "admins", str(a.id))
    try:
        out, err = download.communicate(timeout=20)
        ended = out != str(size)
        detail = f"下载了 {out!r}；{err.strip()}"
    except subprocess.TimeoutExpired:
        download.kill()
        ended, detail = False, "移出之后 20 秒连接还在"
    t.check("a 移出 admins 之后，正在进行的连接断开", ended, detail)
    gone = eventually(lambda: socks(a, to_b), lambda r: r.returncode != 0)
    t.check("之后也连不上了", gone.returncode != 0)

    controller.ctl("group", "add", "admins", str(a.id))
    back = eventually(lambda: socks(a, to_b), lambda r: r.returncode == 0)
    t.check("重新加入之后又能访问", back.returncode == 0, back.stderr.strip())
    deleted = controller.ctl("group", "delete", "admins")
    t.check("删掉分组的应答说出删掉了哪条规则和哪些成员",
            [rule["name"] for rule in deleted["rules"]] == ["admin"] and [node["id"] for node in deleted["nodes"]] == [a.id], str(deleted))
    t.check("用到它的规则没了", controller.ctl("rule", "list") == [])
    t.check("a 不再在任何分组里", group_names(controller, a) == [], str(group_names(controller, a)))
    after = eventually(lambda: socks(a, to_b), lambda r: r.returncode != 0)
    t.check("删掉分组之后 a 到不了 b", after.returncode != 0)
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), tail(controller.log.read_text(errors="replace")), sep="\n  ---\n")


SCENARIOS = {
    "node_service": node_service,
    "port_forward": port_forward,
    "lan_behind_exit": lan_behind_exit,
    "public_listener": public_listener,
    "groups": groups,
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
