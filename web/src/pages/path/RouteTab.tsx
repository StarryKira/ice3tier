import { Fragment, useState } from 'react'

import { type Choice, type DecisionView, type NodeId, type PathLeg, type PathView, type TestView } from '@/api/types'
import { InfoRow } from '@/components/page'
import { StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { BoxCard, BoxCardBody, BoxCardHeader, BoxCardList } from '@/components/ui/card'
import { Segmented } from '@/components/ui/segmented'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { msText, problemText } from '@/lib/tests'
import { routeName } from '@/topology/diagnosis'
import { describeCandidates, describeLeg } from '@/topology/path'

import { ItemState, type Named } from './parts'

const p = t.path
const d = t.diagnosis

const BADGE_TONE = { ok: 'ok', idle: 'neutral', relay: 'relay', warn: 'warn', danger: 'danger' } as const

/** 一个方向：现在走的路（大字）、经过的节点、对端地址；下面几行是什么时候换的、为什么、选路模式。 */
export function LegCard({ leg, name, testId }: { leg: PathLeg; name: (id: NodeId) => string; testId: string }) {
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
export function Candidates({ path, name }: { path: PathView; name: (id: NodeId) => string }) {
  const [direction, setDirection] = useState<Direction>('forward')
  const known = (['forward', 'reverse'] as const).filter(
    (key) => describeCandidates(path[key], name).state !== 'unknown',
  )
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

export function Querying() {
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

const routeTone = (choice: Choice) =>
  choice.route?.via === 'direct' ? 'ok' : choice.route?.via === 'relay' ? 'relay' : ('idle' as const)

function reasonText(reason: string): string {
  return Object.hasOwn(d.reason, reason) ? d.reason[reason as keyof typeof d.reason] : reason
}

/** 一个方向两种模式各会走哪条：现在用的那种标出来；延迟优先留在原地时写出那条更便宜的。 */
function DecisionRows({ decision, nodes }: { decision: DecisionView; nodes: Named }) {
  const name = (id: NodeId) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)
  const rows = [
    { mode: 'direct_first' as const, choice: decision.direct_first },
    { mode: 'lowest_cost' as const, choice: decision.lowest_cost },
  ]
  return (
    <>
      <tr className="bg-muted/40">
        <th colSpan={4} scope="colgroup" className="px-5 py-2 text-left text-[13px] font-medium">
          {p.forward(name(decision.from), name(decision.to))}
        </th>
      </tr>
      {rows.map(({ mode, choice }) => (
        <Tr key={mode} data-testid={`decision-${decision.from}-${mode}`}>
          <Td className="whitespace-nowrap">
            <span className="text-[13px]">{p.mode[mode]}</span>
            {decision.mode === mode ? (
              <Badge tone="accent" className="ml-2">
                {p.current}
              </Badge>
            ) : null}
          </Td>
          <Td className="whitespace-nowrap">
            <StatusText tone={routeTone(choice)} className="text-[13px] font-medium">
              {routeName(choice.route, nodes)}
            </StatusText>
          </Td>
          <Td className="text-[13px] text-muted-foreground">
            {reasonText(choice.reason)}
            {choice.cheaper !== null && choice.cheaper !== undefined ? (
              <span className="block text-xs">
                {d.cheaperLine(routeName(choice.cheaper.route, nodes), msText(choice.cheaper.cost_us))}
              </span>
            ) : null}
          </Td>
          <Td numeric className="text-[13px] tabular">
            {choice.cost_us === null ? '—' : msText(choice.cost_us)}
          </Td>
        </Tr>
      ))}
    </>
  )
}

/** 诊断里两端各自算的：两种选路模式各会走哪条、为什么、代价。 */
export function Decisions({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  const route = test?.route ?? null
  const decisions = [route?.forward, route?.reverse].filter(
    (decision): decision is DecisionView => decision !== null && decision !== undefined,
  )
  return (
    <BoxCard data-testid="decisions">
      <BoxCardHeader title={d.decisionsTitle} tip={d.decisionsHint} />
      {route === null ? (
        <p className="px-5 py-4 text-sm text-muted-foreground">{d.decisionsNone}</p>
      ) : route.state !== 'done' ? (
        <div className="px-5 py-4">
          <ItemState state={route.state} problem={route.problem} nodes={nodes} />
        </div>
      ) : (
        <BoxCardBody className="pb-1">
          <Table aria-label={d.decisionsTitle}>
            <thead>
              <tr>
                <Th>{d.colMode}</Th>
                <Th>{d.colChoice}</Th>
                <Th>{d.colReason}</Th>
                <Th numeric>{d.colCost}</Th>
              </tr>
            </thead>
            <tbody>
              {decisions.map((decision) => (
                <DecisionRows key={decision.from} decision={decision} nodes={nodes} />
              ))}
            </tbody>
          </Table>
          {route.problem !== null ? (
            <p className="px-5 pb-3 text-[13px] text-muted-foreground">{problemText(route.problem, nodes)}</p>
          ) : null}
        </BoxCardBody>
      )}
    </BoxCard>
  )
}

/** 路由：两个方向现在的路（来源、什么时候换的、为什么），候选路径，两种模式各会怎么选。 */
export function RouteTab({
  path,
  test,
  nodes,
  name,
}: {
  path: PathView | undefined
  test: TestView | undefined
  nodes: Named
  name: (id: NodeId) => string
}) {
  return (
    <div className="space-y-5">
      {path === undefined ? (
        <Querying />
      ) : (
        <>
          <div className="grid grid-cols-1 items-start gap-5 @3xl:grid-cols-2">
            <LegCard leg={path.forward} name={name} testId="leg-forward" />
            <LegCard leg={path.reverse} name={name} testId="leg-reverse" />
          </div>
          <Candidates path={path} name={name} />
        </>
      )}
      <Decisions test={test} nodes={nodes} />
    </div>
  )
}
