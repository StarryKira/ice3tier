#!/usr/bin/env python3
"""controller 的备份与恢复：运行中备份，停掉，在备份上启动，原来的节点自己连回来。

    python3 tests/e2e/test_backup.py [场景名...]
"""

from __future__ import annotations

import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab as lablib  # noqa: E402
from harness import (  # noqa: E402
    Checks, Controller, Node, RestoredController, back_up, controller_data, create_groups, start_whoami, tail,
)
from lab import Lab, wait_until  # noqa: E402


def socks(node: Node, url: str, timeout: float = 8):
    """从 `node` 所在的机器经它的本机 SOCKS5 入口访问 `url`。"""
    return node.host.run("curl", "-sS", "--max-time", str(timeout), "-x", "socks5h://127.0.0.1:1080", url, check=False, timeout=timeout + 5)


def reaches(node: Node, peer: Node, timeout: float = 30) -> bool:
    """反复尝试，直到 `node` 经覆盖网访问到 `peer` 上的服务（它回答 `peer` 的地址），或者超时。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if socks(node, f"http://{peer.virtual_ip}:8080/").stdout == peer.host.ip:
            return True
        time.sleep(0.5)
    return False


def blocked(node: Node, peer: Node, timeout: float = 20) -> bool:
    """反复尝试，直到 `node` 访问不到 `peer` 上的服务，或者超时。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if socks(node, f"http://{peer.virtual_ip}:8080/", timeout=4).returncode != 0:
            return True
        time.sleep(0.5)
    return False


def waited(condition, timeout: float, what: str) -> bool:
    try:
        wait_until(condition, timeout=timeout, interval=0.5, what=what)
    except TimeoutError:
        return False
    return True


def backup_and_restore(lab: Lab, work: Path, t: Checks) -> None:
    """两个节点在线时备份；之后加入第三个节点、建一个分组；停掉 controller，在备份上以同一个地址启动。"""
    controller = Controller(lab.host("ctl"), work)
    controller.ctl("network", "create", "lab", "--cidr", "10.66.0.0/16")
    create_groups(controller, "laptop")
    a, b, c = Node(lab.host("a"), work), Node(lab.host("b"), work), Node(lab.host("c"), work)
    a.join(controller, "laptop")
    b.join(controller)
    for node in (a, b):
        start_whoami(node.host, 8080)
    wait_until(lambda: all(controller.node(node)["online"] for node in (a, b)), what="a 和 b 上线")
    t.check("备份之前 a 和 b 互相访问得到", reaches(a, b) and reaches(b, a))

    data = controller_data(controller)
    wal = data / "controller.db-wal"
    t.check("备份时数据库还有没写回主文件的提交（-wal 不是空的）", wal.exists() and wal.stat().st_size > 0)
    backup = work / "backup"
    done = back_up(controller.host, data, backup)
    t.check("controller 运行中备份成功，不需要 --public-addr", done.returncode == 0, done.stderr.strip())
    t.check("备份列出了写下的文件", "controller.db" in done.stdout and "controller.key" in done.stdout, done.stdout)
    files = sorted(path.name for path in backup.iterdir()) if backup.is_dir() else []
    t.check("备份里是数据库和密钥", files == ["controller.db", "controller.key"], str(files))
    t.check("备份里的密钥只有属主可读", (backup / "controller.key").stat().st_mode & 0o777 == 0o600)
    again = back_up(controller.host, data, backup)
    unchanged = sorted(path.name for path in backup.iterdir()) == files
    t.check("不往已经有东西的目录里写，也不动它", again.returncode != 0 and "不是空目录" in again.stderr and unchanged,
            again.stderr.strip())

    # 备份之后的改动。
    create_groups(controller, "later")
    c.join(controller)
    wait_until(lambda: controller.node(c)["online"], what="c 上线")

    controller.host.stop(controller.proc)
    restored = RestoredController(controller.host, work, backup, controller.token)
    t.check("备份时的管理令牌照常能用", restored.ctl("status", check=False).returncode == 0)
    t.check("a 和 b 自己连回了恢复的 controller",
            waited(lambda: all(restored.node(node)["online"] for node in (a, b)), 60, "a 和 b 重新上线"))
    t.check("a 和 b 一直在运行，没有重启", a.proc.poll() is None and b.proc.poll() is None
            and a.read_log().count("节点启动") == 1 and b.read_log().count("节点启动") == 1)
    t.check("恢复之后 a 和 b 互相访问得到", reaches(a, b) and reaches(b, a))
    for rule in restored.ctl("rule", "list"):
        restored.ctl("rule", "delete", str(rule["id"]))
    t.check("恢复的 controller 删掉 All <-> All 之后，a 访问不到 b：配置由它下发", blocked(a, b))

    names = sorted(node["name"] for node in restored.ctl("node", "list"))
    t.check("备份之后加入的 c 不在节点列表里", names == ["a", "b"], str(names))
    groups = [group["name"] for group in restored.ctl("group", "list")]
    t.check("备份之后建的分组不在，之前的在", "later" not in groups and "laptop" in groups, str(groups))
    stopped = waited(lambda: c.proc.poll() is not None, 90, "c 退出")
    t.check("c 被当成不认识的节点拒绝，不再重连，进程退出并说明要重新加入",
            stopped and "不认这个节点" in c.read_log() and "i3t join" in c.read_log(), tail(c.read_log(), 5))
    t.check("controller 的日志里有拒绝的原因", "这把公钥没有注册" in restored.log.read_text(errors="replace"))
    if t.failures:
        print(tail(a.read_log()), tail(c.read_log()), tail(restored.log.read_text(errors="replace")), sep="\n  ---\n")


SCENARIOS = {
    "backup_and_restore": backup_and_restore,
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
