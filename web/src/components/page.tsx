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

/**
 * 页面的标题区：20 像素半粗的标题，下面一句灰色的说明，右端最多一个主按钮加一两个次要按钮。
 * 和下面的内容之间留 32 像素。
 */
export function PageHeader({ title, description, actions, className }: PageHeaderProps) {
  return (
    <header className={cn('mb-8 flex items-start justify-between gap-6', className)}>
      <div className="min-w-0">
        <h1 className="flex items-center gap-2.5 text-xl leading-7 font-semibold tracking-tight">{title}</h1>
        {description !== undefined ? (
          <p className="mt-1 max-w-2xl text-[13px] leading-relaxed text-muted-foreground">{description}</p>
        ) : null}
      </div>
      {actions !== undefined ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  )
}

const WIDTHS = {
  /** 以读和填为主的页面：设置、网络、说明页。 */
  narrow: 'max-w-[880px]',
  /** 列表和总览。 */
  default: 'max-w-[1200px]',
  /** 画图的页面：拓扑、访问关系图。 */
  wide: 'max-w-[1480px]',
} as const

/** 页面的内容区：左右留白，限制最大宽度，宽屏上不会把一行字拉得太长。 */
export function Page({
  children,
  wide = false,
  size,
  className,
}: {
  children: ReactNode
  /** 旧的写法，等于 `size="wide"`。 */
  wide?: boolean
  size?: keyof typeof WIDTHS
  className?: string
}) {
  return (
    <div
      className={cn('mx-auto w-full px-8 pt-8 pb-16 xl:px-10', WIDTHS[size ?? (wide ? 'wide' : 'default')], className)}
    >
      {children}
    </div>
  )
}

/** 只读信息里的一行：左边是名字（灰），右边是值。几行放在一个 `<dl>` 里，行与行之间不画线。 */
export function InfoRow({ label, children, className }: { label: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={cn('grid grid-cols-[8.5rem_minmax(0,1fr)] items-baseline gap-4 py-1.5', className)}>
      <dt className="text-[13px] text-muted-foreground">{label}</dt>
      <dd className="min-w-0 text-sm">{children}</dd>
    </div>
  )
}
