import { describe, expect, it } from 'vitest'

import { type MatrixNode, type RouteMatrix } from '@/api/types'
import { direct, makeEntry, noRoute, viaRelays } from '@/test/fixtures'

import { buildTopology, layoutSignature, pointId } from './graph'
import { nextPick, pairPath, readPair, writePair } from './pair'
import { classify, entryText, hopsOf, relayedPairs, routeText } from './route'

const node = (id: number, name: string, online = true): MatrixNode => ({
  id,
  name,
  virtual_ip: `10.66.0.${id}`,
  online,
})

/**
 * hk(1) 是 relay。laptop(2) 和 nas(3) 互相直连；laptop 到 office(4) 经 hk，office 回来也经 hk；
 * phone(5) 离线，它离线前报的到 nas 的路过期了；nas 和 tv(6) 两个方向都无路可走。
 */
const MATRIX: RouteMatrix = {
  nodes: [node(1, 'hk'), node(2, 'laptop'), node(3, 'nas'), node(4, 'office'), node(5, 'phone', false), node(6, 'tv')],
  entries: [
    makeEntry(2, 3, direct('192.168.1.3:20330'), { rtt_ms: 3 }),
    makeEntry(3, 2, direct('192.168.1.2:20330'), { rtt_ms: 4 }),
    makeEntry(2, 4, viaRelays(1), { rtt_ms: 40 }),
    makeEntry(4, 2, viaRelays(1), { rtt_ms: 42 }),
    makeEntry(5, 3, direct(), { rtt_ms: 9, stale: true, age_secs: 5210 }),
    makeEntry(3, 6, noRoute, { rtt_ms: null }),
    makeEntry(6, 3, noRoute, { rtt_ms: null }),
  ],
}

const RELAYS = new Set([1])

describe('buildTopology：路由矩阵整理成拓扑图', () => {
  it('每个节点一个点；relay 在中间，其余按名字排在外圈', () => {
    const shape = buildTopology(MATRIX, RELAYS, null)
    expect(shape.points).toHaveLength(6)
    const hk = shape.points.find((point) => point.id === pointId(1))
    expect(hk).toMatchObject({ relay: true, x: 0, y: 0 })
    const outer = shape.points.filter((point) => !point.relay)
    expect(outer.map((point) => point.node.name)).toEqual(['laptop', 'nas', 'office', 'phone', 'tv'])
    // 外圈的点离中心一样远，而且不和中间的 relay 挤在一起。
    const radii = outer.map((point) => Math.round(Math.hypot(point.x, point.y)))
    expect(new Set(radii).size).toBe(1)
    expect(radii[0]).toBeGreaterThan(200)
  })

  it('两个点之间不管有几条路经过都只画一条线；直连的标延迟，经 relay 的拆成几段', () => {
    const shape = buildTopology(MATRIX, RELAYS, null)
    const byId = new Map(shape.links.map((link) => [link.id, link]))
    expect([...byId.keys()].sort()).toEqual(['link:1-2', 'link:1-4', 'link:2-3', 'link:3-5'])
    // laptop ⇄ nas 两个方向都是直连，合成一条，延迟取低的。
    expect(byId.get('link:2-3')).toMatchObject({ direct: true, relayed: false, stale: false, rtt: 3 })
    expect(byId.get('link:2-3')?.entries).toHaveLength(2)
    // laptop → hk → office 和 office → hk → laptop：每一段各经过两格。
    expect(byId.get('link:1-2')).toMatchObject({ direct: false, relayed: true, rtt: null })
    expect(byId.get('link:1-4')?.entries).toHaveLength(2)
  })

  it('过期的格子画出来但标成过期；两个方向都无路可走的一对画一条不可达的线', () => {
    const shape = buildTopology(MATRIX, RELAYS, null)
    expect(shape.links.find((link) => link.id === 'link:3-5')).toMatchObject({ stale: true })
    expect(shape.unreachable).toEqual([{ id: 'none:3-6', source: pointId(3), target: pointId(6), stale: false }])
  })

  it('一端离线时没有路是理所当然的，不画不可达的线（矩阵里照样有那一格）', () => {
    const matrix: RouteMatrix = {
      nodes: [node(1, 'a'), node(2, 'b', false)],
      entries: [makeEntry(1, 2, noRoute)],
    }
    expect(buildTopology(matrix, new Set(), null).unreachable).toEqual([])
  })

  it('一个方向有路时那一对不算不可达', () => {
    const matrix: RouteMatrix = {
      nodes: [node(1, 'a'), node(2, 'b')],
      entries: [makeEntry(1, 2, noRoute), makeEntry(2, 1, direct())],
    }
    const shape = buildTopology(matrix, new Set(), null)
    expect(shape.unreachable).toEqual([])
    expect(shape.links).toHaveLength(1)
  })

  it('选中一对：两个方向的路一段一段带方向；走同一段时一左一右弯开', () => {
    const shape = buildTopology(MATRIX, RELAYS, { a: 2, b: 3 })
    expect(shape.forward?.route).toEqual(direct('192.168.1.3:20330'))
    expect(shape.highlight).toEqual([
      expect.objectContaining({
        direction: 'forward',
        source: pointId(2),
        target: pointId(3),
        kind: 'direct',
        index: 0,
        siblings: 2,
      }),
      expect.objectContaining({
        direction: 'reverse',
        source: pointId(3),
        target: pointId(2),
        kind: 'direct',
        index: 1,
        siblings: 2,
      }),
    ])
    expect([...shape.involved].sort()).toEqual([pointId(2), pointId(3)])
    expect(shape.points.find((point) => point.id === pointId(2))?.picked).toBe('a')
    expect(shape.points.find((point) => point.id === pointId(3))?.picked).toBe('b')
  })

  it('选中一对经 relay 的：经过的 relay 也算在"相关"里，图上不变暗', () => {
    const shape = buildTopology(MATRIX, RELAYS, { a: 2, b: 4 })
    expect(shape.highlight.filter((hop) => hop.direction === 'forward').map((hop) => [hop.source, hop.target])).toEqual(
      [
        [pointId(2), pointId(1)],
        [pointId(1), pointId(4)],
      ],
    )
    expect(shape.highlight.every((hop) => hop.kind === 'relay')).toBe(true)
    expect(shape.involved.has(pointId(1))).toBe(true)
  })

  it('选中的一对在矩阵里没有数据：没有可高亮的，两个方向都是空的', () => {
    const shape = buildTopology(MATRIX, RELAYS, { a: 4, b: 6 })
    expect(shape.forward).toBeNull()
    expect(shape.reverse).toBeNull()
    expect(shape.highlight).toEqual([])
  })

  it('路由里作为中间一跳出现的节点也画成 relay；经过已删除节点的路不画', () => {
    const matrix: RouteMatrix = {
      nodes: [node(1, 'a'), node(2, 'b'), node(3, 'c')],
      entries: [makeEntry(1, 2, viaRelays(3)), makeEntry(2, 1, viaRelays(99))],
    }
    const shape = buildTopology(matrix, new Set(), null)
    expect(shape.points.find((point) => point.id === pointId(3))?.relay).toBe(true)
    expect(shape.links.map((link) => link.id).sort()).toEqual(['link:1-3', 'link:2-3'])
  })

  it('布局的指纹只跟点和 relay 的集合有关：路变了，点的位置不动', () => {
    const before = buildTopology(MATRIX, RELAYS, null)
    const after = buildTopology({ ...MATRIX, entries: [] }, RELAYS, { a: 2, b: 3 })
    expect(layoutSignature(after)).toBe(layoutSignature(before))
    // hk 不再是 relay（也没有路经过它）：它挪到外圈去，点要重新排。
    expect(layoutSignature(buildTopology({ ...MATRIX, entries: [] }, new Set(), null))).not.toBe(
      layoutSignature(before),
    )
  })

  it('几十个节点也排得开：外圈相邻两个点的中心距离放得下一个点', () => {
    const many: RouteMatrix = {
      nodes: Array.from({ length: 40 }, (_, index) => node(index + 1, `n${index + 1}`)),
      entries: [],
    }
    const points = buildTopology(many, new Set([1, 2, 3]), null).points.filter((point) => !point.relay)
    const [first, second] = points
    if (first === undefined || second === undefined) throw new Error('没有点')
    expect(Math.hypot(first.x - second.x, first.y - second.y)).toBeGreaterThanOrEqual(first.width)
  })
})

