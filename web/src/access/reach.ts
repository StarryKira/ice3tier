import { type Group, type GroupId, type GroupRef, type Rule } from '@/api/types'
import { isAllGroup } from '@/lib/groups'

/**
 * 按分组和规则算"谁能访问谁"，用来在保存之前说出一次改动的影响（"有 4 对节点将失去访问"）。
 *
 * 算法和 controller 的一样：节点 x 能访问节点 y，当且仅当有一条启用的规则，x 在它的 src 里、y 在它的 dst 里；
 * 双向的规则反过来也算。每个节点都在内置分组 All 里。
 *
 * 不逐对节点去算：所在分组相同的节点对任何规则都是同一种结果，所以先把节点按"在哪些分组里"归成组，
 * 再按"组对组"算，对数是两组大小的乘积。组数只跟分组的组合数有关，千级节点也是一瞬间。
 */

/** 规则里算影响要用到的：两头只看编号。 */
export interface RuleLike {
  src: Pick<GroupRef, 'id'>
  dst: Pick<GroupRef, 'id'>
  direction: Rule['direction']
  enabled: boolean
}

interface Cohort {
  groups: ReadonlySet<GroupId>
  size: number
}

/** 把节点按"在哪些分组里"归组。`groups` 是关系图接口给的每个分组的成员，其中 All 包含全部节点。 */
function cohorts(groups: readonly Pick<Group, 'id' | 'nodes'>[]): Cohort[] {
  const byNode = new Map<number, GroupId[]>()
  for (const group of groups) {
    for (const node of group.nodes) {
      const list = byNode.get(node.id)
      if (list === undefined) byNode.set(node.id, [group.id])
      else list.push(group.id)
    }
  }
  const bySignature = new Map<string, Cohort>()
  for (const list of byNode.values()) {
    const signature = [...list].sort((a, b) => a - b).join(',')
    const cohort = bySignature.get(signature)
    if (cohort === undefined) bySignature.set(signature, { groups: new Set(list), size: 1 })
    else cohort.size += 1
  }
  return [...bySignature.values()]
}

/** `from` 组的节点能不能访问 `to` 组的节点。 */
function allows(rules: readonly RuleLike[], from: Cohort, to: Cohort): boolean {
  return rules.some(
    (rule) =>
      rule.enabled &&
      ((from.groups.has(rule.src.id) && to.groups.has(rule.dst.id)) ||
        (rule.direction === 'both' && from.groups.has(rule.dst.id) && to.groups.has(rule.src.id))),
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
  groups: readonly Pick<Group, 'id' | 'nodes'>[],
  before: readonly RuleLike[],
  after: readonly RuleLike[],
): AccessDiff {
  const list = cohorts(groups)
  const diff: AccessDiff = { gained: 0, lost: 0, connectedAfter: 0, connectedBefore: 0 }
  for (let i = 0; i < list.length; i += 1) {
    for (let j = i; j < list.length; j += 1) {
      const a = list[i]
      const b = list[j]
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

type Ends = Pick<RuleLike, 'src' | 'dst' | 'direction'>

/** 两条规则说的是不是同一件事。双向规则两边对调之后还是同一条。 */
export function sameRule(a: Ends, b: Ends): boolean {
  if (a.direction !== b.direction) return false
  if (a.src.id === b.src.id && a.dst.id === b.dst.id) return true
  return a.direction === 'both' && a.src.id === b.dst.id && a.dst.id === b.src.id
}

/** 一条规则是不是默认的那条 All ⇄ All（方向不论：两边都是 All 时单向和双向等价）。 */
export function isAllToAll(rule: Pick<Rule, 'src' | 'dst'>): boolean {
  return isAllGroup(rule.src) && isAllGroup(rule.dst)
}
