import { MarkerType, type Node, type NodeProps, type XYPosition } from '@xyflow/react'
import { useMemo, useRef, useState } from 'react'

import { type NodeId } from '@/api/types'
import { Flag } from '@/components/flag'
import { OsIcon } from '@/components/os-icon'
import { t, useLocale } from '@/i18n'
import { cn } from '@/lib/utils'
import { GraphCanvas } from '@/pages/access/flow'
import { type LineColors, useLineColors } from '@/topology/colors'
import { HiddenHandles, LineEdgeView, type LineFlowEdge } from '@/topology/line-edge'
import { type HighlightHop, layoutSignature, type TopoLink, type TopologyShape, type TopoPoint } from '@/topology/graph'

const g = t.topology

/** 节点列表里才有的东西：在哪个国家（画旗子）、什么系统（没有国家时画系统的图标）。 */
export interface NodeLook {
  country: string | null
  platform: string
}

type TopoFlowNode = Node<{ point: TopoPoint; dimmed: boolean; look: NodeLook | null; order: number }, 'topo'>

/**
 * 一张节点卡片：左边一块圆角的方块放国旗（不知道国家时是系统的图标，再不知道是名字的头一个字），
 * 右下角是在线的点（在线时一圈一圈往外散）；右边是名字、relay 的标签和虚拟 IP。
 * 选中的 A、B 外面一圈蓝，右上角挂一个圆的 A、B。离线的虚线框、淡下去。
 */
function TopoNodeView({ data }: NodeProps<TopoFlowNode>) {
  const { point, dimmed, look, order } = data
  const { node } = point
  const flag = look?.country ?? null
  return (
    <div
      style={{ width: point.width, height: point.height, animationDelay: `${order * 45}ms` }}
      className={cn('motion-safe:animate-topo-in', dimmed && 'opacity-35 saturate-50', 'transition-opacity')}
    >
      <div
        className={cn(
          'relative flex size-full cursor-pointer items-center gap-2.5 rounded-xl border bg-card px-2.5 shadow-card transition-[box-shadow,transform,border-color] duration-200 hover:-translate-y-0.5 hover:border-border-strong hover:shadow-pop in-[.dragging]:cursor-grabbing in-[.dragging]:shadow-pop',
          !node.online && 'border-dashed bg-card/70 shadow-none',
          point.picked !== null && 'border-primary ring-2 ring-primary/25 hover:border-primary',
        )}
      >
        <HiddenHandles />
        <span
          className={cn(
            'relative grid size-9 shrink-0 place-items-center rounded-lg',
            point.relay ? 'bg-relay-soft text-relay' : 'bg-muted text-muted-foreground',
            !node.online && 'grayscale',
          )}
        >
          {flag !== null ? (
            <Flag code={flag} className="size-5" />
          ) : look !== null ? (
            <OsIcon
              platform={look.platform}
              className="size-4"
              fallback={<span className="text-[13px] font-semibold uppercase">{node.name.slice(0, 1)}</span>}
            />
          ) : (
            <span className="text-[13px] font-semibold uppercase">{node.name.slice(0, 1)}</span>
          )}
          {/* 在线的点：后面一层一圈一圈往外散，前面的点带一圈卡片底色和方块隔开。 */}
          <span aria-hidden className="absolute -right-0.5 -bottom-0.5 size-2.5">
            {node.online ? <span className="absolute inset-0 rounded-full motion-safe:animate-topo-breathe" /> : null}
            <span
              className={cn('absolute inset-0 rounded-full ring-2 ring-card', node.online ? 'bg-ok' : 'bg-idle-dot')}
            />
          </span>
        </span>
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex min-w-0 items-center gap-1.5">
            <span className={cn('truncate text-[13px] font-semibold', !node.online && 'text-muted-foreground')}>
              {node.name}
            </span>
            {point.relay ? (
              <span className="shrink-0 rounded-full bg-relay-soft px-1.5 text-[10px] leading-4 font-medium text-relay">
                {g.relayBadge}
              </span>
            ) : null}
          </span>
          <span className="truncate font-mono text-[11px] text-muted-foreground">{node.virtual_ip}</span>
        </span>
        {point.picked !== null ? (
          <span className="absolute -top-2 -right-2 grid size-5 place-items-center rounded-full bg-primary text-[11px] font-semibold text-primary-foreground shadow-card ring-2 ring-card">
            {point.picked.toUpperCase()}
          </span>
        ) : null}
      </div>
    </div>
  )
}

const nodeTypes = { topo: TopoNodeView }
const edgeTypes = { line: LineEdgeView }

/** 线不多时把直连的延迟写在线上；多了就不写，免得糊成一片（精确的数看矩阵）。 */
const LABELS_UP_TO = 24
/** 线上跑的小光点也一样：线太多时不跑，满屏乱闪。 */
const FLOW_UP_TO = 60

