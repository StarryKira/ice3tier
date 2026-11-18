import { describe, expect, it } from 'vitest'

import {
  type AccessEdge,
  type AccessGraph,
  type Group,
  type GroupRef,
  type NodeAccess,
  type NodeRef,
  type Rule,
} from '@/api/types'

import { borderPoint, curveBetween } from './geometry'
import { buildGroupGraph, GROUP_NODE, layoutSignature, pointId } from './groupGraph'
import { buildNodeGraph, SIDE_LIMIT } from './nodeGraph'
import { accessDiff, isAllToAll, sameRule } from './reach'

function ref(id: number, name = `node-${id}`): NodeRef {
  return { id, name, virtual_ip: `10.66.0.${id}` }
}

const ALL: GroupRef = { id: 1, name: 'All' }
const HOME: GroupRef = { id: 2, name: 'home' }
const LAPTOPS: GroupRef = { id: 3, name: 'laptops' }
const SERVERS: GroupRef = { id: 4, name: 'servers' }

function rule(id: number, src: GroupRef, dst: GroupRef, extra: Partial<Rule> = {}): Rule {
  return { id, name: '', src, dst, direction: 'both', enabled: true, ...extra }
}

function group(of: GroupRef, nodes: NodeRef[]): Group {
  return { ...of, builtin: of.id === ALL.id, nodes, rules: [] }
}

const gw = ref(1, 'gw-hk')
const nas = ref(2, 'nas-home')
const laptop = ref(3, 'laptop')
const office = ref(4, 'office-pc')
const spare = ref(5, 'spare')

const DEFAULT_RULE = rule(1, ALL, ALL, { name: 'default' })

/** 接口应答的样子：groups 里 All 在最前面，其余按名字。 */
function graphOf(overrides: Partial<AccessGraph> = {}): AccessGraph {
  return {
    groups: [
      group(ALL, [gw, nas, laptop, office, spare]),
      group(HOME, [nas]),
      group(LAPTOPS, [laptop, office]),
      group(SERVERS, [gw, nas]),
    ],
    rules: [DEFAULT_RULE],
    unconnected: [],
    unacknowledged: [],
    ...overrides,
  }
}

