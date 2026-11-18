import { type ReactNode } from 'react'

import {
  type Choice,
  type DecisionView,
  type DirectResult,
  type Node,
  type PathResult,
  type RouteResult,
  type TestItemState,
  type TestProblem,
  type TestView,
} from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { InfoRow } from '@/components/page'
import { Skeleton } from '@/components/ui/skeleton'
import { StatusText } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { BoxCard, BoxCardBody, BoxCardHeader, BoxCardList } from '@/components/ui/card'
import { t } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { msText, permilleText, problemText, rateText, routeText } from '@/lib/tests'

const s = t.tests

type Named = readonly Pick<Node, 'id' | 'name'>[]

/** 一项还没有结果、没有做、没做成时，卡片里就是这一句（带原因）。 */
function ItemState({
  state,
  problem,
  nodes,
}: {
  state: TestItemState
  problem: TestProblem | null
  nodes: Named
}): ReactNode {
  if (state === 'pending') {
    return (
      <div aria-busy="true" aria-label={s.pending} className="space-y-2 py-4">
        <Skeleton className="h-3.5 w-2/5" />
        <Skeleton className="h-3.5 w-3/5" />
      </div>
    )
  }
  return (
    <p className="py-4 text-sm text-muted-foreground">
      {state === 'skipped' ? s.skipped : s.failed}
      {problem !== null ? `：${problemText(problem, nodes)}` : null}
    </p>
  )
}

/** 直连：能不能、怎么通的、用了多久；不能时最能说明问题的原因。下面是两端各自看到的。 */
function DirectCard({ result, nodes }: { result: DirectResult; nodes: Named }) {
  return (
    <BoxCard data-testid="test-direct">
      <BoxCardHeader title={s.directTitle} />
      <BoxCardBody>
        {result.state !== 'done' ? (
          <ItemState state={result.state} problem={result.problem} nodes={nodes} />
        ) : (
          <>
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-4">
              <StatusText tone={result.ok === true ? 'ok' : 'danger'} className="text-[15px]">
                {result.ok === true ? s.directOk : s.directNo}
              </StatusText>
              {result.ok === true && result.kind !== null ? (
                <span className="text-sm text-muted-foreground">
                  {t.routes.directKind[result.kind]}
                  {' · '}
                  {result.already === true
                    ? s.directAlready
                    : result.elapsed_ms !== null
                      ? s.directIn(result.elapsed_ms)
                      : null}
                </span>
              ) : null}
              {result.ok !== true && result.problem !== null ? (
                <span className="text-sm text-muted-foreground">{problemText(result.problem, nodes)}</span>
              ) : null}
            </div>
            <BoxCardList className="border-t">
              {result.sides.map((side) => (
                <InfoRow key={side.node} label={nodeName(nodes, side.node)}>
                  <span className="text-[13px]">
                    {[
                      side.udp === true ? s.udpOk : side.udp === false ? s.udpBlocked : s.udpUnknown,
                      side.mapping_varies === true ? s.natVaries : null,
                      side.candidates !== null ? s.candidates(side.candidates) : null,
                    ]
                      .filter((part) => part !== null)
                      .join(' · ')}
                  </span>
                </InfoRow>
              ))}
            </BoxCardList>
          </>
        )}
      </BoxCardBody>
    </BoxCard>
  )
}

