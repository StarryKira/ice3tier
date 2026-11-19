import { type ReactNode } from 'react'

import {
  type DirectResult,
  type Node,
  type PathResult,
  type Route,
  type TestItemState,
  type TestProblem,
  type TestView,
} from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { StatusText, type Tone } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { t } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { msText, permilleText, problemText, rateText, routeText } from '@/lib/tests'
import { cn } from '@/lib/utils'

const s = t.tests

type Named = readonly Pick<Node, 'id' | 'name'>[]

function routeTone(route: Route | null): Tone {
  if (route?.via === 'direct') return 'ok'
  if (route?.via === 'relay') return 'relay'
  return 'idle'
}

/** 一项还没有结果、没有做、没做成时就是这一句（带原因）。 */
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
      <div aria-busy="true" aria-label={s.pending} className="space-y-2 py-1">
        <Skeleton className="h-3.5 w-24" />
        <Skeleton className="h-3.5 w-32" />
      </div>
    )
  }
  return (
    <p className="text-[13px] text-muted-foreground">
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
      {result.state !== 'done' ? (
        <div className="px-5 py-4">
          <ItemState state={result.state} problem={result.problem} nodes={nodes} />
        </div>
      ) : (
        <>
          <div className="px-5 py-4">
            <StatusText tone={result.ok === true ? 'ok' : 'danger'} className="text-[15px] font-medium">
              {result.ok === true ? s.directOk : s.directNo}
            </StatusText>
            {result.ok === true && result.kind !== null ? (
              <p className="mt-1 text-[13px] text-muted-foreground">
                {t.routes.directKind[result.kind]}
                {' · '}
                {result.already === true
                  ? s.directAlready
                  : result.elapsed_ms !== null
                    ? s.directIn(result.elapsed_ms)
                    : null}
              </p>
            ) : null}
            {result.ok !== true && result.problem !== null ? (
              <p className="mt-1 text-[13px] text-muted-foreground">{problemText(result.problem, nodes)}</p>
            ) : null}
          </div>
          {result.sides.length > 0 ? (
            <ul className="divide-y border-t">
              {result.sides.map((side) => (
                <li key={side.node} className="px-5 py-2.5">
                  <p className="text-[13px] font-medium">{nodeName(nodes, side.node)}</p>
                  <p className="text-xs text-muted-foreground">
                    {[
                      side.udp === true ? s.udpOk : side.udp === false ? s.udpBlocked : s.udpUnknown,
                      side.mapping_varies === true ? s.natVaries : null,
                      side.candidates !== null ? s.candidates(side.candidates) : null,
                    ]
                      .filter((part) => part !== null)
                      .join(' · ')}
                  </p>
                </li>
              ))}
            </ul>
          ) : null}
        </>
      )}
    </BoxCard>
  )
}

/** 测了的一条路，和它是不是直连那一条（路还不知道时靠这个起名字）。 */
interface Measured {
  result: PathResult
  direct: boolean
}

interface QualityRow {
  key: string
  label: ReactNode
  cell: (path: PathResult) => ReactNode
}

const dash = <span className="text-muted-foreground">—</span>

function labelWithTip(label: string, tip: string): ReactNode {
  return (
    <span className="inline-flex items-center gap-1">
      {label}
      <InfoTip>{tip}</InfoTip>
    </span>
  )
}

/** 表里的行：延迟、抖动、丢包、代价总有；各段、吞吐只在有一条路用得上时才有。 */
function qualityRows(paths: readonly PathResult[], nodes: Named): QualityRow[] {
  const rows: QualityRow[] = [
    {
      key: 'latency',
      label: labelWithTip(s.latency, s.latencyHint),
      cell: ({ latency }) =>
        latency === null ? (
          dash
        ) : (
          <>
            <span className="font-medium tabular">
              {s.latencyValue(msText(latency.min_us), msText(latency.avg_us), msText(latency.p95_us))}
            </span>
            <span className="block text-xs text-muted-foreground">{s.answered(latency.answered, latency.probes)}</span>
          </>
        ),
    },
    {
      key: 'jitter',
      label: s.jitter,
      cell: ({ latency }) => (latency === null ? dash : <span className="tabular">{msText(latency.jitter_us)}</span>),
    },
    {
      key: 'loss',
      label: s.loss,
      cell: ({ latency }) =>
        latency === null ? (
          dash
        ) : (
          <>
            <span className="tabular">{permilleText(latency.loss_permille)}</span>
            {latency.loss_up_permille !== null && latency.loss_down_permille !== null ? (
              <span className="ml-2 text-xs text-muted-foreground tabular">
                {s.lossSplit(permilleText(latency.loss_up_permille), permilleText(latency.loss_down_permille))}
              </span>
            ) : null}
          </>
        ),
    },
    {
      key: 'cost',
      label: labelWithTip(s.cost, s.costHint),
      cell: ({ cost_us }) => (cost_us === null ? dash : <span className="tabular">{msText(cost_us)}</span>),
    },
  ]
  if (paths.some((path) => path.hops.length > 1)) {
    rows.push({
      key: 'hops',
      label: s.hops,
      cell: ({ hops }) =>
        hops.length < 2 ? (
          dash
        ) : (
          <ul className="space-y-0.5 text-[13px]">
            {hops.map((hop) => (
              <li key={`${hop.from}-${hop.to}`} className="tabular">
                {nodeName(nodes, hop.from)} → {nodeName(nodes, hop.to)}：{msText(hop.rtt_us)}
                <span className="ml-1.5 text-xs text-muted-foreground">{s.hopSource[hop.source]}</span>
              </li>
            ))}
          </ul>
        ),
    })
  }
  if (paths.some((path) => path.throughput !== null)) {
    rows.push({
      key: 'throughput',
      label: labelWithTip(s.throughputTitle, s.throughputCaveat),
      cell: ({ throughput }) => {
        if (throughput === null) return dash
        if (throughput.state !== 'done') {
          return (
            <span className="text-[13px] text-muted-foreground">
              {throughput.state === 'pending' ? s.pending : s.skipped}
              {throughput.problem !== null ? `：${problemText(throughput.problem, nodes)}` : null}
            </span>
          )
        }
        const ms = Math.max(throughput.up?.ms ?? 0, throughput.down?.ms ?? 0)
        return (
          <>
            <span className="block font-medium tabular">
              {s.up} {rateText(throughput.up?.bits_per_second)}
            </span>
            <span className="block font-medium tabular">
              {s.down} {rateText(throughput.down?.bits_per_second)}
            </span>
            <span className="block text-xs text-muted-foreground">
              {s.throughputNote((throughput.congestion ?? '').toUpperCase(), ms)}
            </span>
          </>
        )
      },
    })
  }
  return rows
}

