#!/usr/bin/env python3
"""UDP 的端到端测试：SOCKS5 的 UDP ASSOCIATE 经出口出去（直连和经 relay）、真的 DNS 查询、UDP 的端口转发。

节点用静态的 netmap 运行，不需要 controller。

    python3 tests/e2e/test_udp.py [场景名...]
"""

from __future__ import annotations

import shutil
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import (  # noqa: E402
    Checks, Node, dns_a_records, dns_query, socks5_udp, start_dns, start_udp_echo, tail, udp_exchange,
)
from lab import Lab, NatKind  # noqa: E402


def echoed(answer: dict | None) -> tuple[str, bytes]:
    """回显服务回来的报文：它看到的来源地址，和原样带回来的数据。没有回音时两样都是空的。"""
    if answer is None:
        return "", b""
    seen, _, data = answer["data"].partition(b"|")
    return seen.decode(), data


def last_route(node: Node, peer: Node) -> str:
    """`node` 的日志里最后一条"到 `peer` 的路变了"。路径层每次换路都记一条，里面写着走的是直连还是哪个 relay。"""
    changes = [line for line in node.read_log().splitlines() if f"到对端的路变了 peer={peer.id} " in line]
    return changes[-1] if changes else ""


def udp_direct(lab: Lab, work: Path, t: Checks) -> None:
    """两个公网节点直连；a 上的 SOCKS5 客户端发 UDP，用户名指定从 b 出去。"""
    echo = lab.host("echo")
    for port in (443, 8080):
        start_udp_echo(echo, port)
    a, b = Node(lab.host("a"), work, 1), Node(lab.host("b"), work, 2, declare=("exit",))
    a.start_static(a.netmap(peers=[b.as_peer(exit=True)]))
    b.start_static(b.netmap(peers=[a.as_peer()], me={"exit": {}}))

    answer = socks5_udp(a.host, echo.ip, 443, b"ping", user=b.virtual_ip)
    seen, data = echoed(answer)
    t.check("用户名写 b：UDP 报文到了目标，回音回来了", data == b"ping", f"回来的是 {answer!r}")
    t.check("目标看到的来源是 b 的地址", seen == b.host.ip, f"看到 {seen!r}")
    t.check("回音带的来源是目标的地址和端口", answer is not None and (answer["source"], answer["port"]) == (echo.ip, 443), f"{answer!r}")
    route = last_route(a, b)
    t.check("两个节点之间走的是直连", "Direct" in route or "直连" in route, route)

    # 比两个节点之间一个包装得下的长：在节点之间切片、重组，两头看不出来。
    large = bytes(index % 251 for index in range(4000))
    seen, data = echoed(socks5_udp(a.host, echo.ip, 443, large, user=b.virtual_ip))
    t.check("4000 字节的报文原样来回", data == large and seen == b.host.ip, f"回来 {len(data)} 字节，来源 {seen!r}")

    seen, data = echoed(socks5_udp(a.host, echo.ip, 443, b"reliable", user=f"{b.virtual_ip}+c-r"))
    t.check("等级写 reliable（每个报文走一条单向流）也通", (seen, data) == (b.host.ip, b"reliable"), f"{seen!r} {data!r}")

    seen, data = echoed(socks5_udp(a.host, echo.ip, 8080, b"local"))
    t.check("不写用户名：从本机出去，不受出口的端口限制", (seen, data) == (a.host.ip, b"local"), f"{seen!r} {data!r}")
    refused = socks5_udp(a.host, echo.ip, 8080, b"blocked", user=b.virtual_ip, tries=3)
    t.check("出口不放行 80 / 443 以外的端口：报文被丢掉", refused is None, f"回来的是 {refused!r}")
    t.check("出口的日志里有拒绝的原因", "出口不允许目标端口 8080" in b.read_log())
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), sep="\n  ---\n")


def udp_via_relay(lab: Lab, work: Path, t: Checks) -> None:
    """a 和 b 都在对称型 NAT 后面，打不了洞，UDP 报文跟着连接经公网上的 relay r 走。"""
    echo = lab.host("echo")
    start_udp_echo(echo, 443)
    r = Node(lab.host("r"), work, 9, declare=("relay",))
    a = Node(lab.nat("nat-a", NatKind.SYMMETRIC).host("a"), work, 1)
    b = Node(lab.nat("nat-b", NatKind.SYMMETRIC).host("b"), work, 2, declare=("exit",))
    r.start_static(r.netmap(me={"relay": {}}, relays=[r.as_relay()], relay_clients=[a, b, r]))
    a.start_static(a.netmap(peers=[b.as_peer(endpoints=False, attached=[(r, 5)], exit=True)], relays=[r.as_relay()]))
    b.start_static(b.netmap(peers=[a.as_peer(endpoints=False, attached=[(r, 5)])], relays=[r.as_relay()], me={"exit": {}}))

    seen, data = echoed(socks5_udp(a.host, echo.ip, 443, b"through the relay", user=b.virtual_ip, tries=15))
    t.check("经 relay：UDP 报文到了目标，回音回来了", data == b"through the relay", f"回来的是 {data!r}")
    t.check("目标看到的来源是 b 所在 NAT 的公网地址", seen == lab.hosts["nat-b"].ip, f"看到 {seen!r}")
    route = last_route(a, b)
    t.check("两个节点之间走的是 relay", "Relay" in route or "经 relay" in route, route)
    large = bytes(index % 251 for index in range(4000))
    seen, data = echoed(socks5_udp(a.host, echo.ip, 443, large, user=b.virtual_ip))
    t.check("经 relay：4000 字节的报文原样来回", data == large, f"回来 {len(data)} 字节")
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), tail(r.read_log()), sep="\n  ---\n")


