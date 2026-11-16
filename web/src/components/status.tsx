import { TriangleAlert } from 'lucide-react'
import { type ReactNode } from 'react'

import { Tooltip } from '@/components/ui/tooltip'
import { type Timestamp } from '@/api/types'
import { t } from '@/i18n'
import { formatFullTime, formatRelative } from '@/lib/time'
import { cn } from '@/lib/utils'
import { useNow } from '@/lib/useNow'

export type Tone = 'ok' | 'idle' | 'relay' | 'warn' | 'danger'

const DOT: Record<Tone, string> = {
  ok: 'bg-ok-dot',
  idle: 'bg-idle-dot',
  relay: 'bg-relay-dot',
  warn: 'bg-warn-dot',
  danger: 'bg-danger-dot',
}

/**
 * 状态文字的颜色。正常、不活动、经 relay 的字都是普通的黑字，只靠前面的圆点区分；
 * 值得注意的（warn）加粗、圆点是琥珀色，字仍是黑的——一屏上常有好几个，整片琥珀字会很吵；
 * 不能用的（danger）才是红字，一眼能从一列里挑出来。
 */
const TEXT: Record<Tone, string> = {
  ok: 'text-foreground',
  idle: 'text-muted-foreground',
  relay: 'text-foreground',
  warn: 'font-medium text-foreground',
  danger: 'font-medium text-danger',
}

/** 状态圆点（8 像素）。颜色不单独承担含义：`label` 是给读屏软件的文字，旁边没有可见文字时必须给。 */
export function StatusDot({ tone, label, className }: { tone: Tone; label?: string; className?: string }) {
  return (
    <span className={cn('inline-flex shrink-0 items-center', className)}>
      <span
        aria-hidden
        className={cn(
          'size-2 rounded-full',
          DOT[tone],
          tone === 'idle' && 'bg-transparent ring-[1.5px] ring-idle-dot ring-inset',
        )}
      />
      {label !== undefined ? <span className="sr-only">{label}</span> : null}
    </span>
  )
}

/** 在线状态：全站同一个记号，紧贴在名字前面。在线是实心的绿点，离线是空心的灰圈。 */
export function OnlineDot({ online, className }: { online: boolean; className?: string }) {
  return (
    <StatusDot
      tone={online ? 'ok' : 'idle'}
      label={online ? t.common.online : t.common.offline}
      className={className}
    />
  )
}

/** 圆点加文字的状态。 */
export function StatusText({ tone, children, className }: { tone: Tone; children: ReactNode; className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-2', TEXT[tone], className)}>
      <StatusDot tone={tone} />
      {children}
    </span>
  )
}

/** 相对时间（"3 分钟前"），悬停显示带时区的完整时间。 */
export function RelativeTime({ at, className }: { at: Timestamp; className?: string }) {
  const now = useNow()
  return (
    <Tooltip content={formatFullTime(at)}>
      <time
        dateTime={new Date(at * 1000).toISOString()}
        tabIndex={0}
        className={cn('rounded-sm whitespace-nowrap', className)}
      >
        {formatRelative(at, now)}
      </time>
    </Tooltip>
  )
}

/** "需要处理"的记号：一个警告三角，悬停的文字写清是什么、为什么、去哪里处理。 */
export function IssueIcon({ message, tone = 'warn' }: { message: string; tone?: 'warn' | 'danger' }) {
  return (
    <Tooltip content={message}>
      <span
        tabIndex={0}
        role="img"
        aria-label={message}
        className={cn('inline-flex rounded-sm', tone === 'danger' ? 'text-danger' : 'text-warn-dot')}
      >
        <TriangleAlert className="size-3.5" aria-hidden />
      </span>
    </Tooltip>
  )
}
