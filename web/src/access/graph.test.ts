import { describe, expect, it } from 'vitest'

import { type AccessEdge, type AccessGraph, type NodeAccess, type NodeRef, type Rule } from '@/api/types'

import { borderPoint, curveBetween, loopAround } from './geometry'
import { buildNodeGraph, SIDE_LIMIT } from './nodeGraph'
import { accessDiff, isAllToAll, sameRule } from './reach'
import { buildTagGraph, layoutSignature, tagPointId, UNTAGGED_ID } from './tagGraph'

function ref(id: number, name = `node-${id}`): NodeRef {
  return { id, name, virtual_ip: `10.66.0.${id}` }
}

function rule(id: number, src: string, dst: string, extra: Partial<Rule> = {}): Rule {
  return { id, name: '', src, dst, direction: 'both', enabled: true, ...extra }
}

const gw = ref(1, 'gw-hk')
const nas = ref(2, 'nas-home')
const laptop = ref(3, 'laptop')
const office = ref(4, 'office-pc')
const spare = ref(5, 'spare')

const DEFAULT_RULE = rule(1, 'all', 'all', { name: 'default' })

/** 接口应答的样子：tags 里总有 all，其余按字典序。 */
function graphOf(overrides: Partial<AccessGraph> = {}): AccessGraph {
  return {
    tags: [
      { tag: 'all', nodes: [gw, nas, laptop, office, spare] },
      { tag: 'home', nodes: [nas] },
      { tag: 'laptops', nodes: [laptop, office] },
      { tag: 'servers', nodes: [gw, nas] },
    ],
    rules: [DEFAULT_RULE],
    unconnected: [],
    unacknowledged: [],
    ...overrides,
  }
}

