import { MarkerType, type Node, type NodeProps, type XYPosition } from '@xyflow/react'
import { useMemo, useRef, useState } from 'react'

import { type NodeId } from '@/api/types'
import { Flag } from '@/components/flag'
import { OsIcon } from '@/components/os-icon'
import { t, useLocale } from '@/i18n'
import { cn } from '@/lib/utils'
import { GraphCanvas } from '@/pages/access/flow'
import { type LineColors, useLineColors } from '@/topology/colors'
import { curveOf, type Rect } from '@/access/geometry'
import { LABEL_HEIGHT, type LabelItem, labelWidth, placeLabels } from '@/topology/labels'
import { HiddenHandles, type LabelPart, LineEdgeView, type LineFlowEdge } from '@/topology/line-edge'
import { entryText } from '@/topology/route'
import {
  type HighlightHop,
  layoutSignature,
  pointId,
  type TopoLink,
  type TopologyShape,
  type TopoPoint,
} from '@/topology/graph'

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
/** ice3proxy 的线是长虚线：颜色之外线型也不一样。 */
export const LANE_DASH = '9 4'
/** 线上跑的小光点也一样：线太多时不跑，满屏乱闪。 */
const FLOW_UP_TO = 60

function linkColor(link: TopoLink, colors: LineColors): string {
  if (link.stale) return colors.stale
  return colors[link.kind]
}

type Name = (id: string) => string

const relayNames = (link: TopoLink, name: Name) =>
  link.relays.map((id) => name(pointId(id))).join(t.routes.hopSeparator)
const rttText = (rtt: number | null) => (rtt === null ? null : t.routes.rtt(rtt))

function linkDescription(link: TopoLink, name: Name): string {
  if (link.stale) return g.linkStale
  const rtt = rttText(link.rtt) ?? t.routes.rttUnknown
  if (link.kind === 'direct') return g.linkDirect(rtt)
  if (link.kind === 'relay') return g.linkRelay(relayNames(link, name), rtt)
  return g.linkLane
}

/** 线上的字：直连"UDP 1 ms"，经中继"经 h2 TCP 300 ms"（到第一个中继走 TCP 时黄字），ice3proxy"ice3proxy TCP 2 条在用"。 */
function linkParts(link: TopoLink, name: Name): LabelPart[] {
  const rtt = rttText(link.rtt)
  const parts: LabelPart[] = []
  if (link.kind === 'direct') {
    parts.push({ text: g.udp, tone: 'ok' })
  } else if (link.kind === 'relay') {
    parts.push({ text: t.routes.via(relayNames(link, name)), tone: 'relay' })
    if (link.transport === 'tcp') parts.push({ text: g.tcp, tone: 'warn' })
    else if (link.transport === 'udp') parts.push({ text: g.udp })
  } else {
    const inUse = link.lanes?.in_use ?? 0
    parts.push({ text: t.routes.protocol.lane, tone: 'lane' }, { text: g.tcp })
    parts.push({ text: inUse > 0 ? g.laneInUse(inUse) : g.laneIdle })
    return parts
  }
  if (rtt !== null) parts.push({ text: rtt })
  return parts
}

/** 悬停在线上的字时：哪一对、怎么连的，两个方向各一行（节点报的原样），经中继时每一段的延迟，协议。 */
function LinkTip({ link, name }: { link: TopoLink; name: Name }) {
  const a = name(link.source)
  const b = name(link.target)
  const byId = (id: NodeId) => name(pointId(id))
  const title =
    link.kind === 'direct'
      ? g.tipDirect(a, b)
      : link.kind === 'relay'
        ? g.tipRelay(a, b, relayNames(link, name))
        : g.tipLane(a, b)
  return (
    <div className="space-y-1">
      <p className="font-medium">{title}</p>
      {link.kind === 'lane' ? (
        <>
          <p>{g.tipLaneCounts(link.lanes?.in_use ?? 0, link.lanes?.idle ?? 0)}</p>
          <p>{g.tipLaneControl(link.lanes?.control ?? false)}</p>
        </>
      ) : (
        <>
          {link.entries.map((entry) => (
            <p key={`${entry.from}>${entry.to}`}>
              {t.topology.forward(byId(entry.from), byId(entry.to))}：{entryText(entry, byId)}
            </p>
          ))}
          {link.legs.length > 0 ? (
            <p>
              {g.tipLegs(
                link.legs
                  .map((leg) => g.tipLeg(byId(leg.from), byId(leg.to), rttText(leg.rtt) ?? t.routes.rttUnknown))
                  .join(' · '),
              )}
            </p>
          ) : null}
          <p>
            {link.kind === 'direct'
              ? g.tipProtocolDirect
              : link.transport === 'tcp' || link.transport === 'udp'
                ? g.tipProtocolRelay(link.transport === 'tcp' ? g.tcp : g.udp)
                : g.tipProtocolRelayUnknown}
          </p>
        </>
      )}
    </div>
  )
}

