#!/usr/bin/env python3
"""把 lab.py 跑出来的 JSONL 汇总成 Markdown 表格，输出到标准输出。

用法：report.py [结果目录]
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path
from typing import Callable, Iterable

DEFAULT_DIR = Path(__file__).resolve().parents[3] / "docs" / "internal" / "pretest" / "results"


def load(directory: Path, suite: str) -> list[dict]:
    path = directory / f"{suite}.jsonl"
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def table(headers: list[str], rows: Iterable[list]) -> str:
    def cell(value) -> str:
        if value is None:
            return "—"
        if isinstance(value, float):
            return f"{value:.0f}" if abs(value) >= 100 else f"{value:.1f}" if abs(value) >= 10 else f"{value:.2f}"
        return str(value)

    lines = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    lines += ["| " + " | ".join(cell(v) for v in row) + " |" for row in rows]
    return "\n".join(lines) + "\n"


def pivot(records: list[dict], row_key: Callable[[dict], tuple], col_key: Callable[[dict], str],
          value: Callable[[dict], object], row_headers: list[str], col_order: list[str] | None = None) -> str:
    grid: dict[tuple, dict[str, object]] = defaultdict(dict)
    columns: list[str] = list(col_order or [])
    for record in records:
        column = col_key(record)
        if column not in columns:
            columns.append(column)
        grid[row_key(record)][column] = value(record)
    rows = [[*key, *(cells.get(column) for column in columns)] for key, cells in sorted(grid.items(), key=lambda kv: kv[0])]
    return table([*row_headers, *columns], rows)


# ---- 从一条记录里取数 ----------------------------------------------------------------

def client(record: dict) -> dict:
    return record.get("client") or {}


def mbps(record: dict):
    if record.get("error"):
        return "失败"
    return client(record).get("bulk", {}).get("mbps")


def others(record: dict, role: str) -> list[dict]:
    return [o for o in record.get("others", []) if o.get("role") == role]


def cpu_s(usage: dict | None):
    return None if not usage else usage["user_s"] + usage["sys_s"]


def gigabytes(record: dict):
    size = client(record).get("bulk", {}).get("bytes")
    return size / 1e9 if size else None


def per_gb(seconds, record: dict):
    size = gigabytes(record)
    return None if seconds is None or not size else seconds / size


def server_usage(record: dict) -> dict | None:
    servers = others(record, "server")
    return servers[0]["usage"] if servers else None


def data_sender_stats(record: dict) -> dict | None:
    """下行时数据是服务端发的，丢包和重传要看服务端那条连接的统计。"""
    connections = [o["quic"] for o in others(record, "server-connection")]
    return max(connections, key=lambda q: q["sent_packets"], default=None)


def loss_pct(record: dict):
    stats = data_sender_stats(record)
    return None if stats is None else stats["loss_ratio"] * 100


def wire_overhead_pct(record: dict):
    """服务端网卡实际发出的字节数比应用收到的字节数多出多少。含包头、确认和重传。"""
    size = client(record).get("bulk", {}).get("bytes")
    sent = sum(v["tx_bytes"] for dev, v in record.get("wire", {}).items() if dev.endswith("-b"))
    return None if not size or not sent else (sent / size - 1) * 100


def echo(record: dict, field: str):
    if record.get("error"):
        return "失败"
    return client(record).get("echo", {}).get("rtt_ms", {}).get(field)


def echo_cell(record: dict) -> str:
    if record.get("error"):
        return "失败"
    rtt = client(record).get("echo", {}).get("rtt_ms")
    return "—" if not rtt else f"{rtt['p50_ms']:.0f} / {rtt['p99_ms']:.0f} / {rtt['max_ms']:.0f}"


def transport(record: dict) -> str:
    return record["params"].get("transport", record["case"].split("/")[1])


def shape_row(record: dict) -> tuple:
    params = record["params"]
    return params["rtt_ms"], params["loss_pct"], params["rate_mbit"]


# ---- 各测试组 ----------------------------------------------------------------------

def report_t1(records: list[dict]) -> str:
    out = ["## T1 不加限制的直连（卸载打开）\n"]
    rows = []
    for record in records:
        params = record["params"]
        relays = others(record, "relay")
        relay_gb = relays[0]["bytes"] / 1e9 if relays and relays[0]["bytes"] else None
        relay_cpu = cpu_s(relays[0]["usage"]) / relay_gb if relay_gb else None
        batching = relays[0]["packets"] / relays[0]["sends"] if relays and relays[0]["sends"] else None
        down = params["dir"] == "down"
        sender, receiver = (server_usage(record), client(record).get("usage")) if down else (client(record).get("usage"), server_usage(record))
        rows.append([transport(record), params["streams"], params["dir"], mbps(record), per_gb(cpu_s(sender), record),
                     per_gb(cpu_s(receiver), record), relay_cpu, batching, (client(record).get("usage") or {}).get("peak_rss_mb")])
    out.append(table(["传输", "流数", "方向", "Mbit/s", "发送方 CPU 秒/GB", "接收方 CPU 秒/GB", "relay CPU 秒/GB", "relay 每次发送合并的包数", "客户端内存峰值 MB"], rows))
    return "\n".join(out)


def report_matrix(records: list[dict], title: str) -> str:
    out = [f"## {title}\n"]
    order = ["quic-cubic", "quic-bbr", "tcp", "kcp-fast", "quic-cubic-w16"]
    out.append("吞吐（Mbit/s）：\n")
    out.append(pivot(records, shape_row, transport, mbps, ["往返 ms", "丢包 %", "带宽 Mbit/s"], order))
    quic_kcp = [r for r in records if not transport(r).startswith("tcp")]
    out.append("发送方判定的丢包比例（%），只有 QUIC 有这个统计：\n")
    out.append(pivot([r for r in quic_kcp if transport(r).startswith("quic")], shape_row, transport, loss_pct,
                     ["往返 ms", "丢包 %", "带宽 Mbit/s"]))
    out.append("线上多发的字节（%）：服务端网卡发出的字节数相对应用收到的字节数多出多少，含包头、确认和重传：\n")
    out.append(pivot(records, shape_row, transport, wire_overhead_pct, ["往返 ms", "丢包 %", "带宽 Mbit/s"], order))
    return "\n".join(out)


def report_t2s(records: list[dict]) -> str:
    out = ["## T2 补充：多条并发流与上行\n"]
    out.append(pivot(records, shape_row, lambda r: f"{transport(r)} {r['params']['streams']} 流 {r['params']['dir']}", mbps,
                     ["往返 ms", "丢包 %", "带宽 Mbit/s"]))
    return "\n".join(out)


def report_lat(records: list[dict]) -> str:
    out = ["## 小消息的往返延迟，以及第一次访问的耗时\n"]
    echoes = [r for r in records if r["params"]["workload"] == "echo"]
    out.append("每 20ms 发一条 100 字节的消息，往返时间 p50 / p99 / 最大值（ms）：\n")
    out.append(pivot(echoes, shape_row, transport, echo_cell, ["往返 ms", "丢包 %", "带宽 Mbit/s"]))
    setups = [r for r in records if r["params"]["workload"] == "setup"]
    rows = []
    for record in setups:
        setup = client(record).get("setup", {})
        new_connection, new_stream = setup.get("new_connection_ms", {}), setup.get("new_stream_ms") or {}
        rows.append([*shape_row(record), transport(record), new_connection.get("p50_ms"), new_connection.get("p99_ms"),
                     new_stream.get("p50_ms"), new_stream.get("p99_ms")])
    out.append("新建连接到收到第一条回显（ms）；QUIC 另有在已有连接上开新流的耗时：\n")
    out.append(table(["往返 ms", "丢包 %", "带宽", "传输", "新连接 p50", "新连接 p99", "新流 p50", "新流 p99"], sorted(rows, key=str)))
    return "\n".join(out)


def report_relay(records: list[dict], title: str, row_key: Callable[[dict], tuple], row_headers: list[str]) -> str:
    out = [f"## {title}\n", "吞吐（Mbit/s）：\n"]
    out.append(pivot(records, row_key, transport, mbps, row_headers))
    relayed = [r for r in records if others(r, "relay")]

    def relay_cpu(record: dict):
        relays = others(record, "relay")
        size = sum(r["bytes"] for r in relays) / 1e9
        return None if not size else sum(cpu_s(r["usage"]) for r in relays) / size

    out.append("relay 每转发 1GB 耗的 CPU 秒（逐包发送，没有卸载；两个 relay 时是两个的合计）：\n")
    out.append(pivot(relayed, row_key, transport, relay_cpu, row_headers))
    return "\n".join(out)


def report_t6(records: list[dict]) -> str:
    out = ["## T6 一条连接上大流量和小消息同时跑\n",
           "每格是：大流量 Mbit/s；小消息往返 p50 / p99 / 最大值（ms）。shared 是共用一条连接，split 是小消息另走一条连接。\n"]

    def cell(record: dict) -> str:
        rate = mbps(record)
        rate_text = "—" if rate is None else rate if isinstance(rate, str) else f"{rate:.1f}"
        return f"{rate_text}；{echo_cell(record)}"

    out.append(pivot(records, shape_row, transport, cell, ["往返 ms", "丢包 %", "带宽 Mbit/s"]))
    return "\n".join(out)


def phase_mean(series: list[float], start_s: float, end_s: float):
    points = series[int(start_s * 10):int(end_s * 10)]
    return sum(points) / len(points) if points else None


def stall_after(series: list[float], at_s: float, window_s: float = 3.0) -> float:
    """换路之后 window_s 秒内，最长的一段吞吐为 0 的时间（秒）。"""
    longest = current = 0
    for value in series[int(at_s * 10):int((at_s + window_s) * 10)]:
        current = current + 1 if value == 0 else 0
        longest = max(longest, current)
    return longest / 10


def recover_time(series: list[float], at_s: float, target: float | None):
    """换路之后多久，吞吐第一次回到目标值的九成（秒）。"""
    if not target:
        return None
    for index, value in enumerate(series[int(at_s * 10):]):
        if value >= 0.9 * target:
            return index / 10
    return None


def report_t7(records: list[dict]) -> str:
    out = ["## T7 换路\n",
           "第 0～6 秒经 relay，第 6～12 秒直连，第 12～18 秒经 relay。吞吐按 100ms 采样。\n"]
    rows = []
    for record in records:
        params, series = record["params"], client(record).get("bulk", {}).get("series_mbps") or []
        if record.get("error"):
            rows.append([params["transport"], record["case"].split("/")[-1], "失败"] + [None] * 9)
            continue
        before_first, before_second = phase_mean(series, 3, 6), phase_mean(series, 9, 12)
        stats = data_sender_stats(record) or {}
        rows.append([
            params["transport"], record["case"].split("/")[-1], f"{params['relay_rtt']} → {params['direct_rtt']}",
            before_first, phase_mean(series, 8, 12), phase_mean(series, 14, 18),
            stall_after(series, 6) if params["schedule"] else None,
            recover_time(series, 6, min(before_first or 0, 90)) if params["schedule"] else None,
            stall_after(series, 12) if params["schedule"] else None,
            recover_time(series, 12, min(before_second or 0, 90)) if params["schedule"] else None,
            stats.get("lost_packets"), echo_cell(record) if "echo" in client(record) else None,
        ])
    out.append(table(["传输", "用例", "relay / 直连往返 ms", "换路前 Mbit/s", "直连阶段后段 Mbit/s", "换回后后段 Mbit/s",
                      "第一次换路后停顿 s", "恢复用时 s", "第二次换路后停顿 s", "恢复用时 s", "丢包数", "小消息 p50 / p99 / 最大 ms"], rows))
    return "\n".join(out)


def report_t9(records: list[dict]) -> str:
    out = ["## T9 TCP 中转\n",
           "a 到 relay 这一段有丢包，b 到 relay 那一段没有。每格是：大流量 Mbit/s；小消息往返 p50 / p99 / 最大值（ms）；队列满而丢掉的包数。\n"]

    def cell(record: dict) -> str:
        if record.get("error"):
            return "失败"
        rate = mbps(record)
        dropped = (client(record).get("path") or {}).get("tcp_dropped", 0)
        dropped += sum((o.get("path") or {}).get("tcp_dropped", 0) for o in others(record, "server"))
        dropped += sum(o.get("dropped", 0) for o in others(record, "relay"))
        return f"{rate:.1f}；{echo_cell(record)}；{dropped}"

    main = [r for r in records if "-q" not in transport(r)]
    out.append(pivot(main, lambda r: (r["params"]["rtt_ms"], r["params"]["access_loss"], r["params"]["cc"]),
                     lambda r: r["params"]["legs"], cell, ["往返 ms", "TCP 段丢包 %", "拥塞控制"]))
    capped = [r for r in records if "notsent_lowat" in r["params"]]
    sweep = [r for r in records if "-q" in transport(r) and r not in capped]
    out.append("发送队列长度的影响（往返 50ms，TCP 段丢包 1%，a 段 TCP、b 段 UDP）：\n")
    out.append(pivot(sweep, lambda r: (r["params"]["tcp_queue"],), lambda r: r["params"]["cc"], cell, ["队列长度（包）"]))
    if capped:
        out.append("内核里未发出数据的上限（a 段 TCP、b 段 UDP）。左列是上面主表里的同一条件：队列 256 个包，内核不设上限；"
                   "右列是队列 32 个包，内核里未发出的数据最多 16KB：\n")
        conditions = {(r["params"]["rtt_ms"], r["params"]["access_loss"], r["params"]["cc"]) for r in capped}
        compared = [r for r in main if r["params"]["legs"] == "tcp+udp"
                    and (r["params"]["rtt_ms"], r["params"]["access_loss"], r["params"]["cc"]) in conditions]
        out.append(pivot(compared + capped, lambda r: (r["params"]["rtt_ms"], r["params"]["access_loss"], r["params"]["cc"]),
                         lambda r: "内核上限 16KB" if "notsent_lowat" in r["params"] else "不设上限", cell,
                         ["往返 ms", "TCP 段丢包 %", "拥塞控制"]))
    return "\n".join(out)


def report_t10(records: list[dict]) -> str:
    out = ["## T10 LLM 形状的负载\n",
           "2000 个并发、共 4000 个请求：上传 30KB～1MB，等 800ms，收 200 条 80 字节的消息（间隔 30ms）；每 20 个请求里有 1 个中途静默 120 秒。链路往返 50ms、1000 Mbit/s。\n"]
    rows = []
    for record in sorted(records, key=lambda r: r["params"]["loss"]):
        llm = client(record).get("llm")
        if not llm:
            rows.append([transport(record), record["params"]["loss"], record.get("error", "失败")] + [None] * 9)
            continue
        first, token, upload = llm["first_token_overhead_ms"], llm["token_delay_ms"], llm["upload_ms"]
        usage, server = client(record)["usage"], server_usage(record) or {}
        rows.append([
            transport(record), record["params"]["loss"], f"{llm['ok']} / {llm['failed']}", f"{llm['silent_ok']} / {llm['silent_failed']}",
            f"{upload['p50_ms']:.0f} / {upload['p99_ms']:.0f}", f"{first['p50_ms']:.0f} / {first['p99_ms']:.0f}",
            f"{token['p50_ms']:.0f} / {token['p99_ms']:.0f} / {token['max_ms']:.0f}", llm["wall_secs"],
            cpu_s(usage), usage["peak_rss_mb"], cpu_s(server), server.get("peak_rss_mb"),
        ])
    out.append(table(["传输", "丢包 %", "普通请求 成功 / 失败", "静默请求 成功 / 失败", "上传 p50 / p99 ms", "首条消息额外等待 p50 / p99 ms",
                      "每条消息延迟 p50 / p99 / 最大 ms", "总用时 s", "入口 CPU 秒", "入口内存 MB", "出口 CPU 秒", "出口内存 MB"], rows))
    return "\n".join(out)


def failures(directory: Path) -> str:
    rows = []
    for path in sorted(directory.glob("*.jsonl")):
        for line in path.read_text().splitlines():
            record = json.loads(line)
            if record.get("error"):
                rows.append([record["case"], record["error"], "；".join(record.get("stderr", []))[:200]])
    if not rows:
        return "## 失败的用例\n\n没有。\n"
    return "## 失败的用例\n\n" + table(["用例", "现象", "最后的输出"], rows)


def main() -> int:
    directory = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_DIR
    sections = [
        (load(directory, "t1"), report_t1),
        (load(directory, "t2"), lambda r: report_matrix(r, "T2 直连：单条流下行")),
        (load(directory, "t2s"), report_t2s),
        (load(directory, "lat"), report_lat),
        (load(directory, "t4"), lambda r: report_relay(
            r, "T4 经一个 relay，与 T8 对照组",
            lambda rec: (rec["params"]["rtt_ms"], rec["params"]["rate_mbit"], rec["params"]["access_loss"], rec["params"]["far_loss"]),
            ["端到端往返 ms", "带宽 Mbit/s", "a 段丢包 %", "b 段丢包 %"])),
        (load(directory, "t5"), lambda r: report_relay(
            r, "T5 经两个 relay", lambda rec: (rec["params"]["rtt_ms"], str(rec["params"]["losses"])), ["端到端往返 ms", "三段各自的丢包 %"])),
        (load(directory, "t6"), report_t6),
        (load(directory, "t7"), report_t7),
        (load(directory, "t9"), report_t9),
        (load(directory, "t10"), report_t10),
    ]
    for records, render in sections:
        if records:
            print(render(records))
    print(failures(directory))
    return 0


if __name__ == "__main__":
    sys.exit(main())
