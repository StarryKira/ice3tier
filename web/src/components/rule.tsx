import { ArrowLeftRight, ArrowRight } from 'lucide-react'

import { type Direction, type Rule } from '@/api/types'
import { GroupChip } from '@/components/groups'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'

/** 规则的方向符号：双向是两头的箭头，单向是从前者指向后者的箭头。旁边带着给读屏软件的文字。 */
export function DirectionMark({ direction, className }: { direction: Direction; className?: string }) {
  const Icon = direction === 'both' ? ArrowLeftRight : ArrowRight
  return (
    <span className={cn('inline-flex shrink-0 items-center text-muted-foreground', className)}>
      <Icon className="size-3.5" aria-hidden />
      <span className="sr-only">{direction === 'both' ? t.rules.bothSr : t.rules.oneWaySr}</span>
    </span>
  )
}

/** 把一条规则摆成一句话的形状：`dev ⇄ srv`。读起来就是它的含义。 */
export function RuleSentence({
  rule,
  muted = false,
  className,
}: {
  rule: Pick<Rule, 'src' | 'dst' | 'direction'>
  muted?: boolean
  className?: string
}) {
  return (
    <span className={cn('inline-flex items-center gap-1.5', className)}>
      <GroupChip name={rule.src.name} muted={muted} />
      <DirectionMark direction={rule.direction} />
      <GroupChip name={rule.dst.name} muted={muted} />
    </span>
  )
}
