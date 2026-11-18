import {
  BaseEdge,
  type Edge,
  EdgeLabelRenderer,
  type EdgeProps,
  Handle,
  MarkerType,
  type Node,
  type NodeProps,
  Panel,
  Position,
  useInternalNode,
  useNodesState,
} from '@xyflow/react'
import { Boxes, Clock } from 'lucide-react'
import { useEffect, useMemo } from 'react'

import { type AccessGraph, type GroupId, type NodeId } from '@/api/types'
import { useEdgeColors } from '@/access/colors'
import { buildGroupGraph, GROUP_NODE, type GroupPoint, layoutSignature, type RuleLine } from '@/access/groupGraph'
import { InfoTip } from '@/components/info-tip'
import { t, useLocale } from '@/i18n'
import { formatList } from '@/lib/format'
import { ruleLabel } from '@/lib/rules'
import { cn } from '@/lib/utils'

import { GraphCanvas } from './flow'

const g = t.access.graph

// 不叫 `group`：那是画图的库自带的一种点，会带上它自己的框和底色。
type CardNode = Node<
  {
    point: GroupPoint
    active: boolean
    online: (id: NodeId) => boolean | undefined
  },
  'groupCard'
>
type HeadNode = Node<{ label: string }, 'columnHead'>
type GroupFlowNode = CardNode | HeadNode
type RuleFlowEdge = Edge<{ line: RuleLine; highlighted: boolean }, 'rule'>

/** 线从卡片的左右两边出入，接口只是让画图的库认这条线，藏起来。 */
const HIDDEN_HANDLE = { opacity: 0, pointerEvents: 'none' } as const

/**
 * 一个分组是一张卡片，和节点详情页顶上的样子一致：左边一个圆角的图标块（内置的 All 是虚线框），
 * 右边是名字和一行灰字（几个节点、几个在线、几个还没确认新配置）。
 */
function GroupCard({ data }: NodeProps<CardNode>) {
  const { point, active, online } = data
  const up = point.members.filter((member) => online(member.id) !== false).length
  return (
    <div
      style={{ width: GROUP_NODE.width }}
      className={cn(
        'flex items-center gap-3 rounded-xl border bg-card px-3 py-2.5 shadow-xs transition-[border-color,box-shadow]',
        'hover:border-border-strong',
        active && 'border-primary ring-3 ring-primary/15',
      )}
    >
      <Handle type="target" position={Position.Left} style={HIDDEN_HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Right} style={HIDDEN_HANDLE} isConnectable={false} />
      <span
        aria-hidden
        className={cn(
          'flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground/70',
          point.group.builtin && 'border border-dashed border-border-strong bg-card',
        )}
      >
        <Boxes className="size-4" />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-sm leading-5 font-semibold">{point.group.name}</span>
        <span className="flex items-center gap-1.5 text-xs leading-4 whitespace-nowrap text-muted-foreground tabular">
          <span>{g.members(point.members.length)}</span>
          {point.members.length > 0 ? (
            <>
              <span aria-hidden>·</span>
              <span className="inline-flex items-center gap-1">
                <span aria-hidden className="size-1.5 rounded-full bg-ok-dot" />
                {g.online(up)}
              </span>
            </>
          ) : null}
          {point.pending > 0 ? (
            <span className="inline-flex items-center gap-0.5 text-warn" title={g.unackedBadge(point.pending)}>
              <Clock className="size-3" aria-hidden />
              {point.pending}
            </span>
          ) : null}
        </span>
      </span>
    </div>
  )
}

/** 栏的名字：来源、目标。 */
function ColumnHead({ data }: NodeProps<HeadNode>) {
  return (
    <div style={{ width: GROUP_NODE.width }} className="px-1 text-xs font-medium text-muted-foreground">
      {data.label}
    </div>
  )
}

function boxOf(node: ReturnType<typeof useInternalNode>) {
  if (node === undefined) return null
  return {
    x: node.internals.positionAbsolute.x,
    y: node.internals.positionAbsolute.y,
    width: node.measured.width ?? GROUP_NODE.width,
    height: node.measured.height ?? GROUP_NODE.height,
  }
}

/**
 * 一根从左栏卡片右边到右栏卡片左边的线（水平出入的三次曲线）。策略的名字写在线的中间，
 * 是一个小小的胶囊；选中时变成主色。
 */
