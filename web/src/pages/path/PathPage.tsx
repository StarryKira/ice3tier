import { useQuery } from '@tanstack/react-query'
import { RefreshCw, Waypoints } from 'lucide-react'
import { Fragment } from 'react'
import { Link, useSearchParams } from 'react-router'

import { q } from '@/api/queries'
import { type Node, type NodeId, type PathLeg } from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { PairPicker } from '@/components/pair-picker'
import { ErrorState } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardBody, CardHeader } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNow } from '@/lib/useNow'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { type Pair, pairPath, readPair, writePair } from '@/topology/pair'
import { describeCandidates, describeLeg } from '@/topology/path'

const p = t.path

const BADGE_TONE = { ok: 'ok', idle: 'neutral', relay: 'relay', warn: 'warn', danger: 'danger' } as const

function LegCard({ leg, name, testId }: { leg: PathLeg; name: (id: NodeId) => string; testId: string }) {
  const view = describeLeg(leg, name)
  return (
    <Card data-testid={testId} data-source={view.state}>
      <header className="flex flex-wrap items-center justify-between gap-2 border-b pb-3">
        <h2 className="text-base font-semibold">{p.forward(leg.from.name, leg.to.name)}</h2>
        {/* 数据从哪里来的说明在悬停里：现问到的、上报的、还是根本不是对端。 */}
        <Tooltip content={view.sourceHint}>
          <Badge tone={BADGE_TONE[view.sourceTone]} tabIndex={0} data-testid={`${testId}-source`}>
            {view.sourceLabel}
          </Badge>
        </Tooltip>
      </header>
      <CardBody className="space-y-4 pt-5">
        {/* 没有路时只说没有数据：为什么（未连通）写在来源的徽标和页面顶上的提醒里。 */}
        {view.route === null ? (
          <p className="text-sm text-muted-foreground">{t.routes.noData}</p>
        ) : (
          <>
            <div className="flex flex-wrap items-baseline gap-x-1">
              <StatusText tone={view.route.tone} className="text-xl font-semibold tracking-tight">
                {view.route.text}
              </StatusText>
              {view.route.detail !== null ? (
                <Tooltip content={view.route.detailHint ?? undefined}>
                  <Badge
                    tone={leg.relay_leg === 'tcp' ? 'warn' : 'neutral'}
                    tabIndex={view.route.detailHint === null ? undefined : 0}
                    className="ml-2"
                    data-testid={`${testId}-detail`}
                  >
                    {view.route.detail}
                  </Badge>
                </Tooltip>
              ) : null}
              {view.rtt !== null ? (
                <span className="ml-2 text-xl text-muted-foreground tabular">{view.rtt}</span>
              ) : null}
            </div>
            {view.route.hops.length > 1 ? (
              <ol className="flex flex-wrap items-center gap-1.5 text-[13px]" aria-label={view.route.text}>
                {view.route.hops.map((hop, index) => (
                  <Fragment key={`${index}:${hop}`}>
                    {index > 0 ? (
                      <li aria-hidden className="text-muted-foreground">
                        →
                      </li>
                    ) : null}
                    <li className="rounded-sm bg-muted px-2 py-0.5 font-medium">{hop}</li>
                  </Fragment>
                ))}
              </ol>
            ) : null}
            {view.route.addr !== null ? (
              <p className="font-mono text-xs text-muted-foreground">{t.routes.addr(view.route.addr)}</p>
            ) : null}
          </>
        )}
        {/* 选路模式、切换的时间和数据的新旧；原因是节点的原话，作为补充写在下面，字淡一些。 */}
        <div className="space-y-1 text-[13px]">
          {view.age !== null ? <p className="text-warn">{view.age}</p> : null}
          {view.since !== null ? (
            <p data-testid={`${testId}-since`}>
              {view.since}
              {view.sinceReason !== null ? (
                <span className="block text-muted-foreground" data-testid={`${testId}-since-reason`}>
                  {view.sinceReason}
                </span>
              ) : null}
            </p>
          ) : null}
          {view.mode !== null ? (
            <Tooltip content={view.mode.hint}>
              <span tabIndex={0} className="block w-fit rounded-sm">
                {view.mode.label}
              </span>
            </Tooltip>
          ) : null}
          {view.reason !== null ? (
            <p className="text-muted-foreground" data-testid={`${testId}-reason`}>
              <span className="text-foreground">{p.reasonLabel}</span> · {view.reason}
            </p>
          ) : null}
        </div>
      </CardBody>
    </Card>
  )
}

/**
 * 一个方向考虑过的路：按代价从低到高，现在走的那条标出来，每一段的延迟、抖动、丢包都写着。
 * 不是现问到的方向没有这张表（`unknown`），不画。
 */
function Candidates({ leg, name, testId }: { leg: PathLeg; name: (id: NodeId) => string; testId: string }) {
  const view = describeCandidates(leg, name)
  if (view.state === 'unknown') return null
  return (
    <Card data-testid={testId}>
      <CardHeader
        title={p.candidatesTitle(leg.from.name, leg.to.name)}
        action={<InfoTip label={p.colCost}>{p.costHint}</InfoTip>}
      />
      {view.state === 'empty' ? (
        <p className="border-t pt-3 text-sm text-muted-foreground">{p.candidatesEmpty}</p>
      ) : (
        <Table aria-label={p.candidatesTitle(leg.from.name, leg.to.name)}>
          <thead>
            <tr>
              <Th>{p.colRoute}</Th>
              <Th numeric>{p.colCost}</Th>
              <Th>{p.colLegs}</Th>
              <Th numeric>{p.colLoad}</Th>
            </tr>
          </thead>
          <tbody>
            {view.rows.map((row) => (
              <Tr
                key={row.key}
                className={row.current ? 'bg-accent/60' : undefined}
                aria-current={row.current ? 'true' : undefined}
                data-current={row.current || undefined}
              >
                <Td>
                  <StatusText tone={row.tone} className="text-[13px]">
                    {row.text}
                  </StatusText>
                  {row.current ? (
                    <Badge tone="accent" className="ml-2">
                      {p.current}
                    </Badge>
                  ) : null}
                </Td>
                <Td numeric className="text-[13px] font-medium">
                  {row.cost}
                </Td>
                <Td className="text-xs text-muted-foreground tabular">
                  <ul className="space-y-0.5">
                    {row.legs.map((line) => (
                      <li key={line}>{line}</li>
                    ))}
                  </ul>
                </Td>
                <Td numeric className="text-[13px]">
                  {row.load}
                </Td>
              </Tr>
            ))}
          </tbody>
        </Table>
      )}
    </Card>
  )
}

