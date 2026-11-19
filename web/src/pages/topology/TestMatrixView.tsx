import { useQuery } from '@tanstack/react-query'
import { Grid3x3, Play, Square } from 'lucide-react'
import { type ReactNode } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type MatrixCell, type Network, type Node, type NodeId } from '@/api/types'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { RelativeTime, StatusText } from '@/components/status'
import { Toolbar } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { msText, problemText } from '@/lib/tests'
import { cn } from '@/lib/utils'

const s = t.tests

/** 一对节点的那一格（矩阵只存 `a` 小于 `b` 的那一半）。 */
const cellKey = (x: NodeId, y: NodeId) => (x < y ? `${x}-${y}` : `${y}-${x}`)

/** 一格里写什么：延迟测在哪条路上（P2P 绿、中继紫）和平均延迟；没测的是一道横线，悬停看原因。 */
function CellContent({ cell }: { cell: MatrixCell }): ReactNode {
  if (cell.state === 'pending') return <span className="text-faint">·</span>
  if (cell.state === 'running') return <span className="animate-pulse text-relay">…</span>
  if (cell.state !== 'done' || cell.route === null || cell.route.via === 'none') {
    return <span className="text-muted-foreground">—</span>
  }
  const direct = cell.route.via === 'direct'
  return (
    <span className="inline-flex flex-col items-center leading-4">
      <span className={cn('text-[11px] font-medium', direct ? 'text-ok' : 'text-relay')}>
        {direct ? s.matrixCellP2P : s.matrixCellRelay}
      </span>
      <span className="text-xs tabular">{msText(cell.latency?.avg_us)}</span>
    </span>
  )
}

/** 一格的悬停说明：直连怎么样、为什么没测。 */
function cellTip(cell: MatrixCell, nodes: readonly { id: NodeId; name: string }[]): string | null {
  const parts: string[] = []
  if (cell.direct_ok === true) {
    parts.push(cell.direct_kind === null ? s.directOk : `${s.directOk} · ${t.routes.directKind[cell.direct_kind]}`)
  } else if (cell.direct_ok === false) parts.push(s.directNo)
  if (cell.problem !== null) parts.push(problemText(cell.problem, nodes))
  return parts.length === 0 ? null : parts.join('；')
}

interface TestMatrixViewProps {
  network: Network
  nodes: readonly Node[]
  /** 点开一格：在路径诊断里看这一对的完整结果。 */
  onOpen: (test: number, pair: { a: NodeId; b: NodeId }) => void
}

/**
 * 拓扑的"全网测试"：对每一对节点测直连和延迟（不测吞吐、不算选路），controller 分批跑。结果是一张节点 × 节点的表，
 * 两半对称；点开一格去路径诊断看那一对的完整结果。矩阵只在 controller 的内存里，它重启之后这里是空的。
 */
export function TestMatrixView({ network, nodes, onOpen }: TestMatrixViewProps) {
  const run = useQuery(q.testMatrix(network.id))
  const start = useAction(network.id, {
    mutationFn: () => api.startTestMatrix(network.id),
    success: s.matrixStarted,
  })
  const stop = useAction(network.id, {
    mutationFn: () => api.cancelTestMatrix(network.id),
    success: s.matrixCancelled,
  })

  const data = run.data ?? null
  const running = data?.state === 'running'
  const cells = new Map((data?.cells ?? []).map((cell) => [cellKey(cell.a, cell.b), cell]))
  const named = [...nodes, ...(data?.nodes ?? [])]

  return (
    <>
      <Toolbar className="gap-3">
        {running ? (
          <Button onClick={() => stop.mutate()} pending={stop.isPending}>
            <Square aria-hidden />
            {s.matrixCancel}
          </Button>
        ) : (
          <Button variant="primary" onClick={() => start.mutate()} pending={start.isPending}>
            <Play aria-hidden />
            {s.matrixStart}
          </Button>
        )}
        {data !== null ? (
          <span className="flex items-center gap-3 text-[13px] text-muted-foreground" data-testid="matrix-progress">
            <StatusText tone={running ? 'relay' : data.state === 'done' ? 'ok' : 'idle'}>
              {s.matrixState[data.state]}
            </StatusText>
            <span className="tabular">{s.matrixProgress(data.finished, data.total)}</span>
            <RelativeTime at={data.started_at} />
          </span>
        ) : null}
        <span className="ml-auto text-[13px] text-muted-foreground">{s.matrixHint}</span>
      </Toolbar>

      {run.isError ? (
        <ErrorState title={t.topology.viewTests} error={run.error} onRetry={() => void run.refetch()} />
      ) : run.isPending ? (
        <SkeletonBlock lines={4} />
      ) : data === null ? (
        <EmptyState icon={Grid3x3} title={s.matrixEmpty} />
      ) : (
        <>
          <div className="overflow-x-auto rounded-xl border">
            <table className="w-full border-collapse text-sm" data-testid="test-matrix">
              <thead>
                <tr>
                  <th className="sticky left-0 bg-card" aria-hidden />
                  {data.nodes.map((node) => (
                    <th
                      key={node.id}
                      scope="col"
                      className="border-b px-2 py-2.5 text-xs font-medium whitespace-nowrap text-muted-foreground"
                    >
                      {node.name}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {data.nodes.map((row) => (
                  <tr key={row.id} className="border-b last:border-b-0">
                    <th
                      scope="row"
                      className="sticky left-0 border-r bg-card px-3 py-2 text-left text-[13px] font-medium whitespace-nowrap"
                    >
                      {row.name}
                    </th>
                    {data.nodes.map((column) => {
                      if (row.id === column.id) {
                        return <td key={column.id} className="bg-muted/40" aria-hidden />
                      }
                      const cell = cells.get(cellKey(row.id, column.id))
                      if (cell === undefined) return <td key={column.id} />
                      const tip = cellTip(cell, named)
                      const content = <CellContent cell={cell} />
                      const test = cell.test
                      const body =
                        test !== null && cell.state !== 'pending' ? (
                          <button
                            type="button"
                            onClick={() => onOpen(test, { a: cell.a, b: cell.b })}
                            aria-label={s.matrixCellOpen(row.name, column.name)}
                            className="flex h-11 w-full items-center justify-center rounded-md hover:bg-muted"
                          >
                            {content}
                          </button>
                        ) : (
                          <span className="flex h-11 items-center justify-center">{content}</span>
                        )
                      return (
                        <td
                          key={column.id}
                          className="min-w-20 px-1 py-1 text-center"
                          data-testid={`matrix-cell-${row.id}-${column.id}`}
                        >
                          {tip === null ? body : <Tooltip content={tip}>{body}</Tooltip>}
                        </td>
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="mt-3 text-[13px] text-muted-foreground">{s.matrixLegend}</p>
        </>
      )}
    </>
  )
}
