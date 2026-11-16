/**
 * 浏览器本地记的小东西：侧栏是否收起、表格的排序、上次看的网络。
 * 筛选和搜索词不记在这里，它们写在 URL 里。
 * 存储不可用（隐私模式、被禁用）时安静地退回默认值。
 */

const PREFIX = 'i3t.'

export function readStored<T>(key: string, fallback: T, isValid: (value: unknown) => value is T): T {
  try {
    const raw = window.localStorage.getItem(PREFIX + key)
    if (raw === null) return fallback
    const parsed: unknown = JSON.parse(raw)
    return isValid(parsed) ? parsed : fallback
  } catch {
    return fallback
  }
}

export function writeStored(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value))
  } catch {
    // 记不住也不影响使用。
  }
}

export const isBoolean = (value: unknown): value is boolean => typeof value === 'boolean'
export const isNumber = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value)
