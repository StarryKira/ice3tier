import { useQuery } from '@tanstack/react-query'
import { Plus, Waypoints, X } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useSearchParams } from 'react-router'

import { q } from '@/api/queries'
import { type NodeId } from '@/api/types'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { PairPicker } from '@/components/pair-picker'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Segmented } from '@/components/ui/segmented'
import { t } from '@/i18n'
import { REPORT_INTERVAL_SECS } from '@/lib/product'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'
import { AddNodeDialog } from '@/pages/nodes/AddNodeDialog'
import { LINE_COLORS } from '@/topology/colors'
import { buildTopology, type Selection } from '@/topology/graph'
import { nextPick, type Pair, readPair, writePair } from '@/topology/pair'

import { BackboneView } from './BackboneView'
import { MatrixLegend, MatrixView } from './MatrixView'
import { PairPanel } from './PairPanel'
import { TopologyGraph } from './TopologyGraph'

const g = t.topology

type View = 'graph' | 'matrix' | 'backbone'

function GraphLegend() {
  const line = (color: string, dash?: string, width = 1.6) => (
    <svg width="28" height="8" aria-hidden className="shrink-0">
      <line x1="1" y1="4" x2="27" y2="4" stroke={color} strokeWidth={width} strokeDasharray={dash} />
    </svg>
  )
  return (
    <ul
      className="flex flex-wrap items-center gap-x-5 gap-y-1.5 text-xs text-muted-foreground"
      aria-label={t.routes.legendLabel}
    >
      <li className="flex items-center gap-1.5">
        {line(LINE_COLORS.direct)}
        {g.legendDirect}
      </li>
      <li className="flex items-center gap-1.5">
        {line(LINE_COLORS.relay)}
        {g.legendRelay}
      </li>
      <li className="flex items-center gap-1.5">
        {line(LINE_COLORS.stale, '5 4')}
        {g.legendStale}
      </li>
      <li className="flex items-center gap-1.5">
        {line(LINE_COLORS.none, '2 4')}
        {g.legendNone}
      </li>
      <li className="flex items-center gap-1.5">
        {line(LINE_COLORS.neutral, undefined, 2.6)}
        {g.legendForward}
      </li>
      <li className="flex items-center gap-1.5">
        {line(LINE_COLORS.neutral, '7 5', 2.6)}
        {g.legendReverse}
      </li>
      <li className="flex items-center gap-1.5">
        <span className="h-3 w-5 rounded-sm border border-dashed border-border-strong bg-card" aria-hidden />
        {g.legendOffline}
      </li>
    </ul>
  )
}

/**
 * 拓扑：节点、relay 和它们之间现在走的路（白皮书 §7.7）。两种看法共用同一份路由矩阵：
 * 实时图（看得快）和路由矩阵（精确）。选中的一对节点写在地址里，两种看法之间、和路径诊断之间来回切换时跟着走。
 * 第三个标签页"骨干"是另一份数据：relay 之间测的延迟，每次现问节点。
 */
