import { type AccessGraph, type Direction, type GroupId, type GroupRef, type NodeRef } from '@/api/types'

/**
 * 把关系图接口的应答整理成"按分组"视图要画的点和线。纯函数，不依赖画图的库，方便测试。
 *
 * 每个分组一个点，标着成员数；每条规则一条线。这个视图的规模只跟分组数有关，不跟节点数有关。
 */

export interface GroupPoint {
  /** 图上的编号。分组用 `group:<编号>`；只在 All 里的节点合成的那个点是 `ungrouped`。 */
  id: string
  kind: 'group' | 'ungrouped'
  /** 只在 All 里的那个点没有分组，是 `null`。 */
  group: (GroupRef & { builtin: boolean }) | null
  members: NodeRef[]
  /** 成员里还没有确认最新配置的节点数：它们还在按旧的规则判定。 */
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
  /** 两端的点在图上的编号。 */
  source: string
  target: string
  direction: Direction
  enabled: boolean
  /** 两端是同一个分组：画成一个小环。 */
  loop: boolean
  /** 同一对分组之间有几条线、这是第几条：画的时候错开弧度，不叠在一起。 */
  index: number
  siblings: number
}

export interface GroupGraphShape {
  points: GroupPoint[]
  lines: RuleLine[]
}

export const UNGROUPED_ID = 'ungrouped'
export const groupPointId = (group: GroupId): string => `group:${group}`

/** 画点用的尺寸：宽是定的；高随内容（成员、组内的策略），这里是没量到时的估计。 */
export const GROUP_NODE = { width: 232, height: 96 } as const

/**
 * 布局：All 固定在最上面，其余分组沿一个圆排开。分组不多（个人用户通常几个），圆上的连线交叉最少，
 * 也不需要引入自动布局的库。只有 All 时它在正中；"只在 All 里"的点不连线，放在左下角圈外。
 */
function layout(count: number): { x: number; y: number }[] {
  if (count === 1) return [{ x: 0, y: 0 }]
  const radius = Math.max(230, count * 64)
  return Array.from({ length: count }, (_, index) => {
    const angle = -Math.PI / 2 + (2 * Math.PI * index) / count
    return { x: Math.round(radius * Math.cos(angle)), y: Math.round(radius * Math.sin(angle)) }
  })
}

export function buildGroupGraph(graph: AccessGraph): GroupGraphShape {
  const pendingIds = new Set(graph.unacknowledged.map((node) => node.id))
  // All 排第一，其余保持接口给的顺序（按名字）。
  const ordered = [...graph.groups].sort((a, b) => Number(b.builtin) - Number(a.builtin))
  const positions = layout(ordered.length)

  const points: GroupPoint[] = ordered.map((group, index) => ({
    id: groupPointId(group.id),
    kind: 'group',
    group: { id: group.id, name: group.name, builtin: group.builtin },
    members: group.nodes,
    pending: group.nodes.filter((node) => pendingIds.has(node.id)).length,
    x: positions[index]?.x ?? 0,
    y: positions[index]?.y ?? 0,
  }))

  const grouped = new Set(
    graph.groups.filter((group) => !group.builtin).flatMap((group) => group.nodes.map((node) => node.id)),
  )
  const ungrouped = (graph.groups.find((group) => group.builtin)?.nodes ?? []).filter((node) => !grouped.has(node.id))
  // 只有 All 一个分组时，"只在 All 里"就是全部节点，再画一个点是重复的。
  if (ungrouped.length > 0 && ordered.length > 1) {
    const radius = Math.max(230, ordered.length * 64)
    points.push({
      id: UNGROUPED_ID,
      kind: 'ungrouped',
      group: null,
      members: ungrouped,
      pending: ungrouped.filter((node) => pendingIds.has(node.id)).length,
      // 左下角，圆上相邻两个点之间的空当里：离圈近一些，画布不必为它缩得很小。
      x: -radius - 30,
      y: radius - 20,
    })
  }

  const known = new Set(points.map((point) => point.id))
  const pairKey = (a: GroupId, b: GroupId) => (a < b ? `${a}|${b}` : `${b}|${a}`)
  const counts = new Map<string, number>()
  // 规则引用的分组总在列表里；列表还没刷新时（分组刚删掉）这条线不画。
  const usable = graph.rules.filter(
    (rule) => known.has(groupPointId(rule.src.id)) && known.has(groupPointId(rule.dst.id)),
  )
  for (const rule of usable) {
    const key = pairKey(rule.src.id, rule.dst.id)
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }

  const seen = new Map<string, number>()
  const lines: RuleLine[] = usable.map((rule) => {
    const key = pairKey(rule.src.id, rule.dst.id)
    const index = seen.get(key) ?? 0
    seen.set(key, index + 1)
    return {
      id: `rule:${rule.id}`,
      ruleId: rule.id,
      name: rule.name,
      src: rule.src,
      dst: rule.dst,
      source: groupPointId(rule.src.id),
      target: groupPointId(rule.dst.id),
      direction: rule.direction,
      enabled: rule.enabled,
      loop: rule.src.id === rule.dst.id,
      index,
      siblings: counts.get(key) ?? 1,
    }
  })

  return { points, lines }
}

/** 布局的指纹：分组的集合变了才重新排版，只是规则、成员数或者名字变了时保留用户拖动过的位置。 */
export function layoutSignature(shape: GroupGraphShape): string {
  return shape.points.map((point) => point.id).join(',')
}
