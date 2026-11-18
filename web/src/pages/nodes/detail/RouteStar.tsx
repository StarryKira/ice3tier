import { type Node as FlowNode, type NodeProps } from '@xyflow/react'
import { useMemo } from 'react'
import { Link } from 'react-router'

import { type Network, type Node, type NodeDetail, type NodeId, type NodeRoute, type Route } from '@/api/types'
import { Flag } from '@/components/flag'
import { Canvas } from '@/components/page'
import { OnlineDot } from '@/components/status'
import { t, useLocale } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { cn } from '@/lib/utils'
import { GraphCanvas } from '@/pages/access/flow'
import { networkPath } from '@/pages/networks/context'
import { type LineColors, useLineColors } from '@/topology/colors'
import { HiddenHandles, LineEdgeView, type LineFlowEdge, LineSample } from '@/topology/line-edge'

import { type Leg, protocolText } from './routeParts'

const d = t.nodes.detail

/** 星状图上的一个对端：现在走的那条路。 */
export interface Spoke {
  entry: NodeRoute
  /** 节点列表里的它（在线、国家）；列表还没到时没有。 */
  peer: Node | undefined
  leg: Leg
  middle: NodeId[] | null
}

const PEER = { width: 184, height: 66 }
const CENTER = { width: 168, height: 56 }

/**
 * 对端排在一个椭圆上，从正上方开始顺时针，按路由表的顺序（延迟低的在前）。椭圆比圆扁一些：画布总是宽的。
 * 对端多时椭圆跟着变大，框不挤在一起；看全靠画布缩放。
 */
function ring(count: number): { rx: number; ry: number } {
  const ry = Math.max(200, (count * (PEER.width + 24)) / (2 * Math.PI) / 1.3)
  return { rx: ry * 1.6, ry }
}

/** 方式写成字：P2P、中继、不可达，颜色和表格里的标签一样（对端离线时的不可达是灰的，不算出了问题）。 */
function viaText(route: Route, peerOnline = true): { text: string; className: string } {
  if (route.via === 'direct') return { text: d.routesP2P, className: 'text-ok' }
  if (route.via === 'relay') return { text: d.routesRelay, className: 'text-relay' }
  return { text: d.viaNone, className: peerOnline ? 'text-destructive' : 'text-muted-foreground' }
}

/** 一条路说成一句话：方式、延迟、经过谁。读屏和点开的链接用。 */
function describe(spoke: Spoke, nodes: readonly Node[]): string {
  const { route, rtt_ms: rtt } = spoke.entry
  const parts = [viaText(route).text]
  if (route.via !== 'none' && rtt !== null) parts.push(t.common.ms(rtt))
  if (spoke.middle !== null && spoke.middle.length > 0) {
    parts.push(d.routesViaChain(spoke.middle.map((id) => nodeName(nodes, id)).join(' → ')))
  }
  return parts.join(t.common.listSeparator)
}

type CenterFlowNode = FlowNode<{ node: NodeDetail }, 'center'>
type PeerFlowNode = FlowNode<{ spoke: Spoke; via: string; label: string; to: string; protocol: string | null }, 'peer'>

function CenterView({ data }: NodeProps<CenterFlowNode>) {
  const { node } = data
  return (
    <div
      style={CENTER}
      className="flex flex-col justify-center rounded-lg border border-primary bg-card px-3.5 outline-2 outline-primary"
    >
      <HiddenHandles />
      <span className="flex min-w-0 items-center gap-1.5">
        <OnlineDot online={node.online} />
        <span className="truncate text-sm font-semibold">{node.name}</span>
        <Flag code={node.country} />
      </span>
      <span className="text-[11px] text-muted-foreground tabular">{node.virtual_ip}</span>
    </div>
  )
}

function PeerView({ data }: NodeProps<PeerFlowNode>) {
  const { spoke, via, label, to, protocol } = data
  const { entry, peer } = spoke
  const online = peer?.online ?? true
  const how = viaText(entry.route, online)
  const hop = spoke.middle?.[0]
  return (
    <Link
      to={to}
      aria-label={label}
      style={PEER}
      className={cn(
        'flex flex-col justify-center rounded-lg border border-border-strong bg-card px-3 transition-[border-color] hover:border-foreground/40',
        !online && 'border-dashed bg-transparent',
      )}
    >
      <HiddenHandles />
      <span className="flex min-w-0 items-center gap-1.5">
        <OnlineDot online={online} />
        <span className={cn('truncate text-sm font-medium', !online && 'text-muted-foreground')}>
          {entry.peer.name}
        </span>
        <Flag code={peer?.country} />
      </span>
      <span className="text-[11px] text-muted-foreground tabular">{entry.peer.virtual_ip}</span>
      <span className="flex min-w-0 items-center gap-1 text-[11px] whitespace-nowrap">
        <span className={cn('font-medium', how.className)}>{how.text}</span>
        {protocol !== null ? (
          <>
            <span className="text-faint">·</span>
            <span className={spoke.leg === 'tcp' ? 'text-warn' : 'text-muted-foreground'}>{protocol}</span>
          </>
        ) : null}
        {hop !== undefined ? (
          <>
            <span className="text-faint">·</span>
            <span className="truncate text-muted-foreground">{via}</span>
          </>
        ) : null}
      </span>
    </Link>
  )
}

