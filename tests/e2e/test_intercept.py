#!/usr/bin/env python3
"""透明访问（Linux）：本机的程序不配代理，直接连别的节点的虚拟 IP 或者名字。

    python3 tests/e2e/test_intercept.py [场景名...]

实验环境的命名空间里没有特权，但包过滤规则、路由规则和网卡在自己的网络命名空间里都改得了，节点就在这里把它们装上、拆掉。
客户端都是不改的程序（curl、Python 的套接字），不经任何代理。

- access：按虚拟 IP 和按名字连另一个节点上的服务，服务看到的是发起节点的虚拟 IP；不是节点的地址、访问规则不允许的节点、
  没开的端口当场失败；先说话的服务（像 SSH）；UDP；状态命令；节点停下之后系统和装之前一模一样。
- leftover：节点被杀掉（来不及拆）之后再启动，留下的东西被认出来、换掉，不多出一份；`i3t set intercept off` 在节点没运行时
  把留下的拆掉。
- reroute：一条被拦截的连接开着的时候，两个节点之间的路从直连换到 relay，连接不断。
- managed：节点把状态报给 controller，管理接口的节点详情里有它；`i3t hosts` 写出名字，`i3t hosts --remove` 再去掉。
- overlap：这台机器自己在一个和覆盖网的地址段重叠的内网里：什么都不装，状态说出重叠的是哪个地址，那个内网照常通。
- appears：透明访问开着的时候，另一块网卡上多了一个地址段里的地址：几秒之内全部拆掉，那个网络照常通；地址去掉之后再装上。
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
import lab as lablib  # noqa: E402
from harness import (  # noqa: E402
    TICKER_DIRECT, Checks, Controller, Node, binary, blackout_between, current_route, dial, intercept_on, leaked, private_hosts_file,
    settled_snapshot, snapshot_difference, start_banner, start_ticker, start_udp_echo, start_whoami, system_snapshot, tail, udp_dial, use_nameserver, watch_leaks,
)
from lab import Lab, private_link, wait_until  # noqa: E402

# 一条被拒绝的连接最多多久就该失败：没有往返（当场被拒）的不到一秒，要问对端的（端口没开）再加首批数据的等待和一个往返。
REFUSED_FAST = 1.0
RESET_FAST = 2.0


def wait_open(node: Node) -> None:
    wait_until(lambda: "透明访问已打开" in node.read_log() or "透明访问打不开" in node.read_log(), what=f"{node.name} 的透明访问装好")
    if "透明访问打不开" in node.read_log():
        raise RuntimeError(f"{node.name} 的透明访问打不开：\n{tail(node.read_log())}")


def i3t(node: Node, *args: str) -> subprocess.CompletedProcess:
    return node.host.run(binary("i3t"), "--state-dir", str(node.state), *args, check=False)


def access(lab: Lab, work: Path, t: Checks) -> None:
    """两个公网节点直连，都打开透明访问；a 的程序直接连 b。"""
    a, b, c = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2), Node(lab.host("c"), work, 3)
    intercept_on(a)
    intercept_on(b)
    before = settled_snapshot(a.host)
    start_whoami(b.host, 8080)
    start_banner(b.host, 2222, "SSH-2.0-banner")
    start_udp_echo(b.host, 5353)
    # c 在 a 的 netmap 里，但访问规则不允许 a 访问它（只能它访问 a）。c 没有在运行。
    a.start_static(a.netmap(peers=[b.as_peer(), c.as_peer(outbound=False)]))
    b.start_static(b.netmap(peers=[a.as_peer()]))
    wait_open(a)
    wait_open(b)

    seen = a.host.run("curl", "-sS", "--max-time", "10", "http://10.66.0.2:8080/", check=False)
    t.check("按虚拟 IP：curl 不配代理连到 b 上的服务", seen.returncode == 0, seen.stderr.strip())
    t.check("b 上的服务看到的来源是 a 的虚拟 IP", seen.stdout == a.virtual_ip, f"看到 {seen.stdout!r}")
    banner = dial(a.host, b.virtual_ip, 2222)
    t.check("先说话的服务（像 SSH）：连上就收到它的横幅，看到的来源也是 a 的虚拟 IP",
            banner.get("data") == f"SSH-2.0-banner {a.virtual_ip}\r\n", str(banner))

    use_nameserver(a.virtual_ip)
    answer = a.host.run("dig", "+short", "+time=2", "+tries=1", f"@{a.virtual_ip}", "b.lab.i3t", check=False).stdout.strip()
    t.check("名字服务：b.lab.i3t 是 b 的虚拟 IP", answer == b.virtual_ip, answer)
    refused = a.host.run("dig", "+time=2", "+tries=1", f"@{a.virtual_ip}", "example.com", check=False).stdout
    t.check("名字服务只答网络自己的域名，别的名字回 REFUSED", "status: REFUSED" in refused, refused[-300:])
    hidden = a.host.run("dig", "+time=2", "+tries=1", f"@{a.virtual_ip}", "c.lab.i3t", check=False).stdout
    t.check("访问规则不允许的节点的名字不答（NXDOMAIN）", "status: NXDOMAIN" in hidden, hidden[-300:])
    by_name = a.host.run("curl", "-sS", "--max-time", "10", "http://b.lab.i3t:8080/", check=False)
    t.check("按名字：curl http://b.lab.i3t:8080/ 不配代理也能用", by_name.stdout == a.virtual_ip, f"{by_name.stdout!r} {by_name.stderr.strip()}")

    nobody = dial(a.host, "10.66.0.9", 8080)
    t.check("不是节点的地址：当场被拒绝", nobody.get("error") == "ConnectionRefusedError" and nobody["seconds"] < REFUSED_FAST, str(nobody))
    forbidden = dial(a.host, c.virtual_ip, 8080)
    t.check("访问规则不允许的节点：当场被拒绝", forbidden.get("error") == "ConnectionRefusedError" and forbidden["seconds"] < REFUSED_FAST,
            str(forbidden))
    closed = dial(a.host, b.virtual_ip, 8999, send="GET / HTTP/1.0\r\n\r\n")
    t.check("对端没开的端口：很快被重置，不会挂住",
            closed.get("error") == "ConnectionResetError" and not closed.get("data") and closed["seconds"] < RESET_FAST, str(closed))
    curl_closed = a.host.run("curl", "-sS", "--max-time", "10", "http://10.66.0.2:8999/", check=False)
    t.check("curl 连没开的端口：报连接被重置，不是超时", curl_closed.returncode in (52, 56) and "reset" in curl_closed.stderr.lower(),
            f"{curl_closed.returncode} {curl_closed.stderr.strip()}")
    own = dial(a.host, a.virtual_ip, 8080, limit=3)
    t.check("自己的虚拟 IP 不被拦截（本机上没有这个服务：被拒绝）", own.get("error") == "ConnectionRefusedError", str(own))

    echoed = udp_dial(a.host, b.virtual_ip, 5353, b"ping")
    data = bytes.fromhex(echoed.get("data", ""))
    t.check("UDP：报文到了 b 上的服务，回答回来了", data.endswith(b"|ping"), str(echoed))
    t.check("UDP：回答的来源是 b 的虚拟 IP 和那个端口", echoed.get("source") == f"{b.virtual_ip}:5353", str(echoed))
    nowhere = udp_dial(a.host, "10.66.0.9", 5353, b"ping", tries=1)
    t.check("UDP 发往不是节点的地址：当场失败", nowhere.get("error") == "PermissionError" and nowhere["seconds"] < REFUSED_FAST, str(nowhere))

    # 别的协议（ping）发往地址段：当场被拒绝，一个包也不从上网的网卡出去。计数的表装在 before 的快照之后，最后快照之前拆掉。
    watch_leaks(a.host)
    for target in (b.virtual_ip, "10.66.0.9"):
        started = time.monotonic()
        pinged = a.host.run("ping", "-c", "1", "-W", "3", target, check=False)
        took = time.monotonic() - started
        t.check(f"ping {target}：当场失败，不是等超时", pinged.returncode != 0 and took < REFUSED_FAST, f"{took:.2f}s {pinged.stdout.strip()[-200:]}")
    t.check("发往地址段的包一个也没有从 up0 出去", leaked(a.host) == 0, str(leaked(a.host)))
    watch_leaks(a.host, False)

    status = i3t(a, "intercept", "status")
    listed = status.stdout
    t.check("i3t intercept status 列出装了什么",
            all(part in listed for part in ("开着", "table ip i3t", "2033: from all fwmark 0x20000000/0x20000000 lookup 2033 proto 203",
                                            "local default dev lo table 2033 proto 203", "i3t0 10.66.0.1/32")), listed)
    overall = i3t(a, "status").stdout
    t.check("i3t status 里有透明访问的状态和连接数", "透明访问" in overall and "开着" in overall, overall[-600:])

    a.stop()
    after = system_snapshot(a.host)
    t.check("节点停下之后，包过滤规则、路由、网卡和地址和装之前一模一样", after == before, snapshot_difference(before, after))
    gone = a.host.run("curl", "-sS", "--max-time", "3", "http://10.66.0.2:8080/", check=False)
    t.check("拆掉之后不再拦截", gone.returncode != 0)
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log()), sep="\n  ---\n")


def leftover(lab: Lab, work: Path, t: Checks) -> None:
    """节点被杀掉之后留下的东西：下次启动时换掉，节点没运行时关掉透明访问就拆掉。"""
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2)
    intercept_on(a)
    before = settled_snapshot(a.host)
    start_whoami(b.host, 8080)
    netmap = a.netmap(peers=[b.as_peer()])
    b.start_static(b.netmap(peers=[a.as_peer()]))
    a.start_static(netmap)
    wait_open(a)

    a.host.stop(a.proc, signal.SIGKILL)
    a.proc = None
    left = system_snapshot(a.host)
    t.check("被杀掉的节点来不及拆：表、规则、路由和网卡都还在",
            "table ip i3t" in left["nft"] and "lookup 2033" in left["rules"] and "i3t0" in left["links"], snapshot_difference(before, left))
    status = i3t(a, "intercept", "status").stdout
    t.check("i3t intercept status 说这些是上次留下的", "上次留下的" in status, status)

    a.log.write_text("")
    a.start_static(netmap)
    wait_open(a)
    t.check("再启动时认出了上次留下的", "上次留下的透明访问的规则" in a.read_log(), tail(a.read_log()))
    again = system_snapshot(a.host)
    counts = (again["nft"].count("table ip i3t"), again["rules"].count("2033:"), again["routes"].count("table 2033"),
              again["links"].count(" i3t0:"))
    t.check("换掉而不是多出一份：一张表、一条规则、一条路由、一块网卡", counts == (1, 1, 1, 1), str(counts))
    seen = a.host.run("curl", "-sS", "--max-time", "10", "http://10.66.0.2:8080/", check=False)
    # b 没有打开透明访问：它照旧用本机自己的地址去连服务。
    t.check("换掉之后照常可用；目标一侧没打开时服务看到的是目标自己的地址，和以前一样", seen.stdout == b.host.ip,
            f"{seen.stdout!r} {seen.stderr.strip()}")

    a.host.stop(a.proc, signal.SIGKILL)
    a.proc = None
    switched = i3t(a, "set", "intercept", "off")
    t.check("节点没运行时 i3t set intercept off 拆掉留下的", "拆掉了上次留下的" in switched.stdout, switched.stdout + switched.stderr)
    after = system_snapshot(a.host)
    t.check("拆掉之后和装之前一模一样", after == before, snapshot_difference(before, after))
    a.start_static(netmap)
    time.sleep(1)
    t.check("关着启动：什么都不装", system_snapshot(a.host) == before, snapshot_difference(before, system_snapshot(a.host)))
    if t.failures:
        print(tail(a.read_log(), 30))


def reroute(lab: Lab, work: Path, t: Checks) -> None:
    """a 经透明访问连着 b 上一个一直在发消息的服务；中途 a 和 b 之间的直连断掉，路换到 relay，连接不断。"""
    r = Node(lab.host("r"), work, 9, declare=("relay",))
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2)
    intercept_on(a)
    start_ticker(b.host, 9000)
    r.start_static(r.netmap(me={"relay": {}}, relays=[r.as_relay()], relay_clients=[a, b, r]))
    a.start_static(a.netmap(peers=[b.as_peer(attached=[(r, 5)])], relays=[r.as_relay()]))
    b.start_static(b.netmap(peers=[a.as_peer(attached=[(r, 5)])], relays=[r.as_relay()]))
    wait_open(a)

    seconds = 25
    client = subprocess.Popen(a.host.cmd(sys.executable, "-c", TICKER_DIRECT, b.virtual_ip, "9000", str(seconds)),
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    # 连接是按需建的：第一条流开起来之后才有路。
    wait_until(lambda: current_route(a, b).startswith("直连"), timeout=20, what="a 到 b 先走直连")
    time.sleep(3)
    blackout_between(a.host, b.host, None, True)
    try:
        wait_until(lambda: current_route(a, b).startswith("经 relay"), timeout=18, what="a 到 b 换到 relay")
        switched = True
    except TimeoutError:
        switched = False
    out, err = client.communicate(timeout=seconds + 30)
    blackout_between(a.host, b.host, None, False)
    result = json.loads(out) if out.strip() else {"ended": err.strip()[-300:], "gaps": [], "bytes": 0}
    t.check("直连断掉之后路换到了 relay", switched, current_route(a, b))
    t.check("被拦截的连接没有断，一直收到消息", result["ended"] is None and result["bytes"] > 64 * 100 * (seconds - 12), str(result))
    if t.failures:
        print(tail(a.read_log(), 30), tail(b.read_log()), sep="\n  ---\n")


def managed(lab: Lab, work: Path, t: Checks) -> None:
    """由 controller 管理的两个节点：a 打开透明访问；状态出现在管理接口的节点详情里；i3t hosts 写出名字。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab", "--cidr", "10.66.0.0/16")
    a, b = Node(lab.host("a"), work), Node(lab.host("b"), work)
    intercept_on(a)
    a.join(controller)
    b.join(controller)
    start_whoami(b.host, 8080)
    wait_open(a)

    def detail(node: Node) -> dict:
        return controller.ctl("node", "show", str(node.id))

    wait_until(lambda: (detail(a).get("intercept") or {}).get("state") == "on", timeout=30, what="a 把透明访问的状态报给 controller")
    reported = detail(a)["intercept"]
    t.check("节点详情：a 的透明访问开着，TCP 和 UDP，服务看到对端的虚拟 IP",
            reported.get("udp") is True and reported.get("source_address") is True, str(reported))
    t.check("节点详情：这里没有 systemd-resolved，名字靠 i3t hosts", reported.get("names") == {"state": "no_resolver"}, str(reported))
    wait_until(lambda: (detail(b).get("intercept") or {}).get("state") == "off", timeout=30, what="b 报透明访问关着")
    shown = controller.host.run(binary("i3tctl"), "--token", controller.token, "node", "show", str(a.id), check=False).stdout
    t.check("i3tctl node show 写出透明访问的状态", "透明访问" in shown and "开着" in shown, shown[-500:])

    seen = a.host.run("curl", "-sS", "--max-time", "10", f"http://{b.virtual_ip}:8080/", check=False)
    # b 没有打开透明访问：它照旧用本机自己的地址去连服务。
    t.check("由 controller 管理的节点之间也能直接连", seen.stdout == b.host.ip, f"{seen.stdout!r} {seen.stderr.strip()}")
    wait_until(lambda: (detail(a).get("intercept") or {}).get("down_bytes", 0) > 0, timeout=30, what="a 报上透明访问的流量")

    printed = i3t(a, "hosts").stdout
    t.check("i3t hosts 写出网络里的名字", f"{b.virtual_ip}\tb.lab.i3t" in printed and f"{a.virtual_ip}\ta.lab.i3t" in printed, printed)
    hosts = private_hosts_file()
    original = hosts.read_text()
    written = i3t(a, "hosts", "--apply")
    t.check("i3t hosts --apply 写进 /etc/hosts，原来的行留着",
            written.returncode == 0 and hosts.read_text().startswith(original) and "b.lab.i3t" in hosts.read_text(), written.stderr)
    i3t(a, "hosts", "--apply")
    t.check("再写一次只换那一段", hosts.read_text().count("b.lab.i3t") == 1, hosts.read_text())
    by_name = a.host.run("curl", "-sS", "--max-time", "10", "http://b.lab.i3t:8080/", check=False)
    t.check("靠 /etc/hosts 按名字连", by_name.stdout == b.host.ip, f"{by_name.stdout!r} {by_name.stderr.strip()}")

    # 透明访问关着、那一段还在：b 没打开透明访问，它的状态命令提醒；a 开着，不提醒。
    hint = "i3t hosts --remove"
    t.check("透明访问关着的节点：i3t intercept status 说 /etc/hosts 里还有名字", hint in i3t(b, "intercept", "status").stdout)
    t.check("透明访问开着的节点不提醒", hint not in i3t(a, "intercept", "status").stdout)
    switched = i3t(a, "set", "intercept", "off")
    t.check("i3t set intercept off 也提醒", hint in switched.stdout, switched.stdout + switched.stderr)
    removed = i3t(a, "hosts", "--remove")
    t.check("i3t hosts --remove 只去掉那一段，文件和写之前一个字节也不差",
            removed.returncode == 0 and hosts.read_text() == original, removed.stdout + removed.stderr)
    again = i3t(a, "hosts", "--remove")
    t.check("再去掉一次：说没有，文件不动", again.returncode == 0 and "没有" in again.stdout and hosts.read_text() == original, again.stdout)
    t.check("去掉之后不再提醒", hint not in i3t(b, "intercept", "status").stdout)
    if t.failures:
        print(tail(a.read_log(), 30), tail(controller.log.read_text(errors="replace")), sep="\n  ---\n")


