#!/usr/bin/env bash
# 真实局域网测试的客户端一侧（Linux / macOS）。Windows 用 lan-client.bat。
# 用法：lan-client.sh <服务端地址>
set -uo pipefail

S=${1:?用法: lan-client.sh <服务端地址>}
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${BIN:-$HERE/../../../target/release/i3t-pretest}
TESTS=${TESTS:-$HERE/lan-tests.txt}
OUT=${OUT:-lan-results.jsonl}
ERR=${ERR:-lan-errors.txt}

echo "# server=$S started=$(date -Is)" > "$OUT"
echo "# stderr of each test" > "$ERR"
while read -r label args; do
    [[ -z $label || $label == \#* ]] && continue
    echo "[$label]"
    echo "# $label" >> "$OUT"
    echo "# $label" >> "$ERR"
    # 参数要按空格拆开，所以这里故意不加引号。
    # shellcheck disable=SC2086
    "$BIN" ${args//\{S\}/$S} >> "$OUT" 2>> "$ERR" || echo "    失败，见 $ERR"
done < "$TESTS"
echo "测完了。结果在 $OUT，错误输出在 $ERR。"
