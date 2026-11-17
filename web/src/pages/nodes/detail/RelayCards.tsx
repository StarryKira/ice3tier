import { useQuery } from '@tanstack/react-query'
import { RotateCw } from 'lucide-react'
import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type MatrixEntry, type Network, type Node, type NodeId, type RelayState } from '@/api/types'
import { Flag } from '@/components/flag'
import { NodeLink } from '@/components/node'
import { SkeletonBlock } from '@/components/states'
import { StatusText, type Tone } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'
import { nodeName } from '@/lib/nodes'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'
import { formatPermille } from '@/topology/route'

/*
 * 中继页上，中继卡片旁边的几张卡片。都是一样的列表行：左边是节点，右边是数（延迟），
 * 补充的信息写在第二行的小字里。不用横跨整张卡片的表格：卡片放在半宽的栏里时，几列会挤，
 * 放在整宽时，几列又隔得太远。
 */

const d = t.nodes.detail
const b = t.topology.backbone

interface RowProps {
  left: ReactNode
  right?: ReactNode
  /** 第二行的小字。 */
  sub?: ReactNode
}

function Row({ left, right, sub }: RowProps) {
  return (
    <li className="flex min-h-12 flex-col justify-center gap-0.5 px-5 py-2.5">
      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">{left}</div>
        {right !== undefined ? <div className="flex shrink-0 items-center gap-2">{right}</div> : null}
      </div>
      {sub !== undefined && sub !== null ? <div className="text-xs text-muted-foreground">{sub}</div> : null}
    </li>
  )
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="px-5 py-6 text-center text-[13px] text-muted-foreground">{children}</p>
}

function Pending() {
  return (
    <div className="px-5 py-4">
      <SkeletonBlock lines={2} />
    </div>
  )
}

/** 卡片标题：名字，后面一个灰色的数。 */
function CountTitle({ title, count, testId }: { title: string; count?: number; testId?: string }) {
  return (
    <span className="inline-flex items-center gap-2">
      {title}
      {count !== undefined ? (
        <span data-testid={testId} className="text-sm font-normal text-muted-foreground tabular">
          {count}
        </span>
      ) : null}
    </span>
  )
}

/** 去拓扑页某个视图的一个安静的链接。 */
function TopologyLink({
  network,
  view,
  children,
}: {
  network: Network
  view: 'matrix' | 'backbone'
  children: string
}) {
  return (
    <Link
      to={`${networkPath(network.id, 'topology')}?view=${view}`}
      className="rounded-sm text-[13px] text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
    >
      {children}
    </Link>
  )
}

/** 右边的延迟：深色的数；没有时是灰色的说明。 */
function Rtt({ ms, none }: { ms: number | null; none?: string }) {
  if (ms === null) return <span className="text-[13px] text-muted-foreground">{none ?? '—'}</span>
  return <span className="text-sm tabular">{t.common.ms(ms)}</span>
}

const STATE_TONE: Record<RelayState, Tone> = { serving: 'ok', overloaded: 'warn', draining: 'warn', unknown: 'idle' }

/**
 * 中继自己报的状态和负载（骨干矩阵里它那一行）。中继卡片顶上那排数据里用。
 * 骨干要现问节点，最多等 3 秒：没问到时写"无数据"。
 */
export function RelayStateValue({ network, relay }: { network: Network; relay: NodeId }) {
  const backbone = useQuery(q.backbone(network.id))
  if (backbone.isPending) return <span className="text-muted-foreground">{b.querying}</span>
  const row = backbone.data?.relays.find((entry) => entry.relay.id === relay)
  if (row?.state === null || row?.state === undefined) return <span className="text-muted-foreground">{b.noRow}</span>
  return (
    <span className="inline-flex flex-wrap items-center gap-x-2">
      <StatusText tone={STATE_TONE[row.state]} className="gap-1.5">
        {b.state[row.state]}
      </StatusText>
      {row.load_percent !== null && row.load_percent > 0 ? (
        <span className="text-[13px] font-normal text-muted-foreground tabular">{b.load(row.load_percent)}</span>
      ) : null}
    </span>
  )
}

interface ConnectedRow {
  node: Node
  /** 到这个中继的延迟；没连着（只是指定了它）时是 `null`。 */
  rtt: number | null
  /** 这个中继是它延迟最低的那个：别的节点经中继找它时走这一个。 */
  preferred: boolean
  pinned: boolean
  /** 它连着的其他中继，按延迟排。 */
  others: { id: NodeId; rtt: number }[]
}

