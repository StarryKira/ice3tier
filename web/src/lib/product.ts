/**
 * 界面上会写出来的、关于产品本身的固定值。改默认端口、换发布地址时只改这里。
 */

/** 项目的发布页。发布包的文件名不带版本号，所以"最新版的某个文件"有一个固定的下载地址。 */
export const RELEASE_BASE = 'https://github.com/fuqiuluo/ice3tier/releases/latest/download'
export const RELEASES_PAGE = 'https://github.com/fuqiuluo/ice3tier/releases'

/** 节点绑定的端口的默认值。启用为 relay 的节点还在同一个端口号上监听 TCP。 */
export const DEFAULT_NODE_PORT = 20330

/** 每个节点自带的本机代理入口的默认地址（写在节点的 config.toml 里，可以改）。 */
export const LOCAL_SOCKS5 = '127.0.0.1:1080'
export const LOCAL_HTTP = '127.0.0.1:1081'

/** 注册令牌默认的有效期，秒。 */
export const DEFAULT_TOKEN_TTL = 24 * 60 * 60

/** 节点删除之后，它的名字和虚拟 IP 多少天之内不分给别的节点。 */
export const COOLDOWN_DAYS = 90

export const MAX_RELAY_HOPS = { min: 1, max: 4 } as const
export const MAX_RELAY_ADDRESSES = 8
export const MAX_PINNED_RELAYS = 8

/** 节点多久上报一次。轮询得比这更勤没有意义。 */
export const REPORT_INTERVAL_SECS = 10

export type Capability = 'relay' | 'exit' | 'forward'

/** 节点本地声明用的名字：`i3t join --can-exit`、`i3t set can-exit on`、config.toml 里 `[declare]` 下的键。 */
export const CAPABILITY_FLAG: Record<Capability, string> = {
  relay: 'can-relay',
  exit: 'can-exit',
  forward: 'can-forward',
}
export const CAPABILITY_CONFIG_KEY: Record<Capability, string> = {
  relay: 'relay',
  exit: 'exit',
  forward: 'forward',
}

export interface Platform {
  id: string
  /** 发布包的文件名。 */
  file: string
  windows: boolean
}

export const PLATFORMS = [
  { id: 'linux-x86_64', file: 'i3t-linux-x86_64', windows: false },
  { id: 'linux-aarch64', file: 'i3t-linux-aarch64', windows: false },
  { id: 'macos-aarch64', file: 'i3t-macos-aarch64', windows: false },
  { id: 'macos-x86_64', file: 'i3t-macos-x86_64', windows: false },
  { id: 'windows-x86_64', file: 'i3t-windows-x86_64.exe', windows: true },
] as const satisfies readonly Platform[]

export type PlatformId = (typeof PLATFORMS)[number]['id']
