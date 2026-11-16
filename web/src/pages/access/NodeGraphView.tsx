import { type Edge, Handle, MarkerType, type Node, type NodeProps, Position } from '@xyflow/react'
import { useMemo } from 'react'

import { type NodeAccess, type NodeRef, type Rule } from '@/api/types'
import { buildNodeGraph, NODE_BOX } from '@/access/nodeGraph'
import { ruleLabel } from '@/lib/rules'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'

import { GraphCanvas } from './flow'
import { EDGE_COLORS } from '@/access/colors'

const g = t.access.graph

type PeerFlowNode = Node<{ node: NodeRef; side: 'left' | 'right' }, 'peer'>
type FocusFlowNode = Node<{ node: NodeRef }, 'focus'>
type MoreFlowNode = Node<{ hidden: number; side: 'left' | 'right' }, 'more'>
type FlowNode = PeerFlowNode | FocusFlowNode | MoreFlowNode

const BOX = { width: NODE_BOX.width, height: NODE_BOX.height }
const HANDLE = { opacity: 0, pointerEvents: 'none' } as const

function NodeBody({ node, emphasized = false }: { node: NodeRef; emphasized?: boolean }) {
  return (
    <>
      <span className={cn('truncate text-sm', emphasized ? 'font-semibold' : 'font-medium')}>{node.name}</span>
      <span className="truncate font-mono text-[11px] text-muted-foreground">{node.virtual_ip}</span>
    </>
  )
}

function PeerNodeView({ data }: NodeProps<PeerFlowNode>) {
  return (
    <div
      style={BOX}
      className="flex cursor-pointer flex-col justify-center rounded-lg border bg-card px-3 shadow-card hover:border-primary/60"
    >
      <Handle type="target" position={Position.Left} style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Right} style={HANDLE} isConnectable={false} />
      <NodeBody node={data.node} />
    </div>
  )
}

function FocusNodeView({ data }: NodeProps<FocusFlowNode>) {
  return (
    <div style={BOX} className="flex flex-col justify-center rounded-lg border-2 border-primary bg-accent px-3">
      <Handle type="target" position={Position.Left} style={HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Right} style={HANDLE} isConnectable={false} />
      <NodeBody node={data.node} emphasized />
    </div>
  )
}

function MoreNodeView({ data }: NodeProps<MoreFlowNode>) {
  return (
    <div
      style={BOX}
      className="flex cursor-pointer items-center justify-center rounded-lg border border-dashed border-border-strong text-[13px] text-muted-foreground hover:bg-muted"
    >
      {g.more(data.hidden)}
    </div>
  )
}

const nodeTypes = { peer: PeerNodeView, focus: FocusNodeView, more: MoreNodeView }

/** 连线上的名字：最多写两条规则，再多写"等 N 条"。 */
function edgeText(rules: readonly Rule[]): string {
  const labels = rules.map(ruleLabel)
  return labels.length <= 2 ? labels.join('、') : `${labels.slice(0, 2).join('、')} +${labels.length - 2}`
}

interface NodeGraphViewProps {
  access: NodeAccess
  rules: readonly Rule[]
  expanded: { left: boolean; right: boolean }
  onExpand: (side: 'left' | 'right') => void
  /** 点两侧的某个节点：把它换成焦点。 */
  onFocus: (node: NodeRef) => void
}

/**
 * 访问关系图的节点视图：从一个焦点出发，三列——能访问它的节点 → 它 → 它能访问的节点。
 * 图不把所有东西都铺开，只回答"这台机器能到哪、谁能到它"。
 */
export function NodeGraphView({ access, rules, expanded, onExpand, onFocus }: NodeGraphViewProps) {
  const shape = useMemo(() => buildNodeGraph(access, rules, expanded), [access, rules, expanded])

  const nodes = useMemo<FlowNode[]>(
    () => [
      {
        id: shape.focus.id,
        type: 'focus',
        position: { x: shape.focus.x, y: shape.focus.y - BOX.height / 2 },
        data: { node: shape.focus.node },
        ariaLabel: g.focusLabel(shape.focus.node.name),
        draggable: false,
      },
      ...shape.peers.map<PeerFlowNode>((peer) => ({
        id: peer.id,
        type: 'peer',
        position: { x: peer.x, y: peer.y - BOX.height / 2 },
        data: { node: peer.node, side: peer.side },
        ariaLabel: `${peer.node.name}，${g.viaRules(peer.rules.map(ruleLabel).join('、'))}`,
        draggable: false,
      })),
      ...shape.more.map<MoreFlowNode>((more) => ({
        id: more.id,
        type: 'more',
        position: { x: more.x, y: more.y - BOX.height / 2 },
        data: { hidden: more.hidden, side: more.side },
        ariaLabel: g.more(more.hidden),
        draggable: false,
      })),
    ],
    [shape],
  )

  const edges = useMemo<Edge[]>(
    () =>
      shape.peers.map((peer) => ({
        id: `edge:${peer.id}`,
        // 箭头的方向就是访问的方向：左边的指向焦点，焦点指向右边的。
        source: peer.side === 'left' ? peer.id : shape.focus.id,
        target: peer.side === 'left' ? shape.focus.id : peer.id,
        label: edgeText(peer.rules),
        labelStyle: { fontSize: 12, fill: EDGE_COLORS.enabled },
        labelBgPadding: [4, 2] as [number, number],
        labelBgBorderRadius: 3,
        labelBgStyle: { fill: '#f4f6f8' },
        markerEnd: { type: MarkerType.ArrowClosed, color: EDGE_COLORS.enabled, width: 16, height: 16 },
        style: { stroke: EDGE_COLORS.enabled, strokeWidth: 1.4 },
        focusable: false,
      })),
    [shape],
  )

  return (
    <GraphCanvas<FlowNode, Edge>
      key={`${access.node.id}:${shape.peers.length}:${shape.more.length}`}
      label={g.canvasNode}
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      nodesDraggable={false}
      elementsSelectable
      onNodeClick={(_event, node) => {
        if (node.type === 'more') onExpand(node.data.side)
        else if (node.type === 'peer') onFocus(node.data.node)
      }}
      onNodesChange={(changes) => {
        // 用键盘选中（聚焦后按回车）和点击是一回事。
        for (const change of changes) {
          if (change.type !== 'select' || !change.selected) continue
          const node = nodes.find((candidate) => candidate.id === change.id)
          if (node?.type === 'more') onExpand(node.data.side)
          else if (node?.type === 'peer') onFocus(node.data.node)
        }
      }}
    />
  )
}
