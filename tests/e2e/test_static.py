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
from harness import Checks, Node, start_whoami, tail  # noqa: E402
from lab import Lab, NatKind  # noqa: E402


def direct_exit(lab: Lab, work: Path, t: Checks) -> None:
    """两个公网节点直连；a 用代理的用户名指定从 b 出去。"""
    web = lab.host("web")
    start_whoami(web)
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2, declare=("exit",))
    a.start_static(a.netmap(peers=[b.as_peer(exit=True)]))
    b.start_static(b.netmap(peers=[a.as_peer()], me={"exit": {}}))

    seen = a.curl(f"http://{web.ip}/", via=b)
    t.check("SOCKS5：用户名写 b，目标看到的来源是 b 的地址", seen.stdout == b.host.ip, f"看到 {seen.stdout!r} {seen.stderr.strip()}")
    seen = a.curl(f"http://{web.ip}/", via=b, proxy="http")
    t.check("HTTP CONNECT：同上", seen.stdout == b.host.ip, f"看到 {seen.stdout!r} {seen.stderr.strip()}")
    seen = a.curl(f"http://{web.ip}/")
    t.check("不写用户名：从本机出去", seen.stdout == a.host.ip, f"看到 {seen.stdout!r} {seen.stderr.strip()}")
    big = a.curl(f"http://{web.ip}/bytes/20000000", via=b, extra=("-o", "/dev/null", "-w", "%{size_download}"))
    t.check("经出口下载 20MB，字节数对得上", big.stdout == "20000000", f"下载了 {big.stdout!r} {big.stderr.strip()}")
    refused = a.curl(f"http://{web.ip}:8080/", via=b)
    t.check("出口不放行 80 / 443 以外的端口", refused.returncode != 0)
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
    a.start_static(a.netmap(peers=[b.as_peer(endpoints=False, attached=[(r, 5)], exit=True)], relays=[r.as_relay()]))
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
