#!/usr/bin/env bash
# 真实局域网测试的服务端一侧（Linux）：起好各个服务端和一个 relay，按 Ctrl-C 结束。
# 用法：lan-server.sh [日志目录]
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${BIN:-$HERE/../../../target/release/i3t-pretest}
OUT=${1:-lan-server-logs}
# 固定在哪些核上跑。和别的测量同时进行时，用它避开对方占用的核。
CORES=${CORES:-0-3}
mkdir -p "$OUT"

pids=()
start() {
    local name=$1
    shift
    taskset -c "$CORES" "$BIN" "$@" > "$OUT/$name.out" 2>&1 &
    pids+=($!)
}

start quic-cubic   serve quic --listen 0.0.0.0:20330 --id 2 --peer 1 --cc cubic
start quic-bbr     serve quic --listen 0.0.0.0:20333 --id 2 --peer 1 --cc bbr
start quic-nogso   serve quic --listen 0.0.0.0:20334 --id 2 --peer 1 --cc cubic --no-gso
start quic-llm     serve quic --listen 0.0.0.0:20335 --id 2 --peer 1 --cc cubic --idle-secs 0
start tcp          serve tcp  --listen 0.0.0.0:20330
start kcp          serve kcp  --listen 0.0.0.0:20331
start relay        relay --udp 0.0.0.0:20340 --tcp 0.0.0.0:20340
sleep 0.3
start quic-relayed serve quic --listen 0.0.0.0:20332 --id 2 --peer 1 --relay 127.0.0.1:20340 --route relay
# 换路测试专用，时间表从第一条连接到来时开始计，所以每跑一次换路测试都要重启本脚本。
start quic-switch  serve quic --listen 0.0.0.0:20336 --id 4 --peer 3 --relay 127.0.0.1:20340 --route relay --switch 6:direct,12:relay

stop() {
    kill -TERM "${pids[@]}" 2>/dev/null || true
    wait || true
    echo "已停止。各服务端的统计在 $OUT/ 里。"
}
trap stop INT TERM

echo "服务端已启动：UDP 20330-20336、20340，TCP 20330、20340。日志在 $OUT/。"
echo "客户端测完之后按 Ctrl-C。"
wait