describe('按标签的视图', () => {
  it('每个标签一个点，带着成员；all 排在最前面、画在最上面', () => {
    const shape = buildTagGraph(graphOf())
    const tags = shape.points.filter((point) => point.kind === 'tag')
    expect(tags.map((point) => point.tag)).toEqual(['all', 'home', 'laptops', 'servers'])
    expect(tags.map((point) => point.members.length)).toEqual([5, 1, 2, 2])
    expect(tags.map((point) => point.id)).toEqual(['tag:all', 'tag:home', 'tag:laptops', 'tag:servers'])

    const all = tags[0]
    for (const other of tags.slice(1)) expect(all?.y).toBeLessThan(other.y)
  })

  it('接口没有把 all 放在第一个时也一样', () => {
    const graph = graphOf()
    graph.tags = [...graph.tags.slice(1), ...graph.tags.slice(0, 1)]
    expect(buildTagGraph(graph).points[0]?.tag).toBe('all')
  })

  it('没有任何标签的节点合成一个"未打标签"的点，不连线', () => {
    const shape = buildTagGraph(graphOf())
    const untagged = shape.points.find((point) => point.kind === 'untagged')
    expect(untagged?.id).toBe(UNTAGGED_ID)
    expect(untagged?.members).toEqual([spare])
    expect(shape.lines.some((line) => line.source === UNTAGGED_ID || line.target === UNTAGGED_ID)).toBe(false)
  })

  it('每个节点都有标签时没有"未打标签"的点', () => {
    const graph = graphOf({
      tags: [
        { tag: 'all', nodes: [gw, nas] },
        { tag: 'servers', nodes: [gw, nas] },
      ],
    })
    expect(buildTagGraph(graph).points.map((point) => point.id)).toEqual(['tag:all', 'tag:servers'])
  })

  it('只有 all 一个标签时它在正中，不重复画一个"未打标签"', () => {
    const shape = buildTagGraph(graphOf({ tags: [{ tag: 'all', nodes: [gw, nas] }] }))
    expect(shape.points).toHaveLength(1)
    expect(shape.points[0]).toMatchObject({ tag: 'all', x: 0, y: 0 })
  })

  it('空的网络：一个没有成员的 all，一条默认规则画成环', () => {
    const shape = buildTagGraph(graphOf({ tags: [{ tag: 'all', nodes: [] }] }))
    expect(shape.points).toHaveLength(1)
    expect(shape.points[0]?.members).toEqual([])
    expect(shape.lines).toHaveLength(1)
    expect(shape.lines[0]).toMatchObject({ ruleId: 1, loop: true, source: 'tag:all', target: 'tag:all' })
  })

  it('每条规则一条线，带着方向、是否启用和两端的点', () => {
    const shape = buildTagGraph(
      graphOf({
        rules: [
          DEFAULT_RULE,
          rule(2, 'laptops', 'servers', { name: 'ssh', direction: 'one_way' }),
          rule(3, 'servers', 'servers'),
          rule(4, 'laptops', 'home', { name: 'media', enabled: false }),
        ],
      }),
    )
    expect(shape.lines.map((line) => line.id)).toEqual(['rule:1', 'rule:2', 'rule:3', 'rule:4'])
    expect(shape.lines[1]).toMatchObject({
      ruleId: 2,
      name: 'ssh',
      source: tagPointId('laptops'),
      target: tagPointId('servers'),
      direction: 'one_way',
      enabled: true,
      loop: false,
    })
    // 同一个标签自己到自己：画成环。
    expect(shape.lines[0]?.loop).toBe(true)
    expect(shape.lines[2]?.loop).toBe(true)
    // 停用的规则照样画（虚线），不丢。
    expect(shape.lines[3]).toMatchObject({ enabled: false, direction: 'both' })
  })

  it('同一对标签之间的几条线各有编号，方向相反的也算同一对', () => {
    const shape = buildTagGraph(
      graphOf({
        rules: [
          rule(2, 'laptops', 'servers', { direction: 'one_way' }),
          rule(3, 'servers', 'laptops', { direction: 'one_way' }),
          rule(4, 'laptops', 'home'),
        ],
      }),
    )
    expect(shape.lines.map((line) => [line.index, line.siblings])).toEqual([
      [0, 2],
      [1, 2],
      [0, 1],
    ])
  })

  it('规则用到的标签下已经没有节点时，这条线不画（两端没有点可连）', () => {
    const shape = buildTagGraph(graphOf({ rules: [DEFAULT_RULE, rule(2, 'laptops', 'gone')] }))
    expect(shape.lines.map((line) => line.ruleId)).toEqual([1])
  })

  it('还没确认新配置的节点：按标签数出来', () => {
    const shape = buildTagGraph(graphOf({ unacknowledged: [office, spare] }))
    const pending = Object.fromEntries(shape.points.map((point) => [point.id, point.pending]))
    expect(pending).toEqual({ 'tag:all': 2, 'tag:home': 0, 'tag:laptops': 1, 'tag:servers': 0, untagged: 1 })
  })

  it('点的位置互不重合', () => {
    const shape = buildTagGraph(graphOf())
    const seen = new Set(shape.points.map((point) => `${point.x},${point.y}`))
    expect(seen.size).toBe(shape.points.length)
  })

  it('布局的指纹只跟有哪些点有关：规则和成员数变了不重新排版', () => {
    const before = layoutSignature(buildTagGraph(graphOf()))
    const moreRules = layoutSignature(buildTagGraph(graphOf({ rules: [DEFAULT_RULE, rule(2, 'laptops', 'servers')] })))
    expect(moreRules).toBe(before)

    const graph = graphOf()
    graph.tags.push({ tag: 'zoo', nodes: [spare] })
    expect(layoutSignature(buildTagGraph(graph))).not.toBe(before)
  })
})

