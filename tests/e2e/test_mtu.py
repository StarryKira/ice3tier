#!/usr/bin/env python3
"""路径 MTU 黑洞：一台只能走 TCP 的节点，进来的路送不到大包，也不回 ICMP。

    python3 tests/e2e/test_mtu.py [场景名...]

实际网络上量到的：那台服务器收不到 1488 字节的 IP 包，1440 字节的都收得到，网卡的 MTU 却是 1500。握手和小消息照常，
对端一有满 1500 字节的分段要发，连接就卡死。这里在"互联网"的路由器上把发往它的、长于 1440 字节的 IP 包悄悄丢掉。
节点自己开出去、接进来的 TCP 都通告压低的 MSS（`i3t_underlay::tcp_mss`），对端就不会发那种包。
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import (Checks, Controller, Node, binary, blob_digest, blob_file, fetch, lane_counts, lane_line,  # noqa: E402
                     start_blob_server, tail)
from lab import Lab, wait_until  # noqa: E402

# 收得到的最长的 IP 包，和实际网络上量到的一样。
LARGEST_DELIVERED = 1440
MEGABYTE = 1_000_000


def black_hole(host) -> None:
    """发往 `host` 的、长于 [`LARGEST_DELIVERED`] 字节的 IP 包送不到，也没有 ICMP 回去；它的 UDP 两个方向都不通。

    "互联网"的路由器上通向它的那块网卡的 MTU 改成 [`LARGEST_DELIVERED`]，路由器因此要回的"需要分片"的 ICMP 在它自己发出时丢掉。
    不在转发时按包长丢：路由器收包时会把同一条 TCP 的几个分段合成一个大包（GRO），按长度丢会把本来送得到的分段也丢掉；
    按出口的 MTU 转发时内核会先把它们拆回原来的大小，和真实的路上一样。它自己发 UDP 不会出错（像运营商挡掉了 UDP），
    发出去的包也不受长度的限制。"""
    port = f"c-{host.name}"[:15]
    subprocess.run(["ip", "-n", "core", "link", "set", port, "mtu", str(LARGEST_DELIVERED)], check=True)
    rules = f"""
