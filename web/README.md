# ice3tier WebUI

controller 的管理界面。它只和 controller 的管理 API 说话（见 [`docs/admin-api.md`](../docs/admin-api.md)），和 API 同源，每个请求带 `Authorization: Bearer <管理令牌>`。界面语言是简体中文。

技术栈：React、TypeScript、Vite、TanStack Query、React Router、Tailwind、shadcn/ui 风格的部件（Radix）、React Flow（访问关系图、拓扑）。包管理用 `pnpm`。

## 准备

- Node 22 或更新，`pnpm`。
- 安装依赖：

  ```sh
  pnpm install
  ```

## 开发

界面要有一个跑着的 controller 才有数据。先在仓库根目录构建并启动一个：

```sh
cargo build --release -p i3t-controller-bin -p i3t
target/release/i3t-controller --data-dir /tmp/i3t-dev --public-addr 127.0.0.1:20331
```

第一次启动时它会打印管理令牌，只显示这一次，登录界面时要填。然后在 `web/` 里：

```sh
pnpm dev
```

开发服务器（默认 `http://localhost:5173`）把 `/api` 开头的请求代理到 controller 的管理端口，所以浏览器看到的仍然是同源，controller 不需要发 CORS 头。

**controller 的地址由环境变量 `I3T_CONTROLLER` 决定**，默认是 `http://127.0.0.1:20332`（controller 默认的管理地址）。controller 在别处时：

```sh
I3T_CONTROLLER=http://127.0.0.1:20432 pnpm dev
```

也可以把 `.env.example` 复制成 `.env.local` 再改，不用每次写在命令行上。

要看节点加入之后的样子，照界面上"添加节点"给出的命令，在另一个状态目录里执行：

```sh
export I3T_STATE_DIR=/tmp/i3t-dev-node1
echo <注册令牌> | target/release/i3t join
target/release/i3t run
```

## 目录

