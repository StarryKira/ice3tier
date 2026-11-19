import { useQuery } from '@tanstack/react-query'
import { RadioTower, RefreshCw } from 'lucide-react'
import { useState } from 'react'

import { q } from '@/api/queries'
import { type BackboneRelay, type Network, type NodeId } from '@/api/types'
import { Flag } from '@/components/flag'
import { Notice } from '@/components/notice'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { OnlineDot, type Tone } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'
import { formatAge } from '@/lib/time'
import { useNow } from '@/lib/useNow'
import { cn } from '@/lib/utils'
import { linkView, relayRowView } from '@/topology/backbone'

import { type NodeLook } from './TopologyGraph'

const b = t.topology.backbone

/** 和路由矩阵同一套格子：底色很淡，正常的字是黑的，要注意的才上色。 */
const CELL = {
  ok: 'bg-ok-soft',
  warn: 'bg-warn-soft',
  danger: 'bg-danger-soft text-danger',
  idle: '',
  relay: 'bg-relay-soft',
} as const

/** 行头里的状态只用字的颜色：前面已经有在线的点了，再来一个点就是两个。正常的是灰字，满载、即将下线琥珀色。 */
const STATE_TEXT: Record<Tone, string> = {
  ok: '',
  idle: 'text-faint',
  relay: 'text-relay',
  warn: 'font-medium text-warn',
  danger: 'font-medium text-danger',
}

/** 行头：在线的点、国旗、名字；下面是它自己报的状态和负载，再下面一行小字是这一行多新、从谁那里问来的。 */
function RelayHead({
  row,
  look,
  name,
}: {
  row: BackboneRelay
  look: NodeLook | undefined
  name: (id: NodeId) => string
}) {
  const head = relayRowView(row, name)
  const meta = [head.age, head.reportedBy].filter((part) => part !== null)
  return (
    <span className="flex min-w-0 flex-col gap-1">
      <span className="flex min-w-0 items-center gap-1.5 text-[13px] font-medium">
        <OnlineDot online={row.online} />
        <Flag code={look?.country} />
        <span className="truncate">{row.relay.name}</span>
      </span>
      <span className="flex items-center gap-1.5 pl-3.5 text-xs text-muted-foreground">
        <Tooltip content={head.state.hint}>
          <span tabIndex={0} className={cn('rounded-sm', STATE_TEXT[head.state.tone])}>
            {head.state.label}
          </span>
        </Tooltip>
        {head.load !== null ? (
          <>
            <span aria-hidden>·</span>
            <span className="tabular">{head.load}</span>
          </>
        ) : null}
      </span>
      {meta.length > 0 ? <span className="truncate pl-3.5 text-[11px] text-faint">{meta.join(' · ')}</span> : null}
    </span>
  )
}

/**
 * 骨干：relay 之间的延迟矩阵。每次打开都要 controller 现问几个节点（最多 3 秒左右），所以不轮询，
 * 顶上一行左边是这次哪些节点回答了，右边是什么时候问的和"重新查询"。
 * 行是测的那个 relay（带它自己报的状态和负载），列是对面的 relay。和路由矩阵一样横竖铺满：
 * 一格上面大字是延迟，下面一行抖动和丢包（丢包到了百分之一是琥珀色）；悬停一格时它的行头、列头亮起来。
 */
