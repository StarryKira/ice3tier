# 发布

这份文档写给发布 ice3tier 的人，以及想自己放一个镜像、或者改了代码另行发布的人：一次发布里有什么、签的是什么、签名密钥放在哪里、怎么发、镜像怎么搭。

节点的自动更新靠的就是这里说的东西。节点不连 GitHub：controller 从发布源取清单和包，节点从 controller 取。节点只认清单上的签名，所以 controller、发布源、镜像都不需要可信。

## 1. 一次发布里有什么

一次发布是一个平铺的目录，原样上传到 GitHub Release：

```text
i3t-linux-x86_64                 节点程序
i3t-linux-aarch64
i3t-windows-x86_64.exe
i3t-macos-aarch64
i3t-macos-x86_64
i3t-controller-<平台>[.exe]      controller
i3tctl-<平台>[.exe]              管理命令行
manifest.json                    清单
manifest.json.sig                清单的签名
```

- 文件名是 `<程序名>-<平台>`，Windows 上加 `.exe`。平台名是"系统-架构"，和节点自己报给 controller 的一致（`linux-x86_64`、`windows-x86_64`、`macos-aarch64` 等）。文件名里不带版本号，所以"最新版的某个文件"有一个固定的下载地址。
- Linux 的程序按 glibc 2.28 编译（`scripts/build-linux.sh`），2018 年之后的发行版都能运行。
- 清单里只列节点程序 `i3t-*`。controller 和 `i3tctl` 不走自动更新，手动下载替换，所以不在清单里，也就不受签名保护：请只从项目的 GitHub Release 下载它们。

清单是一个 JSON 文件：

```json
{
  "version": "0.3.0",
  "min_controller": "0.2.0",
  "published": 1763200000,
  "artifacts": {
    "linux-x86_64": { "file": "i3t-linux-x86_64", "size": 18234880, "sha256": "…64 个十六进制字符…" },
    "windows-x86_64": { "file": "i3t-windows-x86_64.exe", "size": 17100288, "sha256": "…" }
  }
}
```

| 字段 | 含义 |
|---|---|
| `version` | 版本号，语义化版本。带预发布标识（`0.3.0-rc.1`）的是预发布版 |
| `min_controller` | 这个版本的节点需要的 controller 最低版本。controller 比它旧时不下发这个版本，界面提示先更新 controller |
| `published` | 发布时间，Unix 秒 |
| `artifacts` | 每个平台的节点程序：文件名、字节数、SHA-256 |

`manifest.json.sig` 只有一行：签名的 base64url（64 字节，不带填充）。

类型定义在 `crates/i3t-model/src/update.rs`。以后清单可以增加字段，旧程序忽略不认识的。

## 2. 签的是什么，怎么验

签名是 Ed25519，盖的是 **`manifest.json` 的原文，一个字节不差**，并且带着用途标签 `release-manifest`（发布私钥就算被拿去签别的东西，签出来的也冒充不了清单）。所以清单在传递的过程中不能解析之后再重新生成：controller 把原文当作一个字符串连同签名发给节点。

节点收到之后：

1. 用**编译在程序里的公钥**验签名。验过了才去解析清单的内容。
2. 只接受比自己新的版本，一样的和更旧的都不装。预发布版只装在正在运行预发布版的节点上。
3. 从 controller 把本平台的包取回来，核对大小和 SHA-256。
4. 运行新程序的自检，通过之后才替换并重启；新程序连不上 controller 就换回旧的。

这样能挡住和挡不住的：

- 发布源、镜像或者 controller 被攻破，都伪造不了清单，也换不了包：换了哈希就对不上。
- 它们可以不给新版本，或者拿一份旧的清单冒充最新的。节点不降级，所以后果只是更新不了，不会被换成有漏洞的旧版本。
- **签名私钥泄露时这一切失效。** 拿到私钥的人如果还能控制某个网络的 controller 或者它用的发布源，就能让那个网络的节点运行他的程序。私钥放在哪里见第 4 节。

核对一个下载下来的发布目录：

```sh
i3t-release verify --dir <目录>                 # 用编译在 i3t-release 里的公钥，和节点认的一样
i3t-release verify --dir <目录> --key <公钥>     # 只认指定的这一把
```

