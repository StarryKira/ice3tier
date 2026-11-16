#!/usr/bin/env python3
"""检查实验环境本身：连通性、两种 NAT 的行为、禁 UDP、端口转发。

它不测 ice3tier，只确认 `lab.py` 搭出来的网络是我们以为的样子。产品的测试都建立在这上面，
所以环境有问题要先在这里暴露出来。

    python3 tests/e2e/check_lab.py
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from lab import Lab, NatKind  # noqa: E402

# 向两个不同的公网地址各发一个 UDP 包，打印对方看到的来源端口；用来看映射出来的端口是否随目的地变化。
PROBE = r"""
import socket, sys
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("0.0.0.0", 40000))
sock.settimeout(3)
ports = []
for target in sys.argv[1:]:
    sock.sendto(b"probe", (target, 9000))
    data, _ = sock.recvfrom(64)
    ports.append(data.decode())
print(" ".join(ports))
"""

# 把来源端口原样发回去。
REFLECT = r"""
import socket
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("0.0.0.0", 9000))
while True:
    data, addr = sock.recvfrom(64)
    sock.sendto(str(addr[1]).encode(), addr)
"""


def main() -> int:
    lablib.enter()
    failures = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        print(f"{'通过' if ok else '失败'}  {name}{'：' + detail if detail and not ok else ''}")
        if not ok:
            failures.append(name)

    with Lab() as lab:
        s1, s2 = lab.host("s1"), lab.host("s2")
        cone, sym = lab.nat("cone", NatKind.CONE), lab.nat("sym", NatKind.SYMMETRIC)
        a, a2 = cone.host("a"), cone.host("a2")
        b = sym.host("b")

        check("公网主机之间互通", s1.run("ping", "-c1", "-W1", s2.ip, check=False).returncode == 0)
        check("内网主机能访问公网", a.run("ping", "-c1", "-W1", s1.ip, check=False).returncode == 0)
        check("同一个局域网里的主机互通", a.run("ping", "-c1", "-W1", a2.ip, check=False).returncode == 0)
        check("公网主机访问不到内网地址", s1.run("ping", "-c1", "-W1", a.ip, check=False).returncode != 0)

        for server in (s1, s2):
            server.spawn(sys.executable, "-c", REFLECT)
        lablib.wait_until(lambda: s1.run("ss", "-lun").stdout.count(":9000") and s2.run("ss", "-lun").stdout.count(":9000"),
                          what="反射服务启动")

        seen = a.run(sys.executable, "-c", PROBE, s1.ip, s2.ip, check=False).stdout.split()
        check("CONE：发往不同目的地时映射出来的端口相同", len(seen) == 2 and seen[0] == seen[1], f"看到的端口 {seen}")
        seen = b.run(sys.executable, "-c", PROBE, s1.ip, s2.ip, check=False).stdout.split()
        check("SYMMETRIC：发往不同目的地时映射出来的端口不同", len(seen) == 2 and seen[0] != seen[1], f"看到的端口 {seen}")

        a.block_udp()
        blocked = a.run(sys.executable, "-c", PROBE, s1.ip, check=False)
        check("禁 UDP 之后 UDP 不通", blocked.returncode != 0)
        check("禁 UDP 之后 TCP / ICMP 仍然通", a.run("ping", "-c1", "-W1", s1.ip, check=False).returncode == 0)
        a.block_udp(False)
        check("解除之后 UDP 恢复", a.run(sys.executable, "-c", PROBE, s1.ip, check=False).returncode == 0)

        a.spawn(sys.executable, "-c", REFLECT)
        lablib.wait_until(lambda: a.run("ss", "-lun").stdout.count(":9000"), what="内网的反射服务启动")
        unsolicited = s2.run(sys.executable, "-c", PROBE.replace("40000", "40001"), cone.ip, check=False)
        check("没有端口转发时，公网连不到 NAT 后面的服务", unsolicited.returncode != 0)
        cone.forward_port("udp", 9000, a)
        forwarded = s2.run(sys.executable, "-c", PROBE.replace("40000", "40002"), cone.ip, check=False)
        check("做了端口转发之后连得到", forwarded.returncode == 0, forwarded.stderr[-200:])

        s1.shape(lablib.Shape(rtt_ms=80))
        ping = a.run("ping", "-c3", "-i0.2", "-W2", s1.ip, check=False).stdout
        rtt = float(ping.rsplit("=", 1)[-1].split("/")[1]) if "rtt" in ping else -1
        check("加延迟之后往返时间跟着变", 70 <= rtt <= 110, f"测到 {rtt}ms")

    print("全部通过" if not failures else f"{len(failures)} 项失败")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
