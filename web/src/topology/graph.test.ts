import { describe, expect, it } from 'vitest'

import { type MatrixNode, type RouteMatrix } from '@/api/types'
import { t } from '@/i18n'
import { direct, makeEntry, noRoute, viaRelays } from '@/test/fixtures'

import { bendAround, buildTopology, layoutSignature, NODE_BOX, pointId, STRETCH_X, type TopoPoint } from './graph'
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
    // 外圈的点在同一个横向拉宽的椭圆上，而且不和中间的 relay 挤在一起。
    const radii = outer.map((point) => Math.round(Math.hypot(point.x / STRETCH_X, point.y)))
    expect(Math.max(...radii) - Math.min(...radii)).toBeLessThanOrEqual(1)
    expect(radii[0]).toBeGreaterThan(NODE_BOX.height * 3)
    expect(Math.max(...outer.map((point) => Math.abs(point.x)))).toBeGreaterThan(
      Math.max(...outer.map((point) => Math.abs(point.y))),
    )
  })

  it('relay 和别的节点差不多多（三个 relay、两个节点）：全部排成一圈，任意两点之间的线都不压到第三张卡片', () => {
    const matrix: RouteMatrix = {
      nodes: [node(1, 'h1'), node(2, 'h2'), node(3, 'tx'), node(4, 'tyo1'), node(5, 'suq')],
      entries: [],
    }
    const points = buildTopology(matrix, new Set([1, 2, 5]), null).points
    expect(points.map((point) => point.node.name)).toEqual(['h1', 'h2', 'suq', 'tx', 'tyo1'])
    expect(points.filter((point) => point.relay).map((point) => point.node.name)).toEqual(['h1', 'h2', 'suq'])
    const radii = points.map((point) => Math.round(Math.hypot(point.x / STRETCH_X, point.y)))
    expect(Math.max(...radii) - Math.min(...radii)).toBeLessThanOrEqual(1)
    for (const a of points) for (const b of points) if (a !== b) expect(bendAround(points, a.id, b.id)).toBe(0)
  })

  it('relay 少、别的节点多：relay 在里圈，外圈转开，不和里圈的点对齐', () => {
    const matrix: RouteMatrix = {
      nodes: [node(1, 'r1'), node(2, 'r2'), ...[3, 4, 5, 6].map((id) => node(id, `n${id}`))],
      entries: [],
    }
    const points = buildTopology(matrix, new Set([1, 2]), null).points
    const inner = points.filter((point) => point.relay)
    const outer = points.filter((point) => !point.relay)
    expect(inner.map((point) => point.node.name)).toEqual(['r1', 'r2'])
    const angle = (point: TopoPoint) => Math.atan2(point.y, point.x / STRETCH_X)
    for (const a of outer) {
      for (const b of inner) {
        const gap = Math.abs(angle(a) - angle(b)) % (2 * Math.PI)
        expect(Math.min(gap, 2 * Math.PI - gap)).toBeGreaterThan(Math.PI / 8)
      }
    }
  })

  it('一条线笔直连过去会压到别的卡片时弯开，弯向卡片的另一边；不压到就是直线', () => {
    const at = (id: number, x: number, y: number): TopoPoint => ({
      id: pointId(id),
      node: node(id, `n${id}`),
      relay: false,
      x,
      y,
      ...NODE_BOX,
      picked: null,
    })
    const blocked = [at(1, -400, 0), at(2, 400, 0), at(3, 0, 10)]
    const bend = bendAround(blocked, pointId(1), pointId(2))
    // 卡片在线的下方（y 大）：法线朝 +y，曲线弯向 −y。弯开之后曲线的中点越过卡片的上边，还留出空隙。
    expect(bend).toBeLessThan(0)
    expect(Math.abs(bend) / 2).toBeGreaterThanOrEqual(NODE_BOX.height / 2 - 10 + 16)
    expect(bendAround([at(1, -400, 0), at(2, 400, 0), at(3, 0, 200)], pointId(1), pointId(2))).toBe(0)
  })

  it('两个节点之间每种连法一条线：两个方向都直连合成一条；经 relay 的连着两端，写着经谁', () => {
    const shape = buildTopology(MATRIX, RELAYS, null)
    const byId = new Map(shape.links.map((link) => [link.id, link]))
    expect([...byId.keys()].sort()).toEqual(['link:2-3:direct', 'link:2-4:1', 'link:3-5:direct'])
    // laptop ⇄ nas 两个方向都是直连，合成一条，延迟取低的。
    expect(byId.get('link:2-3:direct')).toMatchObject({
      kind: 'direct',
      forward: true,
      reverse: true,
      stale: false,
      rtt: 3,
      siblings: 1,
    })
    expect(byId.get('link:2-3:direct')?.entries).toHaveLength(2)
    // laptop → hk → office 和 office → hk → laptop：一条连着 laptop 和 office 的线，经 hk，全程的延迟。
    expect(byId.get('link:2-4:1')).toMatchObject({ kind: 'relay', relays: [1], forward: true, reverse: true, rtt: 40 })
  })

  it('两个方向走法不一样时各一条、带方向、并排弯开；经 relay 的写出每一段的延迟和到第一个 relay 的传输', () => {
    const matrix: RouteMatrix = {
      nodes: [node(1, 'hk'), node(2, 'laptop'), node(3, 'nas')],
      entries: [
        makeEntry(1, 3, direct(), { rtt_ms: 45 }),
        makeEntry(2, 1, direct(), { rtt_ms: 30 }),
        makeEntry(2, 3, viaRelays(1), { rtt_ms: 80, relay_leg: 'tcp' }),
        makeEntry(3, 2, direct(), { rtt_ms: 50, direct: 'punched' }),
      ],
    }
    const pair = buildTopology(matrix, new Set([1]), null).links.filter((link) => link.id.startsWith('link:2-3:'))
    expect(pair).toEqual([
      expect.objectContaining({
        id: 'link:2-3:1',
        kind: 'relay',
        forward: true,
        reverse: false,
        transport: 'tcp',
        legs: [
          { from: 2, to: 1, rtt: 30 },
          { from: 1, to: 3, rtt: 45 },
        ],
        index: 1,
        siblings: 2,
      }),
      expect.objectContaining({
        id: 'link:2-3:direct',
        kind: 'direct',
        forward: false,
        reverse: true,
        directKind: 'punched',
        index: 0,
        siblings: 2,
      }),
    ])
  })

  it('ice3proxy：一对节点之间有连接时另一条线，两头报的取大的；过期的、什么都没有的不算', () => {
    const lanes = (in_use: number, idle: number, control = true) => ({ control, in_use, idle })
    const matrix: RouteMatrix = {
      nodes: [node(1, 'hk'), node(2, 'laptop'), node(3, 'nas')],
      entries: [
        makeEntry(1, 2, direct(), { lanes: lanes(2, 1) }),
        makeEntry(2, 1, direct(), { lanes: lanes(1, 3) }),
        makeEntry(1, 3, direct(), { lanes: lanes(0, 0, false) }),
        makeEntry(3, 2, direct(), { lanes: lanes(4, 0), stale: true }),
      ],
    }
    const links = buildTopology(matrix, new Set([1]), null).links
    expect(links.filter((link) => link.kind === 'lane')).toEqual([
      expect.objectContaining({
        id: 'link:1-2:lane',
        lanes: { control: true, in_use: 2, idle: 3 },
        forward: true,
        reverse: true,
        index: 1,
        siblings: 2,
      }),
    ])
  })

  it('过期的格子画出来但标成过期；两个方向都无路可走的一对画一条不可达的线', () => {
    const shape = buildTopology(MATRIX, RELAYS, null)
    expect(shape.links.find((link) => link.id === 'link:3-5:direct')).toMatchObject({ stale: true })
    expect(shape.unreachable).toEqual([
      expect.objectContaining({ id: 'none:3-6', source: pointId(3), target: pointId(6), stale: false }),
    ])
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
    expect(shape.links.map((link) => link.id)).toEqual(['link:1-2:3'])
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
    expect(routeText(viaRelays(1, 4), name)).toBe(t.routes.via(['hk', 'office'].join(t.routes.hopSeparator)))
    expect(routeText(direct(), name)).toBe(t.routes.direct)
  })

  it('矩阵一格的完整说法带地址、延迟、新旧', () => {
    const stale = MATRIX.entries.find((entry) => entry.stale)
    if (stale === undefined) throw new Error('没有过期的格子')
    const r = t.routes
    expect(entryText(stale, name)).toBe(
      [r.direct, r.addr('203.0.113.7:20330'), r.rtt(9), r.age(t.time.hoursAgo(1)), r.stale].join(r.factSeparator),
    )
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