describe('按节点的视图', () => {
  const rules = [
    DEFAULT_RULE,
    rule(2, 'laptops', 'servers', { name: 'ssh', direction: 'one_way' }),
    rule(3, 'servers', 'servers'),
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
  const tags = graphOf().tags
  const ssh = rule(2, 'laptops', 'servers', { direction: 'one_way' })

  it('默认规则开着：5 个节点两两互通，一共 10 对', () => {
    const diff = accessDiff(tags, [DEFAULT_RULE], [DEFAULT_RULE])
    expect(diff).toEqual({ gained: 0, lost: 0, connectedBefore: 10, connectedAfter: 10 })
  })

  it('默认规则开着时再加规则，不多出任何一对', () => {
    expect(accessDiff(tags, [DEFAULT_RULE], [DEFAULT_RULE, ssh]).gained).toBe(0)
  })

  it('停用默认规则、没有别的规则：所有节点互不可达', () => {
    const diff = accessDiff(tags, [DEFAULT_RULE], [{ ...DEFAULT_RULE, enabled: false }])
    expect(diff.lost).toBe(10)
    expect(diff.connectedAfter).toBe(0)
  })

  it('收紧之后加一条规则：新增的对数是两边成员数的乘积', () => {
    // laptops（laptop、office-pc）→ servers（gw-hk、nas-home）：2 × 2 = 4 对。
    const diff = accessDiff(tags, [], [ssh])
    expect(diff).toEqual({ gained: 4, lost: 0, connectedBefore: 0, connectedAfter: 4 })
  })

  it('同一个标签自己到自己：成员之间两两一对', () => {
    // servers 里两个节点：1 对。
    expect(accessDiff(tags, [], [rule(3, 'servers', 'servers')]).gained).toBe(1)
    // 只有一个成员的标签自己到自己，没有任何一对。
    expect(accessDiff(tags, [], [rule(4, 'home', 'home')]).gained).toBe(0)
  })

  it('停用的规则不算', () => {
    expect(accessDiff(tags, [], [{ ...ssh, enabled: false }]).gained).toBe(0)
  })

  it('双向改成单向：这些节点对少了一个方向，算作失去访问，但仍然连着', () => {
    const both = { ...ssh, direction: 'both' as const }
    const diff = accessDiff(tags, [both], [ssh])
    expect(diff.lost).toBe(4)
    expect(diff.gained).toBe(0)
    expect(diff.connectedAfter).toBe(4)
  })

  it('一对节点被两条规则连着时，去掉其中一条不算失去', () => {
    const home = rule(5, 'laptops', 'home', { direction: 'one_way' })
    // laptops → home（nas-home）被 ssh 盖住了：nas-home 也在 servers 里。
    expect(accessDiff(tags, [ssh, home], [ssh]).lost).toBe(0)
  })

  it('规则用到的标签下没有节点时没有影响', () => {
    expect(accessDiff(tags, [], [rule(6, 'laptops', 'nobody')]).gained).toBe(0)
  })

  it('节点很多时按标签的组合算，不逐对算', () => {
    const many = Array.from({ length: 2000 }, (_, index) => ref(index + 1))
    const bigTags = [
      { tag: 'all', nodes: many },
      { tag: 'a', nodes: many.slice(0, 1000) },
      { tag: 'b', nodes: many.slice(1000) },
    ]
    const diff = accessDiff(bigTags, [], [rule(1, 'a', 'b')])
    expect(diff.gained).toBe(1000 * 1000)
  })
})

describe('规则之间的比较', () => {
  it('双向规则两边对调还是同一条；单向的不是', () => {
    expect(sameRule(rule(1, 'dev', 'srv'), rule(2, 'srv', 'dev'))).toBe(true)
    expect(sameRule(rule(1, 'dev', 'srv'), rule(2, 'dev', 'srv'))).toBe(true)
    const oneWay = { direction: 'one_way' as const }
    expect(sameRule(rule(1, 'dev', 'srv', oneWay), rule(2, 'srv', 'dev', oneWay))).toBe(false)
    expect(sameRule(rule(1, 'dev', 'srv', oneWay), rule(2, 'dev', 'srv', oneWay))).toBe(true)
  })

  it('方向不同就不是同一条', () => {
    expect(sameRule(rule(1, 'dev', 'srv'), rule(2, 'dev', 'srv', { direction: 'one_way' }))).toBe(false)
  })

  it('认得出默认的 all ⇄ all', () => {
    expect(isAllToAll(DEFAULT_RULE)).toBe(true)
    expect(isAllToAll(rule(2, 'all', 'servers'))).toBe(false)
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

  it('同一个点上的几个环一个比一个大', () => {
    const small = loopAround(a, 0)
    const large = loopAround(a, 1)
    expect(small.d.startsWith('M 100 ')).toBe(true)
    expect(large.label.x).toBeGreaterThan(small.label.x)
  })
})
