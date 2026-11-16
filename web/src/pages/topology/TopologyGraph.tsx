import {
  BaseEdge,
  type Edge,
  EdgeLabelRenderer,
  type EdgeProps,
  Handle,
  MarkerType,
  type Node,
  type NodeProps,
  Position,
  useInternalNode,
} from '@xyflow/react'
import { useMemo, useRef } from 'react'

import { type NodeId } from '@/api/types'
import { curveBetween, type Rect } from '@/access/geometry'
import { OnlineDot } from '@/components/status'
import { t, useLocale } from '@/i18n'
import { cn } from '@/lib/utils'
import { GraphCanvas } from '@/pages/access/flow'
import { LINE_COLORS } from '@/topology/colors'
import { type HighlightHop, layoutSignature, type TopoLink, type TopologyShape, type TopoPoint } from '@/topology/graph'

const g = t.topology

type TopoFlowNode = Node<{ point: TopoPoint; dimmed: boolean }, 'topo'>

interface LineData extends Record<string, unknown> {
  index: number
  siblings: number
  label: string | null
  strong: boolean
}
type LineFlowEdge = Edge<LineData, 'line'>

/** 连线不从固定的接口出发，而是按两个框的位置算，所以接口藏在框的正中，只是让画图的库认这条线。 */
const HIDDEN_HANDLE = { left: '50%', top: '50%', opacity: 0, pointerEvents: 'none' } as const

function TopoNodeView({ data }: NodeProps<TopoFlowNode>) {
  const { point, dimmed } = data
  const { node } = point
  return (
    <div
      style={{ width: point.width, height: point.height }}
      className={cn(
        'flex cursor-pointer flex-col justify-center rounded-lg border border-border-strong bg-card px-3 transition-[opacity,border-color] hover:border-foreground/40',
        !node.online && 'border-dashed bg-transparent',
        point.picked !== null && 'border-primary outline-2 outline-offset-0 outline-primary',
        dimmed && 'opacity-35',
      )}
    >
      <Handle type="target" position={Position.Top} style={HIDDEN_HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} style={HIDDEN_HANDLE} isConnectable={false} />
      <span className="flex min-w-0 items-center gap-1.5">
        <OnlineDot online={node.online} />
        <span
          className={cn(
            'truncate text-sm',
            point.relay ? 'font-semibold' : 'font-medium',
            !node.online && 'text-muted-foreground',
          )}
        >
          {node.name}
        </span>
        {point.picked !== null ? (
          <span className="ml-auto shrink-0 rounded-sm bg-primary px-1 text-[11px] leading-4 font-semibold text-primary-foreground">
            {point.picked.toUpperCase()}
          </span>
        ) : null}
      </span>
      <span className="flex items-center gap-1.5 truncate text-[11px] text-muted-foreground tabular">
        {node.virtual_ip}
        {point.relay ? <span className="font-medium text-relay">{g.relayBadge}</span> : null}
      </span>
    </div>
  )
}

function rectOf(node: ReturnType<typeof useInternalNode>): Rect | null {
  if (node === undefined) return null
  return {
    x: node.internals.positionAbsolute.x,
    y: node.internals.positionAbsolute.y,
    width: node.measured.width ?? 0,
    height: node.measured.height ?? 0,
  }
}

function LineEdgeView({ id, source, target, data, markerEnd, style }: EdgeProps<LineFlowEdge>) {
  const sourceRect = rectOf(useInternalNode(source))
  const targetRect = rectOf(useInternalNode(target))
  if (sourceRect === null || targetRect === null || data === undefined) return null
  const path = curveBetween(sourceRect, targetRect, data.index, data.siblings)
  return (
    <>
      <BaseEdge id={id} path={path.d} markerEnd={markerEnd} style={style} interactionWidth={14} />
      {data.label !== null ? (
        <EdgeLabelRenderer>
          <span
            style={{ transform: `translate(-50%, -50%) translate(${path.label.x}px, ${path.label.y}px)` }}
            className={cn(
              'pointer-events-none absolute rounded-sm bg-sidebar px-1 text-[11px] leading-4 tabular',
              data.strong ? 'font-medium text-foreground' : 'text-muted-foreground',
            )}
          >
            {data.label}
          </span>
        </EdgeLabelRenderer>
      ) : null}
    </>
  )
}

const nodeTypes = { topo: TopoNodeView }
const edgeTypes = { line: LineEdgeView }

/** 线不多时把直连的延迟写在线上；多了就不写，免得糊成一片（精确的数看矩阵）。 */
const LABELS_UP_TO = 24

function linkColor(link: TopoLink): string {
  if (link.stale) return LINE_COLORS.stale
  return link.direct ? LINE_COLORS.direct : LINE_COLORS.relay
}

function linkDescription(link: TopoLink): string {
  if (link.stale) return g.linkStale
  if (link.direct) return g.linkDirect(link.rtt === null ? t.routes.rttUnknown : t.routes.rtt(link.rtt))
  return g.linkRelayLeg
}

