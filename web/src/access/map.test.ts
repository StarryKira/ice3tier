import { describe, expect, it } from 'vitest'

import { type AccessGraph, type Group, type GroupRef, type Rule } from '@/api/types'
import { nodeGroupIds } from '@/lib/groups'

import { buildAccessMap, layoutAccessMap, MAP_SIZE, pillWidth } from './map'

const ref = (id: number, name: string) => ({ id, name, virtual_ip: `10.77.0.${id}` })

const ALL: GroupRef = { id: 1, name: 'All' }
const LAPTOPS: GroupRef = { id: 2, name: 'laptops' }
const SERVERS: GroupRef = { id: 3, name: 'servers' }

const rule = (id: number, src: GroupRef, dst: GroupRef, extra: Partial<Rule> = {}): Rule => ({
  id,
  name: `r${id}`,
  src,
  dst,
  direction: 'both',
  enabled: true,
  ...extra,
})

const group = (of: GroupRef, nodes: ReturnType<typeof ref>[]): Group => ({
  ...of,
  builtin: of.id === ALL.id,
  nodes,
  rules: [],
})

const graph = (rules: Rule[]): AccessGraph => ({
  groups: [
    group(ALL, [ref(1, 'a'), ref(2, 'b'), ref(3, 'c')]),
    group(LAPTOPS, [ref(1, 'a')]),
    group(SERVERS, [ref(2, 'b'), ref(3, 'c')]),
  ],
  rules,
  unconnected: [],
  unacknowledged: [],
})

/** 节点 a 在 laptops 里；它所在的分组还有 All。 */
const nodeA = (rules: Rule[]) => nodeGroupIds({ groups: [LAPTOPS] }, graph(rules).groups)

describe('访问地图', () => {
  it('节点：它的分组和 All 上的规则都算；流向从它这边看', () => {
    const rules = [
      rule(1, ALL, ALL),
      rule(2, LAPTOPS, SERVERS, { direction: 'one_way' }),
      rule(3, SERVERS, LAPTOPS, { direction: 'one_way' }),
      rule(4, SERVERS, SERVERS),
    ]
    const map = buildAccessMap(graph(rules), nodeA(rules))
    expect(map.rules.map((entry) => [entry.rule.id, entry.flow, entry.other.name])).toEqual([
      [1, 'both', 'All'],
      [2, 'out', 'servers'],
      [3, 'in', 'servers'],
    ])
    // 同一个分组只画一次，写着有几个节点；按编号认，不按名字。
    expect(map.targets).toEqual([
      { id: 'target:1', group: ALL, members: 3 },
      { id: 'target:3', group: SERVERS, members: 2 },
    ])
  })

  it('分组：只看直接写着这个分组的规则；停用的排在后面', () => {
    const map = buildAccessMap(
      graph([rule(1, ALL, ALL), rule(2, LAPTOPS, SERVERS, { enabled: false }), rule(3, SERVERS, LAPTOPS)]),
      new Set([SERVERS.id]),
    )
    expect(map.rules.map((entry) => [entry.rule.id, entry.other.name])).toEqual([
      [3, 'laptops'],
      [2, 'laptops'],
    ])
    expect(map.targets.map((target) => target.group.name)).toEqual(['laptops'])
  })

  it('分组改了名：规则记的是编号，照样连得上，写的是现在的名字', () => {
    const renamed = { ...LAPTOPS, name: 'notebooks' }
    const map = buildAccessMap(graph([rule(2, renamed, SERVERS)]), new Set([SERVERS.id]))
    expect(map.targets).toEqual([{ id: 'target:2', group: renamed, members: 1 }])
  })

  it('没有和它有关的规则：两列都是空的', () => {
    expect(buildAccessMap(graph([rule(1, LAPTOPS, LAPTOPS)]), new Set([SERVERS.id]))).toEqual({
      rules: [],
      targets: [],
    })
  })

  it('节点所在的分组：它自己的加上 All；还不知道 All 的编号时只有它自己的', () => {
    expect([...nodeGroupIds({ groups: [LAPTOPS] }, graph([]).groups)].sort()).toEqual([1, 2])
    expect([...nodeGroupIds({ groups: [LAPTOPS] }, undefined)]).toEqual([2])
  })

  it('布局：三列从左到右，每一列以中线为中心竖着排开', () => {
    const rules = [rule(1, ALL, ALL), rule(2, LAPTOPS, SERVERS)]
    const map = buildAccessMap(graph(rules), nodeA(rules))
    const layout = layoutAccessMap(map, () => '双向')
    expect(layout.subject.y).toBe(-MAP_SIZE.subject.height / 2)
    const pills = [...layout.pills.values()]
    const targets = [...layout.targets.values()]
    expect(pills.every((pill) => pill.x > layout.subject.x + MAP_SIZE.subject.width)).toBe(true)
    expect(targets.every((target) => target.x > Math.max(...pills.map((pill) => pill.x + pill.width)))).toBe(true)
    // 两个胶囊上下对称。
    expect((pills[0]?.y ?? 0) + (pills[1]?.y ?? 0) + MAP_SIZE.pill.height).toBe(0)
  })

  it('胶囊的宽度随名字变，但有上下限', () => {
    expect(pillWidth('a', '双向')).toBe(MAP_SIZE.pill.min)
    expect(pillWidth('一个特别特别特别特别特别特别特别特别长的规则名字', '双向')).toBe(MAP_SIZE.pill.max)
  })
})
