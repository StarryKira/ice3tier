import { type Proxy, type ProxyProtocol } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

/** 状态码的色调：在监听是绿的；用户该去看一眼的（端口被占、出口不行、策略不允许、没有出口）是黄或红。 */
const TONE: Record<string, Tone> = {
  listening: 'ok',
  disabled: 'idle',
  pending: 'idle',
  node_offline: 'idle',
  port_in_use: 'danger',
  bind_failed: 'danger',
  exit_disabled: 'warn',
  exit_offline: 'warn',
  access_denied: 'danger',
  exit_missing: 'warn',
}

/** 要用户去处理的状态：总览上提一句。停用的、等下发的、入口离线的不算。 */
const NEEDS_ATTENTION = new Set([
  'port_in_use',
  'bind_failed',
  'exit_disabled',
  'exit_offline',
  'access_denied',
  'exit_missing',
])

export interface ProxyStatusView {
  /** `t.proxies.status` 里的键；认不出的码是 `unknown`。 */
  code: keyof typeof t.proxies.status
  tone: Tone
  needsAttention: boolean
}

/** 一个代理现在怎样：看 controller 给的状态码（它已经把停用、没有出口这些按先后排好了）。 */
export function proxyStatus(proxy: Pick<Proxy, 'enabled' | 'status'>): ProxyStatusView {
  const code = proxy.status?.code ?? (proxy.enabled ? 'pending' : 'disabled')
  if (!Object.hasOwn(t.proxies.status, code)) return { code: 'unknown', tone: 'idle', needsAttention: false }
  return {
    code: code as keyof typeof t.proxies.status,
    tone: TONE[code] ?? 'idle',
    needsAttention: NEEDS_ATTENTION.has(code),
  }
}

/**
 * 设备里要填的代理地址：`socks5://用户名:密码@主机:端口`。自动识别的代理写成 SOCKS5 的（HTTP 也能用同一个地址）。
 * 用户名和密码按 URL 的规矩转义；没有密码的不写那一段。
 */
export function proxyUrl(parts: {
  protocol: ProxyProtocol
  host: string
  port: number
  username: string
  password: string
}): string {
  const scheme = parts.protocol === 'http' ? 'http' : 'socks5'
  const auth =
    parts.username === '' ? '' : `${encodeURIComponent(parts.username)}:${encodeURIComponent(parts.password)}@`
  const host = parts.host.includes(':') && !parts.host.startsWith('[') ? `[${parts.host}]` : parts.host
  return `${scheme}://${auth}${host}:${parts.port}`
}

const ALPHABET = 'abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'

/** 随机的用户名、密码：去掉了容易看错的字符（0 和 O、1 和 l）。 */
export function randomSecret(length: number): string {
  const bytes = new Uint8Array(length)
  crypto.getRandomValues(bytes)
  return Array.from(bytes, (byte) => ALPHABET[byte % ALPHABET.length]).join('')
}