def overlap(lab: Lab, work: Path, t: Checks) -> None:
    """a 自己就在一个和覆盖网的地址段重叠的内网里（10.66.200.0/24）：打开透明访问也什么都不装，说清楚为什么，别的照常。"""
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2)
    lan = private_link(lab, a.host, "lan", "10.66.200.0/24")
    intercept_on(a)
    start_whoami(lan, 8080)
    start_whoami(b.host, 8080)
    before = settled_snapshot(a.host)
    a.start_static(a.netmap(peers=[b.as_peer()]))
    b.start_static(b.netmap(peers=[a.as_peer()]))
    wait_until(lambda: "透明访问打不开" in a.read_log() or "透明访问已打开" in a.read_log(), what="a 的透明访问有了结果")

    t.check("地址段和本机的一个真实网络重叠：不装", "透明访问打不开" in a.read_log(), tail(a.read_log()))
    after = system_snapshot(a.host)
    t.check("包过滤规则、路由、网卡和地址一样也没动", after == before, snapshot_difference(before, after))
    status = i3t(a, "intercept", "status").stdout
    t.check("i3t intercept status 说出重叠的是哪个地址", "开不起来" in status and "10.66.200.1/24 dev p-lan" in status, status)
    overall = i3t(a, "status").stdout
    t.check("i3t status 里也有", "网络的地址段和本机的 10.66.200.1/24 dev p-lan 重叠" in overall, overall[-500:])
    real = a.host.run("curl", "-sS", "--max-time", "10", "http://10.66.200.2:8080/", check=False)
    t.check("发往那个真实网络的流量照常", real.stdout == "10.66.200.1", f"{real.stdout!r} {real.stderr.strip()}")
    pinged = a.host.run("ping", "-c", "1", "-W", "3", "10.66.200.2", check=False)
    t.check("ping 那个真实网络里的机器照常", pinged.returncode == 0, pinged.stdout.strip()[-200:])
    proxied = a.curl(f"http://{b.virtual_ip}:8080/")
    t.check("节点别的功能照常：经本机入口访问 b", proxied.stdout == b.host.ip, f"{proxied.stdout!r} {proxied.stderr.strip()}")
    # 对照：没有透明访问时，发往地址段的 ping 照旧从上网的网卡出去（access 里数到 0 才说明问题）。
    watch_leaks(a.host)
    a.host.run("ping", "-c", "1", "-W", "1", "10.66.0.9", check=False)
    t.check("对照：没装的时候发往地址段的包从 up0 出去，计数数得到", leaked(a.host) >= 1, str(leaked(a.host)))
    watch_leaks(a.host, False)
    if t.failures:
        print(tail(a.read_log(), 30))


