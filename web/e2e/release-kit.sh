#!/bin/sh
# 端到端测试要用的程序，包括"节点更新"那一组要的东西。`pnpm e2e:kit` 执行它。
#
# 更新要有三样平时没有的东西：
#
# - 一把测试用的发布签名钥匙。controller 和节点只认编译在程序里的发布公钥，所以用
#   I3T_RELEASE_EXTRA_KEY=<公钥> 编译它们，让它们多认这一把。
# - "同一份代码、版本号更高"的第二个节点程序，用 I3T_TEST_VERSION=<版本号> 再编一次 i3t。
# - 签清单用的 i3t-release。
#
# 编好的东西放在 $I3T_RELEASE_KIT（默认 web/.e2e/release-kit）：
#
#   old/       i3t、i3t-controller、i3tctl：现在的版本，认测试钥匙。端到端测试默认用这一组
#   new/i3t    版本号高一级的节点程序
#   i3t-release、release.key、release.pub
#   versions   一行："<现在的版本> <新版本>"
#
# 源码默认是这个仓库；I3T_SOURCE 可以指向别的目录（例如 `git archive` 导出的一份）。
# 编译用 CARGO_TARGET_DIR，没设时是仓库里的 target/e2e-web：编译时的环境变量和平时不一样，放在别处会把缓存来回冲掉。
set -eu

here=$(cd "$(dirname "$0")" && pwd)
web=$(dirname "$here")
repo=$(dirname "$web")
source=${I3T_SOURCE:-$repo}
kit=${I3T_RELEASE_KIT:-$web/.e2e/release-kit}
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target/e2e-web}"
release="$CARGO_TARGET_DIR/release"

build() {
	echo "编译：$*" >&2
	cargo build --release --manifest-path "$source/Cargo.toml" "$@"
}

mkdir -p "$kit/old" "$kit/new"

build -p i3t-release
cp "$release/i3t-release" "$kit/i3t-release"

# 钥匙只生成一次：公钥是编译时的输入，换了钥匙就要把依赖它的都重编。
if [ ! -s "$kit/release.key" ] || [ ! -s "$kit/release.pub" ]; then
	rm -f "$kit/release.key" "$kit/release.pub"
	"$kit/i3t-release" keygen --out "$kit/release.key" >"$kit/release.pub"
fi
public=$(cat "$kit/release.pub")

I3T_RELEASE_EXTRA_KEY="$public" build -p i3t -p i3t-controller-bin -p i3tctl
for name in i3t i3t-controller i3tctl; do
	cp "$release/$name" "$kit/old/$name"
done

old=$("$kit/old/i3t" --version | awk '{ print $NF }')
new=$(echo "$old" | awk -F. '{ printf "%s.%s.%d", $1, $2, $3 + 1 }')
I3T_RELEASE_EXTRA_KEY="$public" I3T_TEST_VERSION="$new" build -p i3t
cp "$release/i3t" "$kit/new/i3t"

got=$("$kit/new/i3t" --version | awk '{ print $NF }')
if [ "$got" != "$new" ]; then
	echo "第二个节点程序的版本号是 $got，不是 $new：I3T_TEST_VERSION 没有生效" >&2
	exit 1
fi
echo "$old $new" >"$kit/versions"
echo "已准备好：$kit（$old → $new）" >&2
