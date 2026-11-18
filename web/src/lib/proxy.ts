import { type Listener, type NodeId, type ProxyProtocol } from '@/api/types'

/**
 * 一个代理（监听器）从哪个出口出去：固定了出口的就是它；没固定、默认动作是走某个出口的，是那个出口。
 * 别的（就地出站、拒绝、按用户名选）不算哪个出口的代理。
 */
export function exitOf(listener: Pick<Listener, 'fixed_exit' | 'default_action'>): NodeId | null {
  if (listener.fixed_exit !== null) return listener.fixed_exit
  return listener.default_action?.action === 'exit' ? listener.default_action.node : null
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