function connectedRows(relay: NodeId, nodes: readonly Node[]): ConnectedRow[] {
  return nodes
    .filter((node) => node.attached.some((entry) => entry.relay === relay) || node.pinned_relays.includes(relay))
    .map((node) => {
      const sorted = [...node.attached].sort((x, y) => x.rtt_ms - y.rtt_ms)
      return {
        node,
        rtt: sorted.find((entry) => entry.relay === relay)?.rtt_ms ?? null,
        preferred: sorted[0]?.relay === relay,
        pinned: node.pinned_relays.includes(relay),
        others: sorted
          .filter((entry) => entry.relay !== relay)
          .map((entry) => ({ id: entry.relay, rtt: entry.rtt_ms })),
      }
    })
    .sort((x, y) => (x.rtt ?? Infinity) - (y.rtt ?? Infinity))
}

/**
 * 连着这个中继的节点：在线、名字、国旗、虚拟 IP；右边是这个中继对它是首选还是备用、是不是它指定的、延迟；
 * 第二行写它还连着哪些中继。按延迟排；指定了它、但现在没连着的排在最后。
 */
export function ConnectedNodesCard({
  network,
  relay,
  nodes,
}: {
  network: Network
  relay: NodeId
  nodes: readonly Node[]
}) {
  const rows = connectedRows(relay, nodes)
  return (
    <BoxCard>
      <BoxCardHeader
        title={
          <CountTitle
            title={d.relayConnectedTitle}
            count={rows.filter((row) => row.rtt !== null).length}
            testId="relay-attached"
          />
        }
        tip={
          <>
            <p>{d.relayAttachedTip}</p>
            <p className="mt-1">{d.relayRankTip}</p>
          </>
        }
      />
      {rows.length === 0 ? (
        <Empty>{d.relayConnectedNone}</Empty>
      ) : (
        <ul className="divide-y" aria-label={d.relayConnectedTitle}>
          {rows.map((row) => (
            <Row
              key={row.node.id}
              left={
                <>
                  <NodeLink network={network.id} node={row.node} online={row.node.online} />
                  <Flag code={row.node.country} />
                  <span className="text-[13px] text-muted-foreground tabular">{row.node.virtual_ip}</span>
                </>
              }
              right={
                <>
                  {row.rtt === null ? null : row.preferred ? (
                    <Badge tone="relay">{d.relayRankFirst}</Badge>
                  ) : (
                    <Badge>{d.relayRankBackup}</Badge>
                  )}
                  {row.pinned ? <Badge tone="outline">{d.relayPinnedTag}</Badge> : null}
                  <Rtt ms={row.rtt} none={d.relayNotConnected} />
                </>
              }
              sub={
                row.others.length === 0
                  ? null
                  : d.relayAlsoConnected(
                      formatList(row.others.map((other) => `${nodeName(nodes, other.id)} ${t.common.ms(other.rtt)}`)),
                    )
              }
            />
          ))}
        </ul>
      )}
    </BoxCard>
  )
}

interface PairRow {
  key: string
  from: NodeId
  to: NodeId
  /** 两个方向都经它。 */
  both: boolean
  relays: NodeId[]
  rtt: number | null
  stale: boolean
}

/**
 * 路由矩阵里经这个中继的格子，两个方向都经它的合成一行。一端就是这个中继自己的不算：
 * 那是别人连到它，不是它在替别人转。
 */
function pairRows(entries: readonly MatrixEntry[], relay: NodeId): PairRow[] {
  const rows: PairRow[] = []
  for (const entry of entries) {
    if (entry.route.via !== 'relay' || !entry.route.relays.includes(relay)) continue
    if (entry.from === relay || entry.to === relay) continue
    const reverse = rows.find((row) => row.from === entry.to && row.to === entry.from)
    if (reverse !== undefined) {
      reverse.both = true
      reverse.stale = reverse.stale || entry.stale
      continue
    }
    rows.push({
      key: `${entry.from}-${entry.to}`,
      from: entry.from,
      to: entry.to,
      both: false,
      relays: entry.route.relays,
      rtt: entry.rtt_ms,
      stale: entry.stale,
    })
  }
  return rows.sort((x, y) => (x.rtt ?? Infinity) - (y.rtt ?? Infinity))
}

/**
 * 经这个中继中转的节点对：哪两个节点、延迟；经过不止这一个中继时，第二行写出经过的中继。
 * 回答"这个中继在帮谁转"；"已连接的节点"只说明谁连着它。数据来自各节点上报的路由表，和路由矩阵一样。
 */
