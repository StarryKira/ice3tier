import { type ReactNode } from 'react'

import { type Timestamp } from '@/api/types'
import { CopyButton } from '@/components/copy'
import { useNow } from '@/lib/useNow'
import { formatDateTime, formatRelative } from '@/lib/time'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'

/**
 * 详情卡片里的一列"名字：值"，一行一项：左边是图标和名字（14 像素，正文的颜色），
 * 右边是值（灰色，靠右），最右是这一项的按钮——改（铅笔）、复制。行与行之间一根细线，横贯整张卡片。
 */
export function DetailList({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn('divide-y', className)}>{children}</div>
}

interface DetailRowProps {
  icon: ReactNode
  label: ReactNode
  children: ReactNode
  /** 给了就在最右边放一个复制按钮，复制的是它。 */
  copy?: string
  /** 复制按钮前面的按钮（例如改虚拟 IP 的铅笔）。 */
  actions?: ReactNode
  /** 值有好几行（候选端点这类列表）：左边的名字和第一行对齐，而不是上下居中。 */
  multiline?: boolean
}

export function DetailRow({ icon, label, children, copy, actions, multiline = false }: DetailRowProps) {
  return (
    <div className={cn('flex min-h-11 justify-between gap-6 px-5 py-2', multiline ? 'items-start' : 'items-center')}>
      <div className={cn('flex shrink-0 items-center gap-3 text-sm', multiline && 'min-h-7')}>
        <span aria-hidden className="flex size-4 items-center justify-center text-muted-foreground [&_svg]:size-4">
          {icon}
        </span>
        {label}
      </div>
      <div className="flex min-w-0 items-center justify-end gap-1 text-sm text-muted-foreground">
        <div className={cn('min-w-0 text-right', multiline ? 'py-1' : 'truncate')}>{children}</div>
        {actions}
        {copy !== undefined ? (
          <CopyButton
            value={copy}
            variant="ghost"
            label={t.common.copyValue(copy)}
            className="size-7 shrink-0 text-muted-foreground hover:text-foreground [&_svg]:size-3.5"
          />
        ) : null}
      </div>
    </div>
  )
}

/** 一个时刻：完整的日期和时分，后面是"多久以前"。 */
export function DateTimeValue({ at }: { at: Timestamp }) {
  const now = useNow()
  return (
    <span className="tabular">
      {formatDateTime(at)} · {formatRelative(at, now)}
    </span>
  )
}
