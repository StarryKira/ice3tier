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
  useNodesState,
} from '@xyflow/react'
import { Clock } from 'lucide-react'
import { useEffect, useMemo } from 'react'

import { type AccessGraph } from '@/api/types'
import { curveBetween, loopAround, type Rect } from '@/access/geometry'
import { buildTagGraph, layoutSignature, type RuleLine, TAG_NODE, type TagPoint, UNTAGGED_ID } from '@/access/tagGraph'
import { ruleLabel } from '@/lib/rules'
import { t, useLocale } from '@/i18n'
import { cn } from '@/lib/utils'

import { GraphCanvas } from './flow'
import { useEdgeColors } from '@/access/colors'

const g = t.access.graph

type TagFlowNode = Node<{ point: TagPoint; active: boolean }, 'tag'>
type RuleFlowEdge = Edge<{ line: RuleLine; highlighted: boolean; showLabel: boolean }, 'rule'>

/** 连线不从固定的接口出发，而是按两个点的位置算，所以接口藏在点的正中，只是让画图的库认这条线。 */
const HIDDEN_HANDLE = { left: '50%', top: '50%', opacity: 0, pointerEvents: 'none' } as const

function TagNodeView({ data }: NodeProps<TagFlowNode>) {
  const { point, active } = data
  const untagged = point.kind === 'untagged'
  return (
    <div
      style={{ width: TAG_NODE.width, height: TAG_NODE.height }}
      className={cn(
        'flex flex-col justify-center rounded-lg border border-border-strong bg-card px-3.5 transition-colors',
        untagged && 'border-dashed bg-transparent',
        active && 'border-primary outline-2 outline-offset-0 outline-primary',
      )}
    >
      <Handle type="target" position={Position.Top} style={HIDDEN_HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} style={HIDDEN_HANDLE} isConnectable={false} />
      <span className="truncate text-sm font-semibold">{untagged ? g.untagged(point.members.length) : point.tag}</span>
      <span className="flex items-center gap-1.5 text-xs text-muted-foreground tabular">
        {untagged ? g.untaggedSub : g.members(point.members.length)}
        {point.pending > 0 ? (
          <span className="inline-flex items-center gap-0.5 text-warn" title={g.unackedBadge(point.pending)}>
            <Clock className="size-3" aria-hidden />
            {point.pending}
          </span>
        ) : null}
      </span>
    </div>
  )
}

function rectOf(node: ReturnType<typeof useInternalNode>): Rect | null {
  if (node === undefined) return null
  return {
    x: node.internals.positionAbsolute.x,
    y: node.internals.positionAbsolute.y,
    width: node.measured.width ?? TAG_NODE.width,
    height: node.measured.height ?? TAG_NODE.height,
  }
}

function RuleEdgeView({ id, source, target, data, markerStart, markerEnd, style }: EdgeProps<RuleFlowEdge>) {
  const sourceRect = rectOf(useInternalNode(source))
  const targetRect = rectOf(useInternalNode(target))
  if (sourceRect === null || targetRect === null || data === undefined) return null
  const { line, highlighted, showLabel } = data
  const path = line.loop
    ? loopAround(sourceRect, line.index)
    : curveBetween(sourceRect, targetRect, line.index, line.siblings)
  return (
    <>
      <BaseEdge
        id={id}
        path={path.d}
        markerStart={markerStart}
        markerEnd={markerEnd}
        style={style}
        interactionWidth={22}
      />
      {line.name !== '' && (showLabel || highlighted) ? (
        <EdgeLabelRenderer>
          <span
            style={{ transform: `translate(-50%, -50%) translate(${path.label.x}px, ${path.label.y}px)` }}
            className={cn(
              'pointer-events-none absolute rounded-sm bg-sidebar px-1 text-[11px] leading-4',
              highlighted
                ? 'font-medium text-primary'
                : line.enabled
                  ? 'text-muted-foreground'
                  : 'text-muted-foreground/70',
            )}
          >
            {line.name}
          </span>
        </EdgeLabelRenderer>
      ) : null}
    </>
  )
}

