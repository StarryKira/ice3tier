# ice3tier WebUI

controller 的管理界面。它只和 controller 的管理 API 说话（见 [`docs/admin-api.md`](../docs/admin-api.md)），和 API 同源，每个请求带 `Authorization: Bearer <管理令牌>`。界面语言是简体中文。

技术栈：React、TypeScript、Vite、TanStack Query、React Router、Tailwind、shadcn/ui 风格的部件（Radix）、React Flow（访问关系图）。包管理用 `pnpm`。

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
| `src/lib/`        | 校验、地址段的计算、给用户执行的命令、时间的写法等                                                                                                                     |
| `src/i18n/`       | 界面上的全部文字。组件里不直接写中文                                                                                                                                   |
| `src/test/`       | 单元测试的准备和假的管理 API                                                                                                                                           |
| `e2e/`            | Playwright 的端到端测试和截图                                                                                                                                          |
| `screenshots/`    | `pnpm screenshots` 截下来的每个页面，给人看版面                                                                                                                        |

几条约定：

- 管理 API 变了，`src/api/types.ts` 和 `src/api/endpoints.ts` 要跟着改，它们是照着文档手写的。
- 提交之前的校验（`src/lib/validate.ts`）和 controller 的规则一致，只是为了早一点指出错误；controller 拒绝时显示它给的原因。
- controller 还没有接口的页面（拓扑、路径诊断、连通性测试、节点更新）是"计划中"的说明页，不放假数据。接口有了之后，在 `src/routes.tsx` 里把对应的那一行换成真正的页面。

## 检查和测试

```sh
pnpm typecheck    # TypeScript
pnpm lint         # ESLint
pnpm test         # 单元测试和组件测试（Vitest + Testing Library，jsdom）
pnpm e2e          # 端到端测试（Playwright，无头 Chromium）
pnpm screenshots  # 重新截一遍每个页面，放到 screenshots/
```

**单元测试**和被测的代码放在一起，文件名是 `*.test.ts` / `*.test.tsx`。组件测试不连真的 controller：`src/test/harness.tsx` 里的 `fakeApi` 接住 `fetch`，按"方法 路径"应答。改代码时可以用 `pnpm test:watch`。

**端到端测试**用的是真的程序：每次运行自己启动一个全新的 controller（数据在临时目录里），用 `pnpm build` 的产物作界面，把界面上给出的加入命令原样交给 shell 执行，结束时把 controller 和节点都停掉、临时目录删掉。所以运行之前不需要开着什么，但要先构建好 Rust 的程序：

```sh
cargo build --release -p i3t-controller-bin -p i3t   # 在仓库根目录
pnpm exec playwright install chromium                # 只在第一次
pnpm e2e
```

`i3t-controller` 和 `i3t` 从哪里找：环境变量 `I3T_BIN_DIR` 指的目录；没设时依次试 `target/agent-web/release`、`target/release`、`target/lead/release`，用第一个两样都在的。

`pnpm e2e` 和 `pnpm screenshots` 都会先执行一次 `pnpm build`。失败时的截图和过程记录（trace）在 `test-results/` 里，后者用 `pnpm exec playwright show-trace <trace.zip>` 打开。

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
