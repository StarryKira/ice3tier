# ice3tier

面向个人的组网工具：把自己的几台机器连成一个网。每个节点有一块虚拟网卡和一个虚拟 IP，能直连就直连，连不通就经自己的中继转发。

> 项目还在开发中，接口和协议都可能变。

## 功能

- **虚拟网络**：每个节点默认装一块虚拟网卡，本机的程序直接访问别的节点的虚拟 IP 和名字，比如 `ssh nas.home.i3t`。
- **直连和中继**：同一个局域网、有公网端口或者打洞成功时直连；不行就经中继节点转发，UDP 不通时走 TCP。中继只转发加密的数据。
- **出口代理**：在一个节点上开 SOCKS5 / HTTP 代理，流量从另一个节点的网络出去。可以设用户名和密码、流量配额，可以打开 ice3proxy。
- **端口转发**：把一个节点上的端口转发到另一个节点，或者它所在局域网里的服务。
- **访问控制**：按分组写规则：分组 a 里的节点可以访问分组 b 里的节点。
- **集中管理**：一个 controller，带 Web 界面和命令行，可以建多个互相隔离的网络。
- **自动更新**：发布包带签名，节点自己取包、验签、替换、重启。

## 组成

| 程序             | 作用                                   |
| ---------------- | -------------------------------------- |
| `i3t`            | 节点，每台要组网的机器上跑一个         |
| `i3t-controller` | 控制器，带 Web 界面，整个部署只有一个 |
| `i3tctl`         | 控制器的命令行                         |

## 快速上手

1. 在一台各个节点都连得上的机器上启动 controller：

   ```sh
   i3t-controller --data-dir /var/lib/i3t-controller --public-addr <公网地址>:20331
   ```

   第一次启动时打印的管理令牌用来登录 Web 界面和 `i3tctl`。Web 界面默认只在 `127.0.0.1:20332` 上，远程访问用 SSH 端口转发：`ssh -L 20332:127.0.0.1:20332 <这台机器>`。

2. 在 Web 界面里新建网络，在「节点」页点「添加节点」，照着给出的命令在每台机器上执行。用命令行的话：

   ```sh
   export I3T_ADMIN_TOKEN=<管理令牌>
   i3tctl network create home
   i3tctl token create
   ```

   在要加入的机器上：

   ```sh
   echo <注册令牌> | i3t join    # 可以当中继的加 --can-relay，可以当出口的加 --can-exit
   i3t run
   ```

3. 在 Web 界面里启用中继和出口，建代理和端口转发。用命令行的话：

   ```sh
   i3tctl relay enable <节点>
   i3tctl exit enable <节点>
   i3tctl proxy add <监听的节点> 127.0.0.1:1080 <出口节点>
   ```

## 平台

节点、controller 和命令行支持 Linux、Windows 和 macOS。虚拟网卡需要权限：

- Linux：`CAP_NET_ADMIN`，`deploy/systemd` 里的单元已经给了。
- Windows：以管理员运行，`i3t.exe` 旁边要有 `wintun.dll`（发布的 zip 里带着）。
- macOS：root。

## 构建

需要 Rust 1.91 或更新的稳定版；Web 界面要 Node.js 22 和 pnpm。

```sh
(cd web && pnpm install && pnpm build)   # controller 会把 Web 界面编进去
cargo build --release
cargo test --workspace
```

产物在 `target/release/`。Web 界面的开发见 [web/README.md](web/README.md)，每一版的变化见 [CHANGELOG.md](CHANGELOG.md)。

## 许可证

MIT 或 Apache-2.0，任选其一。见 [LICENSE-MIT](LICENSE-MIT) 和 [LICENSE-APACHE](LICENSE-APACHE)。