describe('一条路的说法', () => {
  const name = (id: number) => MATRIX.nodes.find((candidate) => candidate.id === id)?.name ?? `#${id}`

  it('分四类：直连、一级中转、多级中转、不可达', () => {
    expect(classify(direct())).toBe('direct')
    expect(classify(viaRelays(1))).toBe('relay')
    expect(classify(viaRelays(1, 4))).toBe('multi')
    expect(classify(noRoute)).toBe('none')
  })

  it('经过的节点和一句话', () => {
    expect(hopsOf(2, 4, viaRelays(1))).toEqual([2, 1, 4])
    expect(hopsOf(2, 3, direct())).toEqual([2, 3])
    expect(hopsOf(2, 3, noRoute)).toBeNull()
    expect(routeText(viaRelays(1, 4), name)).toBe('经 hk → office')
    expect(routeText(direct(), name)).toBe('直连')
  })

  it('矩阵一格的完整说法带地址、延迟、新旧', () => {
    const stale = MATRIX.entries.find((entry) => entry.stale)
    if (stale === undefined) throw new Error('没有过期的格子')
    expect(entryText(stale, name)).toBe('直连，对端地址 203.0.113.7:20330，9 ms，1 小时前的上报，过期')
  })

  it('数得出经一个 relay 中转的节点对：两个方向算一对，过期的不算', () => {
    expect(relayedPairs(MATRIX, 1)).toBe(1)
    expect(relayedPairs(MATRIX, 3)).toBe(0)
    expect(relayedPairs(undefined, 1)).toBe(0)
  })
})

describe('选中的一对写在地址里', () => {
  it('读写 a、b，别的参数不动', () => {
    const params = new URLSearchParams('view=matrix&a=2&b=x')
    expect(readPair(params)).toEqual({ a: 2, b: null })
    expect(writePair(params, { a: 3, b: 4 }).toString()).toBe('view=matrix&a=3&b=4')
    expect(writePair(params, { a: null, b: null }).toString()).toBe('view=matrix')
    expect(pairPath(1, 'diagnosis', 2, 3)).toBe('/n/1/diagnosis?a=2&b=3')
    expect(pairPath(1, 'topology', null, null)).toBe('/n/1/topology')
  })

  it('在图上先后点两个节点：第一下是 A，第二下是 B，再点从 A 重新开始', () => {
    const first = nextPick({ a: null, b: null }, 2)
    expect(first).toEqual({ a: 2, b: null })
    expect(nextPick(first, 2)).toEqual(first)
    const second = nextPick(first, 3)
    expect(second).toEqual({ a: 2, b: 3 })
    expect(nextPick(second, 5)).toEqual({ a: 5, b: null })
  })
})
