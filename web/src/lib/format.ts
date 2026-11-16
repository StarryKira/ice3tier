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