function RuleEdgeView({ id, source, target, data, markerStart, markerEnd, style }: EdgeProps<RuleFlowEdge>) {
  const from = boxOf(useInternalNode(source))
  const to = boxOf(useInternalNode(target))
  if (from === null || to === null || data === undefined) return null
  const { line, highlighted } = data
  const sx = from.x + from.width
  const sy = from.y + from.height / 2 + line.sourceOffset
  const tx = to.x
  const ty = to.y + to.height / 2 + line.targetOffset
  const bend = Math.max(40, (tx - sx) / 2)
  const d = `M ${sx} ${sy} C ${sx + bend} ${sy}, ${tx - bend} ${ty}, ${tx} ${ty}`
  const label = { x: (sx + tx) / 2, y: (sy + ty) / 2 }
  const text = line.name === '' ? ruleLabel(line) : line.name
  return (
    <>
      <BaseEdge id={id} path={d} markerStart={markerStart} markerEnd={markerEnd} style={style} interactionWidth={22} />
      <EdgeLabelRenderer>
        <span
          style={{ transform: `translate(-50%, -50%) translate(${label.x}px, ${label.y}px)` }}
          className={cn(
            'pointer-events-none absolute rounded-full border bg-card px-2 text-[11px] leading-[18px] whitespace-nowrap',
            highlighted
              ? 'border-primary font-medium text-primary'
              : line.enabled
                ? 'text-foreground/80'
                : 'border-dashed text-muted-foreground',
          )}
        >
          {text}
        </span>
      </EdgeLabelRenderer>
    </>
  )
}

/** 图例：画在画布的右上角。线型的样子是小段 SVG，和图上的线一样画。 */
function Legend() {
  const sample = (dashed: boolean, both: boolean) => (
    <svg width="34" height="10" viewBox="0 0 34 10" aria-hidden className="text-foreground/60">
      {both ? <path d="M1 5 L7 2 L7 8 Z" fill="currentColor" /> : null}
      <line
        x1={both ? 6 : 1}
        y1="5"
        x2="28"
        y2="5"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeDasharray={dashed ? '4 3' : undefined}
      />
      {dashed ? null : <path d="M33 5 L27 2 L27 8 Z" fill="currentColor" />}
    </svg>
  )
  return (
    <Panel position="top-right">
      <div className="flex items-center gap-3.5 rounded-md border bg-card/90 px-3 py-1.5 text-xs text-muted-foreground shadow-xs backdrop-blur-sm">
        <span className="inline-flex items-center gap-1.5">
          {sample(false, true)}
          {g.legendBoth}
        </span>
        <span className="inline-flex items-center gap-1.5">
          {sample(false, false)}
          {g.legendOneWay}
        </span>
        <span className="inline-flex items-center gap-1.5">
          {sample(true, false)}
          {g.legendDisabled}
        </span>
        <InfoTip>{g.edgeHint}</InfoTip>
      </div>
    </Panel>
  )
}

const nodeTypes = { groupCard: GroupCard, columnHead: ColumnHead }
const edgeTypes = { rule: RuleEdgeView }

interface GroupGraphViewProps {
  graph: AccessGraph
  /** 节点在不在线：卡片上数在线的用。 */
  online: (id: NodeId) => boolean | undefined
  highlightedRule: number | null
  activeGroup: GroupId | null
  onRuleClick: (ruleId: number) => void
  onRuleOpen: (ruleId: number) => void
  onRuleHover: (ruleId: number | null) => void
  onGroupClick: (group: GroupId) => void
}

/**
 * 关系图的按分组看：左栏来源分组，右栏目标分组，每条策略一根从左到右的线。
 * 双向的两头都有箭头，单向的只指向目标，停用的是虚线：颜色之外总有线型和箭头。
 */