function Querying() {
  return (
    <div className="grid grid-cols-1 gap-x-14 gap-y-10 @3xl:grid-cols-2" aria-busy="true">
      {[0, 1].map((key) => (
        <Card key={key}>
          <CardBody className="space-y-3 border-t pt-5">
            <Skeleton className="h-4 w-2/5" />
            <Skeleton className="h-5 w-3/5" />
            <Skeleton className="h-3.5 w-1/2" />
          </CardBody>
        </Card>
      ))}
      <p className="text-[13px] text-muted-foreground @3xl:col-span-2" role="status">
        {p.querying} {p.queryingHint}
      </p>
    </div>
  )
}

/**
 * 路径诊断：选两个节点，controller 现问两端"到对端现在怎么走"，两个方向分开显示（路是发送方选的，经常不一样）。
 * 每个方向写清数据是现问到的、上报的、还是根本不是对端，以及多久之前的、为什么换到这条路。
 * 现问到的方向还有选路模式、原因和考虑过的候选路径（每一段的延迟、抖动、丢包），放在两张卡片下面。
 */
export function PathPage() {
  useDocumentTitle(p.title)
  const network = useNetwork()
  const now = useNow()
  const [params, setParams] = useSearchParams()
  const pair = readPair(params)
  const nodes = useQuery(q.nodes(network.id))
  const same = pair.a !== null && pair.a === pair.b
  const ready = pair.a !== null && pair.b !== null && !same
  const path = useQuery({ ...q.path(network.id, pair.a ?? 0, pair.b ?? 0), enabled: ready })

  const setPair = (next: Pair) => setParams(writePair(params, next), { replace: true })
  const byId = new Map((nodes.data ?? []).map((node) => [node.id, node]))
  const name = (id: NodeId) => byId.get(id)?.name ?? t.common.nodeMissing(id)
  const offline = [pair.a, pair.b]
    .map((id) => (id === null ? undefined : byId.get(id)))
    .filter((node): node is Node => node !== undefined && !node.online)
  const notPeers =
    path.data !== undefined && [path.data.forward, path.data.reverse].some((leg) => leg.source === 'not_peers')
  const queriedAt = Math.floor(path.dataUpdatedAt / 1000)

  return (
    <Page className="@container">
      <PageHeader
        title={p.title}
        actions={
          <Button asChild variant="ghost">
            <Link to={pairPath(network.id, 'topology', pair.a, pair.b)}>
              <Waypoints aria-hidden />
              {p.toTopology}
            </Link>
          </Button>
        }
      />
      <div className="mb-10 flex flex-wrap items-end justify-between gap-3">
        <PairPicker
          nodes={nodes.data ?? []}
          pair={pair}
          onChange={setPair}
          labels={{ a: p.pickFrom, b: p.pickTo, swap: p.swap }}
        />
        {ready ? (
          <div className="flex items-center gap-3">
            {path.data !== undefined && !path.isFetching ? (
              <span className="text-[13px] text-muted-foreground">
                {p.queried(formatAge(Math.max(0, now - queriedAt)))}
              </span>
            ) : null}
            <Button size="sm" pending={path.isFetching} onClick={() => void path.refetch()}>
              {path.isFetching ? null : <RefreshCw aria-hidden />}
              {path.isFetching ? p.querying : p.query}
            </Button>
          </div>
        ) : null}
      </div>

      <div aria-live="polite" className="space-y-12">
        {same ? (
          <p className="text-sm text-warn">{p.same}</p>
        ) : !ready ? (
          <p className="text-sm text-muted-foreground">{p.pickHint}</p>
        ) : path.isError && path.data === undefined ? (
          <ErrorState title={p.loadFailed} error={path.error} onRetry={() => void path.refetch()} />
        ) : path.data === undefined ? (
          <Querying />
        ) : (
          <>
            <div className="space-y-6">
              {notPeers ? (
                <Notice
                  tone="warn"
                  title={p.notPeersTitle}
                  action={
                    <Button asChild size="sm">
                      <Link to={`${networkPath(network.id, 'access')}?check=1&from=${pair.a ?? ''}&to=${pair.b ?? ''}`}>
                        {p.toAccess}
                      </Link>
                    </Button>
                  }
                />
              ) : null}
              {offline.map((node) => (
                <Notice key={node.id}>{p.offline(node.name)}</Notice>
              ))}
              <div className="grid grid-cols-1 gap-x-14 gap-y-10 @3xl:grid-cols-2">
                <LegCard leg={path.data.forward} name={name} testId="path-forward" />
                <LegCard leg={path.data.reverse} name={name} testId="path-reverse" />
              </div>
            </div>
            <Candidates leg={path.data.forward} name={name} testId="candidates-forward" />
            <Candidates leg={path.data.reverse} name={name} testId="candidates-reverse" />
          </>
        )}
      </div>
    </Page>
  )
}
