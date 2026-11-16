import { type AccessEdge, type NodeAccess, type NodeRef, type Rule } from '@/api/types'

/**
 * "按节点"视图要画的东西：三列，能访问它的节点 → 它 → 它能访问的节点。连线上标着是哪条规则允许的。
 * 每一侧最多画几个，其余折成"还有 N 个"，点了展开：图太密就什么都看不出来了。
 */

export const SIDE_LIMIT = 6

export interface PeerBox {
  id: string
  side: 'left' | 'right'
  node: NodeRef
  /** 允许这条访问的规则，按规则列表的顺序。 */
  rules: Rule[]
  x: number
  y: number
}

export interface MoreBox {
  id: string
  side: 'left' | 'right'
  hidden: number
  x: number
  y: number
}

export interface NodeGraphShape {
  focus: { id: string; node: NodeRef; x: number; y: number }
  peers: PeerBox[]
  more: MoreBox[]
  /** 两侧各有多少个节点（包括折起来的）。 */
  totals: { left: number; right: number }
}

export const NODE_BOX = { width: 150, height: 46, gapY: 14, gapX: 118 } as const

const ROW = NODE_BOX.height + NODE_BOX.gapY

function column(
  side: 'left' | 'right',
  edges: readonly AccessEdge[],
  rules: readonly Rule[],
  expanded: boolean,
  x: number,
): { peers: PeerBox[]; more: MoreBox | null } {
  const sorted = [...edges].sort((a, b) => a.node.name.localeCompare(b.node.name, undefined, { numeric: true }))
  const shown = expanded || sorted.length <= SIDE_LIMIT ? sorted : sorted.slice(0, SIDE_LIMIT - 1)
  const hidden = sorted.length - shown.length
  const rows = shown.length + (hidden > 0 ? 1 : 0)
  const top = -((rows - 1) * ROW) / 2
  const peers = shown.map((edge, index) => ({
    id: `${side}:${edge.node.id}`,
    side,
    node: edge.node,
    rules: rules.filter((rule) => edge.rules.includes(rule.id)),
    x,
    y: top + index * ROW,
  }))
  const more = hidden > 0 ? { id: `${side}:more`, side, hidden, x, y: top + shown.length * ROW } : null
  return { peers, more }
}

export function buildNodeGraph(
  access: NodeAccess,
  rules: readonly Rule[],
  expanded: { left: boolean; right: boolean },
): NodeGraphShape {
  const step = NODE_BOX.width + NODE_BOX.gapX
  const left = column('left', access.reachable_by, rules, expanded.left, -step)
  const right = column('right', access.can_reach, rules, expanded.right, step)
  return {
    focus: { id: 'focus', node: access.node, x: 0, y: 0 },
    peers: [...left.peers, ...right.peers],
    more: [left.more, right.more].filter((box): box is MoreBox => box !== null),
    totals: { left: access.reachable_by.length, right: access.can_reach.length },
  }
}
