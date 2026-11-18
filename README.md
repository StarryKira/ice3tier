# ice3tier

面向个人的组网工具：把自己的几台机器连成一个网，能直连就直连，不能直连就经自己的中转节点转发，不需要虚拟网卡。

> 项目还在开发中，接口和协议都可能变。

## 它做什么

- **节点之间自动直连。** 同一个局域网里直接连；一方有公网端口时直接连；两边都在 NAT 后面时打洞。
- **打不通就中转。** 把一台别的机器连得上的机器标记为中继，连得上它的节点自动经它转发。中继的地址不用填，controller 自己找出来。中继只转发加密的数据包，解不开内容。UDP 完全用不了的节点经 TCP 连中继。
- **换路不断流。** 一对节点之间只有一条端到端的 QUIC 连接。直连打通之后从中继换到直连，或者直连断了退回中继，上面的连接都不受影响。
- **可以按延迟选路。** 默认能直连就直连。打开"延迟优先"之后，节点按测到的延迟、抖动和丢包比较直连和经一个或几个中继的各条路，走代价最低的那条。
- **按出口上网。** 每个节点自带 SOCKS5 / HTTP 代理入口（TCP 和 UDP）。代理的用户名写哪个节点，流量就从哪个节点的网络出去。出口可以设流量配额。
- **端口转发。** 把一个节点上的 TCP 或 UDP 端口转发到另一个节点的本机或局域网里的服务。
- **按分组的访问控制。** 把节点放进分组，规则只有一种：分组 a 里的节点可以访问分组 b 里的节点。每个网络自带分组 `All`，默认 All 可以访问 All。
- **集中管理。** 一个 controller 加一个 Web 界面，管理网络、节点、规则、出口、端口转发，查看拓扑和任意两个节点之间现在怎么走。可以建多个互相隔离的网络。
- **节点自己更新。** 发布包带签名，节点经 controller 取包、验签、替换、重启，新程序起不来就自己换回旧的。自动更新可以按节点关掉；修了安全问题的版本会标出哪些节点该更新。
- **透明访问（Linux，可选）。** 打开之后，本机的程序不配代理就能直接连别的节点的虚拟 IP 或名字，比如 `ssh nas.home.i3t`。
- **不用虚拟网卡。** 不改系统的路由表和 DNS（透明访问除外，它要打开才有），基本功能不需要管理员权限。

不做的事：三层隧道（TUN / TAP）、协议伪装、面向公众的共享中转、多管理员与权限体系。

## 组成

| 程序 | 作用 |
|---|---|
| `i3t` | 节点。每台要组网的机器上跑一个 |
| `i3t-controller` | 控制器，内嵌 Web 界面。整个部署里只有一个 |
| `i3tctl` | 控制器的命令行，能做 Web 界面上的大部分操作 |

## 工作原理

```
应用 ─SOCKS5─▶ 节点 A ══════ 一条端到端的 QUIC 连接 ══════ 节点 B ─TCP─▶ 目标
                        数据包实际走的路：A ─▶ B，或者 A ─▶ relay ─▶ B
```

- 每条用户连接是 A 和 B 之间那条 QUIC 连接上的一个流。在已有的连接上开新的流不需要握手。
- 连接的加密、重传、拥塞控制都在 A 和 B 两端。中继不是连接的一方，它只按包头里的目的节点把包转出去。
- 每个节点只有一把 Ed25519 密钥。节点之间、节点和控制器之间都按公钥互相认证。
- 控制器给每个节点下发一份只包含它用得上的信息的配置。控制器不在线时，已经在线的节点照常工作。

更细的说明见 [docs/architecture.md](docs/architecture.md)。

## 快速上手

下面用三台机器举例：一台有公网地址的服务器（跑 controller，同时当中继），家里的一台机器，和随身的笔记本。

**1. 启动 controller。** 在服务器上：

```sh
i3t-controller --data-dir /var/lib/i3t-controller --public-addr <服务器的公网地址>:20331
```

第一次启动会打印一次管理令牌，`i3tctl` 和 Web 界面用它登录。管理接口和 Web 界面默认只监听 `127.0.0.1:20332`，远程访问用 SSH 端口转发：`ssh -L 20332:127.0.0.1:20332 <服务器>`，然后打开 <http://127.0.0.1:20332>。

**2. 建一个网络，生成注册令牌。** 在 Web 界面里按引导做，或者用命令行：

```sh
export I3T_ADMIN_TOKEN=<管理令牌>
i3tctl network create home
i3tctl token create          # 每台机器用一个
```

**3. 让机器加入。** 在每台要组网的机器上：

```sh
echo <注册令牌> | i3t join     # 服务器上加 --can-relay，要当出口的机器加 --can-exit
i3t run
```

