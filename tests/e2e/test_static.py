#!/usr/bin/env python3
"""数据面的端到端测试：节点用静态的 netmap 运行，不需要 controller。

    python3 tests/e2e/test_static.py [场景名...]
"""

from __future__ import annotations

import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import LOCAL_HTTP, LOCAL_SOCKS5, Checks, Node, Proxy, start_whoami, tail  # noqa: E402
from lab import Lab, NatKind  # noqa: E402


def direct_exit(lab: Lab, work: Path, t: Checks) -> None:
    """两个公网节点直连；a 上有一个经 b 出去的代理（两种协议都说），和一个对局域网开放、认用户名密码的。"""
    web = lab.host("web")
    start_whoami(web)
    start_whoami(web, 8080)
    # b 的配置文件里没有 [proxy]：节点不开任何代理端口。
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2, declare=("exit",), local_entry=False)
    guarded = a.proxy_to(b, port=3128, bind="0.0.0.0", username="alice", password="s3cret", default=False)
    a.start_static(a.netmap(peers=[b.as_peer(exit=True)], proxies=[a.proxy_to(b), guarded]))
    # 出口默认不限端口；这里写明只放行 80 和 443，测端口白名单。
    b.start_static(b.netmap(peers=[a.as_peer()], me={"exit": {"ports": [80, 443]}}))

    seen = a.curl(f"http://{web.ip}/", via=b)
    t.check("SOCKS5：经 b 出去的代理，目标看到的来源是 b 的地址", seen.stdout == b.host.ip, f"看到 {seen.stdout!r} {seen.stderr.strip()}")
    seen = a.curl(f"http://{web.ip}/", via=b, proxy="http")
    t.check("HTTP CONNECT：同一个端口，同上", seen.stdout == b.host.ip, f"看到 {seen.stdout!r} {seen.stderr.strip()}")
    seen = a.curl(f"http://{web.ip}/")
    t.check("组网入口不经出口：网络之外的目标被拒绝", seen.returncode != 0 and seen.stdout == "", f"{seen.stdout!r} {seen.stderr.strip()}")
    listening = b.host.run("ss", "-lnt").stdout
    t.check("配置文件里没有 [proxy] 的节点不开 1080 和 1081",
            f":{LOCAL_SOCKS5} " not in listening and f":{LOCAL_HTTP} " not in listening, listening)

    # 别的机器经 a 的局域网地址用那个认证的代理：对的用户名密码从 b 出去，错的、少了的都不行。
    right = Proxy(a.host.ip, 3128, "alice", "s3cret")
    for scheme, flag in [("socks5h", ()), ("http", ("-p",))]:
        seen = web.run("curl", "-sS", "--max-time", "10", *flag, "-x", right.url(scheme), f"http://{web.ip}/", check=False)
        t.check(f"别的机器用对的用户名密码（{scheme}）：从 b 出去", seen.stdout == b.host.ip, f"{seen.stdout!r} {seen.stderr.strip()}")
        for wrong in [Proxy(a.host.ip, 3128, "alice", "wrong"), Proxy(a.host.ip, 3128, "bob", "s3cret"), Proxy(a.host.ip, 3128)]:
            seen = web.run("curl", "-sS", "--max-time", "10", *flag, "-x", wrong.url(scheme), f"http://{web.ip}/", check=False)
            t.check(f"用户名密码不对或者没写（{scheme} {wrong.username}:{wrong.password}）：拒绝", seen.returncode != 0 and seen.stdout == "",
                    f"{seen.stdout!r}")
    big = a.curl(f"http://{web.ip}/bytes/20000000", via=b, extra=("-o", "/dev/null", "-w", "%{size_download}"))
    t.check("经出口下载 20MB，字节数对得上", big.stdout == "20000000", f"下载了 {big.stdout!r} {big.stderr.strip()}")
    refused = a.curl(f"http://{web.ip}:8080/", via=b)
    t.check("出口写了端口白名单：不放行 80 / 443 以外的端口", refused.returncode != 0 and refused.stdout == "", f"{refused.stdout!r}")
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), sep="\n  ---\n")


def via_relay(lab: Lab, work: Path, t: Checks) -> None:
    """a 和 b 都在对称型 NAT 后面，打不了洞，只能经公网上的 relay r。"""
    web = lab.host("web")
    start_whoami(web)
    r = Node(lab.host("r"), work, 9, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work, 1)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, 2, declare=("exit",))
    r.start_static(r.netmap(me={"relay": {}}, relays=[r.as_relay()], relay_clients=[a, b, r]))
    a.start_static(a.netmap(peers=[b.as_peer(endpoints=False, attached=[(r, 5)], exit=True)], relays=[r.as_relay()], proxies=[a.proxy_to(b)]))
    b.start_static(b.netmap(peers=[a.as_peer(endpoints=False, attached=[(r, 5)])], relays=[r.as_relay()], me={"exit": {}}))

    seen = a.curl(f"http://{web.ip}/", via=b)
    t.check("经 relay：目标看到的来源是 b 所在 NAT 的公网地址", seen.stdout == lab.hosts["nat-b"].ip,
            f"看到 {seen.stdout!r} {seen.stderr.strip()}")
    big = a.curl(f"http://{web.ip}/bytes/5000000", via=b, extra=("-o", "/dev/null", "-w", "%{size_download}"))
    t.check("经 relay 下载 5MB", big.stdout == "5000000", f"下载了 {big.stdout!r} {big.stderr.strip()}")
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), tail(r.read_log()), sep="\n  ---\n")


SCENARIOS = {"direct_exit": direct_exit, "via_relay": via_relay}


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
        # 每个场景用全新的命名空间；上一个场景的要等进程退干净才能重名。
        time.sleep(0.3)
    return t.summary()


if __name__ == "__main__":
    sys.exit(main())
