import { describe, expect, it } from 'vitest'

import { type AccessGraph, type Rule } from '@/api/types'

import { buildAccessMap, layoutAccessMap, MAP_SIZE, nodeTags, pillWidth } from './map'

const ref = (id: number, name: string) => ({ id, name, virtual_ip: `10.77.0.${id}` })

const rule = (id: number, src: string, dst: string, extra: Partial<Rule> = {}): Rule => ({
  id,
  name: `r${id}`,
  src,
  dst,
  direction: 'both',
  enabled: true,
  ...extra,
})

const graph = (rules: Rule[]): AccessGraph => ({
  tags: [
    { tag: 'all', nodes: [ref(1, 'a'), ref(2, 'b'), ref(3, 'c')] },
    { tag: 'laptops', nodes: [ref(1, 'a')] },
    { tag: 'servers', nodes: [ref(2, 'b'), ref(3, 'c')] },
  ],
  rules,
  unconnected: [],
  unacknowledged: [],
})

describe('访问地图', () => {
  it('节点：它的标签和 all 上的规则都算；流向从它这边看', () => {
    const map = buildAccessMap(
      graph([
        rule(1, 'all', 'all'),
        rule(2, 'laptops', 'servers', { direction: 'one_way' }),
        rule(3, 'servers', 'laptops', { direction: 'one_way' }),
        rule(4, 'servers', 'servers'),
      ]),
      nodeTags(['laptops']),
    )
    expect(map.rules.map((entry) => [entry.rule.id, entry.flow, entry.other])).toEqual([
      [1, 'both', 'all'],
      [2, 'out', 'servers'],
      [3, 'in', 'servers'],
    ])
    // 同一个标签只画一次，写着有几个节点。
    expect(map.targets).toEqual([
      { id: 'target:all', tag: 'all', members: 3 },
      { id: 'target:servers', tag: 'servers', members: 2 },
    ])
  })

  it('标签：只看直接写着这个标签的规则；停用的排在后面', () => {
    const map = buildAccessMap(
      graph([rule(1, 'all', 'all'), rule(2, 'laptops', 'servers', { enabled: false }), rule(3, 'servers', 'laptops')]),
      new Set(['servers']),
    )
    expect(map.rules.map((entry) => [entry.rule.id, entry.other])).toEqual([
      [3, 'laptops'],
      [2, 'laptops'],
    ])
    expect(map.targets.map((target) => target.tag)).toEqual(['laptops'])
  })

  it('没有和它有关的规则：两列都是空的', () => {
    expect(buildAccessMap(graph([rule(1, 'laptops', 'laptops')]), new Set(['servers']))).toEqual({
      rules: [],
      targets: [],
    })
  })

  it('布局：三列从左到右，每一列以中线为中心竖着排开', () => {
    const map = buildAccessMap(graph([rule(1, 'all', 'all'), rule(2, 'laptops', 'servers')]), nodeTags(['laptops']))
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
