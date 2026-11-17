import { useQueries, useQuery } from '@tanstack/react-query'
import { Route as RouteIcon, RotateCw, Waypoints } from 'lucide-react'
import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import {
  type Network,
  type NodeDetail,
  type NodeId,
  type NodeRoute,
  type PathCandidate,
  type Route,
  type RoutingMode,
} from '@/api/types'
import { Flag } from '@/components/flag'
import { InfoTip } from '@/components/info-tip'
import { NodeLink } from '@/components/node'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { nodeName } from '@/lib/nodes'
import { REPORT_INTERVAL_SECS } from '@/lib/product'
import { formatAge, formatDuration } from '@/lib/time'
import { cn } from '@/lib/utils'
import { pairPath } from '@/topology/pair'
import { routeDetail, routeDetailHint } from '@/topology/route'

const d = t.nodes.detail

/** 对端多于这么多时，另一种模式的选择不自动去算（每个对端要现问一次两端），点"重新计算"再算。 */
const AUTO_ALTERNATIVE_LIMIT = 24

const modeLabel = (mode: RoutingMode): string => (mode === 'direct_first' ? d.routingDirectFirst : d.routingLowestCost)

const sameRoute = (a: Route, b: Route): boolean => JSON.stringify(a) === JSON.stringify(b)

/**
 * 另一种模式会选哪条：从节点现测的候选路径里挑。延迟优先取代价最低的；默认模式能直连就直连，
 * 否则取代价最低的、只经一个中继的。只是参考：真正切换时还有迟滞（收益不够大时不换）。
 */
function chooseFor(mode: RoutingMode, candidates: readonly PathCandidate[] | null | undefined): PathCandidate | null {
  if (candidates === null || candidates === undefined || candidates.length === 0) return null
  const sorted = [...candidates].sort((a, b) => a.cost_us - b.cost_us)
  if (mode === 'lowest_cost') return sorted[0] ?? null
  return (
    sorted.find((candidate) => candidate.route.via === 'direct') ??
    sorted.find((candidate) => candidate.route.via === 'relay' && candidate.route.relays.length === 1) ??
    null
  )
}

/** 候选路径上各段的延迟加起来。 */
const candidateRtt = (candidate: PathCandidate): number =>
  Math.round(candidate.legs.reduce((sum, leg) => sum + leg.rtt_us, 0) / 1000)

/** 一条路的写法：经过的中继，按顺序。直连没有中继。 */
function relaysOf(route: Route, names: (ids: readonly NodeId[]) => string): string | null {
  return route.via === 'relay' ? names(route.relays) : null
}

/**
 * 路径一格：P2P 直连还是中继的标签，后面紧跟着细节——直连是怎么建立的（局域网、打洞……），
 * 经中继时经过谁、到第一个中继走 UDP 还是 TCP（TCP 是黄字）。一行写完，各行一样高。
 */
function PathCell({
  entry,
  names,
  peerOnline,
}: {
  entry: NodeRoute
  names: (ids: readonly NodeId[]) => string
  peerOnline: boolean
}) {
  const route = entry.route
  const detail = routeDetail(route, entry)
  const hint = routeDetailHint(route, entry)
  const extra =
    detail === null ? null : (
      <Tooltip content={hint ?? undefined}>
        <span
          tabIndex={hint === null ? undefined : 0}
          data-testid={`route-detail-${entry.peer.id}`}
          className={cn(
            'rounded-sm text-xs',
            entry.relay_leg === 'tcp' ? 'text-warn' : 'text-muted-foreground',
            hint !== null && 'cursor-help',
          )}
        >
          {detail}
        </span>
      </Tooltip>
    )
  if (route.via === 'none') {
    return <Badge tone={peerOnline ? 'danger' : 'neutral'}>{d.viaNone}</Badge>
  }
  return (
    <span className="flex min-w-0 items-center gap-2 whitespace-nowrap">
      {route.via === 'direct' ? <Badge tone="ok">{d.routesP2P}</Badge> : <Badge tone="relay">{d.routesRelay}</Badge>}
      {route.via === 'relay' ? <span className="text-sm">{d.viaRelay(names(route.relays))}</span> : null}
      {extra}
    </span>
  )
}

/**
 * 另一种模式那一格：和现在走的一样时只写灰色的"相同"；不一样时写它会怎么走、延迟，
 * 比现在快多少（快的是绿字，慢的是灰字）。
 */
