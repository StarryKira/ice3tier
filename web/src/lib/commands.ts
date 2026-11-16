import { type Listener } from '@/api/types'

import { parseSocketAddr } from './cidr'
import {
  type Capability,
  CAPABILITY_CONFIG_KEY,
  CAPABILITY_FLAG,
  LOCAL_SOCKS5,
  PLATFORMS,
  type PlatformId,
  RELEASE_BASE,
} from './product'

/**
 * 界面上给出的、要用户拿到机器上去执行的命令。都是纯函数：同样的输入给出同样的一行，方便测试。
 * 显示在屏幕上的和复制到剪贴板的是同一个字符串，始终是一行。
 */

/** 加入时在本地声明"这台机器可以承担"的三项。它们是命令的参数，不属于令牌。 */
export interface JoinDeclarations {
  exit: boolean
  relay: boolean
  forward: boolean
}

export const NO_DECLARATIONS: JoinDeclarations = { exit: false, relay: false, forward: false }

/** 声明对应的命令行参数，顺序固定（和 `i3t join --help` 列出的一致）。 */
export function joinFlags(declare: JoinDeclarations): string[] {
  const order: Capability[] = ['exit', 'relay', 'forward']
  return order.filter((capability) => declare[capability]).map((capability) => `--${CAPABILITY_FLAG[capability]}`)
}

function platform(id: PlatformId) {
  return PLATFORMS.find((candidate) => candidate.id === id) ?? PLATFORMS[0]
}

/** 这个平台上怎么调用节点程序：Unix 上装进了 PATH；Windows 上在下载到的目录里直接执行。 */
export function programFor(id: PlatformId): string {
  return platform(id).windows ? '.\\i3t.exe' : 'i3t'
}

/** 不含引号也不会被 shell 误解的字符。注册令牌是 `i3t-` 加 base64url，都在这里面。 */
const SHELL_SAFE = /^[A-Za-z0-9_.=+/-]+$/

function quote(value: string, windows: boolean): string {
  if (SHELL_SAFE.test(value)) return value
  return windows ? `"${value.replaceAll('"', '`"')}"` : `'${value.replaceAll("'", `'\\''`)}'`
}

/** 令牌还没生成时，命令里令牌的位置上显示的占位符。它只给人看，不会被复制走，所以不加引号。 */
export interface TokenPlaceholder {
  placeholder: string
}

/**
 * 加入网络的那一行命令：`echo <令牌> | i3t join [--can-exit] [--can-relay] [--can-forward]`。
 *
 * 令牌从标准输入读，所以用 echo 接管道，整行可以一次复制。
 * 令牌生成之前传占位符：先让用户看懂命令的结构，令牌会出现在哪里。
 */
export function buildJoinCommand(
  token: string | TokenPlaceholder,
  declare: JoinDeclarations,
  platformId: PlatformId = 'linux-x86_64',
): string {
  const windows = platform(platformId).windows
  const shown = typeof token === 'string' ? quote(token, windows) : token.placeholder
  return ['echo', shown, '|', programFor(platformId), 'join', ...joinFlags(declare)].join(' ')
}

/** 启动节点。 */
export function runCommand(platformId: PlatformId = 'linux-x86_64'): string {
  return `${programFor(platformId)} run`
}

/** 下载节点程序并放到能直接执行的地方。每个平台是一行或两行。 */
export function installCommands(platformId: PlatformId): string[] {
  const target = platform(platformId)
  const url = `${RELEASE_BASE}/${target.file}`
  if (target.windows) return [`curl.exe -fLo i3t.exe ${url}`]
  return [`curl -fLo i3t ${url}`, 'sudo install -m 755 i3t /usr/local/bin/i3t']
}

/** 在节点上声明（或者撤回）一项"可以承担"。 */
export function declareCommand(capability: Capability, on = true): string {
  return `i3t set ${CAPABILITY_FLAG[capability]} ${on ? 'on' : 'off'}`
}

/** 不用命令、直接改 config.toml 时要写的两行。 */
export function declareConfig(capability: Capability): string {
  return `[declare]\n${CAPABILITY_CONFIG_KEY[capability]} = true`
}

const EXAMPLE_URL = 'https://example.com'

/** 在某个节点本机，经它自带的代理入口，从 `exit` 这个出口访问互联网。用户名写出口节点的虚拟 IP。 */
export function exitUsage(exitVirtualIp: string): string {
  return `curl -x socks5h://${exitVirtualIp}@${LOCAL_SOCKS5} ${EXAMPLE_URL}`
}

interface ListenerUsageOptions {
  /** 用户名处写的出口节点（虚拟 IP）。监听器固定了出口、或者不想指定出口时不给。 */
  exit?: string
  /** 密码处写的凭据。监听器要求凭据时给；不知道明文时给一个占位符。 */
  credential?: string
  /** 监听在 0.0.0.0 上时，地址要换成这台机器实际的地址，这里给一个占位的说法。 */
  hostPlaceholder: string
}

/** 经一个监听器访问互联网的写法。SOCKS5 用 `socks5h`，域名交给出口节点解析。 */
export function listenerUsage(listener: Pick<Listener, 'bind' | 'protocol'>, options: ListenerUsageOptions): string {
  const scheme = listener.protocol === 'socks5' ? 'socks5h' : 'http'
  const parsed = parseSocketAddr(listener.bind)
  const unspecified = parsed !== null && (parsed.host === '0.0.0.0' || parsed.host === '::')
  const address = parsed !== null && unspecified ? `${options.hostPlaceholder}:${parsed.port}` : listener.bind
  let auth = ''
  if (options.credential !== undefined) auth = `${options.exit ?? ''}:${options.credential}@`
  else if (options.exit !== undefined) auth = `${options.exit}@`
  return `curl -x ${scheme}://${auth}${address} ${EXAMPLE_URL}`
}
