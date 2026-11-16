import { type AccessGraph, type Direction, type Rule } from '@/api/types'

/**
 * 总览的访问地图要画的东西。纯函数，不依赖画图的库，方便测试。
 *
 * 三列：左边是看的那一个（一个节点，或者一个标签）；中间是和它有关的规则，一条一个胶囊；
 * 右边是规则另一头的标签，写着有几个节点。左边连到每个胶囊，每个胶囊连到它另一头的标签。
 *
 * 一条规则和"它"有关：规则的一头是它所在的标签。节点在它自己的每个标签里，也总在 all 里。
 * 流向从"它"这边看：`out` 是它能访问对面，`in` 是对面能访问它，`both` 是双向。
 */

export type MapFlow = 'out' | 'in' | 'both'

export interface MapRule {
  /** 图上的编号。 */
  id: string
  rule: Rule
  flow: MapFlow
  /** 规则另一头的标签。 */
  other: string
}

export interface MapTarget {
  id: string
  tag: string
  /** 标签里有几个节点。 */
  members: number
}

export interface AccessMap {
  rules: MapRule[]
  targets: MapTarget[]
}

export const mapTargetId = (tag: string): string => `target:${tag}`

/** `tags` 是"它"所在的标签：节点的标签加上 all，或者只是那一个标签。 */
export function buildAccessMap(graph: AccessGraph, tags: ReadonlySet<string>): AccessMap {
  const rules: MapRule[] = []
  for (const rule of graph.rules) {
    const fromHere = tags.has(rule.src)
    const toHere = tags.has(rule.dst)
    if (!fromHere && !toHere) continue
    const other = fromHere ? rule.dst : rule.src
    const flow: MapFlow = rule.direction === 'both' ? 'both' : fromHere ? 'out' : 'in'
    rules.push({ id: `rule:${rule.id}`, rule, flow, other })
  }
  // 启用的在前；同样启用或停用的，保持规则列表的顺序。
  rules.sort((a, b) => Number(b.rule.enabled) - Number(a.rule.enabled))

  const members = new Map(graph.tags.map((entry) => [entry.tag, entry.nodes.length]))
  const targets: MapTarget[] = []
  for (const entry of rules) {
    if (targets.some((target) => target.tag === entry.other)) continue
    targets.push({ id: mapTargetId(entry.other), tag: entry.other, members: members.get(entry.other) ?? 0 })
  }
  return { rules, targets }
}

/** 节点所在的标签：它自己的，加上 all。 */
export function nodeTags(tags: readonly string[]): Set<string> {
  return new Set([...tags, 'all'])
}

/** 画图用的尺寸。胶囊的宽度按名字估一个，画的时候就用这个宽度，估短了名字截断。 */
export const MAP_SIZE = {
  subject: { width: 260, height: 64 },
  target: { width: 240, height: 64 },
  pill: { height: 36, min: 150, max: 300 },
  gapY: { pill: 16, target: 18 },
  /** 列与列之间留的横向距离。 */
  gapX: 150,
} as const

/** 估一段文字有多宽（13 像素的字）：汉字一个字宽，其余大约一半多。 */
function textWidth(text: string): number {
  let width = 0
  for (const char of text) width += /[⺀-￿]/u.test(char) ? 13 : 7.4
  return width
}

/** 胶囊的宽度：圆点、名字、分隔线、方向（双向、单向），加上左右的留白。 */
export function pillWidth(name: string, directionLabel: string): number {
  const width = Math.ceil(12 + 8 + 8 + textWidth(name) + 10 + 1 + 10 + textWidth(directionLabel) + 12)
  return Math.min(MAP_SIZE.pill.max, Math.max(MAP_SIZE.pill.min, width))
}

export interface Placed {
  x: number
  y: number
  width: number
}

export interface MapLayout {
  subject: Placed
  pills: Map<string, Placed>
  targets: Map<string, Placed>
}

/** 一列里的东西竖着排开，整体以 0 为中心。 */
function column(count: number, height: number, gap: number): number[] {
  const step = height + gap
  const top = -((count - 1) * step) / 2 - height / 2
  return Array.from({ length: count }, (_, index) => top + index * step)
}

/** 每一块左上角的位置（画图的库按左上角放）。 */
export function layoutAccessMap(map: AccessMap, directionLabel: (direction: Direction) => string): MapLayout {
  const { subject, target, pill, gapY, gapX } = MAP_SIZE
  const widths = new Map(
    map.rules.map((entry) => [entry.id, pillWidth(entry.rule.name, directionLabel(entry.rule.direction))]),
  )
  const pillColumn = Math.max(MAP_SIZE.pill.min, ...widths.values())
  const pillCenter = subject.width + gapX + pillColumn / 2
  const targetX = subject.width + gapX + pillColumn + gapX

  const pillYs = column(map.rules.length, pill.height, gapY.pill)
  const targetYs = column(map.targets.length, target.height, gapY.target)

  return {
    subject: { x: 0, y: -subject.height / 2, width: subject.width },
    pills: new Map(
      map.rules.map((entry, index) => {
        const width = widths.get(entry.id) ?? MAP_SIZE.pill.min
        return [entry.id, { x: pillCenter - width / 2, y: pillYs[index] ?? 0, width }]
      }),
    ),
    targets: new Map(
      map.targets.map((entry, index) => [entry.id, { x: targetX, y: targetYs[index] ?? 0, width: target.width }]),
    ),
  }
}
