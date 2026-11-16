# ice3tier

面向个人的组网工具：把自己的几台机器连成一个网，能直连就直连，不能直连就经自己的中转节点转发，不需要虚拟网卡。

> 项目还在开发中，接口和协议都可能变。

## 它做什么

- **节点之间自动直连。** 同一个局域网里直接连；一方有公网端口时直接连；两边都在 NAT 后面时打洞。
- **打不通就中转。** 把一台有公网端口的机器标记为 relay，其他节点自动经它转发。relay 只转发加密的数据包，解不开内容。UDP 完全用不了的节点经 TCP 连 relay。
- **换路不断流。** 一对节点之间只有一条端到端的 QUIC 连接。直连打通之后从 relay 换到直连，或者直连断了退回 relay，上面的连接都不受影响。
- **可以按延迟选路。** 默认能直连就直连。打开"延迟优先"之后，节点按测到的延迟、抖动和丢包比较直连和经一个或几个 relay 的各条路，走代价最低的那条。
- **按出口上网。** 每个节点自带 SOCKS5 / HTTP 代理入口（TCP 和 UDP）。代理的用户名写哪个节点，流量就从哪个节点的网络出去。出口可以设带宽上限和流量配额。
- **端口转发。** 把一个节点上的 TCP 或 UDP 端口转发到另一个节点的本机或局域网里的服务。
- **标签式的访问控制。** 给节点打标签，规则只有一种：带标签 a 的节点可以访问带标签 b 的节点。
- **集中管理。** 一个 controller 加一个 Web 界面，管理网络、节点、规则、出口、端口转发，查看拓扑和任意两个节点之间现在怎么走。可以建多个互相隔离的网络。
- **节点自己更新。** 发布包带签名，节点经 controller 取包、验签、替换、重启，新程序起不来就自己换回旧的。
- **不用虚拟网卡。** 不改系统的路由表和 DNS，基本功能不需要管理员权限。

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
- 连接的加密、重传、拥塞控制都在 A 和 B 两端。relay 不是连接的一方，它只按包头里的目的节点把包转出去。
- 每个节点只有一把 Ed25519 密钥。节点之间、节点和控制器之间都按公钥互相认证。
- 控制器给每个节点下发一份只包含它用得上的信息的配置。控制器不在线时，已经在线的节点照常工作。

更细的说明见 [docs/architecture.md](docs/architecture.md)。

## 快速上手

下面用三台机器举例：一台有公网地址的服务器（跑 controller，同时当 relay），家里的一台机器，和随身的笔记本。

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

**4. 启用 relay 和出口。** 在 Web 界面的节点页打开开关，或者：

```sh
i3tctl relay enable <服务器的节点名> --udp <公网地址>:20330 --tcp <公网地址>:20330
i3tctl exit enable <家里那台的节点名>
```

**5. 使用。** 每个节点在本机开了两个代理入口，SOCKS5 在 `127.0.0.1:1080`，HTTP 在 `127.0.0.1:1081`：

```sh
# 从家里那台机器的网络出去（用户名写出口节点的名字或虚拟 IP）
curl -x socks5h://<家里那台的节点名>@127.0.0.1:1080 https://example.com

# 访问另一个节点上的服务（目标写它的虚拟 IP 或全名，不需要它是出口）
curl -x socks5h://127.0.0.1:1080 http://nas.home.i3t:8080/
```

节点的本机设置在状态目录的 `config.toml` 里，每一项都有注释。`i3t status` 查看节点的状态，`i3t routes` 查看到各个对端现在怎么走。

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
