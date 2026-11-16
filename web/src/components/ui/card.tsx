import { type ComponentProps, type ReactNode } from 'react'

import { InfoTip } from '@/components/info-tip'
import { cn } from '@/lib/utils'

/**
 * 页面里的一个分区。没有边框、底色和阴影：分区之间靠留白和标题分开，内容和页面标题左对齐。
 * 同一页的分区之间留 `gap-10`（40 像素）左右的距离，由外面的容器给。
 */
function Card({ className, ...props }: ComponentProps<'section'>) {
  return <section className={cn('min-w-0 text-card-foreground', className)} {...props} />
}

interface CardHeaderProps {
  title: ReactNode
  description?: ReactNode
  /** 较长的解释：标题旁边一个小图标，悬停或聚焦时显示（在标题外面，不算进标题的名字）。 */
  tip?: ReactNode
  action?: ReactNode
  className?: string
}

/** 分区的标题：16 像素半粗，只在必要时下面一句灰色的说明，右端放这个分区自己的动作。 */
function CardHeader({ title, description, tip, action, className }: CardHeaderProps) {
  return (
    <header className={cn('mb-3 flex items-start justify-between gap-4', className)}>
      <div className="min-w-0">
        <div className="flex items-center gap-1.5">
          <h2 className="text-base leading-6 font-semibold">{title}</h2>
          {tip !== undefined ? <InfoTip>{tip}</InfoTip> : null}
        </div>
        {description !== undefined ? <p className="mt-0.5 text-[13px] text-muted-foreground">{description}</p> : null}
      </div>
      {action !== undefined ? <div className="flex shrink-0 items-center gap-2">{action}</div> : null}
    </header>
  )
}

function CardBody({ className, ...props }: ComponentProps<'div'>) {
  return <div className={cn(className)} {...props} />
}

export { Card, CardBody, CardHeader }
