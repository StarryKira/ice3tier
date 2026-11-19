import { useMemo, useState } from 'react'

import { type MatrixEntry, type MatrixNode, type NodeId, type RouteMatrix } from '@/api/types'
import { Flag } from '@/components/flag'
import { OnlineDot } from '@/components/status'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Pair } from '@/topology/pair'
import { cellKey, classify, entryText, indexEntries, type RouteClass } from '@/topology/route'

import { type NodeLook } from './TopologyGraph'

const g = t.topology
const r = t.routes

/**
 * 格子的底色和图上线的颜色是同一套：直连淡绿，经 relay 淡紫，不可达淡红。底色都很淡，
 * 延迟照常是黑字（只有不可达是红字），一眼扫过去先看到的是不正常的那几格。
 */
const CELL: Record<RouteClass, string> = {
  direct: 'bg-ok-soft',
  relay: 'bg-relay-soft',
  multi: 'bg-relay-soft ring-1 ring-inset ring-relay-dot/50',
  none: 'bg-danger-soft text-danger',
}

/** 对端离线时不可达是理所当然的：灰色，不和真正的不可达抢眼。 */
const NONE_OFFLINE = 'bg-muted text-muted-foreground'

/** 格子第二行：怎么走的，用字写（"直连""经 h2 · TCP"），不用记号；到中继那一段走 TCP 时黄字写出来。 */
function RouteLine({ entry, to, name }: { entry: MatrixEntry; to: MatrixNode; name: (id: NodeId) => string }) {
  const route = entry.route
  let main: string
  let extra: { text: string; warn: boolean } | null = null
  if (route.via === 'direct') {
    // 直连是怎么建立的（局域网、打洞……）在悬停里：每格都写一遍太吵。
    main = r.direct
  } else if (route.via === 'relay') {
    main = r.via(route.relays.map(name).join(r.hopSeparator))
    if (entry.relay_leg === 'tcp') extra = { text: r.legTransport.tcp, warn: true }
  } else {
    main = to.online ? r.none : r.noneOffline
  }
  return (
    <span className="flex max-w-full min-w-0 items-center justify-center gap-1 text-[11px] leading-4 text-muted-foreground">
      <span className="truncate">{main}</span>
      {extra !== null ? (
        <span className={cn('shrink-0', extra.warn && 'font-semibold text-warn')}>· {extra.text}</span>
      ) : null}
      {entry.stale ? <span className="shrink-0 rounded-sm border border-dashed px-1">{r.stale}</span> : null}
    </span>
  )
}

/** 行头、列头：国旗（不知道国家时不画）和名字。 */
function NodeHead({ node, look, online }: { node: MatrixNode; look: NodeLook | undefined; online?: boolean }) {
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      {online !== undefined ? <OnlineDot online={online} /> : null}
      <Flag code={look?.country} />
      <span className="truncate">{node.name}</span>
    </span>
  )
}

interface MatrixViewProps {
  matrix: RouteMatrix
  pair: Pair
  /** 节点列表里的国家（画旗子），按节点编号。 */
  looks: ReadonlyMap<NodeId, NodeLook>
  onPick: (from: NodeId, to: NodeId) => void
}

/**
 * 路由矩阵：行是发送方，列是接收方，一格是一个方向现在怎么走。和拓扑图是同一份数据，这里是精确的那一种写法。
 * 横竖都铺满剩下的地方（一行最高 5.5rem，节点多了在里面滚动）；一格两行：上面是延迟，下面用字写怎么走，
 * 所以不需要图例。悬停一格时它的行头、列头亮起来，对着读不串行。
 * 有数据的格子是按钮：点了选中这一对。没有数据的格子不能聚焦，免得几十个节点时 Tab 要按上千下。
 */