## 3. 公钥

项目的发布公钥（base64url）：

```text
kWB8NBoJtLGX7mZwXgMSZ1C8RkUDpkdOphusy_JhEk0
```

它写在 `crates/i3t-update/src/trust.rs` 的 `PROJECT_KEYS` 里，编译进节点和 `i3t-release`。节点运行的时候没有任何配置、命令或者 controller 的消息能增加或者替换它认的公钥。

**换钥匙。** `PROJECT_KEYS` 可以有不止一把，任何一把签的清单都算数。换的步骤：

1. 用 `i3t-release keygen` 生成新的密钥，把新的公钥加进 `PROJECT_KEYS`，用**旧**私钥签发这个版本。
2. 等节点都更新到了认识新公钥的版本，之后的发布改用新私钥签。
3. 再往后的某个版本把旧公钥从 `PROJECT_KEYS` 里去掉。

没有更新到第 1 步那个版本的节点不认新私钥签的清单，只能手动更新。旧私钥如果是因为泄露才换的，第 1 步那个版本要尽快发，第 3 步也要尽快做。

**额外的公钥。** 编译时设环境变量 `I3T_RELEASE_EXTRA_KEY=<公钥>`，编出来的程序除了项目的公钥之外还认这一把。它在编译期读取，写错了编译不过。两种人用得上：

- 测试：生成一把临时的钥匙，用它编译节点、签一份测试用的发布，就能把更新从头到尾跑一遍，不需要项目的私钥。
- 改了代码另行发布的人：用自己的钥匙签自己的发布。做法是

  ```sh
  i3t-release keygen --out release.key           # 打印出公钥
  I3T_RELEASE_EXTRA_KEY=<公钥> cargo build --release -p i3t -p i3t-controller-bin -p i3tctl
  i3t-release manifest --version <版本号> --dir <发布目录> --key release.key
  ```

  然后把发布目录按第 6 节的样子放到一个地方，把 controller 的发布源指过去。在 GitHub 上 fork 的仓库里，把公钥设成仓库变量 `I3T_RELEASE_EXTRA_KEY`、私钥设成密钥 `I3T_RELEASE_SIGNING_KEY`，发布流水线就会这样做。

  这样编出来的节点仍然认项目的公钥。想完全不认的话，把 `PROJECT_KEYS` 改成自己的。

## 4. 私钥放在哪里

这件事还没有定。两种做法流水线都支持，取舍如下。

| | 放在 GitHub 的 CI 密钥里 | 放在离线的机器上 |
|---|---|---|
| 发布 | 推一个标签，全自动 | 推标签之后流水线只建草稿，要人下载、签名、上传、公开 |
| GitHub 账号或者流水线被攻破时 | 攻击者可以签发任意程序。开着自动更新、发布源指向本仓库的网络，会在一小时之内开始安装它 | 攻击者签不了名，发不出节点肯装的东西 |
| 保护私钥靠什么 | GitHub 账号的安全、仓库的权限设置、流水线里每一个 action 的可信 | 那台机器和它的备份 |

两种做法都要另外留一份私钥的备份。CI 里的密钥写进去之后读不出来；而私钥一旦丢了，就签不出换钥匙要用的过渡版本（第 3 节），所有节点只能手动更新到认新公钥的版本。

离线签名并不能证明流水线编译出来的程序没有问题：签的人签的是流水线给他的文件。它挡住的是"有人能动 GitHub，但碰不到那台离线的机器"这一种情况下，不经过签名的人就把东西发出去。要连编译也不信任流水线，就得在离线的机器上自己编译再签。

流水线为放在 CI 里的情况做了这些限制：

- 私钥只出现在 `sign` 这一个 job 的一个步骤里。这个 job 不检出代码、不编译，除了上传下载构建产物的官方 action 之外不运行别的 action，只运行前面编好的 `i3t-release`。
- 这个 job 挂在 `release-signing` 环境上。在仓库设置里给这个环境加上"需要审批人"，每次签名都要有人点一下才进行；私钥也可以只放成这个环境的密钥，别的 job 读不到。
- 签完之后用编译进去的公钥验一遍，验不过就不发布。
- 发布的编译不使用缓存。

## 5. 怎么发

准备：

