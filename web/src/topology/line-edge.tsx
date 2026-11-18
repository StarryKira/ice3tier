import {
  BaseEdge,
  type Edge,
  EdgeLabelRenderer,
  type EdgeProps,
  Handle,
  Position,
  useInternalNode,
} from '@xyflow/react'

import { curveBetween, type Rect } from '@/access/geometry'
import { cn } from '@/lib/utils'

/** 一条线：同一对点之间的第几条（几条时各自弯开）、线上写的字、字要不要显眼。 */
export interface LineData extends Record<string, unknown> {
  index: number
  siblings: number
  label: string | null
  strong: boolean
}

export type LineFlowEdge = Edge<LineData, 'line'>

/** 连线不从固定的接口出发，而是按两个框的位置算，所以接口藏在框的正中，只是让画图的库认这条线。 */
const HIDDEN_HANDLE = { left: '50%', top: '50%', opacity: 0, pointerEvents: 'none' } as const

/** 放在每个点里：让画图的库认得连到它的线。 */
export function HiddenHandles() {
  return (
    <>
      <Handle type="target" position={Position.Top} style={HIDDEN_HANDLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} style={HIDDEN_HANDLE} isConnectable={false} />
    </>
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

/** 从一个框的边连到另一个框的边；字写在线的中点上。 */
export function LineEdgeView({ id, source, target, data, markerEnd, style }: EdgeProps<LineFlowEdge>) {
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

/** 图例里的一小段线：颜色、虚线的样子、粗细和图上的线一样。 */
export function LineSample({ color, dash, width = 1.6 }: { color: string; dash?: string; width?: number }) {
  return (
    <svg width="28" height="8" aria-hidden className="shrink-0">
      <line x1="1" y1="4" x2="27" y2="4" stroke={color} strokeWidth={width} strokeDasharray={dash} />
    </svg>
  )
}
