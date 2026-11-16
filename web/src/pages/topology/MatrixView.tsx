import { useMemo } from 'react'

import { type NodeId, type RouteMatrix } from '@/api/types'
import { OnlineDot } from '@/components/status'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Pair } from '@/topology/pair'
import {
  CLASS_LABEL,
  CLASS_SYMBOL,
  cellKey,
  classify,
  entryText,
  indexEntries,
  type RouteClass,
} from '@/topology/route'

const g = t.topology

const CELL: Record<RouteClass, string> = {
  direct: 'bg-ok-soft text-ok',
  relay: 'bg-relay-soft text-relay',
  multi: 'bg-relay-soft text-relay ring-1 ring-inset ring-relay/40',
  none: 'bg-danger-soft text-danger',
}

/** 对端离线时"无路可走"是理所当然的：灰色，不和真正的不可达抢眼（悬停的文字照样写"无路可走"）。 */
const NONE_OFFLINE = 'bg-muted text-muted-foreground'

export function MatrixLegend() {
  const classes: RouteClass[] = ['direct', 'relay', 'multi', 'none']
  return (
    <ul
      className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground"
      aria-label={t.routes.legendLabel}
    >
      {classes.map((cls) => (
        <li key={cls} className="flex items-center gap-1.5">
          <span
            className={cn(
              'inline-flex size-5 items-center justify-center rounded-sm text-[11px] font-semibold',
              CELL[cls],
            )}
          >
            {CLASS_SYMBOL[cls]}
          </span>
          {CLASS_LABEL[cls]}
        </li>
      ))}
      <li className="flex items-center gap-1.5">
        <span
          className={cn(
            'inline-flex size-5 items-center justify-center rounded-sm text-[11px] font-semibold',
            NONE_OFFLINE,
          )}
        >
          {CLASS_SYMBOL.none}
        </span>
        {t.routes.noneOffline}
      </li>
      <li className="flex items-center gap-1.5">
        <span className="inline-flex size-5 items-center justify-center rounded-sm border border-dashed border-border-strong text-[11px] text-muted-foreground">
          {CLASS_SYMBOL.direct}
        </span>
        {t.routes.stale}
      </li>
      <li className="flex items-center gap-1.5">
        <span className="inline-flex size-5 items-center justify-center text-muted-foreground">
          {t.routes.symbolEmpty}
        </span>
        {t.routes.classEmpty}
      </li>
    </ul>
  )
}

interface MatrixViewProps {
  matrix: RouteMatrix
  pair: Pair
  onPick: (from: NodeId, to: NodeId) => void
}

/**
 * 路由矩阵：行是发送方，列是接收方，一格是一个方向现在怎么走。和拓扑图是同一份数据，这里是精确的那一种写法。
 * 格子里有字（直、中、多、✕）和延迟，颜色不单独承担含义；过期的格子是虚框、变淡。
 * 有数据的格子是按钮：点了选中这一对。没有数据的格子不能聚焦，免得几十个节点时 Tab 要按上千下。
 */
export function MatrixView({ matrix, pair, onPick }: MatrixViewProps) {
  const index = useMemo(() => indexEntries(matrix.entries), [matrix.entries])
  const names = useMemo(() => new Map(matrix.nodes.map((node) => [node.id, node.name])), [matrix.nodes])
  const name = (id: NodeId) => names.get(id) ?? t.common.nodeMissing(id)
  const selected = (from: NodeId, to: NodeId) =>
    (pair.a === from && pair.b === to) || (pair.a === to && pair.b === from)

  return (
    <div className="overflow-auto" data-testid="route-matrix">
      <table className="border-separate border-spacing-0 text-sm">
        <caption className="sr-only">{g.matrixCaption}</caption>
        <thead>
          <tr>
            <th
              scope="col"
              className="sticky top-0 left-0 z-20 border-b bg-card px-3 py-2 text-left text-xs font-medium whitespace-nowrap text-muted-foreground"
            >
              {g.matrixFrom}
            </th>
            {matrix.nodes.map((node) => (
              <th
                key={node.id}
                scope="col"
                className={cn(
                  'sticky top-0 z-10 max-w-24 border-b bg-card px-1.5 py-2 text-xs font-medium',
                  (pair.a === node.id || pair.b === node.id) && 'text-primary',
                )}
              >
                <span className="block truncate" title={node.name}>
                  {node.name}
                </span>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {matrix.nodes.map((from) => (
            <tr key={from.id}>
              <th
                scope="row"
                className={cn(
                  'sticky left-0 z-10 border-r bg-card px-3 py-1 text-left text-[13px] font-medium whitespace-nowrap',
                  (pair.a === from.id || pair.b === from.id) && 'text-primary',
                )}
              >
                <span className="inline-flex max-w-44 items-center gap-1.5">
                  <OnlineDot online={from.online} />
                  <span className="truncate">{from.name}</span>
                </span>
              </th>
              {matrix.nodes.map((to) => {
                if (from.id === to.id) {
                  return (
                    <td key={to.id} className="p-0.5 text-center">
                      <span className="block h-8 min-w-12 rounded-sm bg-muted/60" aria-hidden />
                      <span className="sr-only">{g.cellSelf}</span>
                    </td>
                  )
                }
                const entry = index.get(cellKey(from.id, to.id))
                if (entry === undefined) {
                  return (
                    <td key={to.id} className="p-0.5 text-center text-muted-foreground" title={g.cellEmpty}>
                      <span aria-hidden>{t.routes.symbolEmpty}</span>
                      <span className="sr-only">{g.cellLabel(from.name, to.name, g.cellEmpty)}</span>
                    </td>
                  )
                }
                const cls = classify(entry.route)
                const text = entryText(entry, name)
                return (
                  <td key={to.id} className="p-0.5">
                    <Tooltip content={g.cellLabel(from.name, to.name, text)}>
                      <button
                        type="button"
                        onClick={() => onPick(from.id, to.id)}
                        aria-label={g.cellLabel(from.name, to.name, text)}
                        aria-pressed={selected(from.id, to.id)}
                        data-testid={`cell-${from.id}-${to.id}`}
                        className={cn(
                          'flex h-8 w-full min-w-12 items-center justify-center gap-1 rounded-sm px-1 text-[12px] font-medium tabular',
                          cls === 'none' && !to.online ? NONE_OFFLINE : CELL[cls],
                          entry.stale && 'border border-dashed border-border-strong bg-transparent opacity-60',
                          selected(from.id, to.id) && 'ring-2 ring-primary',
                        )}
                      >
                        <span aria-hidden>{CLASS_SYMBOL[cls]}</span>
                        {entry.rtt_ms !== null ? <span className="font-normal">{entry.rtt_ms}</span> : null}
                        {/* 到 relay 退回 TCP 是值得知道的事：格子里写出来，别的细节在悬停文字里。 */}
                        {entry.route.via === 'relay' && entry.relay_leg === 'tcp' ? (
                          <span aria-hidden className="text-[10px] font-semibold text-warn">
                            {t.routes.legTransport.tcp}
                          </span>
                        ) : null}
                      </button>
                    </Tooltip>
                  </td>
                )
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