`--can-relay`、`--can-exit` 只是这台机器声明"可以承担"，本身不启用任何东西；之后也可以用 `i3t set can-exit on` 改。

**4. 启用中继、出口和端口转发的目标。** 一个节点可以承担三种能力，默认都没有。中继和出口要两样都有：这台机器自己声明了，管理员再打开。
端口转发的目标只要管理员打开，机器上不用声明：

```sh
i3tctl relay enable <服务器的节点名>
i3tctl exit enable <家里那台的节点名>
i3tctl node set <家里那台的节点名> --forward-target on    # 之后才能配以它为目标的端口转发
```

也可以在 Web 界面的节点页打开开关。打开端口转发的目标意味着：谁控制了 controller（拿到了管理令牌，或者攻破了它），谁就能经一条端口转发
碰到这台机器能碰到的一切，包括它回环上的服务和它所在的局域网。中继和出口还要机器自己声明过，controller 一方变不出来。

中继不用填地址：controller 自己找出来（它看到的地址、这台机器网卡上的地址），节点挨个去试，用连得上的那个；
UDP 过不去的自动走 TCP。谁连得上它，它就是谁的中继：只有国内的机器连得上的服务器，是国内那些机器的中继；
只在一个局域网里连得上的，是那个局域网里的机器的中继。两台机器挑的中继彼此都连不上时，controller 让它们另外挂上一个两边都连得上的。
`i3tctl relay list` 列出每个中继有几台在线的机器连得上、几台连不上。

只有一种情况要人说：机房或者路由器把另一个 IP 上的端口映射到这台机器，比如服务器网卡上只有内网地址，
外面要经 `203.0.113.9:40330` 才连得进来。在这台服务器上执行一次，重启 i3t：

```sh
i3t set relay-address 203.0.113.9:40330
```

**5. 使用。** 每个节点在本机开了两个代理入口，SOCKS5 在 `127.0.0.1:1080`，HTTP 在 `127.0.0.1:1081`：

```sh
# 从家里那台机器的网络出去（用户名写出口节点的名字或虚拟 IP）
curl -x socks5h://<家里那台的节点名>@127.0.0.1:1080 https://example.com

# 访问另一个节点上的服务（目标写它的虚拟 IP 或全名，不需要它是出口）
curl -x socks5h://127.0.0.1:1080 http://nas.home.i3t:8080/

# HTTP 入口：https 走 CONNECT，http:// 的地址直接转发，所以 http_proxy 和 https_proxy 都可以指向它
export http_proxy=http://<家里那台的节点名>@127.0.0.1:1081 https_proxy=http://<家里那台的节点名>@127.0.0.1:1081
```

用户名后面可以加 QoS：`+c-i` 低延迟、`+c-r` 不丢包、`+c-b` 后台，不写是自动（`<出口>+c-i`，只要 QoS 不指定出口就写 `+c-i`）。
TCP 的连接在哪一种下都不丢数据，QoS 只决定谁先发；UDP 只有低延迟会丢掉来不及发的旧数据报，换来最低的延迟，别的一个都不丢。

不会用代理的工具可以让 `i3t connect` 替它连，比如 ssh：

```sh
ssh -o ProxyCommand='i3t connect %h %p' nas                 # 连网络里叫 nas 的节点
ssh -o ProxyCommand='i3t connect --via home %h %p' 192.168.1.10   # 经 home 这个出口连它局域网里的机器
```

**透明访问（Linux）。** 打开之后这台机器上的程序不用代理，直接连别的节点：`ssh 10.66.0.5`、`curl http://nas.home.i3t:8080/`。

```sh
sudo i3t set intercept on     # 写进 config.toml，重启 i3t 生效
sudo systemctl restart i3t
i3t intercept status          # 状态，和它在系统里装了什么
```

节点进程要 root，或者 `CAP_NET_ADMIN` 和 `CAP_NET_BIND_SERVICE`：以 `i3t` 用户跑的系统服务（`deploy/systemd/i3t.service`）装上
[`deploy/systemd/i3t.service.d/intercept.conf`](deploy/systemd/i3t.service.d/intercept.conf)，里面写着为什么只要这两项：

```sh
sudo install -D -m 0644 deploy/systemd/i3t.service.d/intercept.conf /etc/systemd/system/i3t.service.d/intercept.conf
sudo systemctl daemon-reload && sudo systemctl restart i3t
```

系统里还要有 `nft`（nftables）和 `ip`（iproute2）。它装一张包过滤的表、一条策略路由和一块不走流量的网卡 `i3t0`，节点停下时全部拆掉。
有 systemd-resolved 的机器上名字直接能用；没有的用 `sudo i3t hosts --apply` 把名字写进 `/etc/hosts`，`sudo i3t hosts --remove` 再去掉。别的节点这样连到本机的服务时，
本机打开了透明访问的话，服务看到的是对端的虚拟 IP。做法见 [docs/architecture.md](docs/architecture.md) 的透明访问一节。