| 位置              | 内容                                                                                                                                                                   |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/api/`        | 管理 API 的客户端：`client.ts`（请求、错误、401）、`endpoints.ts`（每个接口一个函数）、`types.ts`（照着 `docs/admin-api.md` 手写的类型）、`queries.ts`（查询键和轮询） |
| `src/auth/`       | 登录页和登录状态（管理令牌存在浏览器里）                                                                                                                               |
| `src/pages/`      | 每个页面一个目录                                                                                                                                                       |
| `src/components/` | 页面共用的部件；`ui/` 下是最基础的那一层                                                                                                                               |
| `src/access/`     | 访问关系图的数据整理和几何，纯函数                                                                                                                                     |
| `src/topology/`   | 路由视图的数据整理：路由矩阵变成拓扑图的点和线、一条路的几种说法、路径诊断里一个方向怎么说、地址里选中的那一对节点，纯函数                                             |
| `src/lib/`        | 校验、地址段的计算、给用户执行的命令、时间和字节数的写法、版本号的比较、节点更新情况的说法、用量和监听状态的说法等                                                     |
| `src/i18n/`       | 界面上的全部文字。组件里不直接写中文                                                                                                                                   |
| `src/test/`       | 单元测试的准备和假的管理 API                                                                                                                                           |
| `e2e/`            | Playwright 的端到端测试和截图                                                                                                                                          |
| `screenshots/`    | `pnpm screenshots` 截下来的每个页面，给人看版面                                                                                                                        |

几条约定：

- 管理 API 变了，`src/api/types.ts` 和 `src/api/endpoints.ts` 要跟着改，它们是照着文档手写的。
- 提交之前的校验（`src/lib/validate.ts`）和 controller 的规则一致，只是为了早一点指出错误；controller 拒绝时显示它给的原因。
- controller 还没有接口的页面（现在只剩连通性测试）是"计划中"的说明页，不放假数据。接口有了之后，在 `src/routes.tsx` 里把对应的那一行换成真正的页面。拓扑、路径诊断和节点更新（设置里的一个分区）就是这样填上的。
- 界面自己的版本号只有一个来处：`package.json` 的 `version`（每次迭代由仓库的脚本改它）。`vite.config.ts` 在构建时把它作为常量 `__APP_VERSION__` 写进代码，`src/lib/version.ts` 的 `WEB_VERSION` 是唯一读它的地方；运行时不读 `package.json`。侧栏底部和设置的「关于」同时写出 controller 的版本和界面的版本，两者不一样时直接说出来；已知有比 controller 新的正式版时，侧栏底部有一个去「节点更新」的提示。
- 节点的更新情况（有新版本、正在更新、等什么、失败、不能自己更新的原因和做法）在节点列表、节点详情和设置里用同一套说法，都出自 `src/lib/nodeUpdate.ts`。
- "已用多少、上限多少"（凭据的流量、出口的配额）都出自 `src/lib/usage.ts`：不到八成绿色，八成以上琥珀色，用完红色，没有上限时不画进度条。字节数一律经 `formatBytes` 写（按 1024 进位），输入上限时的单位（MB、GB、TB）也按 1024 算。
- 一个监听（代理监听器、端口转发的监听一侧、节点自带的回环入口）现在怎么样，在监听器页、端口转发页、节点详情的「监听」和总览里用同一套说法，出自 `src/lib/listenState.ts`。端口写 0 的转发，列表里写节点挑定的端口。
- 凭据的删除分两步：第一次是吊销（危险确认，记录留着，看得到用过多少），对吊销了的再删一次才彻底删掉（普通确认）。注册令牌默认只列还能用的，「全部」里有最近 30 天用掉和过期的，写着是哪个节点用的。
- 操作记录按网络、操作、对象筛（交给 controller，一页一百条往前翻），搜索只在取回的记录里找；条件写在地址里。记录里提到的东西做成链接：列表页带着 `focus=<编号>` 打开，那一行亮出来；东西已经删掉了的，页面上说一句。
- 一条路的细节（直连是局域网、IPv6、端口可达还是打洞；经 relay 时第一段走 UDP 还是 TCP）在节点的路由表、路由矩阵的格子和路径诊断里用同一套说法，出自 `src/topology/route.ts`。比界面旧的 controller 不写这几个字段，界面就不写细节。
- 拓扑的实时图和路由矩阵是同一份数据（`GET /networks/{network}/routes`）的两种写法；路径诊断每次都去问两端（`GET /networks/{network}/path`，最多等 3 秒），所以不轮询，页面上有"再问一次"。两页之间、和两种写法之间，选中的那一对节点写在地址的 `a`、`b` 里跟着走。现问到的方向还有发送方的选路模式、原因和它考虑过的候选路径（代价、每一段的延迟、抖动、丢包、relay 的负载），在两个方向的卡片下面各一张表。拓扑的第三个标签页"骨干"是 relay 之间的延迟矩阵（`GET /networks/{network}/backbone`），和路径诊断一样每次现问节点、不轮询。
- 节点更新的暂停按网络算：同一个版本在几个网络里被暂停时，设置里是一条横幅，写明是哪几个网络，"继续"一次继续全部。

## 检查和测试

```sh
pnpm typecheck    # TypeScript
pnpm lint         # ESLint
pnpm test         # 单元测试和组件测试（Vitest + Testing Library，jsdom）
pnpm e2e          # 端到端测试（Playwright，无头 Chromium）
pnpm screenshots  # 重新截一遍每个页面，放到 screenshots/
```

**单元测试**和被测的代码放在一起，文件名是 `*.test.ts` / `*.test.tsx`。组件测试不连真的 controller：`src/test/harness.tsx` 里的 `fakeApi` 接住 `fetch`，按"方法 路径"应答。改代码时可以用 `pnpm test:watch`。

**端到端测试**用的是真的程序：每个测试文件自己启动一个全新的 controller（数据在临时目录里），用 `pnpm build` 的产物作界面，把界面上给出的加入命令原样交给 shell 执行，结束时把 controller 和节点都停掉、临时目录删掉。所以运行之前不需要开着什么，但要先构建好 Rust 的程序。

`pnpm e2e` 跑四个文件：`flows.spec.ts`（从空的 controller 开始的主要流程）、`routes.spec.ts`（三个真的跑着、互相连过的节点：拓扑的实时图、路由矩阵、路径诊断）、`updates.spec.ts`（节点更新，发布源是一个本机目录：换发布源、立即检查、节点上的标记和做法、确认框、全部更新之后一个节点真的换成新版本重启、一个自检不过的版本被暂停、继续）、`limits.spec.ts`（限额、用量和监听的状态，流量真的经两个跑着的节点走：凭据经要求凭据的监听器用过之后有用量、改上限让它用完、吊销、彻底删掉；出口配了配额、下载之后有用量；端口写 0 的转发显示节点挑的端口并且连得通；监听在被占用的端口上的转发显示端口被占用；操作记录的筛选和链接；标签改名）。

`limits.spec.ts` 下载的目标是测试进程自己开的一个 HTTP 服务，监听在这台机器的私有 IPv4 地址上，出口打开"允许访问本机局域网"之后把它当局域网目标，不连外网。机器上没有私有 IPv4 地址时，要它的几项测试跳过。

节点更新要几样平时没有的东西：一把测试用的发布签名钥匙，认这把钥匙的 controller 和节点（`I3T_RELEASE_EXTRA_KEY`），版本号高一级的节点程序（`I3T_TEST_VERSION`），签清单的 `i3t-release`（见 `docs/releasing.md`）。`pnpm e2e:kit` 把它们编好，放在 `web/.e2e/release-kit`：

```sh
pnpm e2e:kit                             # 在 web/ 里；要能编译 Rust，第一次要几分钟
pnpm exec playwright install chromium    # 只在第一次
pnpm e2e
```

`pnpm e2e:kit` 默认编译这个仓库的源码，目标目录是 `target/e2e-web`。`I3T_SOURCE` 可以指向别的源码目录（例如 `git archive HEAD` 导出的一份），`CARGO_TARGET_DIR` 换目标目录，`I3T_RELEASE_KIT` 换放的位置。

`i3t-controller` 和 `i3t` 从哪里找：环境变量 `I3T_BIN_DIR` 指的目录；没设时依次试 `web/.e2e/release-kit/old`、`target/agent-web/release`、`target/release`、`target/lead/release`，用第一个两样都在的。节点更新的测试和截图总是用 release-kit 里的那一组。

测试里的 controller 一启动就把发布源换成一个空目录：测试不连外网，节点也不会被真的发布更新掉。每个节点跑的是它自己的一份程序文件，自己更新的节点换掉的只是那一份。

`pnpm e2e` 和 `pnpm screenshots` 都会先执行一次 `pnpm build`。截图也要 release-kit：发布源上有一个用测试钥匙签的新版本，节点更新的页面才有东西可看。失败时的截图和过程记录（trace）在 `test-results/` 里，后者用 `pnpm exec playwright show-trace <trace.zip>` 打开。

## 构建

```sh
pnpm build
```

产物在 `web/dist/`：一个 `index.html` 和 `assets/` 下带哈希的文件，全是静态文件。

## controller 怎么提供界面

界面由 controller 自己从管理端口（默认 `127.0.0.1:20332`）提供，不需要另外的 Web 服务器。`/api/v1` 之外的路径取静态文件；不是文件的路径一律回 `index.html`，交给前端的路由。静态文件不需要管理令牌，里面没有任何数据。

界面的文件有两个来路：

1. **编进 controller 的可执行文件**（正式发布是这样）。构建 `i3t-controller` 时，如果 `web/dist` 存在，里面的文件会被打进可执行文件，运行时不需要再带着它。所以要先构建界面，再构建 controller：

   ```sh
   (cd web && pnpm install && pnpm build)
   cargo build --release -p i3t-controller-bin
   ```

   构建 controller 时没有 `web/dist` 也能构建成功，只是这样的 controller 不带界面，打开管理地址看到的是一页说明。

2. **运行时用 `--static-dir` 指定一个目录**。给了就用这个目录，而不用程序自带的那一份。适合改界面时反复看效果（`pnpm build` 之后刷新页面就是新的，不用重新编译 controller），或者想换一套界面：

   ```sh
   i3t-controller --data-dir <数据目录> --public-addr <主机:端口> --static-dir web/dist
   ```

   目录里必须有 `index.html`，否则 controller 启动时就报错。

管理端口默认只在本机可达。要从别的机器打开界面，用 SSH 端口转发（`ssh -L 20332:127.0.0.1:20332 <controller 所在的机器>`），然后访问 `http://127.0.0.1:20332`。