describe('按分组的视图：来源 → 目标两栏', () => {
  const ssh = rule(2, LAPTOPS, SERVERS, { name: 'ssh', direction: 'one_way' })
  const media = rule(3, LAPTOPS, HOME, { name: 'media', enabled: false })

  it('All ⇄ All：左右各一张 All 的卡片，一根线，不画环', () => {
    const shape = buildGroupGraph(graphOf({ groups: [group(ALL, [gw, nas])] }))
    expect(shape.points.map((point) => [point.id, point.side])).toEqual([
      [pointId('src', ALL.id), 'src'],
      [pointId('dst', ALL.id), 'dst'],
    ])
    expect(shape.lines).toHaveLength(1)
    expect(shape.lines[0]).toMatchObject({
      ruleId: 1,
      source: pointId('src', ALL.id),
      target: pointId('dst', ALL.id),
      direction: 'both',
    })
    // 左栏在左，右栏在右，同一行。
    const [left, right] = shape.points
    expect((left?.x ?? 0) < (right?.x ?? 0)).toBe(true)
    expect(left?.y).toBe(right?.y)
  })

  it('左栏是策略的来源分组，右栏是目标分组；没被策略用到的分组不画，单独列出', () => {
    const shape = buildGroupGraph(graphOf({ rules: [DEFAULT_RULE, ssh, media] }))
    const side = (which: 'src' | 'dst') =>
      shape.points.filter((point) => point.side === which).map((point) => point.group.name)
    expect(side('src')).toEqual(['All', 'laptops'])
    expect(side('dst').sort()).toEqual(['All', 'home', 'servers'])
    expect(shape.unused).toEqual([])
    const fewer = buildGroupGraph(graphOf({ rules: [ssh] }))
    expect(fewer.unused.map((group) => group.name)).toEqual(['All', 'home'])
  })

  it('每条策略一根从左到右的线，带着方向、是否启用', () => {
    const shape = buildGroupGraph(graphOf({ rules: [ssh, media] }))
    expect(shape.lines.map((line) => [line.source, line.target, line.direction, line.enabled])).toEqual([
      [pointId('src', LAPTOPS.id), pointId('dst', SERVERS.id), 'one_way', true],
      [pointId('src', LAPTOPS.id), pointId('dst', HOME.id), 'both', false],
    ])
  })

  it('右栏的卡片按连着它的左栏卡片的位置排，交叉少', () => {
    const shape = buildGroupGraph(graphOf({ rules: [rule(2, SERVERS, HOME), rule(3, LAPTOPS, SERVERS), DEFAULT_RULE] }))
    const right = shape.points.filter((point) => point.side === 'dst').map((point) => point.group.name)
    // 左栏：servers、laptops、All；右栏的 home 跟着 servers（第 0 行），servers 跟着 laptops（第 1 行）。
    expect(right).toEqual(['home', 'servers', 'All'])
  })

  it('一张卡片上进出的几根线按另一头的高低排开，不挤在同一点', () => {
    const shape = buildGroupGraph(graphOf({ rules: [ssh, rule(4, SERVERS, SERVERS), rule(5, LAPTOPS, HOME)] }))
    const into = shape.lines.filter((line) => line.target === pointId('dst', SERVERS.id))
    expect(new Set(into.map((line) => line.targetOffset)).size).toBe(into.length)
    const out = shape.lines.filter((line) => line.source === pointId('src', LAPTOPS.id))
    expect(new Set(out.map((line) => line.sourceOffset)).size).toBe(out.length)
    // 只有一根线的那一头在正中。
    expect(shape.lines.find((line) => line.ruleId === 5)?.targetOffset).toBe(0)
  })

  it('分组改名之后策略照样连着：线按编号找卡片', () => {
    const renamed = graphOf({
      rules: [ssh],
      groups: [group(ALL, []), group({ ...LAPTOPS, name: 'notebooks' }, [laptop]), group(SERVERS, [gw])],
    })
    const shape = buildGroupGraph(renamed)
    expect(shape.points.find((point) => point.id === pointId('src', LAPTOPS.id))?.group.name).toBe('notebooks')
    expect(shape.lines[0]?.source).toBe(pointId('src', LAPTOPS.id))
  })

  it('策略用到的分组不在列表里（列表还没刷新）时，这条线不画', () => {
    const shape = buildGroupGraph(graphOf({ rules: [DEFAULT_RULE, rule(2, LAPTOPS, { id: 99, name: 'gone' })] }))
    expect(shape.lines.map((line) => line.ruleId)).toEqual([1])
  })

  it('还没确认新配置的节点：按分组数出来', () => {
    const shape = buildGroupGraph(graphOf({ rules: [ssh], unacknowledged: [office, spare] }))
    expect(shape.points.find((point) => point.id === pointId('src', LAPTOPS.id))?.pending).toBe(1)
  })

  it('一栏里的卡片不重叠，上下居中', () => {
    const shape = buildGroupGraph(graphOf({ rules: [DEFAULT_RULE, ssh, media] }))
    const right = shape.points.filter((point) => point.side === 'dst').map((point) => point.y)
    for (let index = 1; index < right.length; index += 1) {
      expect((right[index] ?? 0) - (right[index - 1] ?? 0)).toBeGreaterThanOrEqual(GROUP_NODE.height)
    }
    expect((right[0] ?? 0) + (right.at(-1) ?? 0)).toBe(0)
  })

  it('布局的指纹只跟两栏里有哪些卡片有关：名字、启用与否变了不重新排版', () => {
    const before = layoutSignature(buildGroupGraph(graphOf({ rules: [ssh] })))
    expect(layoutSignature(buildGroupGraph(graphOf({ rules: [{ ...ssh, enabled: false, name: 'x' }] })))).toBe(before)
    expect(layoutSignature(buildGroupGraph(graphOf({ rules: [ssh, media] })))).not.toBe(before)
  })
})