function linkColor(link: TopoLink, colors: LineColors): string {
  if (link.stale) return colors.stale
  return link.direct ? colors.direct : colors.relay
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
  /** 节点列表里的国家和系统，按节点编号；还没取到时是空的。 */
  looks: ReadonlyMap<NodeId, NodeLook>
  onPick: (node: NodeId) => void
}

/**
 * 拓扑的实时图。颜色之外总有线型：直连实线，经 relay 的那一段紫色实线，过期的灰色虚线，不可达红色点线；
 * 选中的那一对 A → B 是粗实线，B → A 是粗虚线，都带箭头，其余的变暗。
 * 有连接的线上两个方向各跑一个小光点；选中一对时只在它的路上沿方向跑。卡片可以拖开，换了节点的集合才回到原位。
 */
export function TopologyGraph({ shape, rtt, looks, onPick }: TopologyGraphProps) {
  // 点和线上的文字（读屏的说明、线上的延迟）算好存着，换语言时要重新算。
  const locale = useLocale()
  const colors = useLineColors()
  const signature = layoutSignature(shape)
  const focused = shape.involved.size > 0
  const name = (id: string) => shape.points.find((point) => point.id === id)?.node.name ?? id
  // 拖开的卡片停在哪里：只对这一套节点有效，节点的集合变了（重新排了）就作废。
  const [moved, setMoved] = useState<{ signature: string; at: Record<string, XYPosition> }>({ signature, at: {} })
  const dragged = moved.signature === signature ? moved.at : {}

  const nodes = useMemo<TopoFlowNode[]>(
    () =>
      shape.points.map((point, order) => ({
        id: point.id,
        type: 'topo',
        position: dragged[point.id] ?? { x: point.x - point.width / 2, y: point.y - point.height / 2 },
        data: {
          point,
          dimmed: focused && !shape.involved.has(point.id),
          look: looks.get(point.node.id) ?? null,
          order,
        },
        // 卡片的大小是定的，直接告诉画图库：不然每拖一下都换了一份节点，它先把没量过的卡片藏起来、量完再显示，一闪一闪的。
        measured: { width: point.width, height: point.height },
        ariaLabel: g.nodeLabel(point.node.name, point.relay, point.node.online),
        draggable: true,
        connectable: false,
      })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [shape, focused, locale, looks, dragged],
  )

  const edges = useMemo<LineFlowEdge[]>(() => {
    const showLabels = shape.links.length <= LABELS_UP_TO && !focused
    const flows = shape.links.length <= FLOW_UP_TO
    // 光点跑一趟多久按排好时的线长算，拖动卡片时不跟着变：一变，正在跑的点就跳到别处、忽明忽暗。
    const at = new Map(shape.points.map((point) => [point.id, point]))
    const lengthOf = (source: string, target: string) => {
      const a = at.get(source)
      const b = at.get(target)
      return a === undefined || b === undefined ? 0 : Math.hypot(b.x - a.x, b.y - a.y)
    }
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
        bend: link.bend,
        flow: flows && !focused && !link.stale ? 'both' : undefined,
        length: lengthOf(link.source, link.target),
      },
      style: {
        stroke: linkColor(link, colors),
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
      data: { index: 0, siblings: 1, label: null, strong: false, bend: pair.bend },
      style: {
        stroke: pair.stale ? colors.stale : colors.none,
        strokeWidth: 1.4,
        strokeDasharray: '2 4',
        opacity: focused ? 0.15 : 0.85,
      },
      focusable: false,
      ariaLabel: g.linkLabel(name(pair.source), name(pair.target), g.linkUnreachable),
    }))
    const highlight: LineFlowEdge[] = shape.highlight.map((hop: HighlightHop) => {
      const color = hop.stale ? colors.stale : hop.kind === 'direct' ? colors.direct : colors.relay
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
        data: {
          index: hop.index,
          siblings: hop.siblings,
          label,
          strong: true,
          bend: hop.bend,
          flow: hop.stale ? undefined : 'forward',
          length: lengthOf(hop.source, hop.target),
        },
        markerEnd: { type: MarkerType.ArrowClosed, color, width: 16, height: 16 },
        // 不另给 zIndex：排在数组后面就画在上面；给了反而会盖住线上的字。
        style: { stroke: color, strokeWidth: 2.6, strokeDasharray: hop.direction === 'reverse' ? '7 5' : undefined },
        focusable: false,
      }
    })
    return [...base, ...none, ...highlight]
    // name 只读 shape.points；语言变了线上的说明要换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape, focused, rtt.forward, rtt.reverse, locale, colors])

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
      nodesDraggable
      selectNodesOnDrag={false}
      elementsSelectable
      onNodeClick={(_event, node) => pick(node.id)}
      onNodesChange={(changes) => {
        for (const change of changes) {
          // 用键盘选中（聚焦后按回车）和点击是一回事。
          if (change.type === 'select' && change.selected) pick(change.id)
          if (change.type === 'position' && change.position !== undefined) {
            const { id, position } = change
            setMoved((previous) => ({
              signature,
              at: { ...(previous.signature === signature ? previous.at : {}), [id]: position },
            }))
          }
        }
      }}
    />
  )
}
