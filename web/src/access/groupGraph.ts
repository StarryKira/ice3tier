import { type AccessGraph, type Direction, type GroupId, type GroupRef, type NodeRef } from '@/api/types'

/**
 * 把关系图接口的应答整理成"按分组"视图要画的卡片和线。纯函数，不依赖画图的库，方便测试。
 * 这个视图的规模只跟分组数有关，不跟节点数有关。
 */

export type Side = 'src' | 'dst'

export interface GroupPoint {
  /** 图上的编号：`src:<分组编号>` 是左边来源那一栏的，`dst:<分组编号>` 是右边目标那一栏的。 */
  id: string
  side: Side
  group: GroupRef & { builtin: boolean }
  members: NodeRef[]
  /** 成员里还没有确认最新配置的节点数：它们还在按旧的策略判定。 */
  pending: number
  x: number
  y: number
}

export interface RuleLine {
  id: string
  ruleId: number
  name: string
  src: GroupRef
  dst: GroupRef
  /** 两端的卡片在图上的编号：总是从左边的来源连到右边的目标。 */
  source: string
  target: string
  direction: Direction
  enabled: boolean
  /** 线在两头卡片边上的上下偏移：一张卡片上进出好几根线时各占一个位置，不挤在同一点。 */
  sourceOffset: number
  targetOffset: number
}

export interface GroupGraphShape {
  points: GroupPoint[]
  lines: RuleLine[]
  /** 没有被任何策略用到的分组：图上不画，在角上写一句。 */
  unused: GroupRef[]
}

export const pointId = (side: Side, group: GroupId): string => `${side}:${group}`

/** 卡片的尺寸：宽是定的，高是没量到时的估计。两栏之间留出写策略名字的地方。 */
export const GROUP_NODE = { width: 236, height: 62 } as const
const COLUMN_GAP = 300
const ROW_GAP = 22
const SLOT_GAP = 12

/**
 * 两栏：左边是来源分组，右边是目标分组，每条策略是一根从左到右的线（双向的两头都有箭头）。
 * 两头是同一个分组的策略（例如 All ⇄ All）也是一根左右的直线，不画绕回自己的环。
 *
 * 左栏按策略的顺序（第一次出现的先后）排；右栏的每张卡片尽量和连着它的那几张左栏卡片对齐，
 * 按它们位置的平均数排，交叉少。两栏各自上下居中。
 */
export function buildGroupGraph(graph: AccessGraph): GroupGraphShape {
  const pendingIds = new Set(graph.unacknowledged.map((node) => node.id))
  const byId = new Map(graph.groups.map((group) => [group.id, group]))
  // 策略用到的分组总在列表里；列表还没刷新时（分组刚删掉）这条线不画。
  const rules = graph.rules.filter((rule) => byId.has(rule.src.id) && byId.has(rule.dst.id))

  const left: GroupId[] = []
  for (const rule of rules) if (!left.includes(rule.src.id)) left.push(rule.src.id)
  const rank = new Map(left.map((id, index) => [id, index]))
  const right: GroupId[] = []
  for (const rule of rules) if (!right.includes(rule.dst.id)) right.push(rule.dst.id)
  const weight = (id: GroupId) => {
    const sources = rules.filter((rule) => rule.dst.id === id).map((rule) => rank.get(rule.src.id) ?? 0)
    return sources.reduce((sum, value) => sum + value, 0) / Math.max(1, sources.length)
  }
  right.sort((a, b) => weight(a) - weight(b))

  const step = GROUP_NODE.height + ROW_GAP
  const column = (ids: GroupId[], side: Side, x: number): GroupPoint[] => {
    const top = -((ids.length - 1) * step) / 2
    return ids.flatMap((id, index) => {
      const group = byId.get(id)
      if (group === undefined) return []
      return [
        {
          id: pointId(side, id),
          side,
          group: { id: group.id, name: group.name, builtin: group.builtin },
          members: group.nodes,
          pending: group.nodes.filter((node) => pendingIds.has(node.id)).length,
          x,
          y: Math.round(top + index * step),
        },
      ]
    })
  }
  const points = [...column(left, 'src', 0), ...column(right, 'dst', GROUP_NODE.width + COLUMN_GAP)]

  // 一张卡片边上的几根线按另一头的高低排开，间隔 SLOT_GAP，围绕卡片的中线对称。
  const yOf = new Map(points.map((point) => [point.id, point.y]))
  const slots = (ends: { rule: number; here: string; there: string }[]) => {
    const offsets = new Map<number, number>()
    const byCard = new Map<string, typeof ends>()
    for (const end of ends) byCard.set(end.here, [...(byCard.get(end.here) ?? []), end])
    for (const list of byCard.values()) {
      const sorted = [...list].sort((a, b) => (yOf.get(a.there) ?? 0) - (yOf.get(b.there) ?? 0) || a.rule - b.rule)
      sorted.forEach((end, index) => offsets.set(end.rule, (index - (sorted.length - 1) / 2) * SLOT_GAP))
    }
    return offsets
  }
  const out = slots(
    rules.map((rule) => ({ rule: rule.id, here: pointId('src', rule.src.id), there: pointId('dst', rule.dst.id) })),
  )
  const into = slots(
    rules.map((rule) => ({ rule: rule.id, here: pointId('dst', rule.dst.id), there: pointId('src', rule.src.id) })),
  )
  const lines: RuleLine[] = rules.map((rule) => {
    return {
      id: `rule:${rule.id}`,
      ruleId: rule.id,
      name: rule.name,
      src: rule.src,
      dst: rule.dst,
      source: pointId('src', rule.src.id),
      target: pointId('dst', rule.dst.id),
      direction: rule.direction,
      enabled: rule.enabled,
      sourceOffset: out.get(rule.id) ?? 0,
      targetOffset: into.get(rule.id) ?? 0,
    }
  })

  const used = new Set([...left, ...right])
  const unused = graph.groups
    .filter((group) => !used.has(group.id))
    .map((group) => ({ id: group.id, name: group.name, builtin: group.builtin }))
  return { points, lines, unused }
}

/** 布局的指纹：两栏里有哪些卡片、什么顺序变了才重新排版；只是名字、成员数、启用与否变了时保留拖动过的位置。 */
export function layoutSignature(shape: GroupGraphShape): string {
  return shape.points.map((point) => point.id).join(',')
}