export function MatrixView({ matrix, pair, looks, onPick }: MatrixViewProps) {
  const index = useMemo(() => indexEntries(matrix.entries), [matrix.entries])
  const names = useMemo(() => new Map(matrix.nodes.map((node) => [node.id, node.name])), [matrix.nodes])
  const name = (id: NodeId) => names.get(id) ?? t.common.nodeMissing(id)
  const [hover, setHover] = useState<{ from: NodeId; to: NodeId } | null>(null)
  const selected = (from: NodeId, to: NodeId) =>
    (pair.a === from && pair.b === to) || (pair.a === to && pair.b === from)
  const picked = (id: NodeId) => pair.a === id || pair.b === id
  const count = matrix.nodes.length

  return (
    <div className="relative min-h-72 flex-1" data-testid="route-matrix">
      <div className="absolute inset-0 overflow-auto">
        <div
          role="table"
          aria-label={g.matrixCaption}
          className="grid h-full gap-1"
          style={{
            gridTemplateColumns: `9.5rem repeat(${count}, minmax(5.5rem, 1fr))`,
            gridTemplateRows: `2.25rem repeat(${count}, minmax(3.25rem, 5.5rem))`,
            alignContent: 'start',
          }}
        >
          <div role="row" className="contents">
            <div
              role="columnheader"
              className="sticky top-0 left-0 z-20 flex items-center bg-card pr-3 text-xs font-medium whitespace-nowrap text-muted-foreground"
            >
              {g.matrixFrom}
            </div>
            {matrix.nodes.map((node) => (
              <div
                key={node.id}
                role="columnheader"
                className={cn(
                  'sticky top-0 z-10 flex items-center justify-center rounded-md bg-card px-1.5 text-xs font-medium text-muted-foreground transition-colors',
                  (picked(node.id) || hover?.to === node.id) && 'text-foreground',
                  hover?.to === node.id && 'bg-muted',
                )}
              >
                <NodeHead node={node} look={looks.get(node.id)} />
              </div>
            ))}
          </div>
          {matrix.nodes.map((from) => (
            <div key={from.id} role="row" className="contents">
              <div
                role="rowheader"
                className={cn(
                  'sticky left-0 z-10 flex items-center rounded-md bg-card pr-3 pl-1 text-[13px] font-medium transition-colors',
                  (picked(from.id) || hover?.from === from.id) && 'text-primary',
                  hover?.from === from.id && 'bg-muted',
                )}
              >
                <NodeHead node={from} look={looks.get(from.id)} online={from.online} />
              </div>
              {matrix.nodes.map((to) => {
                if (from.id === to.id) {
                  return (
                    <div key={to.id} role="cell" className="rounded-md bg-muted/40">
                      <span className="sr-only">{g.cellSelf}</span>
                    </div>
                  )
                }
                const entry = index.get(cellKey(from.id, to.id))
                if (entry === undefined) {
                  return (
                    <div
                      key={to.id}
                      role="cell"
                      className="grid place-items-center rounded-md border border-dashed text-faint"
                      title={r.classEmpty}
                    >
                      <span aria-hidden>{r.symbolEmpty}</span>
                      <span className="sr-only">{g.cellLabel(from.name, to.name, r.classEmpty)}</span>
                    </div>
                  )
                }
                const cls = classify(entry.route)
                const text = entryText(entry, name)
                return (
                  <div key={to.id} role="cell" className="min-w-0">
                    <Tooltip content={g.cellLabel(from.name, to.name, text)}>
                      <button
                        type="button"
                        onClick={() => onPick(from.id, to.id)}
                        onMouseEnter={() => setHover({ from: from.id, to: to.id })}
                        onMouseLeave={() => setHover(null)}
                        onFocus={() => setHover({ from: from.id, to: to.id })}
                        onBlur={() => setHover(null)}
                        aria-label={g.cellLabel(from.name, to.name, text)}
                        aria-pressed={selected(from.id, to.id)}
                        data-testid={`cell-${from.id}-${to.id}`}
                        className={cn(
                          'flex size-full flex-col items-center justify-center gap-0.5 rounded-md px-2 tabular transition-[box-shadow,transform] hover:-translate-y-px hover:shadow-card hover:ring-1 hover:ring-foreground/20',
                          cls === 'none' && !to.online ? NONE_OFFLINE : CELL[cls],
                          entry.stale && 'bg-transparent opacity-60 ring-1 ring-border-strong ring-inset',
                          selected(from.id, to.id) && 'ring-2 ring-primary hover:ring-2 hover:ring-primary',
                        )}
                      >
                        <span className="text-[15px] leading-5 font-semibold">
                          {cls === 'none' ? (
                            r.symbolNone
                          ) : entry.rtt_ms !== null ? (
                            <>
                              {entry.rtt_ms}
                              <span className="ml-0.5 text-[11px] font-normal text-muted-foreground">ms</span>
                            </>
                          ) : (
                            <span className="text-muted-foreground">—</span>
                          )}
                        </span>
                        <RouteLine entry={entry} to={to} name={name} />
                      </button>
                    </Tooltip>
                  </div>
                )
              })}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