def appears(lab: Lab, work: Path, t: Checks) -> None:
    """透明访问开着的时候，a 的另一块网卡上多了一个地址段里的地址（DHCP 换了地址、VPN 连上）：当场拆掉，地址去掉之后再装上。"""
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2)
    # 先是一个和地址段无关的内网，后来它换成了 10.66.200.0/24。
    lan = private_link(lab, a.host, "lan", "192.168.77.0/24")
    intercept_on(a)
    start_whoami(lan, 8080)
    start_whoami(b.host, 8080)
    a.start_static(a.netmap(peers=[b.as_peer()]))
    b.start_static(b.netmap(peers=[a.as_peer()]))
    wait_open(a)
    # b 没有打开透明访问：它照旧用本机自己的地址去连服务。
    seen = a.host.run("curl", "-sS", "--max-time", "10", f"http://{b.virtual_ip}:8080/", check=False)
    t.check("开始时透明访问照常可用", seen.stdout == b.host.ip, f"{seen.stdout!r} {seen.stderr.strip()}")

    def installed() -> bool:
        return "table ip i3t" in a.host.run("nft", "list", "ruleset").stdout

    a.host.run("ip", "addr", "add", "10.66.200.1/24", "dev", "p-lan")
    lan.run("ip", "addr", "add", "10.66.200.2/24", "dev", "up0")
    started = time.monotonic()
    wait_until(lambda: not installed(), timeout=20, what="a 拆掉透明访问")
    took = time.monotonic() - started
    t.check("多了一个地址段里的地址：几秒之内全部拆掉", took < 10, f"{took:.1f}s")
    t.check("日志说出为什么拆", "重叠了，透明访问当场拆掉" in a.read_log(), tail(a.read_log()))
    status = i3t(a, "intercept", "status").stdout
    t.check("i3t intercept status 说出重叠的是哪个地址", "开不起来" in status and "10.66.200.1/24 dev p-lan" in status, status)
    real = a.host.run("curl", "-sS", "--max-time", "10", "http://10.66.200.2:8080/", check=False)
    t.check("那个网络照常通", real.stdout == "10.66.200.1", f"{real.stdout!r} {real.stderr.strip()}")
    proxied = a.curl(f"http://{b.virtual_ip}:8080/")
    t.check("节点别的功能照常：经本机入口访问 b", proxied.stdout == b.host.ip, f"{proxied.stdout!r} {proxied.stderr.strip()}")

    a.host.run("ip", "addr", "del", "10.66.200.1/24", "dev", "p-lan")
    started = time.monotonic()
    wait_until(installed, timeout=20, what="a 再装上透明访问")
    took = time.monotonic() - started
    t.check("地址去掉之后几秒之内再装上", took < 10 and "透明访问再装上" in a.read_log(), f"{took:.1f}s")
    wait_until(lambda: "开着" in i3t(a, "intercept", "status").stdout, timeout=5, what="状态变回开着")
    again = a.host.run("curl", "-sS", "--max-time", "10", f"http://{b.virtual_ip}:8080/", check=False)
    t.check("再装上之后透明访问照常可用", again.stdout == b.host.ip, f"{again.stdout!r} {again.stderr.strip()}")
    if t.failures:
        print(tail(a.read_log(), 30))


SCENARIOS = {"access": access, "leftover": leftover, "reroute": reroute, "managed": managed, "overlap": overlap, "appears": appears}


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
