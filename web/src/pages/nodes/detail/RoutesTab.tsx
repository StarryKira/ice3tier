import { useQuery } from '@tanstack/react-query'
import { Route as RouteIcon } from 'lucide-react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type Network, type NodeDetail, type NodeRoute } from '@/api/types'
import { NodeLink } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { REPORT_INTERVAL_SECS } from '@/lib/product'
import { formatAge, formatDuration } from '@/lib/time'
import { pairPath } from '@/topology/pair'
import { routeDetail, routeDetailHint } from '@/topology/route'

const d = t.nodes.detail

/**
 * 一条路径的写法。直连是绿点，经 relay 是紫点：中转是正常的工作状态，不用警告色，字也照常是黑的。只有无路可走是红色。
 */
function PathCell({
  entry,
  relayNames,
  peerOnline,
}: {
  entry: NodeRoute
  relayNames: (ids: number[]) => string
  /** 对端离线时"无路可走"是理所当然的：灰色，和路由矩阵里一样，不和真正的不可达抢眼。 */
  peerOnline: boolean
}) {
  const route = entry.route
  // 直连的方式、到第一个 relay 的传输：节点说得出来时写在后面，悬停有解释。
  const detail = routeDetail(route, entry)
  const hint = routeDetailHint(route, entry)
  const extra =
    detail === null ? null : (
      <Tooltip content={hint ?? undefined}>
        <Badge
          tone={entry.relay_leg === 'tcp' ? 'warn' : 'neutral'}
          tabIndex={hint === null ? undefined : 0}
          className="ml-2"
          data-testid={`route-detail-${entry.peer.id}`}
        >
          {detail}
        </Badge>
      </Tooltip>
    )
  switch (route.via) {
    case 'direct':
      return (
        <div>
          <StatusText tone="ok">{d.viaDirect}</StatusText>
          {extra}
          <div className="mt-0.5 font-mono text-xs text-muted-foreground">{d.viaDirectAddr(route.addr)}</div>
        </div>
      )
    case 'relay':
      return (
        <div>
          <StatusText tone="relay">{d.viaRelay(relayNames(route.relays))}</StatusText>
          {extra}
        </div>
      )
    case 'none':
      return (
        <div>
          <StatusText tone={peerOnline ? 'danger' : 'idle'}>{d.viaNone}</StatusText>
          <div className="mt-0.5 text-xs text-muted-foreground">{d.viaNoneHint}</div>
        </div>
      )
  }
}

/** 这个节点到其他节点现在怎么走，按它自己最近一次的上报。 */
export function RoutesTab({ network, node }: { network: Network; node: NodeDetail }) {
  const routes = useQuery(q.routes(network.id, node.id))
  const nodes = useQuery(q.nodes(network.id))
  const relayNames = (ids: number[]) => ids.map((id) => nodeName(nodes.data ?? [], id)).join(' → ')
  // 一份上报里的各行一样新；取最新的那个写在表的右上角，比它旧得多的行单独标出来。
  const rows = routes.data?.routes ?? []
  const freshest = rows.length === 0 ? null : Math.min(...rows.map((entry) => entry.age_secs))

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
        <p className="text-[13px] text-muted-foreground">{d.routesLead}</p>
        <p className="text-[13px] text-muted-foreground" data-testid="routes-updated">
          {freshest === null ? t.common.updatedEvery(REPORT_INTERVAL_SECS) : d.routesUpdated(formatAge(freshest))}
        </p>
      </div>
      <div>
        {routes.isError ? (
          <ErrorState title={d.routesLoadFailed} error={routes.error} onRetry={() => void routes.refetch()} />
        ) : routes.isSuccess && !routes.data.online ? (
          <EmptyState icon={RouteIcon} title={d.routesOffline} />
        ) : routes.isSuccess && routes.data.routes.length === 0 ? (
          <EmptyState icon={RouteIcon} title={d.routesEmpty} description={d.routesEmptyHint} />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{d.colPeer}</Th>
                <Th>{d.colPath}</Th>
                <Th numeric>{d.colRtt}</Th>
                <Th>
                  <Tooltip content={d.colSinceTip}>
                    <span
                      tabIndex={0}
                      className="cursor-help rounded-sm underline decoration-dotted underline-offset-4"
                    >
                      {d.colSince}
                    </span>
                  </Tooltip>
                </Th>
                <Th>
                  <span className="sr-only">{t.common.moreActions}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {routes.isPending ? <SkeletonRows columns={5} rows={3} /> : null}
              {rows.map((entry) => (
                <Tr key={entry.peer.id}>
                  <Td>
                    <NodeLink
                      network={network.id}
                      node={entry.peer}
                      online={nodes.data?.find((other) => other.id === entry.peer.id)?.online}
                    />
                    <div className="mt-0.5 font-mono text-xs text-muted-foreground">{entry.peer.virtual_ip}</div>
                  </Td>
                  <Td>
                    <PathCell
                      entry={entry}
                      relayNames={relayNames}
                      peerOnline={nodes.data?.find((other) => other.id === entry.peer.id)?.online ?? true}
                    />
                  </Td>
                  <Td numeric>
                    {entry.rtt_ms === null ? (
                      <span className="text-muted-foreground">{d.rttUnknown}</span>
                    ) : (
                      t.common.ms(entry.rtt_ms)
                    )}
                  </Td>
                  <Td className="text-[13px] whitespace-nowrap tabular" data-testid={`route-since-${entry.peer.id}`}>
                    {formatDuration(entry.since_secs)}
                    {freshest !== null && entry.age_secs - freshest > 2 * REPORT_INTERVAL_SECS ? (
                      <span className="block text-xs text-warn">{d.rowStale(formatAge(entry.age_secs))}</span>
                    ) : null}
                  </Td>
                  <Td className="text-right text-[13px] whitespace-nowrap">
                    <Link
                      to={pairPath(network.id, 'topology', node.id, entry.peer.id)}
                      className="rounded-sm text-muted-foreground hover:text-foreground hover:underline"
                    >
                      {d.routesOnTopology}
                    </Link>
                    <span className="mx-2 text-faint" aria-hidden>
                      ·
                    </span>
                    <Link
                      to={pairPath(network.id, 'diagnosis', node.id, entry.peer.id)}
                      className="rounded-sm text-muted-foreground hover:text-foreground hover:underline"
                    >
                      {d.routesDiagnose}
                    </Link>
                  </Td>
                </Tr>
              ))}
            </tbody>
          </Table>
        )}
      </div>
    </div>
  )
}