export function RelayPairsCard({ network, relay }: { network: Network; relay: NodeId }) {
  const matrix = useQuery(q.matrix(network.id))
  const names = matrix.data?.nodes ?? []
  const rows = pairRows(matrix.data?.entries ?? [], relay)
  const name = (id: NodeId) => names.find((node) => node.id === id)?.name ?? `#${id}`
  const online = (id: NodeId) => names.find((node) => node.id === id)?.online

  return (
    <BoxCard data-testid="relay-pairs">
      <BoxCardHeader
        title={<CountTitle title={d.relayPairsTitle} count={matrix.isSuccess ? rows.length : undefined} />}
        tip={d.relayPairsTip}
        action={
          <TopologyLink network={network} view="matrix">
            {d.relayPairsMatrix}
          </TopologyLink>
        }
      />
      {matrix.isPending ? (
        <Pending />
      ) : rows.length === 0 ? (
        <Empty>{d.relayPairsNone}</Empty>
      ) : (
        <ul className="divide-y" aria-label={d.relayPairsTitle}>
          {rows.map((row) => (
            <Row
              key={row.key}
              left={
                <>
                  <NodeLink
                    network={network.id}
                    node={{ id: row.from, name: name(row.from) }}
                    online={online(row.from)}
                  />
                  <span className="text-faint" aria-label={row.both ? d.relayBoth : d.relayOneWay}>
                    {row.both ? '⇄' : '→'}
                  </span>
                  <NodeLink network={network.id} node={{ id: row.to, name: name(row.to) }} online={online(row.to)} />
                  {row.stale ? <Badge tone="outline">{t.routes.stale}</Badge> : null}
                </>
              }
              right={<Rtt ms={row.rtt} />}
              sub={row.relays.length > 1 ? d.relayVia(row.relays.map(name).join(' → ')) : null}
            />
          ))}
        </ul>
      )}
    </BoxCard>
  )
}

/**
 * 它到网络里其他中继的链路（骨干矩阵里它那一行）：右边是延迟，第二行是抖动和丢包。多级中转按这些选路。
 * 骨干要现问节点，不轮询：标题右边有"重新查询"。
 */
export function RelayLinksCard({ network, relay }: { network: Network; relay: NodeId }) {
  const backbone = useQuery(q.backbone(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const row = backbone.data?.relays.find((entry) => entry.relay.id === relay)
  const others = (backbone.data?.relays ?? []).filter((entry) => entry.relay.id !== relay)

  let body: ReactNode
  if (backbone.isPending) body = <Pending />
  else if (backbone.isError) body = <Empty>{b.loadFailed}</Empty>
  else if (others.length === 0) body = <Empty>{b.oneRelay}</Empty>
  else if (row === undefined || row.links.length === 0) body = <Empty>{b.noRowHint}</Empty>
  else {
    body = (
      <ul className="divide-y" aria-label={d.relayLinksTitle}>
        {[...row.links]
          .sort((x, y) => (x.rtt_ms ?? Infinity) - (y.rtt_ms ?? Infinity))
          .map((link) => {
            const other = others.find((entry) => entry.relay.id === link.to)
            const country = nodes.data?.find((candidate) => candidate.id === link.to)?.country
            return (
              <Row
                key={link.to}
                left={
                  <>
                    <NodeLink
                      network={network.id}
                      node={{ id: link.to, name: other?.relay.name ?? nodeName(nodes.data ?? [], link.to) }}
                      online={other?.online}
                    />
                    <Flag code={country} />
                  </>
                }
                right={
                  link.rtt_ms === null ? (
                    <span className="text-[13px] text-warn">{d.relayLinkDown}</span>
                  ) : (
                    <Rtt ms={link.rtt_ms} />
                  )
                }
                sub={
                  link.rtt_ms === null ? null : (
                    <span className={cn(link.loss_permille >= 10 && 'text-warn')}>
                      {d.relayLinkStats(t.common.ms(link.jitter_ms), formatPermille(link.loss_permille))}
                    </span>
                  )
                }
              />
            )
          })}
      </ul>
    )
  }

  return (
    <BoxCard data-testid="relay-links">
      <BoxCardHeader
        title={<CountTitle title={d.relayLinksTitle} count={others.length > 0 ? row?.links.length : undefined} />}
        tip={d.relayLinksTip}
        action={
          <>
            <TopologyLink network={network} view="backbone">
              {d.relayLinksBackbone}
            </TopologyLink>
            <Button
              variant="ghost"
              size="icon-sm"
              className="text-muted-foreground"
              disabled={backbone.isFetching}
              onClick={() => void backbone.refetch()}
              aria-label={backbone.isFetching ? b.querying : b.query}
            >
              <RotateCw className={cn(backbone.isFetching && 'animate-spin')} aria-hidden />
            </Button>
          </>
        }
      />
      {body}
    </BoxCard>
  )
}