function AlternativeCell({
  now,
  alternative,
  names,
}: {
  now: NodeRoute
  alternative: PathCandidate | null
  names: (ids: readonly NodeId[]) => string
}) {
  if (alternative === null) return <span className="text-muted-foreground">—</span>
  if (sameRoute(alternative.route, now.route)) {
    return <span className="text-[13px] text-muted-foreground">{d.routesSame}</span>
  }
  const rtt = candidateRtt(alternative)
  const gain = now.rtt_ms === null ? null : now.rtt_ms - rtt
  const via = relaysOf(alternative.route, names)
  return (
    <span className="flex min-w-0 items-center gap-2 whitespace-nowrap">
      <span className="text-sm">{via === null ? d.routesP2P : d.viaRelay(via)}</span>
      <span className="text-sm tabular">{t.common.ms(rtt)}</span>
      {gain !== null && gain !== 0 ? (
        <span className={cn('text-xs tabular', gain > 0 ? 'text-ok' : 'text-muted-foreground')}>
          {gain > 0 ? d.routesFaster(gain) : d.routesSlower(-gain)}
        </span>
      ) : null}
    </span>
  )
}

/** 一个小图标链接：悬停写它是什么，读屏读的也是这句。 */
function IconLink({ to, label, icon }: { to: string; label: string; icon: ReactNode }) {
  return (
    <Tooltip content={label}>
      <Link
        to={to}
        aria-label={label}
        className="flex size-7 items-center justify-center rounded-md text-faint hover:bg-muted hover:text-foreground [&>svg]:size-4"
      >
        {icon}
      </Link>
    </Tooltip>
  )
}

/**
 * 这个节点到其他节点怎么走，按它自己最近一次的上报。一行一个对端：路径（P2P 直连还是中继、经过谁）、延迟、
 * 换成另一种选路模式会怎么走（从现问到的候选路径里挑；和现在一样时只写"相同"）、流量、已持续多久。
 * 走得通的按延迟排在前面，不可达的、对端离线的淡下去排在后面。
 */
