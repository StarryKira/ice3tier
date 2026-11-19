import { useQuery } from '@tanstack/react-query'
import { RefreshCw, Route as RouteIcon, Waypoints } from 'lucide-react'
import { Fragment, useState } from 'react'
import { Link, useSearchParams } from 'react-router'

import { q } from '@/api/queries'
import { type Node, type NodeId, type PathLeg, type PathView } from '@/api/types'
import { Notice } from '@/components/notice'
import { InfoRow, Page, PageHeader } from '@/components/page'
import { PairPicker } from '@/components/pair-picker'
import { EmptyState, ErrorState } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { BoxCard, BoxCardBody, BoxCardHeader, BoxCardList, Card, CardHeader } from '@/components/ui/card'
import { Segmented } from '@/components/ui/segmented'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNow } from '@/lib/useNow'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { type Pair, pairPath, readPair, writePair } from '@/topology/pair'
import { describeCandidates, describeLeg } from '@/topology/path'

import { TestHistory, TestSection } from './TestSection'

const p = t.path

const BADGE_TONE = { ok: 'ok', idle: 'neutral', relay: 'relay', warn: 'warn', danger: 'danger' } as const

/** 一个方向：现在走的路（大字）、经过的节点、对端地址；下面几行是什么时候换的、为什么、选路模式。 */
function LegCard({ leg, name, testId }: { leg: PathLeg; name: (id: NodeId) => string; testId: string }) {
  const view = describeLeg(leg, name)
  // 节点说的"为什么是这条"和"为什么换到这条"多半是同一句：只写一次，不一样时换路的那句跟在时间下面。
  const reason = view.reason ?? view.sinceReason
  const sinceReason = view.reason !== null && view.sinceReason !== view.reason ? view.sinceReason : null
  const facts = view.since !== null || reason !== null || view.mode !== null
  return (
    <BoxCard data-testid={testId} data-source={view.state}>
      <BoxCardHeader
        title={p.forward(leg.from.name, leg.to.name)}
        action={
          // 数据从哪里来的说明在悬停里：现问到的、上报的、还是根本不是对端。
          <Tooltip content={view.sourceHint}>
            <Badge tone={BADGE_TONE[view.sourceTone]} tabIndex={0} data-testid={`${testId}-source`}>
              {view.sourceLabel}
            </Badge>
          </Tooltip>
        }
      />
      <BoxCardBody className="py-4">
        {/* 没有路时只说没有数据：为什么（未连通）写在来源的徽标和上面的提醒里。 */}
        {view.route === null ? (
          <p className="text-sm text-muted-foreground">{t.routes.noData}</p>
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
              <StatusText tone={view.route.tone} className="text-lg font-semibold tracking-tight">
                {view.route.text}
              </StatusText>
              {view.route.detail !== null ? (
                <Tooltip content={view.route.detailHint ?? undefined}>
                  <Badge
                    tone={leg.relay_leg === 'tcp' ? 'warn' : 'neutral'}
                    tabIndex={view.route.detailHint === null ? undefined : 0}
                    data-testid={`${testId}-detail`}
                  >
                    {view.route.detail}
                  </Badge>
                </Tooltip>
              ) : null}
              {view.rtt !== null ? <span className="text-lg text-muted-foreground tabular">{view.rtt}</span> : null}
            </div>
            {/* 经中继时写出经过的节点；直连就是标题里那两个，不重复。 */}
            {view.route.hops.length > 2 ? (
              <ol className="mt-2.5 flex flex-wrap items-center gap-1.5 text-[13px]" aria-label={view.route.text}>
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
              <p className="mt-1.5 font-mono text-xs text-muted-foreground">{t.routes.addr(view.route.addr)}</p>
            ) : null}
          </>
        )}
        {view.age !== null ? <p className="mt-2 text-[13px] text-warn">{view.age}</p> : null}
      </BoxCardBody>
      {facts ? (
        <BoxCardList className="mx-0 border-t [&>div]:grid-cols-[6.5rem_minmax(0,1fr)]">
          {view.since !== null ? (
            <InfoRow label={p.sinceLabel}>
              <span data-testid={`${testId}-since`}>{view.since}</span>
              {sinceReason !== null ? (
                <span className="block text-[13px] text-muted-foreground" data-testid={`${testId}-since-reason`}>
                  {sinceReason}
                </span>
              ) : null}
            </InfoRow>
          ) : null}
          {reason !== null ? (
            <InfoRow label={p.reasonLabel}>
              {/* 节点的原话。 */}
              <span data-testid={`${testId}-reason`}>{reason}</span>
            </InfoRow>
          ) : null}
          {view.mode !== null ? (
            <InfoRow label={p.modeLabel}>
              <Tooltip content={view.mode.hint}>
                <span tabIndex={0} className="rounded-sm" data-testid={`${testId}-mode`}>
                  {view.mode.label}
                </span>
              </Tooltip>
            </InfoRow>
          ) : null}
        </BoxCardList>
      ) : null}
    </BoxCard>
  )
}

