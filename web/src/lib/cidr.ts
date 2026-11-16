/**
 * IPv4 地址和地址段的解析与比较。
 *
 * 只用来在提交之前把明显的错误指出来（地址段重叠、虚拟 IP 不在地址段里），省一次往返；
 * controller 仍然是最后的裁判，它拒绝时显示它给的原因。
 */

export interface Cidr {
  /** 网络地址，32 位无符号整数。 */
  network: number
  prefix: number
}

const OCTET = /^(0|[1-9]\d{0,2})$/

export function parseIpv4(text: string): number | null {
  const parts = text.trim().split('.')
  if (parts.length !== 4) return null
  let value = 0
  for (const part of parts) {
    if (!OCTET.test(part)) return null
    const octet = Number(part)
    if (octet > 255) return null
    value = value * 256 + octet
  }
  return value
}

export function formatIpv4(value: number): string {
  return [value >>> 24, (value >>> 16) & 255, (value >>> 8) & 255, value & 255].join('.')
}

function mask(prefix: number): number {
  return prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0
}

function size(prefix: number): number {
  return 2 ** (32 - prefix)
}

/** 解析 `10.66.0.0/16` 这样的写法。主机位不是 0 的会被规整成网络地址，和 controller 的做法一样。 */
export function parseCidr(text: string): Cidr | null {
  const match = /^([^/]+)\/(\d{1,2})$/.exec(text.trim())
  if (match === null) return null
  const address = parseIpv4(match[1] ?? '')
  const prefix = Number(match[2])
  if (address === null || prefix > 32) return null
  return { network: (address & mask(prefix)) >>> 0, prefix }
}

export function formatCidr(cidr: Cidr): string {
  return `${formatIpv4(cidr.network)}/${cidr.prefix}`
}

export function cidrContainsAddress(cidr: Cidr, address: number): boolean {
  return (address & mask(cidr.prefix)) >>> 0 === cidr.network
}

/** `outer` 是否完整地包含 `inner`（相等也算）。 */
export function cidrContains(outer: Cidr, inner: Cidr): boolean {
  return outer.prefix <= inner.prefix && cidrContainsAddress(outer, inner.network)
}

export function cidrOverlaps(a: Cidr, b: Cidr): boolean {
  return cidrContains(a, b) || cidrContains(b, a)
}

/** 地址段里的网络地址和广播地址不能分给节点。 */
export function isNetworkOrBroadcast(cidr: Cidr, address: number): boolean {
  return address === cidr.network || address === cidr.network + size(cidr.prefix) - 1
}

/** 运营商级 NAT 的保留段。别的组网工具和家宽的 WAN 口常常落在这里，用它当虚拟地址段容易冲突。 */
export const SHARED_ADDRESS_SPACE: Cidr = { network: 0x64400000, prefix: 10 }

/** controller 接受的最长前缀：再小就放不下两个节点了。 */
export const MAX_NETWORK_PREFIX = 30

/**
 * 新建网络时预填的地址段：依次试 10.66.0.0/16、10.67.0.0/16……取第一个不和已有网络重叠的。
 * 和 controller 在省略 `cidr` 时的做法相同，所以预填的值就是不填时会得到的值。
 */
export function suggestCidr(existing: readonly string[]): string {
  const taken = existing.map(parseCidr).filter((cidr): cidr is Cidr => cidr !== null)
  for (let second = 66; second <= 255; second += 1) {
    const candidate: Cidr = { network: ((10 << 24) | (second << 16)) >>> 0, prefix: 16 }
    if (!taken.some((other) => cidrOverlaps(candidate, other))) return formatCidr(candidate)
  }
  return ''
}

export interface SocketAddr {
  host: string
  port: number
  ipv6: boolean
}

/** 解析 `203.0.113.7:20330` 和 `[2001:db8::1]:20330`。主机部分只接受 IP 地址。 */
export function parseSocketAddr(text: string): SocketAddr | null {
  const value = text.trim()
  const v6 = /^\[([0-9a-fA-F:.]+)\]:(\d{1,5})$/.exec(value)
  if (v6 !== null) {
    const port = Number(v6[2])
    const host = v6[1] ?? ''
    if (port > 65535 || !host.includes(':')) return null
    return { host, port, ipv6: true }
  }
  const v4 = /^([^:]+):(\d{1,5})$/.exec(value)
  if (v4 === null) return null
  const host = v4[1] ?? ''
  const port = Number(v4[2])
  if (parseIpv4(host) === null || port > 65535) return null
  return { host, port, ipv6: false }
}

export function formatSocketAddr(host: string, port: number): string {
  return host.includes(':') ? `[${host}]:${port}` : `${host}:${port}`
}

export function isLoopbackHost(host: string): boolean {
  if (host === '::1') return true
  const address = parseIpv4(host)
  return address !== null && address >>> 24 === 127
}

export function isUnspecifiedHost(host: string): boolean {
  return host === '0.0.0.0' || host === '::'
}

/** 一个监听地址是不是只有本机能连上。解析不了时按"不是"算，宁可多提醒一句。 */
export function isLoopbackBind(bind: string): boolean {
  const parsed = parseSocketAddr(bind)
  return parsed !== null && isLoopbackHost(parsed.host)
}
