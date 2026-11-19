import {
  BaseEdge,
  type Edge,
  EdgeLabelRenderer,
  type EdgeProps,
  Handle,
  Position,
  useInternalNode,
} from '@xyflow/react'

import { type ReactNode } from 'react'

import { curveBetween, curveOf, pointOnCurve, type Rect } from '@/access/geometry'
import { Tooltip } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'

/** 线上的字分几段：协议、经过的中继、延迟，各有各的颜色。 */
export interface LabelPart {
  text: string
  tone?: 'ok' | 'relay' | 'lane' | 'warn'
}

const PART_TONE = { ok: 'text-ok', relay: 'text-relay', lane: 'text-lane', warn: 'text-warn' } as const

/** 一条线：同一对点之间的第几条（几条时各自弯开）、线上写的字、字要不要显眼。 */
export interface LineData extends Record<string, unknown> {
  index: number
  siblings: number
  label: string | null
  strong: boolean
  /** 绕开别的卡片要弯多少（`bendAround`）；没有是直线。 */
  bend?: number
  /**
   * 线上跑的小光点：`both` 两个方向各一个（两头在通信），`forward` 只从 source 跑向 target（选中那一对的方向），
   * 没有就不跑（过期、不可达）。颜色是 `flowColor`。
   */
  flow?: 'both' | 'forward'
  flowColor?: string
  /** 光点按这个长度定跑一趟多久（排好时两个点的距离）；没有就按现在画出来的长度。 */
  length?: number
  /** 分段的字，有它时不用 `label`。 */
  parts?: LabelPart[]
  /** 悬停在字上看的详细说明。 */
  tip?: ReactNode
  /** 字放在线上的哪里（0 是起点、1 是终点，见 `placeLabels`）；没有就在正中。 */
  labelAt?: number
  /** 鼠标移到字上、移开：和停在线上一样，让这条线亮起来。 */
  onHover?: (on: boolean) => void
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

/** 同一条线每次画都在同一个时刻起跑，不同的线错开：按编号算出一个 0 到 1 之间的数。 */
function phaseOf(id: string): number {
  let hash = 0
  for (const char of id) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  return (hash % 997) / 997
}

/** 一个沿线跑的小光点：两头淡入淡出，速度固定（长线跑得久）。 */
function Spark({
  d,
  color,
  seconds,
  phase,
  reverse,
  size,
}: {
  d: string
  color: string
  seconds: number
  phase: number
  reverse: boolean
  size: number
}) {
  const timing = { dur: `${seconds}s`, begin: `-${(phase * seconds).toFixed(2)}s`, repeatCount: 'indefinite' }
  return (
    <circle r={size} fill={color} opacity={0}>
      <animateMotion path={d} {...timing} calcMode="linear" keyPoints={reverse ? '1;0' : '0;1'} keyTimes="0;1" />
      <animate attributeName="opacity" values="0;1;1;0" keyTimes="0;0.15;0.85;1" {...timing} />
    </circle>
  )
}

/** 从一个框的边连到另一个框的边；字写在线的中点上，线上可以跑小光点。 */
export function LineEdgeView({ id, source, target, data, markerEnd, style }: EdgeProps<LineFlowEdge>) {
  const sourceRect = rectOf(useInternalNode(source))
  const targetRect = rectOf(useInternalNode(target))
  if (sourceRect === null || targetRect === null || data === undefined) return null
  const path = curveBetween(sourceRect, targetRect, data.index, data.siblings, data.bend ?? 0)
  const at =
    data.labelAt === undefined
      ? path.label
      : pointOnCurve(curveOf(sourceRect, targetRect, data.index, data.siblings, data.bend ?? 0), data.labelAt)
  const length =
    data.length ??
    Math.hypot(
      targetRect.x + targetRect.width / 2 - (sourceRect.x + sourceRect.width / 2),
      targetRect.y + targetRect.height / 2 - (sourceRect.y + sourceRect.height / 2),
    )
  // 选中那一对的路跑得快一些、点大一些。
  const seconds = Math.max(1.4, Math.round((length / (data.strong ? 170 : 110)) * 10) / 10)
  const phase = phaseOf(id)
  const color = data.flowColor ?? String(style?.stroke ?? 'currentColor')
  return (
    <>
      <BaseEdge
        id={id}
        path={path.d}
        markerEnd={markerEnd}
        style={{ strokeLinecap: 'round', ...style }}
        interactionWidth={14}
      />
      {data.flow !== undefined ? (
        // 只是装饰：读屏不念；系统里关了动画时不跑。
        <g aria-hidden className="pointer-events-none motion-reduce:hidden">
          <Spark
            d={path.d}
            color={color}
            seconds={seconds}
            phase={phase}
            reverse={false}
            size={data.strong ? 3.2 : 2.4}
          />
          {data.flow === 'both' ? (
            <Spark
              d={path.d}
              color={color}
              seconds={seconds}
              phase={(phase + 0.5) % 1}
              reverse
              size={data.strong ? 3.2 : 2.4}
            />
          ) : null}
        </g>
      ) : null}
      {data.label !== null || (data.parts?.length ?? 0) > 0 ? (
        <EdgeLabelRenderer>
          <Tooltip content={data.tip}>
            <span
              style={{ transform: `translate(-50%, -50%) translate(${at.x}px, ${at.y}px)` }}
              tabIndex={data.tip === undefined ? undefined : -1}
              onMouseEnter={() => data.onHover?.(true)}
              onMouseLeave={() => data.onHover?.(false)}
              className={cn(
                'absolute inline-flex items-center gap-1 rounded-full border bg-card px-1.5 text-[11px] leading-[18px] whitespace-nowrap tabular shadow-xs',
                data.tip === undefined ? 'pointer-events-none' : 'nodrag nopan pointer-events-auto cursor-default',
                data.strong ? 'border-border-strong font-medium text-foreground' : 'text-muted-foreground',
              )}
            >
              {data.parts !== undefined
                ? data.parts.map((part, index) => (
                    <span
                      key={index}
                      className={cn(part.tone !== undefined && cn('font-medium', PART_TONE[part.tone]))}
                    >
                      {part.text}
                    </span>
                  ))
                : data.label}
            </span>
          </Tooltip>
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