interface TopologyGraphProps {
  shape: TopologyShape
  /** 选中的那一对两个方向全程的延迟，写在各自第一段上。 */
  rtt: { forward: number | null; reverse: number | null }
  onPick: (node: NodeId) => void
}

/**
 * 拓扑的实时图。颜色之外总有线型：直连实线，经 relay 的那一段紫色实线，过期的灰色虚线，不可达红色点线；
 * 选中的那一对 A → B 是粗实线，B → A 是粗虚线，都带箭头，其余的变暗。
 */
export function TopologyGraph({ shape, rtt, onPick }: TopologyGraphProps) {
  // 点和线上的文字（读屏的说明、线上的延迟）算好存着，换语言时要重新算。
  const locale = useLocale()
  const signature = layoutSignature(shape)
  const focused = shape.involved.size > 0
  const name = (id: string) => shape.points.find((point) => point.id === id)?.node.name ?? id

  const nodes = useMemo<TopoFlowNode[]>(
    () =>
      shape.points.map((point) => ({
        id: point.id,
        type: 'topo',
        position: { x: point.x - point.width / 2, y: point.y - point.height / 2 },
        data: { point, dimmed: focused && !shape.involved.has(point.id) },
        ariaLabel: g.nodeLabel(point.node.name, point.relay, point.node.online),
        draggable: false,
        connectable: false,
      })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [shape, focused, locale],
  )

  const edges = useMemo<LineFlowEdge[]>(() => {
    const showLabels = shape.links.length <= LABELS_UP_TO && !focused
    const base: LineFlowEdge[] = shape.links.map((link) => ({
      id: link.id,
      type: 'line',
      source: link.source,
      target: link.target,
      data: {
        index: 0,
        siblings: 1,
        label: showLabels && link.direct && link.rtt !== null ? t.routes.rtt(link.rtt) : null,
        strong: false,
      },
      style: {
        stroke: linkColor(link),
        strokeWidth: 1.6,
        strokeDasharray: link.stale ? '5 4' : undefined,
        opacity: focused ? 0.15 : 0.9,
      },
      focusable: false,
      ariaLabel: g.linkLabel(name(link.source), name(link.target), linkDescription(link)),
    }))
    const none: LineFlowEdge[] = shape.unreachable.map((pair) => ({
      id: pair.id,
      type: 'line',
      source: pair.source,
      target: pair.target,
      data: { index: 0, siblings: 1, label: null, strong: false },
      style: {
        stroke: pair.stale ? LINE_COLORS.stale : LINE_COLORS.none,
        strokeWidth: 1.4,
        strokeDasharray: '2 4',
        opacity: focused ? 0.15 : 0.85,
      },
      focusable: false,
      ariaLabel: g.linkLabel(name(pair.source), name(pair.target), g.linkUnreachable),
    }))
    const highlight: LineFlowEdge[] = shape.highlight.map((hop: HighlightHop) => {
      const color = hop.stale ? LINE_COLORS.stale : hop.kind === 'direct' ? LINE_COLORS.direct : LINE_COLORS.relay
      const total = hop.direction === 'forward' ? rtt.forward : rtt.reverse
      const label =
        hop.step === 0
          ? `${hop.direction === 'forward' ? g.legendForward : g.legendReverse}${total === null ? '' : ` ${t.routes.rtt(total)}`}`
          : null
      return {
        id: hop.id,
        type: 'line',
        source: hop.source,
        target: hop.target,
        data: { index: hop.index, siblings: hop.siblings, label, strong: true },
        markerEnd: { type: MarkerType.ArrowClosed, color, width: 16, height: 16 },
        // 不另给 zIndex：排在数组后面就画在上面；给了反而会盖住线上的字。
        style: { stroke: color, strokeWidth: 2.6, strokeDasharray: hop.direction === 'reverse' ? '7 5' : undefined },
        focusable: false,
      }
    })
    return [...base, ...none, ...highlight]
    // name 只读 shape.points；语言变了线上的说明要换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape, focused, rtt.forward, rtt.reverse, locale])

  // 用鼠标点一下，画图的库既报"点了"又报"选中了"：同一下只算一次，不然第二个点会被算成重新开始选。
  const last = useRef({ id: '', at: 0 })
  const pick = (id: string) => {
    const now = performance.now()
    if (last.current.id === id && now - last.current.at < 300) return
    last.current = { id, at: now }
    const point = shape.points.find((candidate) => candidate.id === id)
    if (point !== undefined) onPick(point.node.id)
  }

  return (
    <GraphCanvas<TopoFlowNode, LineFlowEdge>
      key={signature}
      label={g.canvas}
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      edgeTypes={edgeTypes}
      nodesDraggable={false}
      elementsSelectable
      onNodeClick={(_event, node) => pick(node.id)}
      onNodesChange={(changes) => {
        // 用键盘选中（聚焦后按回车）和点击是一回事。
        for (const change of changes) {
          if (change.type === 'select' && change.selected) pick(change.id)
        }
      }}
    />
  )
}
