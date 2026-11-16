#!/usr/bin/env bash
# 改整个项目的版本号：节点、controller、命令行和 WebUI 用同一个版本。
#
#   scripts/bump-version.sh <新版本>      例如 scripts/bump-version.sh 0.1.1
#   scripts/bump-version.sh --check       只检查各处的版本是不是一致（CI 用）
#
# 每完成一批功能就升一次版本，并在 CHANGELOG.md 最上面加一节说明这一版有什么。
set -euo pipefail
cd "$(dirname "$0")/.."

workspace_version() { sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml; }
web_version() { sed -n 's/^  "version": "\(.*\)",$/\1/p' web/package.json; }

if [[ ${1:-} == --check ]]; then
    cargo_v=$(workspace_version); web_v=$(web_version)
    not_inherited=$(grep -L '^version.workspace = true$' crates/*/Cargo.toml bins/*/Cargo.toml || true)
    status=0
    [[ $cargo_v == "$web_v" ]] || { echo "版本不一致：Cargo.toml 是 $cargo_v，web/package.json 是 $web_v" >&2; status=1; }
    [[ -z $not_inherited ]] || { echo "下面这些 crate 没有用工作区的版本（应该写 version.workspace = true）：" >&2; echo "$not_inherited" >&2; status=1; }
    grep -q "^## $cargo_v " CHANGELOG.md || { echo "CHANGELOG.md 里没有 $cargo_v 这一节" >&2; status=1; }
    [[ $status == 0 ]] && echo "版本 $cargo_v，各处一致"
    exit $status
fi

new=${1:?用法：scripts/bump-version.sh <新版本> | --check}
[[ $new =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "版本号要写成 1.2.3 或 1.2.3-rc.1 这样：$new" >&2; exit 1; }
old=$(workspace_version)
sed -i "/^\[workspace.package\]/,/^\[/s/^version = \".*\"$/version = \"$new\"/" Cargo.toml
sed -i "s/^  \"version\": \".*\",$/  \"version\": \"$new\",/" web/package.json
# 让 Cargo.lock 里本项目各个包的版本跟上。
cargo update --workspace --quiet
echo "版本从 $old 改成了 $new。"
grep -q "^## $new " CHANGELOG.md || echo "记得在 CHANGELOG.md 最上面加一节：## $new （日期）"