describe('按节点的视图', () => {
  const rules = [
    DEFAULT_RULE,
    rule(2, LAPTOPS, SERVERS, { name: 'ssh', direction: 'one_way' }),
    rule(3, SERVERS, SERVERS),
  ]

  function edge(node: NodeRef, ...ruleIds: number[]): AccessEdge {
    return { node, rules: ruleIds }
  }

  function accessOf(reachableBy: AccessEdge[], canReach: AccessEdge[]): NodeAccess {
    return { node: gw, can_reach: canReach, reachable_by: reachableBy, acknowledged: true }
  }

  const collapsed = { left: false, right: false }

  it('三列：能访问它的在左，它在中间，它能访问的在右', () => {
    const shape = buildNodeGraph(accessOf([edge(laptop, 2), edge(nas, 3)], [edge(nas, 3)]), rules, collapsed)
    expect(shape.focus).toMatchObject({ node: gw, x: 0, y: 0 })
    const left = shape.peers.filter((peer) => peer.side === 'left')
    const right = shape.peers.filter((peer) => peer.side === 'right')
    expect(left.map((peer) => peer.node.name)).toEqual(['laptop', 'nas-home'])
    expect(right.map((peer) => peer.node.name)).toEqual(['nas-home'])
    for (const peer of left) expect(peer.x).toBeLessThan(0)
    for (const peer of right) expect(peer.x).toBeGreaterThan(0)
    expect(shape.totals).toEqual({ left: 2, right: 1 })
  })

  it('互相都能访问的节点两侧都出现，编号不冲突', () => {
    const shape = buildNodeGraph(accessOf([edge(nas, 3)], [edge(nas, 3)]), rules, collapsed)
    expect(shape.peers.map((peer) => peer.id)).toEqual(['left:2', 'right:2'])
  })

  it('连线上标着是哪几条规则允许的，按规则列表的顺序', () => {
    const shape = buildNodeGraph(accessOf([edge(laptop, 2, 1)], []), rules, collapsed)
    expect(shape.peers[0]?.rules.map((allowed) => allowed.id)).toEqual([1, 2])
    expect(shape.peers[0]?.rules[1]?.name).toBe('ssh')
  })

  it('规则列表里已经没有的规则不标（列表还没刷新）', () => {
    const shape = buildNodeGraph(accessOf([edge(laptop, 99)], []), rules, collapsed)
    expect(shape.peers[0]?.rules).toEqual([])
  })

  it('一侧的节点按名字排，数字按大小', () => {
    const peers = [ref(12, 'node-10'), ref(11, 'node-2'), ref(10, 'node-1')].map((node) => edge(node, 1))
    const shape = buildNodeGraph(accessOf([], peers), rules, collapsed)
    expect(shape.peers.map((peer) => peer.node.name)).toEqual(['node-1', 'node-2', 'node-10'])
  })

  it(`一侧不超过 ${SIDE_LIMIT} 个时全画`, () => {
    const peers = Array.from({ length: SIDE_LIMIT }, (_, index) => edge(ref(10 + index), 1))
    const shape = buildNodeGraph(accessOf([], peers), rules, collapsed)
    expect(shape.peers).toHaveLength(SIDE_LIMIT)
    expect(shape.more).toEqual([])
  })

  it('再多就折起来：少画一个，腾出的位置写"还有 N 个"', () => {
    const peers = Array.from({ length: 20 }, (_, index) => edge(ref(10 + index), 1))
    const shape = buildNodeGraph(accessOf(peers, peers), rules, collapsed)
    const left = shape.peers.filter((peer) => peer.side === 'left')
    expect(left).toHaveLength(SIDE_LIMIT - 1)
    expect(shape.more).toHaveLength(2)
    expect(shape.more[0]).toMatchObject({ side: 'left', hidden: 20 - (SIDE_LIMIT - 1) })
    // 折起来的也算在总数里。
    expect(shape.totals).toEqual({ left: 20, right: 20 })
    // "还有 N 个"排在这一列的最下面。
    const lowest = Math.max(...left.map((peer) => peer.y))
    expect(shape.more[0]?.y).toBeGreaterThan(lowest)
  })

  it('展开一侧不影响另一侧', () => {
    const peers = Array.from({ length: 20 }, (_, index) => edge(ref(10 + index), 1))
    const shape = buildNodeGraph(accessOf(peers, peers), rules, { left: true, right: false })
    expect(shape.peers.filter((peer) => peer.side === 'left')).toHaveLength(20)
    expect(shape.peers.filter((peer) => peer.side === 'right')).toHaveLength(SIDE_LIMIT - 1)
    expect(shape.more.map((box) => box.side)).toEqual(['right'])
  })

  it('每一列围绕焦点节点上下对称，框不叠在一起', () => {
    const peers = [edge(ref(10), 1), edge(ref(11), 1), edge(ref(12), 1)]
    const shape = buildNodeGraph(accessOf([], peers), rules, collapsed)
    const ys = shape.peers.map((peer) => peer.y)
    expect(ys[0]).toBe(-(ys[2] ?? 0))
    expect(ys[1]).toBe(0)
    expect(new Set(ys).size).toBe(3)
  })

  it('谁也访问不了、也没人能访问的节点：只有它自己', () => {
    const shape = buildNodeGraph(accessOf([], []), rules, collapsed)
    expect(shape.peers).toEqual([])
    expect(shape.more).toEqual([])
    expect(shape.totals).toEqual({ left: 0, right: 0 })
  })
})

