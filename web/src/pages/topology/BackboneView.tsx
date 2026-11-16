import { useQuery } from '@tanstack/react-query'
import { RadioTower, RefreshCw } from 'lucide-react'

import { q } from '@/api/queries'
import { type Network, type NodeId } from '@/api/types'
import { Notice } from '@/components/notice'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { OnlineDot, StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'
import { useNow } from '@/lib/useNow'
import { cn } from '@/lib/utils'
import { linkView, relayRowView } from '@/topology/backbone'

const b = t.topology.backbone

/** 和路由矩阵同一套格子：底色很淡，正常的字是黑的，要注意的才上色。 */
const CELL = {
  ok: 'bg-ok-soft text-foreground',
  warn: 'bg-warn-soft text-warn',
  danger: 'bg-danger-soft text-danger',
  idle: 'text-faint',
  relay: 'bg-relay-soft text-foreground',
} as const

/**
 * 骨干：relay 之间的延迟矩阵。每次打开都要 controller 现问几个节点（最多 3 秒左右），所以不轮询，
 * 有"再问一次"。行是测的那个 relay（带它自己报的状态和负载），列是对面的 relay。
 */
export function BackboneView({ network, name }: { network: Network; name: (id: NodeId) => string }) {
  const now = useNow()
  const backbone = useQuery(q.backbone(network.id))
  const data = backbone.data
  const queriedAt = Math.floor(backbone.dataUpdatedAt / 1000)

  return (
    <div className="space-y-6" data-testid="backbone">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <p className="max-w-3xl text-[13px] text-muted-foreground">{b.lead}</p>
        <div className="flex items-center gap-3">
          {data !== undefined && !backbone.isFetching ? (
            <span className="text-[13px] text-muted-foreground">
              {b.queried(formatAge(Math.max(0, now - queriedAt)))}
            </span>
          ) : null}
          <Button size="sm" pending={backbone.isFetching} onClick={() => void backbone.refetch()}>
            {backbone.isFetching ? null : <RefreshCw aria-hidden />}
            {backbone.isFetching ? b.querying : b.query}
          </Button>
        </div>
      </div>
      <div aria-live="polite">
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
          <EmptyState icon={RadioTower} title={b.noRelaysTitle} description={b.noRelaysBody} />
        ) : (
          <div className="space-y-4">
            {data.answered.length === 0 ? <Notice tone="warn">{b.nobody}</Notice> : null}
            {data.relays.length === 1 ? <Notice>{b.oneRelay}</Notice> : null}
            <div className="overflow-x-auto">
              {/* 和路由矩阵一样按内容定宽：relay 少的时候格子不会被拉成一长条。 */}
              <table className="border-collapse text-sm">
                <caption className="sr-only">{b.caption}</caption>
                <thead>
                  <tr>
                    <th
                      scope="col"
                      className="border-b bg-card py-2 pr-4 text-left text-xs font-medium whitespace-nowrap text-muted-foreground"
                    >
                      {b.from}
                    </th>
                    {data.relays.map((relay) => (
                      <th
                        key={relay.relay.id}
                        scope="col"
                        className="max-w-28 border-b bg-card px-1.5 py-2 text-xs font-medium text-muted-foreground"
                      >
                        <span className="block truncate" title={relay.relay.name}>
                          {relay.relay.name}
                        </span>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {data.relays.map((row) => {
                    const head = relayRowView(row, name)
                    return (
                      <tr
                        key={row.relay.id}
                        className="border-b last:border-b-0"
                        data-testid={`backbone-row-${row.relay.id}`}
                      >
                        <th scope="row" className="bg-card py-2.5 pr-4 text-left align-top font-normal">
                          <span className="flex items-center gap-1.5 text-[13px] font-medium">
                            <OnlineDot online={row.online} />
                            {row.relay.name}
                          </span>
                          <span className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
                            <Tooltip content={head.state.hint}>
                              <span tabIndex={0} className="rounded-sm">
                                <StatusText tone={head.state.tone}>{head.state.label}</StatusText>
                              </span>
                            </Tooltip>
                            {head.load !== null ? <span className="tabular">{head.load}</span> : null}
                          </span>
                          {head.age !== null || head.reportedBy !== null ? (
                            <span className="mt-0.5 block text-xs text-muted-foreground">
                              {[head.age, head.reportedBy].filter((part) => part !== null).join(' · ')}
                            </span>
                          ) : null}
                        </th>
                        {data.relays.map((column) => {
                          if (column.relay.id === row.relay.id) {
                            return (
                              <td key={column.relay.id} className="p-0.5 text-center">
                                <span className="block h-8 min-w-14 rounded-sm bg-muted/50" aria-hidden />
                                <span className="sr-only">{t.topology.cellSelf}</span>
                              </td>
                            )
                          }
                          const link = linkView(row, column.relay.id)
                          const label = b.cellLabel(row.relay.name, column.relay.name, link?.text ?? b.noLink)
                          return (
                            <td key={column.relay.id} className="p-0.5 text-center">
                              <Tooltip content={label}>
                                <span
                                  tabIndex={0}
                                  role="img"
                                  aria-label={label}
                                  data-testid={`backbone-cell-${row.relay.id}-${column.relay.id}`}
                                  className={cn(
                                    'flex h-8 min-w-14 items-center justify-center rounded-sm px-1 text-xs font-medium tabular',
                                    CELL[link?.tone ?? 'idle'],
                                  )}
                                >
                                  {link?.short ?? t.routes.symbolEmpty}
                                </span>
                              </Tooltip>
                            </td>
                          )
                        })}
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
            {data.answered.length > 0 ? (
              <p className="text-xs text-muted-foreground">
                {b.answered(data.answered.map((node) => node.name).join('、'))}
              </p>
            ) : null}
          </div>
        )}
      </div>
    </div>
  )
}