export function GroupGraphView({
  graph,
  online,
  highlightedRule,
  activeGroup,
  onRuleClick,
  onRuleOpen,
  onRuleHover,
  onGroupClick,
}: GroupGraphViewProps) {
  // 卡片上的文字（读屏的说明）在下面算好存着，换语言时要重新算。
  const locale = useLocale()
  const colors = useEdgeColors()
  const shape = useMemo(() => buildGroupGraph(graph), [graph])
  const signature = layoutSignature(shape)

  const toNodes = (current: readonly GroupFlowNode[]): GroupFlowNode[] => {
    const heads: GroupFlowNode[] = []
    const top = Math.min(...shape.points.map((point) => point.y))
    if (shape.points.length > 0) {
      for (const side of ['src', 'dst'] as const) {
        const first = shape.points.find((point) => point.side === side)
        if (first === undefined) continue
        heads.push({
          id: `head:${side}`,
          type: 'columnHead',
          position: { x: first.x, y: top - 28 },
          data: { label: side === 'src' ? g.columnSource : g.columnTarget },
          draggable: false,
          selectable: false,
          focusable: false,
        })
      }
    }
    const cards: GroupFlowNode[] = shape.points.map((point) => ({
      id: point.id,
      type: 'groupCard',
      // 用户拖动过的位置保留着；两栏里的卡片变了（signature 变了）才重新排。
      position: current.find((node) => node.id === point.id)?.position ?? { x: point.x, y: point.y },
      data: { point, active: point.group.id === activeGroup, online },
      ariaLabel: g.groupNodeLabel(point.group.name, point.members.length),
    }))
    return [...heads, ...cards]
  }

  const [nodes, setNodes, onNodesChange] = useNodesState<GroupFlowNode>(toNodes([]))

  useEffect(() => {
    setNodes((current) => toNodes(current))
    // toNodes 用到的都列在下面了；语言变了，卡片上的说明也要换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape, activeGroup, online, setNodes, locale])

  useEffect(() => {
    setNodes(toNodes([]))
    // 只在布局的指纹变化时重排。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature])

  const edges = useMemo<RuleFlowEdge[]>(
    () =>
      shape.lines.map((line) => {
        const highlighted = line.ruleId === highlightedRule
        const color = highlighted ? colors.highlighted : line.enabled ? colors.enabled : colors.disabled
        const marker = { type: MarkerType.ArrowClosed, color, width: 16, height: 16 }
        return {
          id: line.id,
          type: 'rule',
          source: line.source,
          target: line.target,
          data: { line, highlighted },
          markerEnd: marker,
          markerStart: line.direction === 'both' ? marker : undefined,
          style: {
            stroke: color,
            strokeWidth: highlighted ? 2.2 : 1.5,
            strokeDasharray: line.enabled ? undefined : '6 5',
          },
          zIndex: highlighted ? 10 : 0,
          ariaLabel: g.edgeLabel(ruleLabel(line), line.enabled ? g.edgeEnabled : g.edgeDisabled),
        }
      }),
    // 线上的说明来自目录：语言变了要重新算。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [shape.lines, highlightedRule, locale, colors],
  )

  return (
    <GraphCanvas<GroupFlowNode, RuleFlowEdge>
      key={signature}
      label={g.canvasGroup}
      nodes={nodes}
      edges={edges}
      onNodesChange={onNodesChange}
      nodeTypes={nodeTypes}
      edgeTypes={edgeTypes}
      onEdgeClick={(_event, edge) => edge.data !== undefined && onRuleClick(edge.data.line.ruleId)}
      onEdgeDoubleClick={(_event, edge) => edge.data !== undefined && onRuleOpen(edge.data.line.ruleId)}
      onEdgeMouseEnter={(_event, edge) => edge.data !== undefined && onRuleHover(edge.data.line.ruleId)}
      onEdgeMouseLeave={() => onRuleHover(null)}
      onNodeClick={(_event, node) => node.type === 'groupCard' && onGroupClick(node.data.point.group.id)}
      onEdgesChange={(changes) => {
        // 用键盘选中一条线（聚焦后按回车）时，和点击一样让对应的策略亮起来。
        for (const change of changes) {
          if (change.type !== 'select' || !change.selected) continue
          const line = shape.lines.find((candidate) => candidate.id === change.id)
          if (line !== undefined) onRuleClick(line.ruleId)
        }
      }}
    >
      <Legend />
      {shape.unused.length > 0 ? (
        <Panel position="bottom-left">
          <p className="rounded-md border bg-card/90 px-3 py-1.5 text-xs text-muted-foreground shadow-xs backdrop-blur-sm">
            {g.unusedGroups(formatList(shape.unused.map((group) => group.name)))}
          </p>
        </Panel>
      ) : null}
    </GraphCanvas>
  )
}
