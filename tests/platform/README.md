# 平台测试

`tests/e2e` 的实验环境只在 Linux 上、用网络命名空间跑。这里的三组脚本在 Docker 容器里看另外几样它够不着的东西。
每个脚本把结果写进调用者给的目录：`summary.txt` 里每项一行 `PASS` 或 `FAIL`（带原因），旁边是各个程序的输出和日志。
都要 Docker（当前用户能用），不要 root。

## wine：Windows 版的三个程序在 Wine 里跑

```sh
cargo zigbuild --profile dist --locked --target x86_64-pc-windows-gnu -p i3t -p i3t-controller-bin -p i3tctl
tests/platform/wine/run.sh wine   target/x86_64-pc-windows-gnu/dist <结果目录>
scripts/build-linux.sh && tests/platform/wine/run.sh native target/dist/x86_64-unknown-linux-gnu/dist <结果目录>   # 对照
```

对照是同一个脚本跑 Linux 版（容器是 Debian 13，程序的 glibc 要求不能比它新，所以用 `scripts/build-linux.sh` 编的）。
`smoke.sh` 在一个容器里（Debian 的 Wine，只有 64 位）走一遍：版本和帮助；不带 `--state-dir` 时密钥写在哪里；controller 在回环上用一个新的
数据目录启动、打出管理令牌、WebUI 是编进去的那一份；`i3tctl` 建网络和注册令牌；两个节点用各自的状态目录和端口加入、上线；一个当出口
（`--lan`），另一个上 `i3tctl proxy add` 一个经它出去的代理，经这个代理的 SOCKS5、HTTP CONNECT 和明文 HTTP 转发访问容器里的一个
HTTP 服务；`i3tctl node list`、`proxy list`、`exit list`，`i3t status`、`routes`、`ping`、`self-check`，指标接口；Ctrl-C 正常停下，
数据库完好、没有留下 `-wal`；再在同样的目录上启动一次，令牌照旧能用，代理照旧能用。

要的：Docker；编好的程序（controller 要带 WebUI 的话先在 `web/` 里构建，见根目录的 README）。容器接在一个 `--internal` 的
Docker 网络上，不通外面（controller 启动时会去查发布源，这里查不到）；`I3T_WINE_NET=none` 时连网卡都没有，脚本自己建一块 dummy 网卡。

说明不了的：这是 Wine，不是 Windows。发布用的是 msvc 的构建，这里是 gnu 的；Windows 的服务、防火墙、真实的网卡列表
（Wine 的 `GetAdaptersAddresses` 和 Windows 的不一样）、NTFS 上替换正在运行的程序（自己更新）和文件的访问控制都看不到：
Wine 下面是 Linux 的文件系统，文件权限是 Linux 的。Wine 没实现的 Winsock 选项会让程序在 Wine 里失败，而在 Windows 上不会，
所以失败要看日志里是哪一个调用，必要时用 `WINEDEBUG=+winsock` 再跑。

## act：两个流水线里 Linux 上的 job

```sh
tests/platform/act/run.sh <act 程序> <工作目录> [ci-check] [ci-check-continue] [ci-web] [release] [release-signed]
```

用 [act](https://github.com/nektos/act) 在 Docker 里跑 `.github/workflows/ci.yml` 和 `release.yml` 里 `runs-on: ubuntu-latest` 的
job，测的是当前提交（`git archive HEAD`）。工作目录要在磁盘上，里面放 act 取下来的 action、产物和缓存。结果在
`<工作目录>/logs/`：每次运行的完整输出和一份每个 job、每一步成败的摘要。

要的：Docker，act（一个可执行文件），`catthehacker/ubuntu:act-24.04` 镜像（`I3T_ACT_IMAGE` 可以换），能上网（取 action、crate、npm 包、rustup）。

和 GitHub 上不一样的地方：镜像不是 GitHub 的 runner 镜像，job 以 root 跑，能同时打开的文件数的软上限是 1024（GitHub 的 runner
是 65536，`i3t-lane` 的 `scale` 测试要几千个）；`release` 没有密钥和仓库变量，走"没有签名密钥"那条路；`release-signed` 用一把临时的
钥匙走签名那条路（私钥文件和公钥由 `I3T_ACT_SIGNING_KEY`、`I3T_ACT_SIGNING_PUBLIC` 给出，用 `i3t-release keygen` 生成，公钥当作
仓库变量 `I3T_RELEASE_EXTRA_KEY` 编进程序）；release 的最后一步要 GitHub 的 API，脚本在副本里把 `gh` 换成只打印参数的函数；
`ci-check-continue` 是改过的副本（Clippy 失败也接着跑到 Test）。windows-latest、macos-latest 的 job act 跑不了，不算在内；
act 0.2.98 算 `runs-on: ${{ matrix.os }}` 时会拿错矩阵项，所以脚本用 `--matrix` 只留 Linux 的那几项。
流水线用的是当时最新的 stable（`dtolnay/rust-toolchain@stable`），本机的 stable 旧一些时结果可能不同。

## resolved：透明访问的名字经真的 systemd-resolved

```sh
scripts/build-linux.sh
tests/platform/resolved/run.sh target/dist/x86_64-unknown-linux-gnu/dist <结果目录> [root|service|service-polkit]
I3T_RESOLVED_BASE=ubuntu:24.04 tests/platform/resolved/run.sh target/dist/x86_64-unknown-linux-gnu/dist <结果目录>
```

两个容器接在一个 `--internal` 的 Docker 网络上：a 以 systemd 为 1 号进程（`--privileged`），开着 systemd-resolved 和 nss-resolve，
跑 controller 和打开了透明访问的节点 a；b 跑节点 b 和一个 HTTP 服务。查的是：`resolvectl status` 里 `i3t0` 的 DNS 服务器和路由域，
`resolvectl query` 和 `getent hosts` 解析对端的名字，`curl http://<对端>.<域名>:8080/` 不配代理就到了 b 上的服务，域名之外的名字照旧
经容器原来的解析器（Docker 的名字服务），controller 看到的名字状态；关掉透明访问之后 `resolvectl status` 和打开之前一样。
`service` 把节点 a 装成 `deploy/systemd` 里的系统服务（`i3t` 用户、透明访问的 drop-in），`service-polkit` 另外给 polkit 一条
允许 `i3t` 用户改 systemd-resolved 设置的规则。

要的：Docker（要能跑 `--privileged` 的容器，cgroup v2），编好的 Linux 版程序（glibc 不能比镜像里的新，`scripts/build-linux.sh` 编的可以）。
镜像里装着 polkit：systemd-resolved 按它决定一个不是 root 的进程能不能改网卡的 DNS。

说明不了的：容器里的 systemd 不是一台真机器：没有 NetworkManager 或 systemd-networkd 管着网卡，`/etc/resolv.conf` 是 Docker 挂进来的
（程序经 nss-resolve 问 systemd-resolved，不经 127.0.0.53 的 stub），`--internal` 的网络没有默认路由（脚本给 a 加一条，透明访问要它）。
