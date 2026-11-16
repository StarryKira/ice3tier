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

interface BoxCardHeaderProps {
  title: ReactNode
  tip?: ReactNode
  action?: ReactNode
  className?: string
}

/**
 * 有边框的卡片：一块圆角的白，和旁边的卡片一样大小的标题条、一样的留白。
 * 一页里几块信息并排放的时候用它（节点详情的概览），左右两栏看起来是同一种东西。
 */
function BoxCard({ className, ...props }: ComponentProps<'section'>) {
  return <section className={cn('min-w-0 rounded-xl border bg-card text-card-foreground', className)} {...props} />
}

/** 卡片顶上的标题条：15 像素半粗的标题，右端放这块卡片自己的动作，下面一根线。 */
function BoxCardHeader({ title, tip, action, className }: BoxCardHeaderProps) {
  return (
    <header className={cn('flex min-h-12 items-center justify-between gap-4 border-b px-5 py-2.5', className)}>
      <div className="flex min-w-0 items-center gap-1.5">
        <h2 className="truncate text-[15px] leading-6 font-semibold">{title}</h2>
        {tip !== undefined ? <InfoTip>{tip}</InfoTip> : null}
      </div>
      {action !== undefined ? <div className="flex shrink-0 items-center gap-2">{action}</div> : null}
    </header>
  )
}

/** 卡片的内容：左右和标题条一样留 20 像素。 */
function BoxCardBody({ className, ...props }: ComponentProps<'div'>) {
  return <div className={cn('px-5', className)} {...props} />
}

/**
 * 卡片里一列"名字：值"：行与行之间一根细线，横贯整张卡片（和能力那几行一样），每行一样高；名字一栏一样宽（10rem），值从同一条线开始。
 * 里面放 `InfoRow`。
 */
function BoxCardList({ className, ...props }: ComponentProps<'dl'>) {
  return (
    <dl
      className={cn('-mx-5 divide-y [&>div]:grid-cols-[10rem_minmax(0,1fr)] [&>div]:px-5 [&>div]:py-2.5', className)}
      {...props}
    />
  )
}

export { BoxCard, BoxCardBody, BoxCardHeader, BoxCardList, Card, CardBody, CardHeader }
