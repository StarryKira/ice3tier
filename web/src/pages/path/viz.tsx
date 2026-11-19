import { type ReactNode } from 'react'

import { InfoTip } from '@/components/info-tip'
import { type Scale } from '@/lib/scale'
import { Tooltip } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'

/**
 * 诊断页的图表零件：面板、刻度、带网格线的横向图、图例。纯 HTML，按百分比摆，宽度跟着面板走。
 *
 * 规矩（数据可视化的那一套）：标记细（条 ≤ 8 像素、两头圆）、网格线是细的实线、轴和网格退在后面；
 * 两个以上的系列一定有图例；数只标在条的末端，别的放在悬停里；字用正文的颜色，不用系列色。
 */

/** 一块面板：小标题条（13 像素半粗），右边可以放这块自己的东西；内容紧凑，不留大片空白。 */
export function Panel({
  id,
  title,
  hint,
  action,
  className,
  bodyClassName,
  children,
}: {
  id?: string
  title: ReactNode
  hint?: ReactNode
  action?: ReactNode
  className?: string
  bodyClassName?: string
  children: ReactNode
}) {
  return (
    <section id={id} className={cn('min-w-0 scroll-mt-4 rounded-lg border bg-card', className)}>
      <header className="flex h-10 items-center justify-between gap-3 border-b px-4">
        <h2 className="flex min-w-0 items-center gap-1.5 truncate text-[13px] font-semibold">
          {title}
          {hint !== undefined ? <InfoTip>{hint}</InfoTip> : null}
        </h2>
        {action !== undefined ? <div className="flex shrink-0 items-center gap-3">{action}</div> : null}
      </header>
      <div className={cn('px-4 py-3', bodyClassName)}>{children}</div>
    </section>
  )
}

/** 面板里没有数据时的一行灰字。 */
export function Empty({ children }: { children: ReactNode }) {
  return <p className="py-2 text-[13px] text-muted-foreground">{children}</p>
}

/** 图例：一个色块和名字，一排。 */
export function Legend({ items }: { items: readonly { label: string; className: string; shape?: 'bar' | 'dot' }[] }) {
  return (
    <span className="flex items-center gap-3 text-xs font-normal text-muted-foreground">
      {items.map((item) => (
        <span key={item.label} className="inline-flex items-center gap-1.5">
          <span
            aria-hidden
            className={cn(item.shape === 'dot' ? 'size-2 rounded-full' : 'h-1.5 w-3 rounded-full', item.className)}
          />
          {item.label}
        </span>
      ))}
    </span>
  )
}

export interface PlotRow {
  key: string
  label: ReactNode
  /** 这一行的标记；`x` 把数换成横向的百分比位置。 */
  marks: (x: (value: number) => string) => ReactNode
  /** 右边一列的数（末端的那个值）。 */
  value?: ReactNode
  /** 悬停这一行时的说明。 */
  tip?: ReactNode
  testId?: string
}

/**
 * 横向的图：一行一项，左边名字、中间画、右边数；竖的网格线在刻度上（细实线），最下面一行刻度。
 * 行与行之间不留缝，网格线连成一条。
 */
export function Plot({
  rows,
  scale,
  format,
  labelWidth = '8.5rem',
  valueWidth = '5.5rem',
  rowClassName = 'h-9',
  label,
}: {
  rows: readonly PlotRow[]
  scale: Scale
  /** 刻度的写法；单位只写在最后一个刻度上（`last`），免得挤。 */
  format: (tick: number, last: boolean) => string
  labelWidth?: string
  valueWidth?: string
  rowClassName?: string
  /** 读屏用的名字。 */
  label: string
}) {
  const span = scale.max - scale.min || 1
  const x = (value: number) => `${Math.max(0, Math.min(100, ((value - scale.min) / span) * 100))}%`
  const columns = { gridTemplateColumns: `${labelWidth} minmax(0,1fr) ${valueWidth}` }
  const grid = (
    <span aria-hidden className="pointer-events-none absolute inset-0">
      {scale.ticks.map((tick) => (
        <span key={tick} className="absolute inset-y-0 w-px bg-border" style={{ left: x(tick) }} />
      ))}
    </span>
  )
  return (
    <div role="img" aria-label={label} className="text-[13px]">
      {rows.map((row) => {
        const plot = (
          <div className={cn('relative', rowClassName)}>
            {grid}
            {row.marks(x)}
          </div>
        )
        return (
          <div key={row.key} className="grid items-center gap-x-3" style={columns} data-testid={row.testId}>
            <div className="min-w-0 truncate">{row.label}</div>
            {row.tip !== undefined ? (
              <Tooltip content={row.tip}>
                <div tabIndex={0} className="rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  {plot}
                </div>
              </Tooltip>
            ) : (
              plot
            )}
            <div className="text-right tabular">{row.value}</div>
          </div>
        )
      })}
      <div className="grid gap-x-3" style={columns} aria-hidden>
        <span />
        <div className="relative h-5 text-[11px] text-muted-foreground tabular">
          {scale.ticks.map((tick, index) => (
            <span
              key={tick}
              className={cn(
                'absolute top-1 whitespace-nowrap',
                index === 0 ? '' : index === scale.ticks.length - 1 ? '-translate-x-full' : '-translate-x-1/2',
              )}
              style={{ left: x(tick) }}
            >
              {format(tick, index === scale.ticks.length - 1)}
            </span>
          ))}
        </div>
        <span />
      </div>
    </div>
  )
}

/** 一根细条：从 `from` 到 `to`（百分比），两头圆。 */
export function Bar({
  from,
  to,
  className,
  top = '50%',
  height = 'h-2',
}: {
  from: string
  to: string
  className: string
  top?: string
  height?: string
}) {
  return (
    <span
      aria-hidden
      className={cn('absolute -translate-y-1/2 rounded-full', height, className)}
      style={{ left: from, width: `max(3px, calc(${to} - ${from}))`, top }}
    />
  )
}

/** 一个点：带一圈底色，压在条或线上也看得清。 */
export function Dot({ at, className, top = '50%' }: { at: string; className: string; top?: string }) {
  return (
    <span
      aria-hidden
      className={cn('absolute size-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full ring-2 ring-card', className)}
      style={{ left: at, top }}
    />
  )
}