def dns(lab: Lab, work: Path, t: Checks) -> None:
    """真的 DNS 查询：SOCKS5 UDP 经出口问公网上的 DNS 服务器；dig 经 UDP 的端口转发问目标节点回环上的 DNS 服务器。"""
    resolver = lab.host("resolver")
    server = start_dns(resolver, resolver.ip, 53, "e2e.example", "11.22.33.44")
    a = Node(lab.host("a"), work, 1)
    b = Node(lab.host("b"), work, 2, declare=("exit", "forward"))
    start_dns(b.host, "127.0.0.1", 5300, "forwarded.example", "11.55.66.77")
    forward = {"id": 1, "protocol": "udp", "bind": "127.0.0.1:5353", "target": b.id}
    target = {"id": 1, "protocol": "udp", "addr": "127.0.0.1:5300", "listener": a.id}
    a.start_static(a.netmap(peers=[b.as_peer(exit=True)], me={"forwards": [forward]}))
    b.start_static(b.netmap(peers=[a.as_peer()], me={"exit": {"ports": [53]}, "forward_targets": [target]}))

    # 一、SOCKS5 的 UDP ASSOCIATE：请求是自己拼的 DNS 报文，回答它的是真的 DNS 服务器。
    query = dns_query("e2e.example")
    answer = socks5_udp(a.host, resolver.ip, 53, query, user=b.virtual_ip)
    try:
        addresses = dns_a_records(answer["data"], query) if answer else []
    except ValueError as err:
        addresses = [repr(err)]
    t.check(f"经出口问 DNS 服务器（{server}）：拿到了 A 记录", addresses == ["11.22.33.44"], f"回来的是 {answer!r}，解析出 {addresses}")
    t.check("应答带的来源是 DNS 服务器的地址和端口", answer is not None and (answer["source"], answer["port"]) == (resolver.ip, 53), f"{answer!r}")

    # 二、UDP 的端口转发：a 的 127.0.0.1:5353 通到 b 回环上的 DNS 服务。客户端是真的 dig。
    if shutil.which("dig"):
        dig = lambda: a.host.run("dig", "@127.0.0.1", "-p", "5353", "forwarded.example", "A", "+short", "+tries=1", "+time=3", check=False)  # noqa: E731
        asked = dig()
        # 第一次查询可能赶在转发的会话建好之前就超时了，再问一次。
        if asked.stdout.strip() != "11.55.66.77":
            asked = dig()
        t.check("dig 经 UDP 的端口转发查到了记录", asked.stdout.strip() == "11.55.66.77", f"{asked.stdout!r} {asked.stderr.strip()}")
    else:
        query = dns_query("forwarded.example")
        response = udp_exchange(a.host, "127.0.0.1", 5353, query)
        addresses = dns_a_records(response, query) if response else []
        t.check("经 UDP 的端口转发查到了记录（这台机器没有 dig，用的是自己拼的请求）", addresses == ["11.55.66.77"], f"{addresses}")
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), sep="\n  ---\n")


def udp_forward(lab: Lab, work: Path, t: Checks) -> None:
    """UDP 的端口转发：a 的本地端口通到 b 回环上的 UDP 服务；没有下发给 b 的转发不通。"""
    a = Node(lab.host("a"), work, 1)
    b = Node(lab.host("b"), work, 2, declare=("forward",))
    start_udp_echo(b.host, 7000, bind="127.0.0.1")
    forwards = [{"id": 1, "protocol": "udp", "bind": "127.0.0.1:9000", "target": b.id},
                {"id": 2, "protocol": "udp", "bind": "127.0.0.1:9001", "target": b.id}]
    # b 只认 1 号；2 号没有下发给它。b 不是出口。
    targets = [{"id": 1, "protocol": "udp", "addr": "127.0.0.1:7000", "listener": a.id}]
    a.start_static(a.netmap(peers=[b.as_peer()], me={"forwards": forwards}))
    b.start_static(b.netmap(peers=[a.as_peer()], me={"forward_targets": targets}))

    answer = udp_exchange(a.host, "127.0.0.1", 9000, b"forwarded")
    t.check("a 的本地 UDP 端口通到了 b 回环上的服务", answer == b"127.0.0.1|forwarded", f"回来的是 {answer!r}")
    large = bytes(index % 251 for index in range(3000))
    answer = udp_exchange(a.host, "127.0.0.1", 9000, large)
    t.check("3000 字节的报文原样来回", answer == b"127.0.0.1|" + large, f"回来 {len(answer or b'')} 字节")
    refused = udp_exchange(a.host, "127.0.0.1", 9001, b"not listed", tries=3)
    t.check("没有下发给目标节点的转发不通", refused is None, f"回来的是 {refused!r}")
    t.check("目标节点的日志里有拒绝的原因", "没有下发给这个节点、由发起节点监听的这条 UDP 端口转发" in b.read_log())
    if t.failures:
        print(tail(a.read_log()), tail(b.read_log()), sep="\n  ---\n")


SCENARIOS = {"udp_direct": udp_direct, "udp_via_relay": udp_via_relay, "dns": dns, "udp_forward": udp_forward}


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
