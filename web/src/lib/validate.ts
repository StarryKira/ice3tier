import { t } from '@/i18n'

import { type ByteUnit, unitBytes } from './format'

import {
  cidrContainsAddress,
  isNetworkOrBroadcast,
  isUnspecifiedHost,
  MAX_NETWORK_PREFIX,
  parseCidr,
  parseIpv4,
  parseSocketAddr,
} from './cidr'

/**
 * 提交之前的校验。规则和 controller 的一致；
 * 每个函数返回一句可以直接显示在输入框下面的话，没有问题时返回 `null`。
 */

export const MAX_NAME_LEN = 63
export const MAX_GROUP_NAME_LEN = 32
export const MAX_TITLE_LEN = 64

/** 网络和节点的名字。 */
export function validateName(raw: string): string | null {
  const name = raw.trim().toLowerCase()
  if (name === '') return t.validate.nameRequired
  if (name.length > MAX_NAME_LEN) return t.validate.nameTooLong(MAX_NAME_LEN)
  if (!/^[a-z0-9-]+$/.test(name)) return t.validate.nameCharset
  if (name.startsWith('-') || name.endsWith('-')) return t.validate.nameHyphen
  if (/^\d+$/.test(name)) return t.validate.nameAllDigits
  return null
}

/**
 * 分组的名字：去掉首尾空白之后 1 到 32 个字符，字母（任何文字的）、数字、连字符、下划线。
 * 和 controller 一样按 Unicode 数字符（码点，不是 UTF-16 的单元），字母和数字按 Unicode 的属性认
 * （Rust 的 `char::is_alphanumeric`：Alphabetic 或者 Numeric），所以要用 `\p{Alphabetic}` 而不是 `\p{L}`。
 * 大小写保留；不分大小写的重名（包括内置的 `All`）由 controller 判断，它知道网络里现在有哪些分组。
 */
export function validateGroupName(raw: string): string | null {
  const name = raw.trim()
  if (name === '') return t.validate.groupRequired
  if (Array.from(name).length > MAX_GROUP_NAME_LEN) return t.validate.groupTooLong(MAX_GROUP_NAME_LEN)
  if (!/^[\p{Alphabetic}\p{N}_-]+$/u.test(name)) return t.validate.groupCharset
  return null
}

/** 规则、凭据、监听器、端口转发的名字。 */
export function validateTitle(raw: string, required: boolean): string | null {
  const title = raw.trim()
  if (required && title === '') return t.validate.titleRequired
  if (Array.from(title).length > MAX_TITLE_LEN) return t.validate.titleTooLong(MAX_TITLE_LEN)
  return null
}

/** 网络的地址段。 */
export function validateNetworkCidr(raw: string): string | null {
  if (raw.trim() === '') return t.validate.cidrRequired
  const cidr = parseCidr(raw)
  if (cidr === null) return t.validate.cidrFormat
  if (cidr.prefix > MAX_NETWORK_PREFIX) return t.validate.cidrTooSmall(MAX_NETWORK_PREFIX)
  return null
}

/** 节点的虚拟 IP：要在网络的地址段里，不能是网络地址和广播地址。 */
export function validateVirtualIp(raw: string, networkCidr: string): string | null {
  const address = parseIpv4(raw)
  if (address === null) return t.validate.ipFormat
  const cidr = parseCidr(networkCidr)
  if (cidr === null) return null
  if (!cidrContainsAddress(cidr, address)) return t.validate.ipOutOfRange(networkCidr)
  if (isNetworkOrBroadcast(cidr, address)) return t.validate.ipReserved
  return null
}

export function validatePort(raw: string): string | null {
  if (!/^\d{1,5}$/.test(raw.trim())) return t.validate.portFormat
  const port = Number(raw)
  if (port < 1 || port > 65535) return t.validate.portFormat
  return null
}

/** `地址:端口`，地址不能是 0.0.0.0，端口不能是 0。给 relay 的公网地址和转发的目标地址用。 */
export function validateSocketAddr(raw: string): string | null {
  const parsed = parseSocketAddr(raw)
  if (parsed === null) return t.validate.socketAddrFormat
  if (parsed.port === 0) return t.validate.portFormat
  if (isUnspecifiedHost(parsed.host)) return t.validate.socketAddrUnspecified
  return null
}

/** 把"一行一个，或者用逗号、空格隔开"的输入拆成一项一项。 */
export function splitList(raw: string): string[] {
  return raw
    .split(/[\s,，;；]+/)
    .map((item) => item.trim())
    .filter((item) => item !== '')
}

/** 来源白名单：每一项是一个地址段或者单个地址。返回第一处错误。 */
export function validateCidrList(items: readonly string[]): string | null {
  for (const item of items) {
    const ok = item.includes('/') ? parseCidr(item) !== null : parseIpv4(item) !== null
    // IPv6 的写法这里不细查，交给 controller。
    if (!ok && !item.includes(':')) return t.validate.cidrListItem(item)
  }
  return null
}

/** 来源白名单里单个地址补成 /32，和 controller 要的写法一致。 */
export function normalizeCidrList(items: readonly string[]): string[] {
  return items.map((item) => (item.includes('/') || item.includes(':') ? item : `${item}/32`))
}

/** 出口允许的端口：`80, 443` 这样的列表。 */
export function parsePortList(raw: string): { ports: number[]; error: string | null } {
  const ports: number[] = []
  for (const item of splitList(raw)) {
    if (validatePort(item) !== null) return { ports: [], error: t.validate.portListItem(item) }
    ports.push(Number(item))
  }
  if (ports.length === 0) return { ports: [], error: t.validate.portListEmpty }
  return { ports: [...new Set(ports)].sort((a, b) => a - b), error: null }
}

/** 可以留空的正整数（带宽上限、并发上限）。留空返回 `null`。 */
export function parseOptionalPositiveInt(raw: string): { value: number | null; error: string | null } {
  const text = raw.trim()
  if (text === '') return { value: null, error: null }
  if (!/^\d{1,9}$/.test(text) || Number(text) < 1) return { value: null, error: t.validate.positiveInt }
  return { value: Number(text), error: null }
}

/**
 * 可以留空的流量上限：一个正数（可以带小数）乘上单位。留空返回 `null`。
 * 结果按字节取整，至少 1 字节，因为 controller 不收 0。
 */
export function parseOptionalBytes(raw: string, unit: ByteUnit): { value: number | null; error: string | null } {
  const text = raw.trim()
  if (text === '') return { value: null, error: null }
  if (!/^\d{1,9}(\.\d{1,3})?$/.test(text) || Number(text) <= 0) return { value: null, error: t.validate.byteSize }
  return { value: Math.max(1, Math.round(Number(text) * unitBytes(unit))), error: null }
}

/** 出口账期从每个月的哪一天开始：1 到 28，每个月都有这一天。 */
export const MAX_QUOTA_RESET_DAY = 28

export function parseResetDay(raw: string): { value: number; error: string | null } {
  const text = raw.trim()
  const day = Number(text)
  if (!/^\d{1,2}$/.test(text) || day < 1 || day > MAX_QUOTA_RESET_DAY)
    return { value: 1, error: t.validate.dayOfMonth(MAX_QUOTA_RESET_DAY) }
  return { value: day, error: null }
}
