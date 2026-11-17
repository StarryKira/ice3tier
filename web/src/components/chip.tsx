import { type ReactNode } from 'react'

import { CHIP_SIZE } from '@/components/chip-size'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'
import { cn } from '@/lib/utils'

/** 表格里的一个小标签：前面一个图标或者圆点，后面是名字，浅灰底。`muted` 是停用的那种，淡一些。 */
export function Chip({ lead, children, muted = false }: { lead: ReactNode; children: ReactNode; muted?: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center border border-transparent bg-muted whitespace-nowrap text-foreground',
        CHIP_SIZE.sm,
        muted && 'opacity-60',
      )}
    >
      {lead}
      <span className="truncate">{children}</span>
    </span>
  )
}

/** 一串小标签：显示前几个，其余折成"+N"，悬停看全部的名字。没有时是一句灰字。 */
export function ChipList<T>({
  items,
  max,
  empty,
  name,
  render,
}: {
  items: readonly T[]
  max: number
  empty?: ReactNode
  name: (item: T) => string
  render: (item: T) => ReactNode
}) {
  if (items.length === 0) return <span className="text-[13px] text-muted-foreground">{empty ?? t.common.none}</span>
  const rest = items.slice(max).map(name)
  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      {items.slice(0, max).map(render)}
      {rest.length > 0 ? (
        <Tooltip content={formatList(rest)}>
          <span
            tabIndex={0}
            aria-label={t.common.moreItems(rest.length, formatList(rest))}
            className="rounded-sm px-1 text-xs text-muted-foreground tabular"
          >
            +{rest.length}
          </span>
        </Tooltip>
      ) : null}
    </span>
  )
}