export function RoutesTab({ network, node }: { network: Network; node: NodeDetail }) {
  const routes = useQuery(q.routes(network.id, node.id))
  const nodes = useQuery(q.nodes(network.id))
  const all = nodes.data ?? []
  const names = (ids: readonly NodeId[]) => ids.map((id) => nodeName(all, id)).join(' → ')
  // 一份上报里的各行一样新；取最新的那个写在表的右上角，比它旧得多的行单独标出来。
  const reported = routes.data?.routes ?? []
  const freshest = reported.length === 0 ? null : Math.min(...reported.map((entry) => entry.age_secs))
  const online = (id: NodeId) => all.find((candidate) => candidate.id === id)?.online ?? true
  const rows = [...reported].sort((a, b) => {
    const rank = (entry: NodeRoute) => (entry.route.via === 'none' ? 2 : online(entry.peer.id) ? 0 : 1)
    return rank(a) - rank(b) || (a.rtt_ms ?? Infinity) - (b.rtt_ms ?? Infinity)
  })

  const mode: RoutingMode = node.routing ?? network.routing
  const other: RoutingMode = mode === 'direct_first' ? 'lowest_cost' : 'direct_first'
  const auto = rows.length <= AUTO_ALTERNATIVE_LIMIT
  const paths = useQueries({
    queries: rows.map((entry) => ({ ...q.path(network.id, node.id, entry.peer.id), enabled: auto })),
  })
  const recompute = () => {
    for (const path of paths) void path.refetch()
  }
  const computing = paths.some((path) => path.isFetching)

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-4 text-[13px] text-muted-foreground">
        <p data-testid="routes-mode" className="flex items-center gap-2">
          {d.routesModeLabel}
          <Badge tone="outline">{modeLabel(mode)}</Badge>
        </p>
        <div className="flex items-center gap-2">
          <p data-testid="routes-updated">
            {freshest === null
              ? t.common.updatedEvery(REPORT_INTERVAL_SECS)
              : t.common.reportedAgo(formatAge(freshest))}
          </p>
          {rows.length > 0 ? (
            <Tooltip content={d.routesRecompute(modeLabel(other))}>
              <Button
                variant="ghost"
                size="icon-sm"
                className="text-muted-foreground"
                onClick={recompute}
                disabled={computing}
                aria-label={d.routesRecompute(modeLabel(other))}
              >
                <RotateCw className={cn(computing && 'animate-spin')} aria-hidden />
              </Button>
            </Tooltip>
          ) : null}
        </div>
      </div>
      {routes.isError ? (
        <ErrorState title={d.routesLoadFailed} error={routes.error} onRetry={() => void routes.refetch()} />
      ) : routes.isSuccess && !routes.data.online ? (
        <EmptyState icon={RouteIcon} title={d.routesOffline} />
      ) : routes.isSuccess && routes.data.routes.length === 0 ? (
        <EmptyState icon={RouteIcon} title={d.routesEmpty} />
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{d.colPeer}</Th>
              <Th>{d.routesColPath}</Th>
              <Th numeric>{d.colRtt}</Th>
              <Th>
                <span className="inline-flex items-center gap-1">
                  {d.routesColOther(modeLabel(other))}
                  <InfoTip>{d.routesGroupOtherTip}</InfoTip>
                </span>
              </Th>
              <Th numeric>{d.routesColTraffic}</Th>
              <Th>
                <span className="inline-flex items-center gap-1">
                  {d.colSince}
                  <InfoTip>{d.colSinceTip}</InfoTip>
                </span>
              </Th>
              <Th>
                <span className="sr-only">{t.common.moreActions}</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {routes.isPending ? <SkeletonRows columns={7} rows={3} /> : null}
            {rows.map((entry) => {
              const peer = all.find((candidate) => candidate.id === entry.peer.id)
              const path = paths[rows.indexOf(entry)]
              const alternative = chooseFor(other, path?.data?.forward.candidates)
              const pending = path?.isPending === true && auto
              // 走不通、对端离线的行淡下去：名字和"不可达"照常能看清。
              const dim = entry.route.via === 'none' || peer?.online === false
              return (
                <Tr key={entry.peer.id} className="h-14">
                  <Td>
                    <span className="inline-flex items-center gap-2">
                      <NodeLink network={network.id} node={entry.peer} online={peer?.online} />
                      <Flag code={peer?.country} />
                    </span>
                    <div className="text-xs text-muted-foreground tabular">{entry.peer.virtual_ip}</div>
                  </Td>
                  <Td>
                    <PathCell entry={entry} names={names} peerOnline={peer?.online ?? true} />
                  </Td>
                  <Td numeric className={cn(dim && 'opacity-50')}>
                    {entry.rtt_ms === null ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <span className="text-sm font-medium tabular">{t.common.ms(entry.rtt_ms)}</span>
                    )}
                  </Td>
                  <Td className={cn(dim && 'opacity-50')}>
                    {entry.route.via === 'none' ? null : pending ? (
                      <Skeleton className="h-3.5 w-24" />
                    ) : (
                      <AlternativeCell now={entry} alternative={alternative} names={names} />
                    )}
                  </Td>
                  <Td numeric className={cn('text-xs whitespace-nowrap text-muted-foreground', dim && 'opacity-50')}>
                    {entry.tx_bytes === null || entry.tx_bytes === undefined ? null : (
                      <span className="tabular">
                        ↑ {formatBytes(entry.tx_bytes)} ↓ {formatBytes(entry.rx_bytes ?? 0)}
                      </span>
                    )}
                  </Td>
                  <Td
                    className={cn('text-xs whitespace-nowrap text-muted-foreground tabular', dim && 'opacity-50')}
                    data-testid={`route-since-${entry.peer.id}`}
                  >
                    {formatDuration(entry.since_secs)}
                    {freshest !== null && entry.age_secs - freshest > 2 * REPORT_INTERVAL_SECS ? (
                      <span className="block text-warn">{d.rowStale(formatAge(entry.age_secs))}</span>
                    ) : null}
                  </Td>
                  <Td>
                    <span className="flex items-center justify-end gap-0.5">
                      <IconLink
                        to={pairPath(network.id, 'topology', node.id, entry.peer.id)}
                        label={d.routesOnTopology}
                        icon={<Waypoints />}
                      />
                      <IconLink
                        to={pairPath(network.id, 'diagnosis', node.id, entry.peer.id)}
                        label={d.routesDiagnose}
                        icon={<RouteIcon />}
                      />
                    </span>
                  </Td>
                </Tr>
              )
            })}
          </tbody>
        </Table>
      )}
    </div>
  )
}
