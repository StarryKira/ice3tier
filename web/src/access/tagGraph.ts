import { type AccessGraph, type Direction, type NodeRef } from '@/api/types'

/**
 * 把关系图接口的应答整理成"按标签"视图要画的点和线。纯函数，不依赖画图的库，方便测试。
 *
 * 每个标签一个点，标着成员数；每条规则一条线。这个视图的规模只跟标签数有关，不跟节点数有关。
 */

export interface TagPoint {
  /** 图上的编号。标签用 `tag:<名字>`；没有任何标签的节点合成的那个点是 `untagged`。 */
  id: string
  kind: 'tag' | 'untagged'
  tag: string
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
  src: string
  dst: string
  /** 两端的点在图上的编号。 */
  source: string
  target: string
  direction: Direction
  enabled: boolean
  /** 两端是同一个标签：画成一个小环。 */
  loop: boolean
  /** 同一对标签之间有几条线、这是第几条：画的时候错开弧度，不叠在一起。 */
  index: number
  siblings: number
}

export interface TagGraphShape {
  points: TagPoint[]
  lines: RuleLine[]
}

export const UNTAGGED_ID = 'untagged'
export const tagPointId = (tag: string): string => `tag:${tag}`

/** 画点用的尺寸，布局和连线的几何都按它算。 */
export const TAG_NODE = { width: 148, height: 58 } as const

/**
 * 布局：all 固定在最上面，其余标签沿一个圆排开。标签不多（个人用户通常几个），圆上的连线交叉最少，
 * 也不需要引入自动布局的库。只有 all 时它在正中；"未打标签"的点不连线，放在左下角圈外。
 */
function layout(count: number): { x: number; y: number }[] {
  if (count === 1) return [{ x: 0, y: 0 }]
  const radius = Math.max(150, count * 44)
  return Array.from({ length: count }, (_, index) => {
    const angle = -Math.PI / 2 + (2 * Math.PI * index) / count
    return { x: Math.round(radius * Math.cos(angle)), y: Math.round(radius * Math.sin(angle)) }
  })
}

export function buildTagGraph(graph: AccessGraph): TagGraphShape {
  const pendingIds = new Set(graph.unacknowledged.map((node) => node.id))
  // all 排第一，其余保持接口给的字典序。
  const ordered = [...graph.tags].sort((a, b) => Number(b.tag === 'all') - Number(a.tag === 'all'))
  const positions = layout(ordered.length)

  const points: TagPoint[] = ordered.map((entry, index) => ({
    id: tagPointId(entry.tag),
    kind: 'tag',
    tag: entry.tag,
    members: entry.nodes,
    pending: entry.nodes.filter((node) => pendingIds.has(node.id)).length,
    x: positions[index]?.x ?? 0,
    y: positions[index]?.y ?? 0,
  }))

  const tagged = new Set(
    graph.tags.filter((entry) => entry.tag !== 'all').flatMap((entry) => entry.nodes.map((node) => node.id)),
  )
  const untagged = (graph.tags.find((entry) => entry.tag === 'all')?.nodes ?? []).filter((node) => !tagged.has(node.id))
  // 只有 all 一个标签时，"未打标签"就是全部节点，再画一个点是重复的。
  if (untagged.length > 0 && ordered.length > 1) {
    const radius = Math.max(150, ordered.length * 44)
    points.push({
      id: UNTAGGED_ID,
      kind: 'untagged',
      tag: '',
      members: untagged,
      pending: untagged.filter((node) => pendingIds.has(node.id)).length,
      // 左下角，圆上相邻两个点之间的空当里：离圈近一些，画布不必为它缩得很小。
      x: -radius - 30,
      y: radius - 20,
    })
  }

  const known = new Set(points.map((point) => point.id))
  const pairKey = (a: string, b: string) => (a < b ? `${a}|${b}` : `${b}|${a}`)
  const counts = new Map<string, number>()
  const usable = graph.rules.filter((rule) => known.has(tagPointId(rule.src)) && known.has(tagPointId(rule.dst)))
  for (const rule of usable) counts.set(pairKey(rule.src, rule.dst), (counts.get(pairKey(rule.src, rule.dst)) ?? 0) + 1)

  const seen = new Map<string, number>()
  const lines: RuleLine[] = usable.map((rule) => {
    const key = pairKey(rule.src, rule.dst)
    const index = seen.get(key) ?? 0
    seen.set(key, index + 1)
    return {
      id: `rule:${rule.id}`,
      ruleId: rule.id,
      name: rule.name,
      src: rule.src,
      dst: rule.dst,
      source: tagPointId(rule.src),
      target: tagPointId(rule.dst),
      direction: rule.direction,
      enabled: rule.enabled,
      loop: rule.src === rule.dst,
      index,
      siblings: counts.get(key) ?? 1,
    }
  })

  return { points, lines }
}

/** 布局的指纹：标签的集合变了才重新排版，只是规则或成员数变了时保留用户拖动过的位置。 */
export function layoutSignature(shape: TagGraphShape): string {
  return shape.points.map((point) => point.id).join(',')
}