type Direction = 'forward' | 'reverse'

/**
 * 发送方考虑过的路：按代价从低到高，现在走的那条标出来，每一段的延迟、抖动、丢包都写着。
 * 两个方向都知道时右上角切换方向；都不知道（不是现问到的，或者发送方的版本不会说）时不画。
 */
function Candidates({ path, name }: { path: PathView; name: (id: NodeId) => string }) {
  const [direction, setDirection] = useState<Direction>('forward')
  const known = (['forward', 'reverse'] as const).filter((key) => describeCandidates(path[key], name).state !== 'unknown')
  const active = known.includes(direction) ? direction : known[0]
  if (active === undefined) return null
  const leg = path[active]
  const view = describeCandidates(leg, name)
  const label = (key: Direction) => p.forward(path[key].from.name, path[key].to.name)
  return (
    <BoxCard data-testid={`candidates-${active}`}>
      <BoxCardHeader
        title={p.candidatesTitle}
        tip={p.costHint}
        action={
          known.length > 1 ? (
            <Segmented<Direction>
              size="sm"
              aria-label={p.direction}
              value={active}
              onChange={setDirection}
              options={known.map((key) => ({ value: key, label: label(key) }))}
            />
          ) : (
            <span className="text-[13px] text-muted-foreground">{label(active)}</span>
          )
        }
      />
      {view.state === 'empty' ? (
        <p className="px-5 py-4 text-sm text-muted-foreground">{p.candidatesEmpty}</p>
      ) : view.state === 'list' ? (
        <BoxCardBody className="pb-1">
          <Table aria-label={`${p.candidatesTitle} ${label(active)}`}>
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
                  <Td className="whitespace-nowrap">
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
        </BoxCardBody>
      ) : null}
    </BoxCard>
  )
}

function Querying() {
  return (
    <div aria-busy="true" className="space-y-3">
      <div className="grid grid-cols-1 gap-5 @3xl:grid-cols-2">
        {[0, 1].map((key) => (
          <BoxCard key={key}>
            <div className="space-y-3 px-5 py-5">
              <Skeleton className="h-4 w-2/5" />
              <Skeleton className="h-5 w-3/5" />
              <Skeleton className="h-3.5 w-1/2" />
            </div>
          </BoxCard>
        ))}
      </div>
      <p className="text-[13px] text-muted-foreground" role="status">
        {p.querying} {p.queryingHint}
      </p>
    </div>
  )
}

/** 还没选好两个节点：说一句该做什么；测过的话，下面列出整个网络最近的测试，点一行就看那一对。 */
function PickFirst({ network, onOpen }: { network: number; onOpen: (pair: Pair, test: number) => void }) {
  const recent = useQuery(q.tests(network))
  const rows = recent.data ?? []
  return (
    <div className="space-y-10">
      <EmptyState icon={RouteIcon} title={p.pickHint} className="py-12" />
      {rows.length > 0 ? (
        <TestHistory
          title={t.tests.historyTitle}
          tests={rows}
          shown={null}
          pairs
          onOpen={(test) => onOpen({ a: test.a.id, b: test.b.id }, test.id)}
        />
      ) : null}
    </div>
  )
}