/**
 * 各条路并排比：一列一条路（直连在前，经中继的在后），行是延迟、抖动、丢包、代价……
 * 还没测完、没测的那一列只有一句（占满整列）。
 */
function QualityCard({ paths, nodes }: { paths: readonly Measured[]; nodes: Named }) {
  const rows = qualityRows(
    paths.map((path) => path.result),
    nodes,
  )
  return (
    <BoxCard>
      <BoxCardHeader title={s.qualityTitle} />
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-sm" data-testid="test-quality">
          <thead>
            <tr className="border-b">
              <td className="w-32" />
              {paths.map(({ result: path, direct }, column) => (
                <th
                  key={column}
                  scope="col"
                  className="min-w-44 px-5 py-3 text-left align-top font-normal"
                  data-testid={`test-path-${column}`}
                >
                  <span className="inline-flex flex-wrap items-center gap-x-2">
                    <StatusText
                      tone={direct ? 'ok' : routeTone(path.route)}
                      className="text-[13px] font-semibold text-foreground"
                    >
                      {direct || path.route === null
                        ? direct
                          ? s.qualityDirect
                          : s.qualityRelay
                        : routeText(path.route, nodes)}
                    </StatusText>
                    {path.relay_leg === 'tcp' ? (
                      <span className="text-xs text-warn">{t.routes.legTransport.tcp}</span>
                    ) : null}
                  </span>
                  {/* 为什么测这一条：直连不用说；经中继的写哪种选路模式会走它。 */}
                  {path.picked_by.some((why) => why !== 'direct') ? (
                    <span className="mt-1.5 flex flex-wrap gap-1">
                      {path.picked_by
                        .filter((why) => why !== 'direct')
                        .map((why) => (
                          <Badge key={why} tone="outline">
                            {s.pickedBy[why]}
                          </Badge>
                        ))}
                    </span>
                  ) : null}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y">
            {rows.map((row, index) => (
              <tr key={row.key}>
                <th
                  scope="row"
                  className="px-5 py-2.5 text-left align-top text-[13px] font-normal whitespace-nowrap text-muted-foreground"
                >
                  {row.label}
                </th>
                {paths.map(({ result: path }, column) =>
                  path.state === 'done' ? (
                    <td key={column} className="px-5 py-2.5 align-top" data-testid={`test-path-${column}-${row.key}`}>
                      {row.cell(path)}
                    </td>
                  ) : index === 0 ? (
                    <td
                      key={column}
                      rowSpan={rows.length}
                      className="px-5 py-2.5 align-top"
                      data-testid={`test-path-${column}-state`}
                    >
                      <ItemState state={path.state} problem={path.problem} nodes={nodes} />
                    </td>
                  ) : null,
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </BoxCard>
  )
}

/**
 * 一个测试的结果：左边直连能不能通，右边各条路的延迟、丢包（要了就有吞吐）并排比。
 * 进行中的项先占着位置；整个测试做不了时只有原因。
 */
export function TestResult({ test, nodes }: { test: TestView; nodes: Named }) {
  const all: Named = [...nodes, test.a, test.b]
  const paths: Measured[] = [
    ...(test.direct_quality === null ? [] : [{ result: test.direct_quality, direct: true }]),
    ...test.relay_quality.map((result) => ({ result, direct: false })),
  ]
  const both = test.direct !== null && paths.length > 0
  return (
    <div className="space-y-4">
      {test.error !== null ? <Notice tone="danger">{problemText(test.error, all)}</Notice> : null}
      {test.notes.map((note, index) => (
        // 同一个原因可能出现两次（两端各一次），用下标做键。
        <Notice key={index} tone="warn">
          {problemText(note, all)}
        </Notice>
      ))}
      {test.direct !== null || paths.length > 0 ? (
        <div className={cn('grid grid-cols-1 items-start gap-5', both && '@4xl:grid-cols-[17rem_minmax(0,1fr)]')}>
          {test.direct !== null ? <DirectCard result={test.direct} nodes={all} /> : null}
          {paths.length > 0 ? <QualityCard paths={paths} nodes={all} /> : null}
        </div>
      ) : null}
    </div>
  )
}
