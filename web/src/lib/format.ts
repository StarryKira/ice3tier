const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const

/** 字节数写成人看的大小，按 1024 进位。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1
  return `${value.toFixed(digits)} ${UNITS[unit] ?? 'B'}`
}

/** 公钥太长，界面上只显示开头的一段用来辨认；复制出去的是完整的。 */
export function shortKey(key: string): string {
  return key.length <= 12 ? key : `${key.slice(0, 10)}…`
}

/** 输入流量上限时可选的单位。和 {@link formatBytes} 一样按 1024 进位，写出来的和读到的对得上。 */
export const BYTE_UNITS = ['MB', 'GB', 'TB'] as const
export type ByteUnit = (typeof BYTE_UNITS)[number]

const UNIT_BYTES: Record<ByteUnit, number> = { MB: 1024 ** 2, GB: 1024 ** 3, TB: 1024 ** 4 }

export function unitBytes(unit: ByteUnit): number {
  return UNIT_BYTES[unit]
}

/**
 * 一个已有的字节数放回输入框：取能让数字不小于 1 的最大单位，最多三位小数（输入框也收三位）。
 * 原样提交回去时差的只是四舍五入的那一点，所以表单只在用户改过这一项时才提交它。
 */
export function bytesToField(bytes: number | null): { value: string; unit: ByteUnit } {
  if (bytes === null) return { value: '', unit: 'GB' }
  const unit = [...BYTE_UNITS].reverse().find((candidate) => bytes >= UNIT_BYTES[candidate]) ?? 'MB'
  const value = Math.round((bytes / UNIT_BYTES[unit]) * 1000) / 1000
  return { value: String(value), unit }
}

/** 比例写成百分数，取整；用完了以后可能超过 100。 */
export function formatPercent(ratio: number): number {
  return Math.floor(ratio * 100)
}
