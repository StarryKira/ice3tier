import { t } from '@/i18n'
import { cn } from '@/lib/utils'

/** 彩色表情用的字体：各个系统自带的那一种。 */
const EMOJI_FONT = "'Apple Color Emoji', 'Segoe UI Emoji', 'Noto Color Emoji', sans-serif"

/** 三块冰（🧊）叠在一起：下面两块，上面一块压在它们中间，各自稍稍重叠。和 `public/favicon.svg` 是同一个图。 */
export function IceStack({ className, label }: { className: string; label?: string }) {
  return (
    <svg
      viewBox="0 0 32 32"
      className={className}
      role={label === undefined ? undefined : 'img'}
      aria-label={label}
      aria-hidden={label === undefined ? true : undefined}
    >
      <g fontSize="15" textAnchor="middle" dominantBaseline="central" style={{ fontFamily: EMOJI_FONT }}>
        <text x="9.5" y="23">
          🧊
        </text>
        <text x="22.5" y="23">
          🧊
        </text>
        <text x="16" y="11.5">
          🧊
        </text>
      </g>
    </svg>
  )
}

/** 标志：三块叠起来的冰。`markOnly` 时只画图形（登录页左上角、收起的侧栏），名字作为它的无障碍名称。 */
export function BrandMark({ className, markOnly = false }: { className?: string; markOnly?: boolean }) {
  return (
    <div className={cn('flex items-center gap-2', className)}>
      <IceStack className={markOnly ? 'size-8' : 'size-7'} label={markOnly ? t.common.appName : undefined} />
      {markOnly ? null : (
        <span className="truncate text-xl leading-6 font-bold tracking-tight">{t.common.appName}</span>
      )}
    </div>
  )
}