Linux 上可以用 `i3t tune` 检查会影响节点表现的内核参数（UDP 缓冲的上限、TCP 的拥塞控制等），它只列出建议；`sudo i3t tune --apply` 才会改。它也说明这个内核能不能给 TCP 的重传设上限（Linux 6.15 起）：能的话，ice3proxy 和 TCP 中转断网之后恢复得快。

性能档位：`low-footprint`（一个核、几百 MB 内存的机器）、`balanced`（默认）、`high-throughput`（对端多、流量大）。在 Web 界面的节点页或者 `i3tctl node set <节点名> --profile low-footprint` 给一个节点选；没有选的节点用本机 `config.toml` 里的（`i3t set profile low-footprint`），其中每一项也可以单独改。各档位的内容见 [docs/architecture.md](docs/architecture.md) 的性能一节。

节点的本机设置在状态目录的 `config.toml` 里，每一项都有注释。`i3t status` 查看节点的状态，`i3t routes` 查看到各个对端现在怎么走，
`i3t ping <节点>` 在到那个节点的连接上量几个往返。`i3tctl exit test <出口> <主机[:端口]>` 让一个出口按它自己的规矩试着连一个目标，
说出解析、连接、TLS 握手各花多久、卡在哪一步。

三个程序（和发布工具 `i3t-release`）在终端上的文字跟着系统的语言：简体中文或英文。要指定的话设环境变量 `I3T_LANG=zh-CN` 或 `I3T_LANG=en`。`config.toml` 的注释用第一次写它时的语言；日志不翻译。

## 备份和恢复

备份不用停 controller：

```sh
i3t-controller --data-dir /var/lib/i3t-controller --backup-to /srv/backup/i3t-controller-2026-11-17
```

它把数据库的一份一致的快照和 controller 的密钥（权限 0600）写进一个新的或者空的目录，已经有东西的目录不写。不要直接拷贝 `controller.db`：controller 开着时最近的改动还在旁边的 `controller.db-wal` 里，只拷主文件会丢掉它们。

备份里有全部的设置和记录：网络、节点、分组、规则、出口、端口转发、注册令牌、凭据、操作记录、备份那一刻的管理令牌，以及 controller 的密钥。没有更新包的缓存（`releases/`，要用时重新取），也没有节点在不在线、上报的路由这类只在内存里的状态。

恢复：停掉 controller，把备份目录放到数据目录的位置（或者让 `--data-dir` 指向它），照常启动。节点连的地址（`--public-addr`）不变；换了机器的话让这个地址指到新机器上。用 systemd 跑的话先 `chown -R i3t-controller: /var/lib/i3t-controller`。

- 已经加入的节点自己连回来：controller 的密钥没有变，节点认得它。
- 备份之后的改动都没有了。备份之后加入的节点会被当成不认识的节点拒绝，`i3t run` 停下并提示重新加入：生成一个新的注册令牌，`echo <令牌> | i3t join`，再 `i3t run`。备份之后删掉的节点又出现在列表里。
- 旧版本做的备份照常能用：启动时数据库按新版本升级。

## 构建

需要 Rust 1.88 或更新的稳定版；Web 界面要 Node.js 22 和 pnpm。

```sh
(cd web && pnpm install && pnpm build)   # 先构建 Web 界面，controller 会把它编进去；不构建的话 controller 没有界面，别的不受影响
cargo build --release
```

产物在 `target/release/` 下。要构建能在较旧的 Linux 发行版上运行的版本，用 `scripts/build-linux.sh`。

运行测试：

```sh
cargo test --workspace
python3 tests/e2e/check_lab.py     # 检查端到端测试用的网络环境，需要 ip、tc、nft，不需要 root
I3T_BIN_DIR=$PWD/target/release python3 tests/e2e/test_managed.py   # 端到端测试，每个文件一组场景
```

版本号全项目只有一个，用 `scripts/bump-version.sh` 修改；每一版的变化在 [CHANGELOG.md](CHANGELOG.md)。

## 目录

```
crates/      各层的实现，见 docs/architecture.md
bins/        可执行文件：i3t、i3t-controller、i3tctl
web/         Web 界面
tests/e2e/   端到端测试：用网络命名空间搭出公网、NAT 和局域网
bench/       测量工具：拥塞控制的对照基准，以及开发早期用来验证设计的工具
scripts/     构建和改版本号的脚本
deploy/      systemd 单元
docs/        文档
```

## 许可证

MIT 或 Apache-2.0，任选其一。见 [LICENSE-MIT](LICENSE-MIT) 和 [LICENSE-APACHE](LICENSE-APACHE)。
