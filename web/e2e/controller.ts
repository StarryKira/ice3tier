import { type ChildProcess, spawn, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 测试用的 controller 和节点：真的程序，真的管理 API，不是模拟。
 *
 * 程序从 `I3T_BIN_DIR` 找；没设时依次试仓库里的 `target/agent-web/release`、`target/release` 和 `target/lead/release`，
 * 用第一个两样程序都在的目录。
 * 界面用的是 `pnpm build` 的产物，由 controller 自己从 `--static-dir` 提供，和正式部署时一样。
 */

const here = dirname(fileURLToPath(import.meta.url))
const webRoot = resolve(here, '..')
const repoRoot = resolve(webRoot, '..')

function binDir(): string {
  const candidates = [
    process.env.I3T_BIN_DIR,
    join(repoRoot, 'target/agent-web/release'),
    join(repoRoot, 'target/release'),
    join(repoRoot, 'target/lead/release'),
  ].filter((dir): dir is string => dir !== undefined && dir !== '')
  for (const dir of candidates) {
    if (['i3t-controller', 'i3t'].every((name) => existsSync(join(dir, name)))) return dir
  }
  throw new Error(
    `找不到 i3t-controller 和 i3t。先构建：cargo build --release -p i3t-controller-bin -p i3t，或者用 I3T_BIN_DIR 指出它们在哪。试过的目录：${candidates.join('、')}`,
  )
}

function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer()
    server.unref()
    server.on('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      server.close(() => {
        if (address !== null && typeof address === 'object') resolvePort(address.port)
        else reject(new Error('拿不到空闲端口'))
      })
    })
  })
}

async function waitFor<T>(
  what: string,
  probe: () => Promise<T | undefined> | T | undefined,
  timeoutMs = 15_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const value = await probe()
    if (value !== undefined) return value
    if (Date.now() > deadline) throw new Error(`等了 ${timeoutMs} 毫秒，${what}还是没有发生`)
    await new Promise((done) => setTimeout(done, 100))
  }
}

export interface NodeHandle {
  /** 节点的状态目录。 */
  dir: string
  /** `i3t join` 打印的内容。 */
  output: string
  /** 节点跑起来之后，它本机 SOCKS5 入口的端口。 */
  socksPort?: number
}

export class Controller {
  readonly baseUrl: string
  readonly adminToken: string
  readonly dir: string
  /** 这次用的 `i3t-controller` 和 `i3t` 所在的目录。 */
  readonly bins: string
  private readonly process: ChildProcess
  private readonly nodes: ChildProcess[] = []
  private nodeCount = 0

  private constructor(options: {
    baseUrl: string
    adminToken: string
    dir: string
    bins: string
    process: ChildProcess
  }) {
    this.baseUrl = options.baseUrl
    this.adminToken = options.adminToken
    this.dir = options.dir
    this.bins = options.bins
    this.process = options.process
  }

  /** 启动一个全新的、空的 controller。管理令牌从它第一次启动时打印的内容里取。 */
  static async start(): Promise<Controller> {
    const bins = binDir()
    const dist = join(webRoot, 'dist')
    if (!existsSync(join(dist, 'index.html'))) throw new Error('web/dist 里没有构建产物，先执行 pnpm build')

    const dir = mkdtempSync(join(tmpdir(), 'i3t-web-e2e-'))
    const nodePort = await freePort()
    const adminPort = await freePort()
    const child = spawn(
      join(bins, 'i3t-controller'),
      [
        '--data-dir',
        join(dir, 'controller'),
        '--public-addr',
        `127.0.0.1:${nodePort}`,
        '--listen',
        `127.0.0.1:${nodePort}`,
        '--admin-listen',
        `127.0.0.1:${adminPort}`,
        '--static-dir',
        dist,
      ],
      { stdio: ['ignore', 'pipe', 'pipe'] },
    )
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (chunk: Buffer) => (stdout += chunk.toString()))
    child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
    let exited: number | null = null
    child.on('exit', (code) => (exited = code ?? -1))

