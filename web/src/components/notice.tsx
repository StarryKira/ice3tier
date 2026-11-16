import { CircleAlert, Info, TriangleAlert } from 'lucide-react'
import { type ReactNode } from 'react'

import { cn } from '@/lib/utils'

/**
 * 两种分量：
 * - info、relay 是一句旁注：不画框不铺底色，灰字前面一个小图标，读的时候在，平时不抢眼。
 * - warn、danger 要人注意：一层很淡的底色，图标和标题上色。
 */
const STYLES = {
  info: { box: 'px-0 text-muted-foreground', icon: 'text-faint', Icon: Info },
  relay: { box: 'px-0 text-muted-foreground', icon: 'text-relay-dot', Icon: Info },
  warn: { box: 'bg-warn-soft px-3.5 py-2.5 text-foreground', icon: 'text-warn', Icon: TriangleAlert },
  danger: { box: 'bg-danger-soft px-3.5 py-2.5 text-foreground', icon: 'text-danger', Icon: CircleAlert },
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
      className={cn('flex items-start gap-2.5 rounded-md text-[13px] leading-relaxed', box, className)}
    >
      <Icon className={cn('mt-0.5 size-4 shrink-0', icon)} aria-hidden />
      <div className="min-w-0 flex-1">
        {title !== undefined ? <p className="font-medium text-foreground">{title}</p> : null}
        {children !== undefined ? (
          <div className={cn(title !== undefined && 'mt-0.5 text-muted-foreground')}>{children}</div>
        ) : null}
      </div>
      {action !== undefined ? <div className="shrink-0">{action}</div> : null}
    </div>
  )
}
