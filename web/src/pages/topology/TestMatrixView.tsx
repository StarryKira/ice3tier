import { useQuery } from '@tanstack/react-query'
import { Grid3x3, Play, Square } from 'lucide-react'
import { type ReactNode } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type MatrixCell, type Network, type Node, type NodeId } from '@/api/types'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { OnlineDot, RelativeTime, StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { msText, problemText } from '@/lib/tests'
import { cn } from '@/lib/utils'
import { gridWidth } from '@/topology/grid'

const s = t.tests

/** 一对节点的那一格（矩阵只存 `a` 小于 `b` 的那一半）。 */
const cellKey = (x: NodeId, y: NodeId) => (x < y ? `${x}-${y}` : `${y}-${x}`)

/** 格子的底色和路由矩阵一套：直连淡绿，经中继淡紫；没测成的灰。 */
function cellTone(cell: MatrixCell): string {
  if (cell.state === 'pending') return 'bg-muted/40 text-faint'
  if (cell.state === 'running') return 'bg-relay-soft/60 text-relay motion-safe:animate-pulse'
  if (cell.state !== 'done' || cell.route === null || cell.route.via === 'none') return 'bg-muted text-muted-foreground'
  return cell.route.via === 'direct' ? 'bg-ok-soft' : 'bg-relay-soft'
}

/** 一格里写什么：延迟测在哪条路上（P2P 绿、中继紫）和平均延迟；没测的是一道横线，悬停看原因。 */
function CellContent({ cell }: { cell: MatrixCell }): ReactNode {
  if (cell.state === 'pending') return <span>·</span>
  if (cell.state === 'running') return <span>…</span>
  if (cell.state !== 'done' || cell.route === null || cell.route.via === 'none') return <span>—</span>
  const direct = cell.route.via === 'direct'
  return (
    <>
      <span className={cn('text-xs font-semibold', direct ? 'text-ok' : 'text-relay')}>
        {direct ? s.matrixCellP2P : s.matrixCellRelay}
      </span>
      <span className="tabular">{msText(cell.latency?.avg_us)}</span>
    </>
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
 * 两半对称，和路由矩阵一样铺满；点开一格去路径诊断看那一对的完整结果。跑着的时候顶上一条进度。
 * 还没跑过时（矩阵只在 controller 的内存里，它重启之后也是这样）是一块铺满的虚线框，和代理、端口转发的空状态一样内容靠上：
 * 说明、要测几对、开始的按钮。
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
  const online = new Map(nodes.map((node) => [node.id, node.online]))
  const startButton = (
    <Button variant="primary" onClick={() => start.mutate()} pending={start.isPending}>
      <Play aria-hidden />
      {s.matrixStart}
    </Button>
  )

  if (run.isError) {
    return <ErrorState title={t.topology.viewTests} error={run.error} onRetry={() => void run.refetch()} />
  }
  if (run.isPending) return <SkeletonBlock lines={4} />
  if (data === null) {
    const pairs = (nodes.length * (nodes.length - 1)) / 2
    return (
      <div className="flex flex-1 flex-col items-center rounded-xl border border-dashed px-6 pt-16 pb-12 text-center">
        <span className="mb-4 grid size-11 place-items-center rounded-xl bg-muted text-muted-foreground">
          <Grid3x3 className="size-5" aria-hidden />
        </span>
        <h2 className="text-base font-semibold">{s.matrixEmpty}</h2>
        <p className="mt-1.5 max-w-md text-[13px] leading-relaxed text-muted-foreground">{s.matrixHint}</p>
        {nodes.length > 1 ? (
          <p className="mt-3 text-xs text-muted-foreground tabular">{s.matrixScope(nodes.length, pairs)}</p>
        ) : null}
        <div className="mt-6">{startButton}</div>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        {running ? (
          <Button onClick={() => stop.mutate()} pending={stop.isPending}>
            <Square aria-hidden />
            {s.matrixCancel}
          </Button>
        ) : (
          startButton
        )}
        <span className="flex items-center gap-3 text-[13px] text-muted-foreground" data-testid="matrix-progress">
          <StatusText tone={running ? 'relay' : data.state === 'done' ? 'ok' : 'idle'}>
            {s.matrixState[data.state]}
          </StatusText>
          <span className="tabular">{s.matrixProgress(data.finished, data.total)}</span>
          <RelativeTime at={data.started_at} />
        </span>
        <span className="ml-auto text-[13px] text-muted-foreground">{s.matrixHint}</span>
      </div>
      {running ? (
        <div className="h-1 overflow-hidden rounded-full bg-muted" aria-hidden>
          <div
            className="h-full rounded-full bg-relay transition-[width] duration-500"
            style={{ width: `${data.total === 0 ? 0 : (100 * data.finished) / data.total}%` }}
          />
        </div>
      ) : null}

      <div className="overflow-x-auto">
        <table
          className="table-fixed border-separate border-spacing-1 text-sm"
          style={gridWidth(data.nodes.length, '9rem')}
          data-testid="test-matrix"
        >
          <thead>
            <tr>
              <th className="w-36" aria-hidden />
              {data.nodes.map((node) => (
                <th key={node.id} scope="col" className="px-1.5 py-2 text-xs font-medium text-muted-foreground">
                  <span className="block truncate" title={node.name}>
                    {node.name}
                  </span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {data.nodes.map((row) => (
              <tr key={row.id}>
                <th scope="row" className="pr-4 text-left text-[13px] font-medium whitespace-nowrap">
                  <span className="flex min-w-0 items-center gap-1.5">
                    <OnlineDot online={online.get(row.id) ?? false} />
                    <span className="truncate">{row.name}</span>
                  </span>
                </th>
                {data.nodes.map((column) => {
                  if (row.id === column.id) {
                    return (
                      <td key={column.id} className="p-0" aria-hidden>
                        <span className="block h-11 rounded-md bg-muted/40" />
                      </td>
                    )
                  }
                  const cell = cells.get(cellKey(row.id, column.id))
                  if (cell === undefined) return <td key={column.id} />
                  const tip = cellTip(cell, named)
                  const content = <CellContent cell={cell} />
                  const test = cell.test
                  const tile = cn(
                    'flex h-11 w-full items-center justify-center gap-1.5 rounded-md px-1.5 text-[13px]',
                    cellTone(cell),
                  )
                  const body =
                    test !== null && cell.state !== 'pending' ? (
                      <button
                        type="button"
                        onClick={() => onOpen(test, { a: cell.a, b: cell.b })}
                        aria-label={s.matrixCellOpen(row.name, column.name)}
                        className={cn(
                          tile,
                          'transition-[box-shadow,transform] hover:-translate-y-px hover:shadow-card hover:ring-1 hover:ring-foreground/20',
                        )}
                      >
                        {content}
                      </button>
                    ) : (
                      <span className={tile}>{content}</span>
                    )
                  return (
                    <td key={column.id} className="p-0 text-center" data-testid={`matrix-cell-${row.id}-${column.id}`}>
                      {tip === null ? body : <Tooltip content={tip}>{body}</Tooltip>}
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="text-xs text-muted-foreground">{s.matrixLegend}</p>
    </div>
  )
}