table inet blackhole {{
    chain through {{
        type filter hook forward priority filter;
        ip saddr {host.ip} meta l4proto udp drop
        ip daddr {host.ip} meta l4proto udp drop
    }}
    chain silent {{
        type filter hook output priority filter;
        icmp type destination-unreachable icmp code frag-needed drop
    }}
}}
"""
    subprocess.run(["ip", "netns", "exec", "core", "nft", "-f", "-"], input=rules, text=True, check=True)


def attached_over_tcp(node: Node, relay: Node) -> bool:
    return f"到 relay 的中转会话建立（TCP） relay={relay.id}" in node.read_log()


def waited(condition, timeout: float, interval: float = 0.5) -> bool:
    try:
        wait_until(condition, timeout=timeout, interval=interval)
        return True
    except TimeoutError:
        return False


def both_ways(t: Checks, node: Node, web, exit_node: Node, how: str) -> None:
    """经 `exit_node` 从数据服务下载 1 MB、再上传 1 MB，字节一个不差。"""
    started = time.monotonic()
    code, body, err = fetch(node, f"http://{web.ip}/blob/{MEGABYTE}", via=exit_node, timeout=60)
    took = time.monotonic() - started
    t.check(f"{how}：下载 1 MB 完整（{took:.1f} 秒）", code == 0 and body == blob_digest(MEGABYTE), f"curl {code} {err[-200:]}")
    started = time.monotonic()
    code, body, err = fetch(node, f"http://{web.ip}/digest", via=exit_node, upload=blob_file(node, MEGABYTE), timeout=60)
    took = time.monotonic() - started
    t.check(f"{how}：上传 1 MB 完整（{took:.1f} 秒）", code == 0 and body == blob_digest(MEGABYTE), f"curl {code} {err[-200:]}")


def tcp_only_node_behind_a_black_hole(lab: Lab, work: Path, t: Checks) -> None:
    """只能走 TCP、进来的路有 MTU 黑洞的节点（它自己也是 relay）：拿到 netmap，经 TCP 挂上 relay，经 TCP 中转、经 lane 各双向传 1 MB；
    重启之后马上 ping 对端，第一个往返不卡住。"""
    web = lab.host("web")
    start_blob_server(web, work)
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab")
    relay = Node(lab.host("r"), work, declare=("relay",))
    exit_node = Node(lab.host("b"), work, declare=("relay", "exit"))
    # 几个不怎么说话的节点，让 netmap 比一个分段长：控制连接上的大消息也要过得去。
    others = [Node(lab.host(f"n{index}"), work) for index in range(4)]
    s = Node(lab.host("s"), work, declare=("relay",))
    black_hole(s.host)
    # 和实际网络上一样，离别的机器 250 毫秒。
    s.host.shape(lablib.Shape(rtt_ms=250))
    for node in (relay, exit_node, *others):
        node.join(controller)
    for node in (relay, exit_node):
        controller.ctl("relay", "enable", str(node.id))
    controller.ctl("exit", "enable", str(exit_node.id))
    s.join(controller)
    controller.ctl("relay", "enable", str(s.id))

    def netmap_applied() -> bool:
        seen = controller.node(s)
        return seen["online"] and seen["acked_version"] > 0 and seen["acked_version"] == seen["netmap_version"]
    t.check("黑洞后面的节点拿到了 netmap、在线", waited(netmap_applied, 60), tail(s.read_log(), 8))
    controller.add_proxy(s, exit_node)
    attached = waited(lambda: attached_over_tcp(s, relay) and attached_over_tcp(s, exit_node), 60)
    t.check("它经 TCP 挂上了两个 relay", attached, tail(s.read_log(), 8))
    reached = waited(lambda: any("中转会话建立" in line and f"node={s.id}" in line for line in exit_node.read_log().splitlines()), 60)
    t.check("对端也经 TCP 挂到了它的 relay 上（接进来的连接）", reached, tail(exit_node.read_log(), 8))

    both_ways(t, s, web, exit_node, "经 TCP 中转")
    t.check("这时还没有 lane", lane_counts(s, exit_node)["carried_bytes"] == 0, lane_line(s, exit_node))

    controller.ctl("pair", "set", s.name, exit_node.name, "--lane", "always")
    ready = waited(lambda: lane_counts(s, exit_node)["control_up"] >= 1, 60)
    t.check("两边拨好了 lane", ready, lane_line(s, exit_node))
    carried = lane_counts(s, exit_node)["carried_bytes"]
    both_ways(t, s, web, exit_node, "经 lane")
    counts = lane_counts(s, exit_node)
    # lane 数的是两个方向搬的数据，不算 HTTP 的头；两次各 1 MB，留一点余量。
    t.check("这两次走的是 lane", counts["carried_bytes"] >= carried + 1.9 * MEGABYTE and counts["fallbacks"] == 0, lane_line(s, exit_node))

    # 冷的一对节点：重启之后马上 ping，第一个往返不卡在黑洞里。
    controller.ctl("pair", "unset", s.name, exit_node.name)
    s.stop()
    s.start()
    waited(lambda: attached_over_tcp(s, exit_node), 30)
    started = time.monotonic()
    pinged = s.host.run(binary("i3t"), "--state-dir", str(s.state), "ping", exit_node.name, "--count", "1", check=False, timeout=30)
    took = time.monotonic() - started
    t.check(f"重启之后马上 ping 对端，几个往返之内就回来（{took:.1f} 秒）", pinged.returncode == 0 and took < 3,
            f"{pinged.stdout.strip()} {pinged.stderr.strip()}")
    if t.failures:
        print(tail(s.read_log(), 30), tail(exit_node.read_log(), 15), tail(controller.log.read_text(errors="replace"), 10), sep="\n  ---\n")


SCENARIOS = {
    "tcp_only_node_behind_a_black_hole": tcp_only_node_behind_a_black_hole,
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
