import { spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 节点更新的端到端测试要的东西：认测试钥匙的程序、版本号高一级的节点程序、签清单的工具和钥匙。
 * 由 `pnpm e2e:kit`（`e2e/release-kit.sh`）编出来，放在 `I3T_RELEASE_KIT`，默认 `web/.e2e/release-kit`。
 */

const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

export function kitDir(): string {
  const configured = process.env.I3T_RELEASE_KIT
  return configured !== undefined && configured !== '' ? configured : join(webRoot, '.e2e', 'release-kit')
}

export interface ReleaseKit {
  /** 现在的版本：controller、i3t、i3tctl，都认测试钥匙。 */
  oldBins: string
  oldVersion: string
  /** 版本号高一级的 i3t。 */
  newProgram: string
  newVersion: string
  tool: string
  key: string
}

export function releaseKit(): ReleaseKit {
  const dir = kitDir()
  const versions = join(dir, 'versions')
  if (!existsSync(versions)) {
    throw new Error(
      `${dir} 里没有更新测试要的程序。先在 web/ 里执行 pnpm e2e:kit（要能编译 Rust），或者用 I3T_RELEASE_KIT 指出它在哪。`,
    )
  }
  const [oldVersion = '', newVersion = ''] = readFileSync(versions, 'utf8').trim().split(/\s+/)
  return {
    oldBins: join(dir, 'old'),
    oldVersion,
    newProgram: join(dir, 'new', 'i3t'),
    newVersion,
    tool: join(dir, 'i3t-release'),
    key: join(dir, 'release.key'),
  }
}

/** 节点自己报的平台名：`系统-架构`，发布包的文件名是 `i3t-<平台>`。 */
export function platformName(): string {
  const arch = { x64: 'x86_64', arm64: 'aarch64' }[process.arch as 'x64' | 'arm64'] ?? process.arch
  const system = process.platform === 'darwin' ? 'macos' : process.platform
  return `${system}-${arch}`
}

/** 一个本机目录里的发布源，布局见 docs/releasing.md 第 6 节。 */
export class ReleaseSource {
  readonly root: string
  private readonly kit: ReleaseKit

  constructor(root: string, kit: ReleaseKit) {
    this.root = root
    this.kit = kit
    mkdirSync(join(root, 'latest', 'download'), { recursive: true })
  }

  /** 发布 `version`，节点程序是 `program`：先放好这个版本的文件，再把清单放到"最新"的位置上。 */
  publish(version: string, program: string): void {
    const directory = join(this.root, 'download', `v${version}`)
    mkdirSync(directory, { recursive: true })
    copyFileSync(program, join(directory, `i3t-${platformName()}`))
    const signed = spawnSync(
      this.kit.tool,
      ['manifest', '--version', version, '--dir', directory, '--key', this.kit.key],
      {
        encoding: 'utf8',
      },
    )
    if (signed.status !== 0) throw new Error(`签发布清单失败：${signed.stderr}`)
    for (const name of ['manifest.json', 'manifest.json.sig']) {
      copyFileSync(join(directory, name), join(this.root, 'latest', 'download', name))
    }
  }
}