export function TopologyPage() {
  useDocumentTitle(g.title)
  const network = useNetwork()
  const [params, setParams] = useSearchParams()
  const [adding, setAdding] = useState(false)
  const requested = params.get('view')
  const view: View = requested === 'matrix' || requested === 'backbone' ? requested : 'graph'
  const pair = readPair(params)

  const matrix = useQuery(q.matrix(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const relays = useMemo(
    () => new Set((nodes.data ?? []).filter((node) => node.relay !== null).map((node) => node.id)),
    [nodes.data],
  )
  const known = (id: NodeId | null) => id !== null && matrix.data?.nodes.some((node) => node.id === id) === true
  const selection: Selection | null =
    pair.a !== null && pair.b !== null && pair.a !== pair.b && known(pair.a) && known(pair.b)
      ? { a: pair.a, b: pair.b }
      : null
  const shape = useMemo(
    () => (matrix.data === undefined ? null : buildTopology(matrix.data, relays, selection)),
    // selection 由 pair 算出，按两个编号比较就够了。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [matrix.data, relays, selection?.a, selection?.b],
  )

  const setPair = (next: Pair) => setParams(writePair(params, next), { replace: true })
  const setView = (next: View) => {
    const updated = new URLSearchParams(params)
    if (next === 'graph') updated.delete('view')
    else updated.set('view', next)
    setParams(updated, { replace: true })
  }

  const names = new Map((matrix.data?.nodes ?? []).map((node) => [node.id, node.name]))
  const name = (id: NodeId) => names.get(id) ?? t.common.nodeMissing(id)
  const nodeA = matrix.data?.nodes.find((node) => node.id === selection?.a)
  const nodeB = matrix.data?.nodes.find((node) => node.id === selection?.b)

  const tooFew = matrix.data !== undefined && matrix.data.nodes.length < 2
  const noRoutes = matrix.data?.entries.length === 0

  const panel =
    shape !== null && nodeA !== undefined && nodeB !== undefined ? (
      <PairPanel network={network.id} a={nodeA} b={nodeB} forward={shape.forward} reverse={shape.reverse} name={name} />
    ) : null

  return (
    <Page wide className="@container">
      <PageHeader
        title={g.title}
        actions={
          <Segmented<View>
            aria-label={g.viewLabel}
            value={view}
            onChange={setView}
            options={[
              { value: 'graph', label: g.viewGraph },
              { value: 'matrix', label: g.viewMatrix },
              { value: 'backbone', label: g.viewBackbone },
            ]}
          />
        }
      />

      {view === 'backbone' ? (
        // 骨干是另一份数据（relay 之间测的），和选中的那一对节点无关，也不看路由矩阵。
        <BackboneView network={network} name={name} />
      ) : (
        <>
          <div className="mb-6 space-y-3">
            <PairPicker
              nodes={nodes.data ?? []}
              pair={pair}
              onChange={setPair}
              labels={{ a: g.pickA, b: g.pickB, swap: g.swap }}
            />
            <div className="flex min-h-7 flex-wrap items-center justify-between gap-x-6 gap-y-1 text-[13px]">
              {nodeA !== undefined && nodeB !== undefined ? (
                <span className="flex items-center gap-2">
                  <span className="font-medium" data-testid="watching">
                    {g.watching(nodeA.name, nodeB.name)}
                  </span>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-foreground"
                    onClick={() => setPair({ a: null, b: null })}
                  >
                    <X aria-hidden />
                    {g.clearPair}
                  </Button>
                </span>
              ) : (
                <span className="text-muted-foreground">{g.pickHint}</span>
              )}
              <span className="text-xs text-muted-foreground">{t.common.updatedEvery(REPORT_INTERVAL_SECS)}</span>
            </div>
          </div>

          {matrix.isError && matrix.data === undefined ? (
            <ErrorState title={g.loadFailed} error={matrix.error} onRetry={() => void matrix.refetch()} />
          ) : shape === null || matrix.data === undefined ? (
            <SkeletonBlock lines={8} />
          ) : tooFew ? (
            <EmptyState
              icon={Waypoints}
              title={g.emptyTitle}
              action={
                <Button variant="primary" onClick={() => setAdding(true)}>
                  <Plus aria-hidden />
                  {g.addNode}
                </Button>
              }
            />
          ) : (
            <div className="space-y-4">
              {noRoutes ? <Notice title={g.noRoutesTitle}>{g.noRoutesBody}</Notice> : null}
              {view === 'graph' ? (
                <div
                  className={cn('grid grid-cols-1 gap-10', panel !== null && '@4xl:grid-cols-[minmax(0,1fr)_16rem]')}
                >
                  <div className="space-y-3">
                    <div className="h-[600px] rounded-lg bg-sidebar" data-testid="topology-graph">
                      <TopologyGraph
                        shape={shape}
                        rtt={{ forward: shape.forward?.rtt_ms ?? null, reverse: shape.reverse?.rtt_ms ?? null }}
                        onPick={(id) => setPair(nextPick(pair, id))}
                      />
                    </div>
                    <GraphLegend />
                  </div>
                  {panel}
                </div>
              ) : (
                <div className="space-y-4">
                  <MatrixLegend />
                  <div
                    className={cn('grid grid-cols-1 gap-10', panel !== null && '@4xl:grid-cols-[minmax(0,1fr)_16rem]')}
                  >
                    <div className="min-w-0">
                      <MatrixView matrix={matrix.data} pair={pair} onPick={(a, b) => setPair({ a, b })} />
                    </div>
                    {panel}
                  </div>
                </div>
              )}
            </div>
          )}
        </>
      )}
      <AddNodeDialog network={network} open={adding} onClose={() => setAdding(false)} />
    </Page>
  )
}