/** 一条路径的质量：哪条路、延迟（最小 / 平均 / P95）、抖动、丢包（分去回）、代价、各段，要了吞吐时上下行。 */
function PathCard({ title, result, nodes }: { title: string; result: PathResult; nodes: Named }) {
  const latency = result.latency
  const throughput = result.throughput
  return (
    <BoxCard data-testid="test-path">
      <BoxCardHeader
        title={title}
        action={
          result.picked_by.length > 0 ? (
            <span className="flex gap-1">
              {result.picked_by.map((why) => (
                <Badge key={why} tone="outline">
                  {s.pickedBy[why]}
                </Badge>
              ))}
            </span>
          ) : undefined
        }
      />
      <BoxCardBody>
        {result.state !== 'done' ? (
          <ItemState state={result.state} problem={result.problem} nodes={nodes} />
        ) : (
          <BoxCardList>
            <InfoRow label={s.path}>
              <span className="inline-flex items-center gap-2">
                {routeText(result.route, nodes)}
                {result.relay_leg === 'tcp' ? (
                  <span className="text-[13px] text-warn">{t.routes.legTransport.tcp}</span>
                ) : null}
              </span>
            </InfoRow>
            {latency !== null ? (
              <>
                <InfoRow
                  label={
                    <span className="inline-flex items-center gap-1">
                      {s.latency}
                      <InfoTip>{s.latencyHint}</InfoTip>
                    </span>
                  }
                >
                  <span className="font-medium tabular">
                    {s.latencyValue(msText(latency.min_us), msText(latency.avg_us), msText(latency.p95_us))}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {s.answered(latency.answered, latency.probes)}
                  </span>
                </InfoRow>
                <InfoRow label={s.jitter}>
                  <span className="tabular">{msText(latency.jitter_us)}</span>
                </InfoRow>
                <InfoRow label={s.loss}>
                  <span className="tabular">{permilleText(latency.loss_permille)}</span>
                  {latency.loss_up_permille !== null && latency.loss_down_permille !== null ? (
                    <span className="ml-2 text-[13px] text-muted-foreground tabular">
                      {s.lossSplit(permilleText(latency.loss_up_permille), permilleText(latency.loss_down_permille))}
                    </span>
                  ) : null}
                </InfoRow>
              </>
            ) : null}
            {result.cost_us !== null ? (
              <InfoRow
                label={
                  <span className="inline-flex items-center gap-1">
                    {s.cost}
                    <InfoTip>{s.costHint}</InfoTip>
                  </span>
                }
              >
                <span className="tabular">{msText(result.cost_us)}</span>
              </InfoRow>
            ) : null}
            {result.hops.length > 1 ? (
              <InfoRow label={s.hops}>
                <ul className="space-y-0.5 text-[13px]">
                  {result.hops.map((hop) => (
                    <li key={`${hop.from}-${hop.to}`} className="tabular">
                      {nodeName(nodes, hop.from)} → {nodeName(nodes, hop.to)}：{msText(hop.rtt_us)}
                      <span className="ml-1.5 text-xs text-muted-foreground">{s.hopSource[hop.source]}</span>
                    </li>
                  ))}
                </ul>
              </InfoRow>
            ) : null}
            {throughput !== null ? (
              <InfoRow
                label={
                  <span className="inline-flex items-center gap-1">
                    {s.throughputTitle}
                    <InfoTip>{s.throughputCaveat}</InfoTip>
                  </span>
                }
              >
                {throughput.state !== 'done' ? (
                  <span className="text-[13px] text-muted-foreground">
                    {throughput.state === 'pending' ? s.pending : s.skipped}
                    {throughput.problem !== null ? `：${problemText(throughput.problem, nodes)}` : null}
                  </span>
                ) : (
                  <>
                    <span className="font-medium tabular">
                      {s.up} {rateText(throughput.up?.bits_per_second)} · {s.down}{' '}
                      {rateText(throughput.down?.bits_per_second)}
                    </span>
                    <span className="block text-xs text-muted-foreground">
                      {s.throughputNote(
                        (throughput.congestion ?? '').toUpperCase(),
                        String(Math.round(Math.max(throughput.up?.ms ?? 0, throughput.down?.ms ?? 0) / 100) / 10),
                      )}
                    </span>
                  </>
                )}
              </InfoRow>
            ) : null}
          </BoxCardList>
        )}
      </BoxCardBody>
    </BoxCard>
  )
}

