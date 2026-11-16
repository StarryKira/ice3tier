import { type Version } from '@/api/types'

/** 这个界面自己的版本号：构建时从 package.json 写进来（见 vite.config.ts），全站只从这里取。 */
export const WEB_VERSION: Version = __APP_VERSION__

interface Parsed {
  core: [number, number, number]
  /** 预发布标识按 `.` 分开的各段。正式版是空的。 */
  pre: string[]
}

const PATTERN = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/

function parse(version: string): Parsed | null {
  const match = PATTERN.exec(version.trim())
  if (match === null) return null
  return {
    core: [Number(match[1]), Number(match[2]), Number(match[3])],
    pre: match[4] === undefined ? [] : match[4].split('.'),
  }
}

const NUMERIC = /^\d+$/

function comparePre(a: readonly string[], b: readonly string[]): number {
  // 正式版比同样三个数字的预发布版大。
  if (a.length === 0 || b.length === 0) return b.length - a.length
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    const x = a[index] ?? ''
    const y = b[index] ?? ''
    if (x === y) continue
    const xNumeric = NUMERIC.test(x)
    const yNumeric = NUMERIC.test(y)
    if (xNumeric && yNumeric) return Number(x) - Number(y)
    // 纯数字的段比带字母的段小。
    if (xNumeric !== yNumeric) return xNumeric ? -1 : 1
    return x < y ? -1 : 1
  }
  return a.length - b.length
}

/**
 * 按语义化版本比较，和 controller 的规则一样：先比三个数字，数字相同时预发布版比正式版小，
 * 预发布标识逐段比。返回负数、0、正数。写法不对的版本号比不了，返回 `null`。
 */
export function compareVersions(a: Version, b: Version): number | null {
  const x = parse(a)
  const y = parse(b)
  if (x === null || y === null) return null
  for (let index = 0; index < 3; index += 1) {
    const diff = (x.core[index] ?? 0) - (y.core[index] ?? 0)
    if (diff !== 0) return Math.sign(diff)
  }
  return Math.sign(comparePre(x.pre, y.pre))
}

/** `version` 比 `than` 旧。比不了（节点没报版本号、写法不对）时算不旧：不知道的事不报警。 */
export function isOlder(version: Version, than: Version): boolean {
  const order = compareVersions(version, than)
  return order !== null && order < 0
}
