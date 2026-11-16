import { type UpdateOverview } from '@/api/types'
import { t } from '@/i18n'

/** 发布源的三种：项目默认的、一个镜像地址、controller 所在机器上的一个目录。 */
export type SourceKind = 'default' | 'url' | 'dir'

export function sourceKind(overview: Pick<UpdateOverview, 'source' | 'source_is_default'>): SourceKind {
  if (overview.source_is_default) return 'default'
  return /^https?:\/\//i.test(overview.source) ? 'url' : 'dir'
}

/**
 * 发布源的写法，和 controller 的规则一样：`http://` 或 `https://` 开头的地址，或者一个绝对路径
 * （controller 可能跑在 Windows 上，所以盘符和 `\\` 开头的写法也算）。只是为了早一点指出错误，controller 拒绝时显示它的原因。
 */
export function validateSource(kind: SourceKind, value: string): string | null {
  const text = value.trim()
  if (kind === 'default') return null
  if (text === '') return t.updates.source.required
  if (kind === 'url') return /^https?:\/\/\S+$/i.test(text) ? null : t.updates.source.urlInvalid
  const absolute = text.startsWith('/') || /^[A-Za-z]:[\\/]/.test(text) || text.startsWith('\\\\')
  return absolute ? null : t.updates.source.dirInvalid
}