/**
 * 路径诊断：选两个节点。
 * - 现在怎么走：controller 现问两端"到对端现在怎么走"，两个方向分开显示（路是发送方选的，经常不一样），
 *   每个方向写清数据是现问到的、上报的、还是根本不是对端，什么时候换的、为什么；下面是考虑过的候选路径。
 * - 实测：发包测能不能直连、各条路的延迟和丢包（文档 §19），默认显示这一对最近的一次。
 *
 * 选的两个节点和看的测试写在地址里（`?a=&b=&test=`）；拓扑和这里来回跳时跟着走。
 */
export function PathPage() {
  useDocumentTitle(p.title)
  const network = useNetwork()
  const now = useNow()
  const [params, setParams] = useSearchParams()
  const pair = readPair(params)
  const rawTest = Number(params.get('test'))
  const test = Number.isInteger(rawTest) && rawTest > 0 ? rawTest : null
  const nodes = useQuery(q.nodes(network.id))
  const same = pair.a !== null && pair.a === pair.b
  const ready = pair.a !== null && pair.b !== null && !same
  const path = useQuery({ ...q.path(network.id, pair.a ?? 0, pair.b ?? 0), enabled: ready })

  // 换了节点，原来看的测试就不是这一对的了。
  const setPair = (next: Pair, nextTest: number | null = null) => {
    const updated = writePair(params, next)
    if (nextTest === null) updated.delete('test')
    else updated.set('test', String(nextTest))
    setParams(updated, { replace: true })
  }
  const openTest = (id: number) => {
    const updated = new URLSearchParams(params)
    updated.set('test', String(id))
    setParams(updated, { replace: true })
  }
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
        description={p.subtitle}
        actions={
          <Button asChild variant="ghost">
            <Link to={pairPath(network.id, 'topology', pair.a, pair.b)}>
              <Waypoints aria-hidden />
              {p.toTopology}
            </Link>
          </Button>
        }
      />
      <div className="mb-10">
        <PairPicker
          nodes={nodes.data ?? []}
          pair={pair}
          onChange={(next) => setPair(next)}
          labels={{ a: p.pickFrom, b: p.pickTo, swap: p.swap }}
        />
        {same ? <p className="mt-2 text-[13px] text-warn">{p.same}</p> : null}
      </div>

      {!ready ? (
        same ? null : (
          <PickFirst network={network.id} onOpen={(next, id) => setPair(next, id)} />
        )
      ) : (
        <div className="space-y-12">
          <Card aria-live="polite">
            <CardHeader
              title={p.nowTitle}
              action={
                <>
                  {path.data !== undefined && !path.isFetching ? (
                    <span className="text-[13px] text-muted-foreground">
                      {p.queried(formatAge(Math.max(0, now - queriedAt)))}
                    </span>
                  ) : null}
                  <Button size="sm" pending={path.isFetching} onClick={() => void path.refetch()}>
                    {path.isFetching ? null : <RefreshCw aria-hidden />}
                    {path.isFetching ? p.querying : p.query}
                  </Button>
                </>
              }
            />
            {path.isError && path.data === undefined ? (
              <ErrorState title={p.loadFailed} error={path.error} onRetry={() => void path.refetch()} />
            ) : path.data === undefined ? (
              <Querying />
            ) : (
              <div className="space-y-5">
                {notPeers ? (
                  <Notice
                    tone="warn"
                    title={p.notPeersTitle}
                    action={
                      <Button asChild size="sm">
                        <Link
                          to={`${networkPath(network.id, 'access')}?check=1&from=${pair.a ?? ''}&to=${pair.b ?? ''}`}
                        >
                          {p.toAccess}
                        </Link>
                      </Button>
                    }
                  />
                ) : null}
                {offline.map((node) => (
                  <Notice key={node.id}>{p.offline(node.name)}</Notice>
                ))}
                <div className="grid grid-cols-1 items-start gap-5 @3xl:grid-cols-2">
                  <LegCard leg={path.data.forward} name={name} testId="path-forward" />
                  <LegCard leg={path.data.reverse} name={name} testId="path-reverse" />
                </div>
                <Candidates path={path.data} name={name} />
              </div>
            )}
          </Card>
          {pair.a !== null && pair.b !== null ? (
            <TestSection
              network={network}
              nodes={nodes.data ?? []}
              a={pair.a}
              b={pair.b}
              test={test}
              onOpen={openTest}
            />
          ) : null}
        </div>
      )}
    </Page>
  )
}
