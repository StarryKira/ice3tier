#!/usr/bin/env bash
# 用 act（https://github.com/nektos/act）在 Docker 里跑两个流水线里 Linux 上的 job，不经过 GitHub。
#
#   tests/platform/act/run.sh <act 程序> <工作目录> [ci-check] [ci-check-continue] [ci-web] [release] [release-signed]
#
# release-signed 走有签名密钥的那条路：用一把临时的钥匙（i3t-release keygen 生成），私钥文件由 I3T_ACT_SIGNING_KEY
# 给出，当作密钥 I3T_RELEASE_SIGNING_KEY；它的公钥由 I3T_ACT_SIGNING_PUBLIC 给出，当作仓库变量 I3T_RELEASE_EXTRA_KEY
# 编进程序，所以"用节点认的公钥验证"那一步能过。
#
# ci-check-continue 不是原样的流水线：Clippy 那一步失败了也接着跑（continue-on-error），后面再加一步把新版 clippy 的
# chunks_exact_to_as_chunks 放过、看别的 crate 还有没有别的问题，好让 Test 那一步也跑一遍。
#
# 工作目录要在磁盘上（不要放在 tmpfs 里）：act 缓存的 action、产物（artifact）和缓存服务的数据都在里面。
# 测的是当前提交（git archive HEAD），不是工作区。每个流水线的完整输出在 <工作目录>/logs/<流水线>.txt，
# 每个 job 和步骤的结果摘在 <工作目录>/logs/<流水线>-summary.txt。
#
# 和 GitHub 上不同的地方：
# - 跑在 catthehacker 的 ubuntu 镜像里（I3T_ACT_IMAGE 可以换），以 root 身份，不是 GitHub 的 runner 镜像和 runner 用户；
# - windows-latest、macos-latest 的 job act 跑不了，这里也不跑。act 0.2.98 算 `runs-on: ${{ matrix.os }}` 时会拿错矩阵里的
#   那一项：不加筛选时 ci 的三个 check 都当成 macos-latest 跳过（连 ubuntu-latest 那个），release 的 build 有的算成空的
#   （"Start image=" 之后 invalid reference format），有的把 windows、macos 那几项放进 Linux 的容器里跑。所以这里用
#   --matrix 只留 Linux 的那几项（ci：os:ubuntu-latest；release：platform:linux-x86_64、linux-aarch64），一次跑一个 job；
# - 没有密钥和仓库变量：release 走"没有签名密钥"的那条路，只编译、上传、建草稿；
# - release 的最后一步要 GitHub 的 API（gh release create）：这份副本里把 gh 换成只打印参数的函数，别的步骤原样。
set -uo pipefail

act=$(realpath "${1:?用法：run.sh <act 程序> <工作目录> [ci-check] [ci-check-continue] [ci-web] [release] [release-signed]}")
work=$(realpath -m "${2:?缺工作目录}")
shift 2
[ $# -gt 0 ] || set -- ci-check ci-web release
repo=$(git -C "$(dirname "$(realpath "$0")")" rev-parse --show-toplevel)
image=${I3T_ACT_IMAGE:-catthehacker/ubuntu:act-24.04}
src=$work/src
logs=$work/logs

rm -rf "$src"
mkdir -p "$src" "$logs" "$work/actions" "$work/cache" "$work/artifacts"
git -C "$repo" archive HEAD | tar -x -C "$src"
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$src/Cargo.toml")

python3 -I - "$src/.github/workflows/release.yml" <<'EOF'
import sys
path = sys.argv[1]
text = open(path).read()
line = '          flags=(--title "$TAG" --generate-notes --verify-tag)\n'
assert text.count(line) == 1, "release.yml 里找不到 gh release create 那一步"
stub = "          gh() { printf 'act: would run: gh'; printf ' %q' \"$@\"; echo; }\n"
open(path, "w").write(text.replace(line, stub + line))
EOF
python3 -I - "$src/.github/workflows/ci.yml" "$src/.github/workflows/ci-continue.yml" <<'EOF'
import sys
source, target = sys.argv[1:]
text = open(source).read()
step = """      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
"""
assert text.count(step) == 1, "ci.yml 里找不到 Clippy 那一步"
extra = """      - name: Clippy
        continue-on-error: true
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: Clippy, chunks_exact_to_as_chunks allowed
        continue-on-error: true
        run: cargo clippy --workspace --all-targets -- -D warnings -A clippy::chunks_exact_to_as_chunks
"""
open(target, "w").write(text.replace(step, extra).replace("name: CI\n", "name: CI (continue after clippy)\n", 1))
EOF

# --action-offline-mode：取过的 action 不再去 GitHub 更新，网络不稳时少失败几次。
common=(-P "ubuntu-latest=$image" --pull=false --rm --action-offline-mode --action-cache-path "$work/actions"
    --cache-server-path "$work/cache" --artifact-server-path "$work/artifacts")

summarize() { # 名字：每个 job 的每一步的结果，和 act 跳过的 job
    grep -aE '✅  Success|❌  Failure|🏁  Job|Skipping unsupported platform|skipping|Error:|::error' "$logs/$1.txt" \
        | sed -E 's/\x1b\[[0-9;]*m//g' | grep -av '^\s*$' > "$logs/$1-summary.txt"
    echo "== $1: $(grep -c 'Job succeeded' "$logs/$1-summary.txt") jobs succeeded, $(grep -c 'Job failed' "$logs/$1-summary.txt") failed, \
$(grep -c 'Skipping unsupported platform' "$logs/$1-summary.txt") skipped as unsupported"
}

cd "$src" || exit 1
status=0
printf '{"ref": "refs/heads/main"}\n' > "$work/event-ci.json"
printf '{"ref": "refs/tags/v%s"}\n' "$version" > "$work/event-release.json"
for run in "$@"; do
    case $run in
        ci-check) args=(-W .github/workflows/ci.yml -e "$work/event-ci.json" -j check --matrix os:ubuntu-latest) ;;
        ci-check-continue) args=(-W .github/workflows/ci-continue.yml -e "$work/event-ci.json" -j check --matrix os:ubuntu-latest) ;;
        ci-web) args=(-W .github/workflows/ci.yml -e "$work/event-ci.json" -j web) ;;
        release) args=(-W .github/workflows/release.yml -e "$work/event-release.json" --concurrent-jobs 1
            --matrix platform:linux-x86_64 --matrix platform:linux-aarch64) ;;
        release-signed)
            export I3T_RELEASE_SIGNING_KEY
            I3T_RELEASE_SIGNING_KEY=$(cat "${I3T_ACT_SIGNING_KEY:?release-signed 要 I3T_ACT_SIGNING_KEY（私钥文件）}")
            args=(-W .github/workflows/release.yml -e "$work/event-release.json" --concurrent-jobs 1
                --matrix platform:linux-x86_64 --matrix platform:linux-aarch64
                -s I3T_RELEASE_SIGNING_KEY --var "I3T_RELEASE_EXTRA_KEY=${I3T_ACT_SIGNING_PUBLIC:?release-signed 要 I3T_ACT_SIGNING_PUBLIC（公钥）}") ;;
        *) echo "不认识的：$run" >&2; exit 2 ;;
    esac
    "$act" push "${args[@]}" "${common[@]}" > "$logs/$run.txt" 2>&1 || status=1
    summarize "$run"
done
exit $status