1. 改版本号。节点报的、`i3t --version` 显示的，是 `bins/i3t/Cargo.toml` 里的版本号；它必须和标签一致，否则流水线第一步就会停下。三个程序的版本号保持一样。
2. 如果这个版本的节点不能和旧的 controller 一起工作，把 `.github/workflows/release.yml` 顶上的 `MIN_CONTROLLER` 改成需要的最低版本。
3. 提交，打标签 `v<版本号>`（例如 `v0.3.0`，预发布版 `v0.3.0-rc.1`），推上去。

流水线（`.github/workflows/release.yml`）接下来：

1. `check`：核对标签和版本号。
2. `build`：编译五个平台的 `i3t`、`i3t-controller`、`i3tctl`。
3. `sign`：有私钥就生成 `manifest.json` 并签名，没有就跳过。
4. `publish`：建 GitHub Release，把文件都传上去。

**私钥在 CI 里**（配置了密钥 `I3T_RELEASE_SIGNING_KEY`，内容是 `i3t-release keygen` 写出的那个文件）：到这里就发完了。正式版被标为 latest；预发布版被标为 pre-release，不会成为 latest。

**私钥在离线的机器上**（没有配置那个密钥）：流水线建的是一个**草稿**，里面只有程序，没有清单。草稿不公开，也不会成为 latest。接下来在有私钥的机器上：

```sh
gh release download v0.3.0 --repo fuqiuluo/ice3tier --dir dist     # 取下草稿里的文件
i3t-release manifest --version 0.3.0 --dir dist --key release.key  # 生成清单并签名
i3t-release verify --dir dist                                      # 用编译进去的公钥验一遍
gh release upload v0.3.0 --repo fuqiuluo/ice3tier dist/manifest.json dist/manifest.json.sig
gh release edit v0.3.0 --repo fuqiuluo/ice3tier --draft=false --latest   # 预发布版把 --latest 换成 --prerelease
```

`i3t-release` 用和这次发布同一份代码编译（`cargo build --release -p i3t-release`），这样第三行验的就是节点认的公钥。`manifest` 不会覆盖已经存在的清单；`--min-controller` 不写的话是 `0.0.0`，也就是不作要求。

**预发布版。** 版本号带预发布标识就是预发布版，清单上看得出来（`i3t-release verify` 会写明）。它不在 latest 下面，controller 查最新版时取不到它；正在运行正式版的节点即使收到它的清单也不装。要试用预发布版，手动在节点上换上它，之后这个节点可以一路更新到后面的预发布版和正式版。

## 6. 镜像

controller 默认的发布源是项目的 GitHub Release。连不上 GitHub 时，把发布源改成一个镜像，或者自己放一个目录结构相同的静态目录。发布源是一个地址前缀，controller 只用下面这些路径，都是普通的 GET，不需要令牌：

```text
<发布源>/latest/download/manifest.json          最新正式版的清单
<发布源>/latest/download/manifest.json.sig
<发布源>/download/v<版本号>/<文件名>             清单里列出的文件，例如 download/v0.3.0/i3t-linux-x86_64
```

项目的发布源是 `https://github.com/fuqiuluo/ice3tier/releases`，上面的路径就是 GitHub Release 的下载地址。

搭一个镜像，就是把一次发布的目录放到 `download/v<版本号>/` 下面，再把其中的 `manifest.json` 和 `manifest.json.sig` 拷一份到 `latest/download/`，用任何能提供静态文件的服务器发出去：

```text
releases/
├── latest/download/
│   ├── manifest.json
│   └── manifest.json.sig
└── download/v0.3.0/
    ├── manifest.json
    ├── manifest.json.sig
    ├── i3t-linux-x86_64
    └── …
```

- 先放好 `download/v<版本号>/` 里的文件，再更新 `latest/download/` 里的清单；反过来的话 controller 可能拿到新清单却取不到包。
- `latest/download/` 里只放正式版的清单。
- 只需要放网络里用得到的平台的节点程序。
- 放之前可以用 `i3t-release verify --dir releases/download/v0.3.0` 核对。
- 镜像不需要可信，也不需要 HTTPS：清单有签名，包的哈希在清单里。

改了代码另行发布的人用的也是这个结构，只是清单是用自己的钥匙签的（第 3 节）。