const nodeTypes = { tag: TagNodeView }
const edgeTypes = { rule: RuleEdgeView }

/** 线不多时名字一直显示；多了只在选中或鼠标停着时显示，免得糊成一片。 */
const ALWAYS_LABEL_UP_TO = 10

interface TagGraphViewProps {
  graph: AccessGraph
  highlightedRule: number | null
  activeTag: string | null
  onRuleClick: (ruleId: number) => void
  onRuleOpen: (ruleId: number) => void
  onRuleHover: (ruleId: number | null) => void
  onTagClick: (tag: string) => void
}

/**
 * 访问关系图的标签视图：每个标签是一个点，每条规则是一条线。
 * 双向规则两头都有箭头，单向规则只有指向目的标签的箭头，停用的规则画虚线：颜色之外总有线型和箭头。
 */
export function TagGraphView({
  graph,
  highlightedRule,
  activeTag,
  onRuleClick,
  onRuleOpen,
  onRuleHover,
  onTagClick,
}: TagGraphViewProps) {
  // 点和线上的文字（读屏的说明）在下面算好存着，换语言时要重新算。
  const locale = useLocale()
  const colors = useEdgeColors()
  const shape = useMemo(() => buildTagGraph(graph), [graph])
  const signature = layoutSignature(shape)

  const toNodes = (current: readonly TagFlowNode[]): TagFlowNode[] =>
    shape.points.map((point) => ({
      id: point.id,
      type: 'tag',
      // 用户拖动过的位置保留着；标签的集合变了（signature 变了）才重新排。
      position: current.find((node) => node.id === point.id)?.position ?? { x: point.x, y: point.y },
      data: { point, active: point.kind === 'tag' && point.tag === activeTag },
      ariaLabel:
        point.kind === 'untagged' ? g.untagged(point.members.length) : g.tagNodeLabel(point.tag, point.members.length),
      selectable: point.id !== UNTAGGED_ID,
    }))

  const [nodes, setNodes, onNodesChange] = useNodesState<TagFlowNode>(toNodes([]))

  useEffect(() => {
    setNodes((current) => toNodes(current))
    // toNodes 用到的都列在下面了；语言变了，点上的说明也要换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape, activeTag, setNodes, locale])

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
          data: { line, highlighted, showLabel: shape.lines.length <= ALWAYS_LABEL_UP_TO },
          markerEnd: marker,
          markerStart: line.direction === 'both' ? marker : undefined,
          style: {
            stroke: color,
            strokeWidth: highlighted ? 2.4 : 1.5,
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
    <GraphCanvas<TagFlowNode, RuleFlowEdge>
      key={signature}
      label={g.canvasTag}
      nodes={nodes}
      edges={edges}
      onNodesChange={onNodesChange}
      nodeTypes={nodeTypes}
      edgeTypes={edgeTypes}
      onEdgeClick={(_event, edge) => edge.data !== undefined && onRuleClick(edge.data.line.ruleId)}
      onEdgeDoubleClick={(_event, edge) => edge.data !== undefined && onRuleOpen(edge.data.line.ruleId)}
      onEdgeMouseEnter={(_event, edge) => edge.data !== undefined && onRuleHover(edge.data.line.ruleId)}
      onEdgeMouseLeave={() => onRuleHover(null)}
      onNodeClick={(_event, node) => node.data.point.kind === 'tag' && onTagClick(node.data.point.tag)}
      onEdgesChange={(changes) => {
        // 用键盘选中一条线（聚焦后按回车）时，和点击一样让对应的规则亮起来。
        for (const change of changes) {
          if (change.type !== 'select' || !change.selected) continue
          const line = shape.lines.find((candidate) => candidate.id === change.id)
          if (line !== undefined) onRuleClick(line.ruleId)
        }
      }}
    />
  )
}
