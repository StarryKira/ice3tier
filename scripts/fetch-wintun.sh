#!/usr/bin/env bash
# 取 Wintun（Windows 上虚拟网卡的驱动，https://www.wintun.net）的预编译 wintun.dll，放到节点程序旁边要放的地方。
#
#   scripts/fetch-wintun.sh <架构> <目录>
#
# 架构是 amd64、arm64、x86、arm（Wintun 的写法）。目录里得到 wintun.dll 和 wintun-LICENSE.txt。
#
# 版本和摘要钉死在这里：下载的压缩包和里面的 DLL 都核对 SHA-256，对不上就失败，什么都不留下。换版本时把三处一起改
# （版本号、压缩包的摘要、各架构 DLL 的摘要），摘要自己下载算过，再和 wintun.net 上公布的比一遍。
#
# DLL 不进仓库，原样随 Windows 的发布包分发：它的许可（压缩包里的 LICENSE.txt，"Prebuilt Binaries License"）只允许
# 原样、和只经 wintun.h 的接口使用它的软件放在一起分发，不许修改、不许去掉它的版权说明、不许拿 WireGuard 和 Wintun 的名字
# 给别的东西做宣传，条款作者可以改。所以许可原文跟着 DLL 一起放。
set -euo pipefail

VERSION=0.14.1
ZIP_SHA256=07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51
arch=${1:?用法：fetch-wintun.sh <amd64|arm64|x86|arm> <目录>}
out=${2:?缺目录}
# 每个架构的 DLL 的摘要（写成 case 而不是关联数组：macOS 自带的 bash 3 没有关联数组）。
case $arch in
    amd64) expected=e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce ;;
    arm64) expected=f7ba89005544be9d85231a9e0d5f23b2d15b3311667e2dad0debd344918a3f80 ;;
    x86) expected=d694fa46ab4cfebcb2632d094c7aa97278eef2f8052438621766d863ae98a931 ;;
    arm) expected=daad267411ecdc70a0535e274d2c3e9da3d0084bdac7662cb8424dd4a031b4d9 ;;
    *)
        echo "不认识的架构：$arch（amd64、arm64、x86、arm）" >&2
        exit 2
        ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL --retry 3 -o "$work/wintun.zip" "https://www.wintun.net/builds/wintun-$VERSION.zip"

# 摘要和解压都用 Python：Linux、macOS 和 Windows 的 Git Bash 上都有，不依赖 sha256sum、unzip 在不在。
# Windows 的 CI 机器上它叫 python（python3 可能是应用商店的占位程序）。
python=$(command -v python3 2>/dev/null || command -v python)
"$python" -I - "$work/wintun.zip" "$ZIP_SHA256" "$arch" "$expected" "$out" <<'PY'
import hashlib, os, sys, zipfile

archive, zip_sha, arch, dll_sha, out = sys.argv[1:6]
with open(archive, "rb") as file:
    actual = hashlib.sha256(file.read()).hexdigest()
if actual != zip_sha:
    sys.exit(f"wintun 的压缩包摘要不对：{actual}，应该是 {zip_sha}")
with zipfile.ZipFile(archive) as bundle:
    dll = bundle.read(f"wintun/bin/{arch}/wintun.dll")
    licence = bundle.read("wintun/LICENSE.txt")
actual = hashlib.sha256(dll).hexdigest()
if actual != dll_sha:
    sys.exit(f"{arch} 的 wintun.dll 摘要不对：{actual}，应该是 {dll_sha}")
os.makedirs(out, exist_ok=True)
with open(os.path.join(out, "wintun.dll"), "wb") as file:
    file.write(dll)
with open(os.path.join(out, "wintun-LICENSE.txt"), "wb") as file:
    file.write(licence)
print(f"wintun {arch} -> {out}/wintun.dll")
PY