describe('一次改动的影响', () => {
  const groups = graphOf().groups
  const ssh = rule(2, LAPTOPS, SERVERS, { direction: 'one_way' })

  it('默认规则开着：5 个节点两两互通，一共 10 对', () => {
    const diff = accessDiff(groups, [DEFAULT_RULE], [DEFAULT_RULE])
    expect(diff).toEqual({ gained: 0, lost: 0, connectedBefore: 10, connectedAfter: 10 })
  })

  it('默认规则开着时再加规则，不多出任何一对', () => {
    expect(accessDiff(groups, [DEFAULT_RULE], [DEFAULT_RULE, ssh]).gained).toBe(0)
  })

  it('停用默认规则、没有别的规则：所有节点互不可达', () => {
    const diff = accessDiff(groups, [DEFAULT_RULE], [{ ...DEFAULT_RULE, enabled: false }])
    expect(diff.lost).toBe(10)
    expect(diff.connectedAfter).toBe(0)
  })

  it('收紧之后加一条规则：新增的对数是两边成员数的乘积', () => {
    // laptops（laptop、office-pc）→ servers（gw-hk、nas-home）：2 × 2 = 4 对。
    const diff = accessDiff(groups, [], [ssh])
    expect(diff).toEqual({ gained: 4, lost: 0, connectedBefore: 0, connectedAfter: 4 })
  })

  it('同一个分组自己到自己：成员之间两两一对', () => {
    // servers 里两个节点：1 对。
    expect(accessDiff(groups, [], [rule(3, SERVERS, SERVERS)]).gained).toBe(1)
    // 只有一个成员的分组自己到自己，没有任何一对。
    expect(accessDiff(groups, [], [rule(4, HOME, HOME)]).gained).toBe(0)
  })

  it('停用的规则不算', () => {
    expect(accessDiff(groups, [], [{ ...ssh, enabled: false }]).gained).toBe(0)
  })

  it('双向改成单向：这些节点对少了一个方向，算作失去访问，但仍然连着', () => {
    const both = { ...ssh, direction: 'both' as const }
    const diff = accessDiff(groups, [both], [ssh])
    expect(diff.lost).toBe(4)
    expect(diff.gained).toBe(0)
    expect(diff.connectedAfter).toBe(4)
  })

  it('一对节点被两条规则连着时，去掉其中一条不算失去', () => {
    const home = rule(5, LAPTOPS, HOME, { direction: 'one_way' })
    // laptops → home（nas-home）被 ssh 盖住了：nas-home 也在 servers 里。
    expect(accessDiff(groups, [ssh, home], [ssh]).lost).toBe(0)
  })

  it('规则用到的分组里没有节点时没有影响', () => {
    const empty = [...groups, group({ id: 8, name: 'nobody' }, [])]
    expect(accessDiff(empty, [], [rule(6, LAPTOPS, { id: 8, name: 'nobody' })]).gained).toBe(0)
  })

  it('节点很多时按分组的组合算，不逐对算', () => {
    const many = Array.from({ length: 2000 }, (_, index) => ref(index + 1))
    const a = { id: 2, name: 'a' }
    const b = { id: 3, name: 'b' }
    const big = [group(ALL, many), group(a, many.slice(0, 1000)), group(b, many.slice(1000))]
    const diff = accessDiff(big, [], [rule(1, a, b)])
    expect(diff.gained).toBe(1000 * 1000)
  })
})