const nodeTypes = { center: CenterView, peer: PeerView }
const edgeTypes = { line: LineEdgeView }

function lineStyle(spoke: Spoke, colors: LineColors): { stroke: string; strokeDasharray?: string } {
  if (spoke.peer?.online === false) return { stroke: colors.stale, strokeDasharray: '5 4' }
  switch (spoke.entry.route.via) {
    case 'direct':
      return { stroke: colors.direct }
    case 'relay':
      return { stroke: colors.relay }
    case 'none':
      return { stroke: colors.none, strokeDasharray: '2 4' }
  }
}

function StarLegend({ colors }: { colors: LineColors }) {
  const items: [string, string, string | undefined][] = [
    [d.routesP2P, colors.direct, undefined],
    [d.routesRelay, colors.relay, undefined],
    [d.viaNone, colors.none, '2 4'],
    [t.topology.legendOffline, colors.stale, '5 4'],
  ]
  return (
    <ul
      className="flex flex-wrap items-center gap-x-5 gap-y-1.5 text-xs text-muted-foreground"
      aria-label={t.routes.legendLabel}
    >
      {items.map(([label, color, dash]) => (
        <li key={label} className="flex items-center gap-1.5">
          <LineSample color={color} dash={dash} />
          {label}
        </li>
      ))}
    </ul>
  )
}

/**
 * 路由的星状图：这个节点在正中，对端围成一圈，一个对端一条线，按现在走的那条路画：P2P 绿、经中继紫、
 * 不可达红色点线，对端离线时灰色虚线；线上写延迟。经别的中继转的，框里写下一跳。
 * 点一个对端，去它的路由星状图：换它在中间。
 */
export function RouteStar({
  network,
  node,
  spokes,
  nodes,
}: {
  network: Network
  node: NodeDetail
  spokes: readonly Spoke[]
  nodes: readonly Node[]
}) {
  const locale = useLocale()
  const colors = useLineColors()

  const flowNodes = useMemo<(CenterFlowNode | PeerFlowNode)[]>(() => {
    const { rx, ry } = ring(spokes.length)
    const center: CenterFlowNode = {
      id: 'center',
      type: 'center',
      position: { x: -CENTER.width / 2, y: -CENTER.height / 2 },
      data: { node },
      draggable: false,
      selectable: false,
      focusable: false,
    }
    const peers = spokes.map((spoke, index): PeerFlowNode => {
      const angle = -Math.PI / 2 + (2 * Math.PI * index) / spokes.length
      const hop = spoke.middle?.[0]
      return {
        id: `peer-${spoke.entry.peer.id}`,
        type: 'peer',
        position: { x: rx * Math.cos(angle) - PEER.width / 2, y: ry * Math.sin(angle) - PEER.height / 2 },
        data: {
          spoke,
          via: hop === undefined ? '' : d.routesViaChain(nodeName(nodes, hop)),
          label: d.routesStarPeer(spoke.entry.peer.name, describe(spoke, nodes)),
          to: `${networkPath(network.id, `nodes/${spoke.entry.peer.id}`)}?tab=routes&view=star`,
          protocol: protocolText(spoke.entry.route, spoke.leg),
        },
        draggable: false,
        selectable: false,
        // 框本身是个链接，键盘停在链接上；画图的库自己的那一层不再聚焦。
        focusable: false,
      }
    })
    return [center, ...peers]
    // 语言变了框里的字要换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [network.id, node, spokes, nodes, locale])

  const edges = useMemo<LineFlowEdge[]>(
    () =>
      spokes.map((spoke) => {
        const { route, rtt_ms: rtt } = spoke.entry
        const reachable = route.via !== 'none' && spoke.peer?.online !== false
        return {
          id: `line-${spoke.entry.peer.id}`,
          type: 'line',
          source: 'center',
          target: `peer-${spoke.entry.peer.id}`,
          data: {
            index: 0,
            siblings: 1,
            label: reachable && rtt !== null ? t.common.ms(rtt) : null,
            strong: true,
          },
          style: { ...lineStyle(spoke, colors), strokeWidth: 1.8, opacity: reachable ? 0.9 : 0.7 },
          focusable: false,
        }
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [spokes, colors, locale],
  )

  // 对端换了一批（多了、少了）时重新排、重新缩放；只是数变了不动。
  const signature = spokes.map((spoke) => spoke.entry.peer.id).join(',')

  return (
    <div className="flex flex-1 flex-col gap-3">
      <Canvas data-testid="routes-star">
        <GraphCanvas<CenterFlowNode | PeerFlowNode, LineFlowEdge>
          key={signature}
          label={d.routesStar(node.name)}
          nodes={flowNodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          nodesDraggable={false}
          elementsSelectable={false}
        />
      </Canvas>
      <StarLegend colors={colors} />
    </div>
  )
}