export function BackboneView({
  network,
  name,
  looks,
}: {
  network: Network
  name: (id: NodeId) => string
  /** 节点列表里的国家（画旗子），按节点编号。 */
  looks: ReadonlyMap<NodeId, NodeLook>
}) {
  const now = useNow()
  const backbone = useQuery(q.backbone(network.id))
  const data = backbone.data
  const queriedAt = Math.floor(backbone.dataUpdatedAt / 1000)
  const [hover, setHover] = useState<{ from: NodeId; to: NodeId } | null>(null)

  return (
    <div className="flex flex-1 flex-col gap-4" data-testid="backbone">
      <div className="flex min-h-9 flex-wrap items-center justify-between gap-x-6 gap-y-2">
        <p className="text-xs text-muted-foreground">
          {data !== undefined && data.answered.length > 0
            ? b.answered(formatList(data.answered.map((node) => node.name)))
            : null}
        </p>
        <div className="flex items-center gap-3">
          {data !== undefined && !backbone.isFetching ? (
            <span className="text-xs text-muted-foreground">{b.queried(formatAge(Math.max(0, now - queriedAt)))}</span>
          ) : null}
          <Button size="sm" pending={backbone.isFetching} onClick={() => void backbone.refetch()}>
            {backbone.isFetching ? null : <RefreshCw aria-hidden />}
            {backbone.isFetching ? b.querying : b.query}
          </Button>
        </div>
      </div>
      <div aria-live="polite" className="flex flex-1 flex-col">
        {backbone.isError && data === undefined ? (
          <ErrorState title={b.loadFailed} error={backbone.error} onRetry={() => void backbone.refetch()} />
        ) : data === undefined ? (
          <div>
            <SkeletonBlock lines={4} />
            <p className="text-[13px] text-muted-foreground" role="status">
              {b.querying} {b.queryingHint}
            </p>
          </div>
        ) : data.relays.length === 0 ? (
          <EmptyState icon={RadioTower} title={b.noRelaysTitle} />
        ) : (
          <div className="flex flex-1 flex-col gap-4">
            {data.answered.length === 0 ? <Notice tone="warn">{b.nobody}</Notice> : null}
            {data.relays.length === 1 && data.relays[0] !== undefined ? (
              // 只有一个中继：没有链路可画，只写它自己的状态。
              <>
                <Notice>{b.oneRelay}</Notice>
                <div
                  className="w-fit min-w-56 rounded-lg border px-4 py-3"
                  data-testid={`backbone-row-${data.relays[0].relay.id}`}
                >
                  <RelayHead row={data.relays[0]} look={looks.get(data.relays[0].relay.id)} name={name} />
                </div>
              </>
            ) : (
              <div className="relative min-h-72 flex-1">
                <div className="absolute inset-0 overflow-auto">
                  <div
                    role="table"
                    aria-label={b.caption}
                    className="grid gap-1"
                    style={{
                      gridTemplateColumns: `12rem repeat(${data.relays.length}, minmax(7rem, 1fr))`,
                      gridTemplateRows: `2.25rem repeat(${data.relays.length}, minmax(4.5rem, 6.5rem))`,
                      alignContent: 'start',
                    }}
                  >
                    <div role="row" className="contents">
                      <div
                        role="columnheader"
                        className="sticky top-0 left-0 z-20 flex items-center bg-card pr-3 text-xs font-medium whitespace-nowrap text-muted-foreground"
                      >
                        {b.from}
                      </div>
                      {data.relays.map(({ relay }) => (
                        <div
                          key={relay.id}
                          role="columnheader"
                          className={cn(
                            'sticky top-0 z-10 flex min-w-0 items-center justify-center gap-1.5 rounded-md bg-card px-1.5 text-xs font-medium text-muted-foreground transition-colors',
                            hover?.to === relay.id && 'bg-muted text-foreground',
                          )}
                        >
                          <Flag code={looks.get(relay.id)?.country} />
                          <span className="truncate">{relay.name}</span>
                        </div>
                      ))}
                    </div>
                    {data.relays.map((row) => (
                      <div
                        key={row.relay.id}
                        role="row"
                        className="contents"
                        data-testid={`backbone-row-${row.relay.id}`}
                      >
                        <div
                          role="rowheader"
                          className={cn(
                            'sticky left-0 z-10 flex items-center rounded-md bg-card pr-3 pl-1 transition-colors',
                            hover?.from === row.relay.id && 'bg-muted',
                          )}
                        >
                          <RelayHead row={row} look={looks.get(row.relay.id)} name={name} />
                        </div>
                        {data.relays.map((column) => {
                          if (column.relay.id === row.relay.id) {
                            return (
                              <div key={column.relay.id} role="cell" className="rounded-md bg-muted/40">
                                <span className="sr-only">{t.topology.cellSelf}</span>
                              </div>
                            )
                          }
                          const link = linkView(row, column.relay.id)
                          const label = t.topology.cellLabel(row.relay.name, column.relay.name, link?.text ?? b.noLink)
                          const enter = () => setHover({ from: row.relay.id, to: column.relay.id })
                          const leave = () => setHover(null)
                          return (
                            <div key={column.relay.id} role="cell" className="min-w-0">
                              <Tooltip content={label}>
                                <span
                                  tabIndex={0}
                                  role="img"
                                  aria-label={label}
                                  data-testid={`backbone-cell-${row.relay.id}-${column.relay.id}`}
                                  onMouseEnter={enter}
                                  onMouseLeave={leave}
                                  onFocus={enter}
                                  onBlur={leave}
                                  className={cn(
                                    'flex size-full flex-col items-center justify-center gap-0.5 rounded-md px-2 tabular transition-[box-shadow] hover:ring-1 hover:ring-foreground/20',
                                    link === null ? 'border border-dashed text-faint' : CELL[link.tone],
                                  )}
                                >
                                  {link === null ? (
                                    <span aria-hidden>{t.routes.symbolEmpty}</span>
                                  ) : (
                                    <>
                                      <span className="text-[15px] leading-5 font-semibold">
                                        {link.rtt === null ? (
                                          t.routes.symbolNone
                                        ) : (
                                          <>
                                            {link.rtt}
                                            <span className="ml-0.5 text-[11px] font-normal text-muted-foreground">
                                              ms
                                            </span>
                                          </>
                                        )}
                                      </span>
                                      <span
                                        className={cn(
                                          'max-w-full truncate text-[11px] leading-4',
                                          link.lossy
                                            ? 'font-medium text-warn'
                                            : link.rtt === null
                                              ? ''
                                              : 'text-muted-foreground',
                                        )}
                                      >
                                        {link.detail}
                                      </span>
                                    </>
                                  )}
                                </span>
                              </Tooltip>
                            </div>
                          )
                        })}
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
