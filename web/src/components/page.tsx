import { type ReactNode } from 'react'

import { cn } from '@/lib/utils'

interface PageHeaderProps {
  title: ReactNode
  /** 标题下面的一句话：这个页面是做什么的。 */
  description?: ReactNode
  /** 右端：这个页面的主按钮。 */
  actions?: ReactNode
  className?: string
}

export function PageHeader({ title, description, actions, className }: PageHeaderProps) {
  return (
    <header className={cn('mb-6 flex items-start justify-between gap-6', className)}>
      <div className="min-w-0">
        <h1 className="flex items-center gap-2.5 text-xl font-semibold tracking-tight">{title}</h1>
        {description !== undefined ? (
          <p className="mt-1 max-w-3xl text-sm text-muted-foreground">{description}</p>
        ) : null}
      </div>
      {actions !== undefined ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  )
}

/** 页面的内容区：限制最大宽度，宽屏上不会把一行字拉得太长。 */
export function Page({
  children,
  wide = false,
  className,
}: {
  children: ReactNode
  wide?: boolean
  className?: string
}) {
  return (
    <div className={cn('mx-auto w-full px-8 py-7', wide ? 'max-w-[1500px]' : 'max-w-[1200px]', className)}>
      {children}
    </div>
  )
}

/** 只读信息卡里的一行：左边是名字，右边是值。 */
export function InfoRow({ label, children, className }: { label: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={cn('grid grid-cols-[8.5rem_minmax(0,1fr)] items-baseline gap-3 py-2', className)}>
      <dt className="text-[13px] text-muted-foreground">{label}</dt>
      <dd className="min-w-0 text-sm">{children}</dd>
    </div>
  )
}
