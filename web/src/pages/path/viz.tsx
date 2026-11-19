import { type ReactNode } from 'react'

import { Tooltip } from '@/components/ui/tooltip'
import { type Scale } from '@/lib/scale'
import { cn } from '@/lib/utils'

import { dimmed, useFocus } from './focus'

/**
 * 诊断报告的图表零件：横向的图、细条、点、图例。纯 HTML，按百分比摆，宽度跟着面板走，行的高度撑满面板。
 *
 * 规矩：标记细、两头圆；不画网格线，条形图的数写在条的末端，只有点和区间的图在下面写几个刻度；
 * 两个以上的系列一定有图例；字用正文的颜色，不用系列色。鼠标停在一行上，别的图里同一条路也亮着（`focus.ts`）。
 */

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
  /** 联动用的路的键；没有就不联动。 */
  focus?: string
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
 * 横向的图：一行一项，左边名字、中间画、右边数。行的高度平分面板的高度（至少 `minRow`），和旁边的面板一样高时不留空。
 * 给了 `format` 才在下面写刻度（点和区间的图要看位置）；条形图不写刻度，数在右边。
 */
export function Plot({
  rows,
  scale,
  format,
  labelWidth = '8.5rem',
  valueWidth = '5.5rem',
  minRow = 'min-h-8',
  label,
}: {
  rows: readonly PlotRow[]
  scale: Scale
  format?: (tick: number, last: boolean) => string
  labelWidth?: string
  valueWidth?: string
  minRow?: string
  /** 读屏用的名字。 */
  label: string
}) {
  const { focus, setFocus } = useFocus()
  const span = scale.max - scale.min || 1
  const x = (value: number) => `${Math.max(0, Math.min(100, ((value - scale.min) / span) * 100))}%`
  const columns = { gridTemplateColumns: `${labelWidth} minmax(0,1fr) ${valueWidth}` }
  return (
    <div role="img" aria-label={label} className="flex h-full flex-col text-[13px]">
      {rows.map((row) => {
        const faded = row.focus !== undefined && dimmed(focus, row.focus)
        const content = (
          <div
            className={cn(
              'grid flex-1 items-center gap-x-3 rounded-sm transition-opacity',
              minRow,
              faded && 'opacity-30',
            )}
            style={columns}
            data-testid={row.testId}
            onMouseEnter={row.focus === undefined ? undefined : () => setFocus(row.focus ?? null)}
            onMouseLeave={row.focus === undefined ? undefined : () => setFocus(null)}
          >
            <div className="min-w-0 truncate">{row.label}</div>
            <div className="relative h-full min-h-6">{row.marks(x)}</div>
            <div className="text-right tabular">{row.value}</div>
          </div>
        )
        return row.tip === undefined ? (
          <div key={row.key} className="flex flex-1 flex-col">
            {content}
          </div>
        ) : (
          <Tooltip key={row.key} content={row.tip}>
            <div
              tabIndex={0}
              className="flex flex-1 flex-col rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {content}
            </div>
          </Tooltip>
        )
      })}
      {format !== undefined ? (
        <div className="grid gap-x-3" style={columns} aria-hidden>
          <span />
          <div className="relative h-4 text-[10px] text-muted-foreground tabular">
            {scale.ticks.map((tick, index) => (
              <span
                key={tick}
                className={cn(
                  'absolute top-0.5 whitespace-nowrap',
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
      ) : null}
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