    const baseUrl = `http://127.0.0.1:${adminPort}`
    try {
      // 令牌单独占一行，前面有缩进：43 个 base64url 字符。
      const adminToken = await waitFor('controller 打印管理令牌', () => {
        if (exited !== null) throw new Error(`controller 启动后就退出了（${exited}）：\n${stderr}`)
        return /^\s+([A-Za-z0-9_-]{43})\s*$/m.exec(stdout)?.[1]
      })
      await waitFor('管理 API 开始应答', async () => {
        try {
          const response = await fetch(`${baseUrl}/api/v1/status`, {
            headers: { Authorization: `Bearer ${adminToken}` },
          })
          return response.ok ? true : undefined
        } catch {
          return undefined
        }
      })
      return new Controller({ baseUrl, adminToken, dir, bins, process: child })
    } catch (error) {
      child.kill('SIGKILL')
      rmSync(dir, { recursive: true, force: true })
      throw error
    }
  }

  /** 直接调管理 API：用来准备数据，或者核对界面做的事真的落到了 controller 上。 */
  async api<T>(method: string, path: string, body?: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl}/api/v1${path}`, {
      method,
      headers: { Authorization: `Bearer ${this.adminToken}`, 'Content-Type': 'application/json' },
      body: method === 'GET' || method === 'DELETE' ? undefined : JSON.stringify(body ?? {}),
    })
    const text = await response.text()
    if (!response.ok) throw new Error(`${method} ${path} → ${response.status} ${text}`)
    return (text === '' ? undefined : JSON.parse(text)) as T
  }

  /**
   * 在一个新的状态目录里执行界面上给出的那一行加入命令（`echo <令牌> | i3t join …`），原样交给 shell。
   * join 只和 controller 说话，不需要把节点跑起来。
   */
  join(command: string): NodeHandle {
    this.nodeCount += 1
    const dir = join(this.dir, `node-${this.nodeCount}`)
    mkdirSync(dir, { recursive: true })
    const result = spawnSync('sh', ['-c', command], {
      env: { ...process.env, PATH: `${this.bins}:${process.env.PATH ?? ''}`, I3T_STATE_DIR: dir },
      encoding: 'utf8',
    })
    if (result.status !== 0)
      throw new Error(`加入命令失败（${result.status}）：${command}\n${result.stdout}\n${result.stderr}`)
    return { dir, output: result.stdout }
  }

  /** 生成一个令牌并用它加入：准备数据时用，不经过界面。 */
  async enroll(options: { tags?: string[]; flags?: string[] } = {}, network = 1): Promise<NodeHandle> {
    const created = await this.api<{ token: string }>('POST', `/networks/${network}/tokens`, {
      tags: options.tags ?? [],
    })
    return this.join(`echo ${created.token} | i3t join ${(options.flags ?? []).join(' ')}`)
  }

  /** 在节点上改本地声明，和用户在那台机器上敲 `i3t set …` 一样。 */
  set(node: NodeHandle, capability: 'can-exit' | 'can-relay' | 'can-forward', on: boolean): void {
    const result = spawnSync(join(this.bins, 'i3t'), ['--state-dir', node.dir, 'set', capability, on ? 'on' : 'off'], {
      encoding: 'utf8',
    })
    if (result.status !== 0) throw new Error(`i3t set 失败：${result.stderr}`)
  }

  /**
   * 把一个已经加入的节点跑起来，让它在界面上显示为在线。
   * 一台机器上跑好几个节点，所以先把它的端口改成不冲突的。
   */
  async run(node: NodeHandle): Promise<void> {
    const configPath = join(node.dir, 'config.toml')
    const udp = await freePort()
    const socks = await freePort()
    const http = await freePort()
    const config = readFileSync(configPath, 'utf8')
      .replace(/^port = \d+$/m, `port = ${udp}`)
      .replace(/^socks5 = .*$/m, `socks5 = "127.0.0.1:${socks}"`)
      .replace(/^http = .*$/m, `http = "127.0.0.1:${http}"`)
    writeFileSync(configPath, config)
    const child = spawn(join(this.bins, 'i3t'), ['--state-dir', node.dir, 'run'], { stdio: 'ignore' })
    this.nodes.push(child)
    node.socksPort = socks
  }

  /**
   * 让 `from` 经它本机的代理入口去连 `targetVirtualIp` 上一个没人监听的端口。
   * 连接本身会被拒绝，但两个节点之间因此建立了连接，随后各自上报到对方的路径：路由表里就有了真实的数据。
   */
  touch(from: NodeHandle, targetVirtualIp: string): void {
    if (from.socksPort === undefined) throw new Error('这个节点还没有跑起来')
    spawnSync('curl', [
      '-s',
      '-m',
      '8',
      '-o',
      '/dev/null',
      '-x',
      `socks5h://127.0.0.1:${from.socksPort}`,
      `http://${targetVirtualIp}:9/`,
    ])
  }

  /** 等到 controller 看到这么多节点在线。 */
  async waitOnline(count: number, network = 1): Promise<void> {
    await waitFor(`${count} 个节点在线`, async () => {
      const overview = await this.api<{ online: number }>('GET', `/networks/${network}/overview`)
      return overview.online >= count ? true : undefined
    })
  }

  stop(): void {
    for (const node of this.nodes) node.kill('SIGTERM')
    this.process.kill('SIGTERM')
    rmSync(this.dir, { recursive: true, force: true })
  }
}