/** 一种模式的选择：哪条路、为什么、代价；留在现在的路上时写那条更便宜的差多少。 */
function ChoiceRow({ label, choice, inUse, nodes }: { label: string; choice: Choice; inUse: boolean; nodes: Named }) {
  const reason = Object.hasOwn(s.reasons, choice.reason)
    ? s.reasons[choice.reason as keyof typeof s.reasons]
    : choice.reason
  const cheaper = choice.cheaper ?? null
  return (
    <InfoRow
      label={
        <span className="inline-flex items-center gap-1.5">
          {label}
          {inUse ? <Badge tone="ok">{s.inUse}</Badge> : null}
        </span>
      }
    >
      <span>{routeText(choice.route, nodes)}</span>
      <span className="ml-2 text-[13px] text-muted-foreground">
        {reason}
        {choice.cost_us !== null ? ` · ${msText(choice.cost_us)}` : null}
      </span>
      {cheaper !== null && choice.cost_us !== null ? (
        <span className="block text-xs text-muted-foreground">
          {s.withinMargin(routeText(cheaper.route, nodes), msText(choice.cost_us - cheaper.cost_us))}
        </span>
      ) : null}
    </InfoRow>
  )
}

function Decision({ view, nodes }: { view: DecisionView; nodes: Named }) {
  return (
    <div>
      <p className="pt-3 pb-1 text-[13px] font-medium">
        {s.routeDirection(nodeName(nodes, view.from), nodeName(nodes, view.to))}
        <span className="ml-2 font-normal text-muted-foreground">
          {s.routeNow}：{routeText(view.current, nodes)}
        </span>
      </p>
      <BoxCardList>
        <ChoiceRow
          label={s.modeDefault}
          choice={view.direct_first}
          inUse={view.mode === 'direct_first'}
          nodes={nodes}
        />
        <ChoiceRow
          label={s.modeLowestCost}
          choice={view.lowest_cost}
          inUse={view.mode === 'lowest_cost'}
          nodes={nodes}
        />
      </BoxCardList>
    </div>
  )
}

/** 选路：两个方向各自算的两种模式会选哪条、为什么。 */
function RouteCard({ result, nodes }: { result: RouteResult; nodes: Named }) {
  return (
    <BoxCard data-testid="test-route">
      <BoxCardHeader title={s.routeTitle} />
      <BoxCardBody className="pb-1">
        {result.state !== 'done' ? (
          <ItemState state={result.state} problem={result.problem} nodes={nodes} />
        ) : (
          <>
            {result.forward !== null ? <Decision view={result.forward} nodes={nodes} /> : null}
            {result.reverse !== null ? <Decision view={result.reverse} nodes={nodes} /> : null}
          </>
        )}
      </BoxCardBody>
    </BoxCard>
  )
}

/** 一个测试的结果：要了哪几项就有哪几张卡片，进行中的项先占着位置。整个测试做不了时只有原因。 */
export function TestResult({ test, nodes }: { test: TestView; nodes: Named }) {
  const all: Named = [...nodes, test.a, test.b]
  return (
    <div className="space-y-4">
      {test.error !== null ? <Notice tone="danger">{problemText(test.error, all)}</Notice> : null}
      {test.notes.map((note, index) => (
        // 同一个原因可能出现两次（两端各一次），用下标做键。
        <Notice key={index} tone="warn">
          {problemText(note, all)}
        </Notice>
      ))}
      <div className="grid grid-cols-1 items-start gap-5 @4xl:grid-cols-2">
        {test.direct !== null ? <DirectCard result={test.direct} nodes={all} /> : null}
        {test.route !== null ? <RouteCard result={test.route} nodes={all} /> : null}
        {test.direct_quality !== null ? (
          <PathCard title={s.qualityDirect} result={test.direct_quality} nodes={all} />
        ) : null}
        {test.relay_quality.map((result, index) => (
          <PathCard
            // 路径还不知道时没有可以做键的东西。
            key={index}
            title={s.qualityRelay}
            result={result}
            nodes={all}
          />
        ))}
      </div>
    </div>
  )
}