interface Hover {
  kind: 'node' | 'link'
  id: string
}

/** 悬停时亮着的点和线。 */
interface Spot {
  nodes: Set<string>
  links: Set<string>
}

/**
 * 停在节点上：连着它的线、经它中转的线，和这些线两头、经过的节点。停在线上：这条线、两头和经过的节点。
 * 不可达的线（`none:`）只亮它自己和两头。
 */
function spotlight(shape: TopologyShape, hover: Hover): Spot {
  const nodes = new Set<string>()
  const links = new Set<string>()
  const light = (id: string, source: string, target: string, relays: readonly NodeId[]) => {
    links.add(id)
    nodes.add(source)
    nodes.add(target)
    for (const relay of relays) nodes.add(pointId(relay))
  }
  for (const link of shape.links) {
    const touches =
      hover.kind === 'node'
        ? link.source === hover.id ||
          link.target === hover.id ||
          link.relays.some((relay) => pointId(relay) === hover.id)
        : link.id === hover.id
    if (touches) light(link.id, link.source, link.target, link.relays)
  }
  for (const pair of shape.unreachable) {
    const touches = hover.kind === 'node' ? pair.source === hover.id || pair.target === hover.id : pair.id === hover.id
    if (touches) light(pair.id, pair.source, pair.target, [])
  }
  if (hover.kind === 'node') nodes.add(hover.id)
  return { nodes, links }
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
  // `settled` 是松手时的位置：线上的字按它排，拖的时候字跟着线走、不重新排，免得乱跳。
  const [moved, setMoved] = useState<{
    signature: string
    at: Record<string, XYPosition>
    settled: Record<string, XYPosition>
  }>({ signature, at: {}, settled: {} })
  const dragged = moved.signature === signature ? moved.at : {}
  const settled = moved.signature === signature ? moved.settled : {}

  // 鼠标停在一个节点或者一条线上：它和跟它有关的亮着，别的淡下去。选中了一对时不管悬停（那一对已经亮着了）。
  const [hover, setHover] = useState<Hover | null>(null)
  const spot = useMemo(() => (focused || hover === null ? null : spotlight(shape, hover)), [shape, focused, hover])

  const nodes = useMemo<TopoFlowNode[]>(
    () =>
      shape.points.map((point, order) => ({
        id: point.id,
        type: 'topo',
        position: dragged[point.id] ?? { x: point.x - point.width / 2, y: point.y - point.height / 2 },
        data: {
          point,
          dimmed: (focused && !shape.involved.has(point.id)) || (spot !== null && !spot.nodes.has(point.id)),
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
    [shape, focused, locale, looks, dragged, spot],
  )

  const laid = useMemo<LineFlowEdge[]>(() => {
    const showLabels = shape.links.length <= LABELS_UP_TO && !focused
    const flows = shape.links.length <= FLOW_UP_TO
    // 光点跑一趟多久按排好时的线长算，拖动卡片时不跟着变：一变，正在跑的点就跳到别处、忽明忽暗。
    const at = new Map(shape.points.map((point) => [point.id, point]))
    const lengthOf = (source: string, target: string) => {
      const a = at.get(source)
      const b = at.get(target)
      return a === undefined || b === undefined ? 0 : Math.hypot(b.x - a.x, b.y - a.y)
    }
    const base: LineFlowEdge[] = shape.links.map((link) => {
      const color = linkColor(link, colors)
      // 只有一个方向走它时线带箭头、光点只往那边跑；两个方向都走时两头各跑一个。
      const oneWay = link.forward !== link.reverse
      const [source, target] = oneWay && link.reverse ? [link.target, link.source] : [link.source, link.target]
      const quiet = link.kind === 'lane' && (link.lanes?.in_use ?? 0) === 0
      return {
        id: link.id,
        type: 'line',
        source,
        target,
        data: {
          index: link.index,
          siblings: link.siblings,
          label: null,
          parts: showLabels ? linkParts(link, name) : undefined,
          tip: showLabels ? <LinkTip link={link} name={name} /> : undefined,
          strong: false,
          bend: link.bend,
          onHover: (on: boolean) => setHover(on ? { kind: 'link', id: link.id } : null),
          flow: flows && !focused && !link.stale && !quiet ? (oneWay ? 'forward' : 'both') : undefined,
          length: lengthOf(link.source, link.target),
        },
        markerEnd: oneWay ? { type: MarkerType.ArrowClosed, color, width: 12, height: 12 } : undefined,
        style: {
          stroke: color,
          strokeWidth: 1.6,
          strokeDasharray: link.stale ? '5 4' : link.kind === 'lane' ? LANE_DASH : undefined,
          opacity: focused ? 0.15 : 0.9,
        },
        focusable: false,
        ariaLabel: g.linkLabel(name(source), name(target), linkDescription(link, name)),
      }
    })
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
    const all = [...base, ...none, ...highlight]
    // 线上的字自动排开：按松手时卡片的位置算每条线的样子，字沿线挪到不压别的字、不压卡片的地方。
    const rect = (id: string): Rect | null => {
      const point = at.get(id)
      if (point === undefined) return null
      const position = settled[id] ?? { x: point.x - point.width / 2, y: point.y - point.height / 2 }
      return { ...position, width: point.width, height: point.height }
    }
    const items: LabelItem[] = []
    for (const edge of all) {
      const data = edge.data
      const text = data?.parts?.map((part) => part.text) ?? (data?.label === null ? null : [data?.label ?? ''])
      const from = rect(edge.source)
      const to = rect(edge.target)
      if (data === undefined || text === null || from === null || to === null) continue
      items.push({
        id: edge.id,
        curve: curveOf(from, to, data.index, data.siblings, data.bend ?? 0),
        width: labelWidth(text),
        height: LABEL_HEIGHT,
      })
    }
    const cards = shape.points.map((point) => rect(point.id)).filter((card): card is Rect => card !== null)
    const placement = placeLabels(items, cards)
    return all.map((edge) =>
      edge.data === undefined || !placement.has(edge.id)
        ? edge
        : { ...edge, data: { ...edge.data, labelAt: placement.get(edge.id) } },
    )
    // name 只读 shape.points；语言变了线上的说明要换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape, focused, rtt.forward, rtt.reverse, locale, colors, settled])

  // 悬停时：亮着的线粗一点、字和光点照常（光点快一些），别的线淡下去、不写字、不跑光点；亮着的画在最上面。
  const edges = useMemo<LineFlowEdge[]>(() => {
    if (spot === null) return laid
    const lit = (edge: LineFlowEdge) => spot.links.has(edge.id)
    return [...laid.filter((edge) => !lit(edge)), ...laid.filter(lit)].map((edge) => {
      const on = lit(edge)
      const width = Number(edge.style?.strokeWidth ?? 1.6)
      return {
        ...edge,
        style: { ...edge.style, opacity: on ? 1 : 0.08, strokeWidth: on ? width + 1 : width },
        data:
          edge.data === undefined
            ? undefined
            : on
              ? { ...edge.data, strong: true }
              : { ...edge.data, label: null, parts: undefined, tip: undefined, flow: undefined },
      }
    })
  }, [laid, spot])

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
      onNodeMouseEnter={(_event, node) => setHover({ kind: 'node', id: node.id })}
      onNodeMouseLeave={() => setHover(null)}
      onEdgeMouseEnter={(_event, edge) => setHover({ kind: 'link', id: edge.id })}
      onEdgeMouseLeave={() => setHover(null)}
      onNodesChange={(changes) => {
        for (const change of changes) {
          // 用键盘选中（聚焦后按回车）和点击是一回事。
          if (change.type === 'select' && change.selected) pick(change.id)
          if (change.type === 'position') {
            const { id, position, dragging } = change
            setMoved((previous) => {
              const mine = previous.signature === signature ? previous : { signature, at: {}, settled: {} }
              const at = position === undefined ? mine.at : { ...mine.at, [id]: position }
              return { signature, at, settled: dragging === false ? at : mine.settled }
            })
          }
        }
      }}
    />
  )
}
