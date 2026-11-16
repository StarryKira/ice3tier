import { CircleAlert, Info, TriangleAlert } from 'lucide-react'
import { type ReactNode } from 'react'

import { cn } from '@/lib/utils'

const STYLES = {
  info: { box: 'border-border bg-muted/60 text-foreground', icon: 'text-muted-foreground', Icon: Info },
  relay: { box: 'border-relay/25 bg-relay-soft text-foreground', icon: 'text-relay', Icon: Info },
  warn: { box: 'border-warn-line bg-warn-soft text-foreground', icon: 'text-warn', Icon: TriangleAlert },
  danger: { box: 'border-danger-line bg-danger-soft text-foreground', icon: 'text-danger', Icon: CircleAlert },
} as const

interface NoticeProps {
  tone?: keyof typeof STYLES
  title?: ReactNode
  children?: ReactNode
  action?: ReactNode
  className?: string
}

/** 页面里的一条说明或提醒，写在它说的那样东西旁边。 */
export function Notice({ tone = 'info', title, children, action, className }: NoticeProps) {
  const { box, icon, Icon } = STYLES[tone]
  return (
    <div
      role={tone === 'danger' ? 'alert' : 'note'}
      className={cn(
        'flex items-start gap-2.5 rounded-md border px-3.5 py-2.5 text-[13px] leading-relaxed',
        box,
        className,
      )}
    >
      <Icon className={cn('mt-0.5 size-4 shrink-0', icon)} aria-hidden />
      <div className="min-w-0 flex-1">
        {title !== undefined ? <p className="font-medium">{title}</p> : null}
        {children !== undefined ? (
          <div className={cn(title !== undefined && 'mt-0.5 text-muted-foreground')}>{children}</div>
        ) : null}
      </div>
      {action !== undefined ? <div className="shrink-0">{action}</div> : null}
    </div>
  )
}