describe('规则之间的比较', () => {
  const DEV = { id: 5, name: 'dev' }
  const SRV = { id: 6, name: 'srv' }

  it('双向规则两边对调还是同一条；单向的不是', () => {
    expect(sameRule(rule(1, DEV, SRV), rule(2, SRV, DEV))).toBe(true)
    expect(sameRule(rule(1, DEV, SRV), rule(2, DEV, SRV))).toBe(true)
    const oneWay = { direction: 'one_way' as const }
    expect(sameRule(rule(1, DEV, SRV, oneWay), rule(2, SRV, DEV, oneWay))).toBe(false)
    expect(sameRule(rule(1, DEV, SRV, oneWay), rule(2, DEV, SRV, oneWay))).toBe(true)
  })

  it('方向不同就不是同一条', () => {
    expect(sameRule(rule(1, DEV, SRV), rule(2, DEV, SRV, { direction: 'one_way' }))).toBe(false)
  })

  it('按编号比，不按名字：改过名的分组还是同一个', () => {
    expect(sameRule(rule(1, DEV, SRV), rule(2, { ...DEV, name: 'develop' }, SRV))).toBe(true)
  })

  it('认得出默认的 All ⇄ All', () => {
    expect(isAllToAll(DEFAULT_RULE)).toBe(true)
    expect(isAllToAll(rule(2, ALL, SERVERS))).toBe(false)
  })
})

describe('连线的几何', () => {
  const a = { x: 0, y: 0, width: 100, height: 40 }
  const b = { x: 300, y: 0, width: 100, height: 40 }

  function expectNear(point: { x: number; y: number }, x: number, y: number): void {
    expect(point.x).toBeCloseTo(x)
    expect(point.y).toBeCloseTo(y)
  }

  it('线的端点落在矩形的边上，不是中心', () => {
    expectNear(borderPoint(a, { x: 500, y: 20 }), 100, 20)
    expectNear(borderPoint(a, { x: 50, y: -500 }), 50, 0)
    // 斜着出去：先碰到哪条边就停在哪条边上。
    expectNear(borderPoint(a, { x: 150, y: 120 }), 70, 40)
    // 朝自己的中心：没有方向，留在中心。
    expectNear(borderPoint(a, { x: 50, y: 20 }), 50, 20)
  })

  it('两个点之间只有一条线时是直的，从一条边到另一条边，名字在正中间', () => {
    const path = curveBetween(a, b)
    const numbers = (path.d.match(/-?\d+(\.\d+)?/g) ?? []).map(Number)
    expect(path.d).toMatch(/^M \S+ \S+ Q \S+ \S+ \S+ \S+$/)
    const expected = [100, 20, 200, 20, 300, 20]
    expect(numbers).toHaveLength(expected.length)
    numbers.forEach((value, index) => expect(value).toBeCloseTo(expected[index] ?? Number.NaN))
    expectNear(path.label, 200, 20)
  })

  it('有两条线时各弯向一侧，不叠在一起', () => {
    const first = curveBetween(a, b, 0, 2)
    const second = curveBetween(a, b, 1, 2)
    expect(first.label.y).not.toBe(second.label.y)
    expect(first.label.y - 20).toBeCloseTo(-(second.label.y - 20))
  })

  it('方向相反的两条规则也弯向不同的两侧', () => {
    const forward = curveBetween(a, b, 0, 2)
    const backward = curveBetween(b, a, 1, 2)
    expect(forward.label.y).not.toBeCloseTo(backward.label.y)
  })
})
