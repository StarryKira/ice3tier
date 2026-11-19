import { type ReactNode } from 'react'

import { type Node, type PathResult, type Route, type TestItemState, type TestProblem } from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { StatusText, type Tone } from '@/components/status'
import { Skeleton } from '@/components/ui/skeleton'
import { t } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { msText, permilleText, problemText } from '@/lib/tests'
import { type Measured, routeName } from '@/topology/diagnosis'

const s = t.tests

export type Named = readonly Pick<Node, 'id' | 'name'>[]

function routeTone(route: Route | null): Tone {
  if (route?.via === 'direct') return 'ok'
  if (route?.via === 'relay') return 'relay'
  return 'idle'
}

/** 一项还没有结果、没有做、没做成时就是这一句（带原因）。 */
export function ItemState({
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
  return rows
}

/**
 * 各条路并排比：一列一条路（直连在前，经中继的在后），行是延迟、抖动、丢包、代价……
 * 还没测完、没测的那一列只有一句（占满整列）。
 */
export function QualityCard({ paths, nodes }: { paths: readonly Measured[]; nodes: Named }) {
  const rows = qualityRows(
    paths.map((path) => path.result),
    nodes,
  )
  return (
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
                      : routeName(path.route, nodes)}
                  </StatusText>
                  {path.relay_leg === 'tcp' ? (
                    <span className="text-xs text-warn">{t.routes.legTransport.tcp}</span>
                  ) : null}
                </span>
                {/* 为什么测这一条：直连不用说；经中继的写哪种选路模式会走它。 */}
                {path.picked_by.some((why) => why !== 'direct') ? (
                  <span className="mt-0.5 block text-xs text-muted-foreground">
                    {path.picked_by
                      .filter((why) => why !== 'direct')
                      .map((why) => s.pickedBy[why])
                      .join(t.common.listSeparator)}
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
  )
}
