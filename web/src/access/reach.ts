import { type Rule, type TagMembers } from '@/api/types'

/**
 * 按标签和规则算"谁能访问谁"，用来在保存之前说出一次改动的影响（"有 4 对节点将失去访问"）。
 *
 * 算法和 controller 的一样：节点 x 能访问节点 y，当且仅当有一条启用的规则，x 带着它的 src、y 带着它的 dst；
 * 双向的规则反过来也算。每个节点都带着内置标签 all。
 *
 * 不逐对节点去算：标签集合相同的节点对任何规则都是同一种结果，所以先把节点按标签集合归成组，
 * 再按"组对组"算，对数是两组大小的乘积。组数只跟标签的组合数有关，千级节点也是一瞬间。
 */

export type RuleLike = Pick<Rule, 'src' | 'dst' | 'direction' | 'enabled'>

interface Group {
  tags: ReadonlySet<string>
  size: number
}

/** 把节点按"带着哪些标签"归组。`tags` 是关系图接口给的每个标签的成员，其中总有 all。 */
function groupNodes(tags: readonly TagMembers[]): Group[] {
  const byNode = new Map<number, string[]>()
  for (const entry of tags) {
    for (const node of entry.nodes) {
      const list = byNode.get(node.id)
      if (list === undefined) byNode.set(node.id, [entry.tag])
      else list.push(entry.tag)
    }
  }
  const bySignature = new Map<string, Group>()
  for (const list of byNode.values()) {
    const signature = [...list].sort().join('\u0000')
    const group = bySignature.get(signature)
    if (group === undefined) bySignature.set(signature, { tags: new Set(list), size: 1 })
    else group.size += 1
  }
  return [...bySignature.values()]
}

/** `from` 组的节点能不能访问 `to` 组的节点。 */
function allows(rules: readonly RuleLike[], from: Group, to: Group): boolean {
  return rules.some(
    (rule) =>
      rule.enabled &&
      ((from.tags.has(rule.src) && to.tags.has(rule.dst)) ||
        (rule.direction === 'both' && from.tags.has(rule.dst) && to.tags.has(rule.src))),
  )
}

export interface AccessDiff {
  /** 有多少对节点多了至少一个方向的访问。 */
  gained: number
  /** 有多少对节点少了至少一个方向的访问。它们之间正在进行的连接会被断开。 */
  lost: number
  /** 改动之后，还有多少对节点之间存在访问（任一方向）。是 0 表示所有节点互不可达。 */
  connectedAfter: number
  connectedBefore: number
}

/** 规则从 `before` 变成 `after`，访问关系有什么变化。一对节点不分先后，只数一次。 */
export function accessDiff(
  tags: readonly TagMembers[],
  before: readonly RuleLike[],
  after: readonly RuleLike[],
): AccessDiff {
  const groups = groupNodes(tags)
  const diff: AccessDiff = { gained: 0, lost: 0, connectedAfter: 0, connectedBefore: 0 }
  for (let i = 0; i < groups.length; i += 1) {
    for (let j = i; j < groups.length; j += 1) {
      const a = groups[i]
      const b = groups[j]
      if (a === undefined || b === undefined) continue
      const pairs = i === j ? (a.size * (a.size - 1)) / 2 : a.size * b.size
      if (pairs === 0) continue
      const was = [allows(before, a, b), allows(before, b, a)] as const
      const now = [allows(after, a, b), allows(after, b, a)] as const
      if ((now[0] && !was[0]) || (now[1] && !was[1])) diff.gained += pairs
      if ((was[0] && !now[0]) || (was[1] && !now[1])) diff.lost += pairs
      if (was[0] || was[1]) diff.connectedBefore += pairs
      if (now[0] || now[1]) diff.connectedAfter += pairs
    }
  }
  return diff
}

/** 两条规则说的是不是同一件事。双向规则两边对调之后还是同一条。 */
export function sameRule(
  a: Pick<Rule, 'src' | 'dst' | 'direction'>,
  b: Pick<Rule, 'src' | 'dst' | 'direction'>,
): boolean {
  if (a.direction !== b.direction) return false
  if (a.src === b.src && a.dst === b.dst) return true
  return a.direction === 'both' && a.src === b.dst && a.dst === b.src
}

/** 一条规则是不是默认的那条 all ⇄ all（方向不论：两边都是 all 时单向和双向等价）。 */
export function isAllToAll(rule: Pick<Rule, 'src' | 'dst'>): boolean {
  return rule.src === 'all' && rule.dst === 'all'
}
