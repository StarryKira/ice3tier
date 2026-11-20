<div align="center">

<img src="web/public/favicon.svg" alt="ice3tier" width="96" />

# ice3tier

**自己托管的组网工具，把你自己的机器连成一个网。**

[![CI](https://github.com/fuqiuluo/ice3tier/actions/workflows/ci.yml/badge.svg)](https://github.com/fuqiuluo/ice3tier/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/fuqiuluo/ice3tier?include_prereleases)](https://github.com/fuqiuluo/ice3tier/releases)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#许可证)
[![Rust](https://img.shields.io/badge/rust-1.91%2B-orange)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey)](#平台支持)

[English](README.md) | 简体中文

</div>

ice3tier 把你自己的机器连成一个私有网络。每个节点有一块虚拟网卡和一个虚拟 IP，能直连就直连，连不通就经你自己的中继转发。整个网络由一个带 Web 界面的 controller 管理。

> [!NOTE]
> 项目还在活跃开发中，版本之间接口和协议都可能变。

## 功能

- **虚拟网络**：每个节点默认装一块虚拟网卡、分到一个虚拟 IP，本机的程序按 IP 或名字访问别的节点，比如 `ssh nas.home.i3t`。
- **能直连就直连**：同一个局域网、有开放的端口或者打洞成功时直连；不行就经中继转发，UDP 不通时走 TCP。中继只转发加密的数据。
- **出口代理**：在一个节点上开 SOCKS5 / HTTP 代理，流量从另一个节点的网络出去。代理可以设用户名和密码、打开 ice3proxy；出口可以设流量配额。
- **端口转发**：把一个节点上的端口转发到另一个节点，或者它所在局域网里的服务。
- **按分组的访问控制**：规则只有一种：分组 A 里的节点可以访问分组 B 里的节点。
- **集中管理**：一个 controller，带 Web 界面和命令行，可以同时管多个互相隔离的网络。
- **签名的自动更新**：节点自己取新版本、验签、安装、重启，新版本起不来就退回旧的。

## 工作原理

```
                 ┌──────────────────────────────┐
                 │          controller          │   Web 界面 · 管理 API · i3tctl
                 └──────────────┬───────────────┘
                                │ 配置和成员
             ┌──────────────────┼──────────────────┐
             ▼                  ▼                  ▼
        ┌─────────┐        ┌─────────┐        ┌─────────┐
        │  节点 A │◀══════▶│  中继   │◀══════▶│  节点 B │
        └─────────┘        └─────────┘        └─────────┘
             ▲                                     ▲
             └═════════════════ 直连 ══════════════┘
```

- 每一对节点之间只有一条端到端加密的 QUIC 连接；在中继和直连之间切换时，上面的连接不断。
- 节点之间、节点和 controller 之间按 Ed25519 公钥互相认证。
- controller 只负责下发配置；它不在线时，已经在线的节点照常工作。

## 安装

### 一键安装（Linux / macOS）

在 Linux 上，[`deploy/install.sh`](deploy/install.sh) 会自动下载程序、创建系统用户、配置 systemd 服务（并按需装好 `systemd-resolved` 的 polkit 规则和 SELinux 上下文）以及 `i3t` 命令行入口：

```sh
# 安装节点
curl -fsSL https://raw.githubusercontent.com/fuqiuluo/ice3tier/main/deploy/install.sh | sh

# 安装节点并一步加入网络、启动服务
curl -fsSL https://raw.githubusercontent.com/fuqiuluo/ice3tier/main/deploy/install.sh | sh -s -- --token <注册令牌>

# 安装 controller 和 i3tctl
curl -fsSL https://raw.githubusercontent.com/fuqiuluo/ice3tier/main/deploy/install.sh | sh -s -- --controller --public-addr <公网地址>:20331
```

### 手动下载

也可以从 [最新的发布](https://github.com/fuqiuluo/ice3tier/releases/latest) 直接下载对应平台的程序：

| 平台                  | 节点                     | controller                          | 命令行                      |
| --------------------- | ------------------------ | ----------------------------------- | --------------------------- |
| Linux x86_64          | `i3t-linux-x86_64`       | `i3t-controller-linux-x86_64`       | `i3tctl-linux-x86_64`       |
| Linux aarch64         | `i3t-linux-aarch64`      | `i3t-controller-linux-aarch64`      | `i3tctl-linux-aarch64`      |
| Windows x86_64        | `i3t-windows-x86_64.zip` | `i3t-controller-windows-x86_64.exe` | `i3tctl-windows-x86_64.exe` |
| macOS（Apple 芯片）   | `i3t-macos-aarch64`      | `i3t-controller-macos-aarch64`      | `i3tctl-macos-aarch64`      |
| macOS（Intel）        | `i3t-macos-x86_64`       | `i3t-controller-macos-x86_64`       | `i3tctl-macos-x86_64`       |

systemd 单元和 polkit 规则文件在 [`deploy/systemd`](deploy/systemd) 与 [`deploy/polkit`](deploy/polkit)。

### 从源码构建

需要 Rust 1.91 或更新的版本；Web 界面要 Node.js 22 和 pnpm。

```sh
git clone https://github.com/fuqiuluo/ice3tier.git
cd ice3tier
(cd web && pnpm install && pnpm build)   # controller 会把 Web 界面编进去
cargo build --release
```

程序在 `target/release/` 下。

## 快速上手

**1. 启动 controller**，放在一台各个节点都连得上的机器上：

```sh
i3t-controller --data-dir /var/lib/i3t-controller --public-addr <公网地址>:20331
```

第一次启动时会打印管理令牌，登录 Web 界面和 `i3tctl` 都用它。Web 界面只监听 `127.0.0.1:20332`，从别的机器打开要用 SSH 转发端口：

```sh
ssh -L 20332:127.0.0.1:20332 <controller 所在的机器>
```

**2. 建网络、加节点。** 在 Web 界面里新建网络，点「添加节点」，在每台机器上执行它给出的命令。用命令行的话：

```sh
export I3T_ADMIN_TOKEN=<管理令牌>
i3tctl network create home
i3tctl token create
```

在要加入的机器上：

```sh
echo <注册令牌> | i3t join   # 可以当中继或出口的，加 --can-relay 或 --can-exit
i3t run
```

**3. 启用中继和出口，建代理和端口转发。** 在 Web 界面里做，或者用命令行：

```sh
i3tctl relay enable <节点>
i3tctl exit enable <节点>
i3tctl proxy add <监听的节点> 127.0.0.1:1080 <出口节点>
curl -x socks5h://127.0.0.1:1080 https://example.com
```

## 平台支持

| 平台    | 架构                | 虚拟网卡需要                                              |
| ------- | ------------------- | --------------------------------------------------------- |
| Linux   | x86_64、aarch64     | `CAP_NET_ADMIN`、`CAP_NET_RAW`、`CAP_NET_BIND_SERVICE`（[`deploy/systemd/i3t.service`](deploy/systemd/i3t.service) 里已授予）；非 root 配合 `systemd-resolved` 解析名字另装 [`deploy/polkit/50-i3t-resolved.rules`](deploy/polkit/50-i3t-resolved.rules) |
| Windows | x86_64              | 以管理员运行；`i3t.exe` 旁边要有 `wintun.dll`（发布的 zip 里带着） |
| macOS   | Apple 芯片、Intel   | 以 root 运行                                              |

## 开发

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Web 界面见 [`web/README.md`](web/README.md)，每一版的变化见 [CHANGELOG.md](CHANGELOG.md)。

## 参与贡献

欢迎提交问题和 Pull Request。提 Pull Request 之前，请确认 `cargo fmt --check`、`cargo clippy` 和 `cargo test --workspace` 都通过；改了 Web 界面的话，在 `web/` 里跑 `pnpm lint` 和 `pnpm test`。

## 许可证

以下两种许可任选其一：

- Apache License 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT 许可证（[LICENSE-MIT](LICENSE-MIT)）

除非你明确声明，你有意提交、要合入本项目的任何贡献（按 Apache-2.0 许可的定义），都按上面的双许可发布，不附加别的条款或条件。
