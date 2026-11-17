import { useQueries, useQuery } from '@tanstack/react-query'
import { Route as RouteIcon, RotateCw } from 'lucide-react'
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

/** 一条路的下一跳和跳数：直连是对端自己、一跳；经中继是第一个中继、中继数加一。 */
function hopsOf(route: Route): { next: NodeId | null; hops: number | null } {
  switch (route.via) {
    case 'direct':
      return { next: null, hops: 1 }
    case 'relay':
      return { next: route.relays[0] ?? null, hops: route.relays.length + 1 }
    case 'none':
      return { next: null, hops: null }
  }
}

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

/** "P2P"还是"中继"：一眼分清。下面一行小字是直连的方式、到第一个中继走的传输。 */
function ModeCell({ entry, peerOnline }: { entry: NodeRoute; peerOnline: boolean }) {
  const route = entry.route
  const detail = routeDetail(route, entry)
  const hint = routeDetailHint(route, entry)
  const badge =
    route.via === 'direct' ? (
      <Badge tone="ok">{d.routesP2P}</Badge>
    ) : route.via === 'relay' ? (
      <Badge tone="relay">{d.routesRelay}</Badge>
    ) : (
      <Badge tone={peerOnline ? 'danger' : 'neutral'}>{d.viaNone}</Badge>
    )
  return (
    <div className="flex flex-col items-start gap-0.5">
      {badge}
      {detail !== null ? (
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
      ) : null}
    </div>
  )
}

/** 下一跳：直连写"直达"，经中继写第一个中继；经过不止一个中继时悬停看整条路。 */
function NextHop({ route, names }: { route: Route; names: (ids: readonly NodeId[]) => string }) {
  if (route.via === 'none') return <span className="text-muted-foreground">—</span>
  if (route.via === 'direct') return <span className="text-muted-foreground">{d.routesDirectHop}</span>
  const first = names(route.relays.slice(0, 1))
  if (route.relays.length === 1) return <span>{first}</span>
  return (
    <Tooltip content={d.viaRelay(names(route.relays))}>
      <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
        {first}
      </span>
    </Tooltip>
  )
}

/** 一格数，没有时是一道灰色的横线。 */
function Num({ children, muted = false }: { children: ReactNode; muted?: boolean }) {
  if (children === null || children === undefined) return <span className="text-muted-foreground">{d.rttUnknown}</span>
  return <span className={cn('tabular', muted && 'text-muted-foreground')}>{children}</span>
}

/**
 * 这个节点到其他节点怎么走，按它自己最近一次的上报。一行一个对端：
 * 左边一组是现在走的（P2P 还是中继、下一跳、跳数、延迟），右边一组是另一种选路模式会怎么走
 * （从现问到的候选路径里挑；和现在一样时字是灰的），再后面是流量和已持续多久。
 */
export function RoutesTab({ network, node }: { network: Network; node: NodeDetail }) {
  const routes = useQuery(q.routes(network.id, node.id))
  const nodes = useQuery(q.nodes(network.id))
  const all = nodes.data ?? []
  const names = (ids: readonly NodeId[]) => ids.map((id) => nodeName(all, id)).join(' → ')
  // 一份上报里的各行一样新；取最新的那个写在表的右上角，比它旧得多的行单独标出来。
  const rows = routes.data?.routes ?? []
  const freshest = rows.length === 0 ? null : Math.min(...rows.map((entry) => entry.age_secs))

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
        <p data-testid="routes-mode">{d.routesModeNow(modeLabel(mode))}</p>
        <div className="flex items-center gap-3">
          <p data-testid="routes-updated">
            {freshest === null
              ? t.common.updatedEvery(REPORT_INTERVAL_SECS)
              : t.common.reportedAgo(formatAge(freshest))}
          </p>
          {rows.length > 0 ? (
            <Button variant="ghost" size="sm" onClick={recompute} disabled={computing}>
              <RotateCw className={cn(computing && 'animate-spin')} aria-hidden />
              {d.routesRecompute(modeLabel(other))}
            </Button>
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
            {/* 两组列各有一个总标题：现在走的、另一种模式会怎么走。 */}
            <tr>
              <th aria-hidden />
              <th colSpan={4} scope="colgroup" className="px-3 pt-2 text-left text-xs font-medium text-foreground">
                {d.routesGroupNow(modeLabel(mode))}
              </th>
              <th
                colSpan={3}
                scope="colgroup"
                className="border-l px-3 pt-2 text-left text-xs font-medium text-foreground"
              >
                <Tooltip content={d.routesGroupOtherTip}>
                  <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
                    {d.routesGroupOther(modeLabel(other))}
                  </span>
                </Tooltip>
              </th>
              <th colSpan={3} aria-hidden className="border-l" />
            </tr>
            <tr>
              <Th>{d.colPeer}</Th>
              <Th>{d.routesColMode}</Th>
              <Th>{d.routesColNextHop}</Th>
              <Th numeric>{d.routesColHops}</Th>
              <Th numeric>{d.colRtt}</Th>
              <Th className="border-l">{d.routesColNextHop}</Th>
              <Th numeric>{d.routesColHops}</Th>
              <Th numeric>{d.colRtt}</Th>
              <Th numeric className="border-l">
                {d.routesColTraffic}
              </Th>
              <Th>
                <Tooltip content={d.colSinceTip}>
                  <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
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
            {routes.isPending ? <SkeletonRows columns={11} rows={3} /> : null}
            {rows.map((entry, index) => {
              const peer = all.find((candidate) => candidate.id === entry.peer.id)
              const now = hopsOf(entry.route)
              const path = paths[index]
              const alternative = chooseFor(other, path?.data?.forward.candidates)
              const same = alternative !== null && sameRoute(alternative.route, entry.route)
              const altHops = alternative === null ? null : hopsOf(alternative.route)
              const pending = path?.isPending === true && auto
              return (
                <Tr key={entry.peer.id}>
                  <Td>
                    <span className="inline-flex items-center gap-2">
                      <NodeLink network={network.id} node={entry.peer} online={peer?.online} />
                      <Flag code={peer?.country} />
                    </span>
                    <div className="mt-0.5 text-xs text-muted-foreground tabular">{entry.peer.virtual_ip}</div>
                  </Td>
                  <Td>
                    <ModeCell entry={entry} peerOnline={peer?.online ?? true} />
                  </Td>
                  <Td className="text-sm">
                    <NextHop route={entry.route} names={names} />
                  </Td>
                  <Td numeric>
                    <Num>{now.hops}</Num>
                  </Td>
                  <Td numeric>
                    <Num>{entry.rtt_ms === null ? null : t.common.ms(entry.rtt_ms)}</Num>
                  </Td>
                  <Td className={cn('border-l text-sm', same && 'text-muted-foreground')}>
                    {pending ? (
                      <Skeleton className="h-3.5 w-14" />
                    ) : alternative === null ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <NextHop route={alternative.route} names={names} />
                    )}
                  </Td>
                  <Td numeric>{pending ? null : <Num muted={same}>{altHops?.hops}</Num>}</Td>
                  <Td numeric>
                    {pending ? null : (
                      <Num muted={same}>{alternative === null ? null : t.common.ms(candidateRtt(alternative))}</Num>
                    )}
                  </Td>
                  <Td numeric className="border-l text-[13px] whitespace-nowrap">
                    {entry.tx_bytes === null || entry.tx_bytes === undefined ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <span className="tabular">
                        ↑ {formatBytes(entry.tx_bytes)} ↓ {formatBytes(entry.rx_bytes ?? 0)}
                      </span>
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
              )
            })}
          </tbody>
        </Table>
      )}
    </div>
  )
}
