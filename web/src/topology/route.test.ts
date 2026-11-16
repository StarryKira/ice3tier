import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { direct, makeEntry, noRoute, viaRelays } from '@/test/fixtures'

import { entryText, formatMicros, formatPermille, routeDetail, routeTextWithDetail } from './route'

const name = (id: number) => ({ 1: 'hk', 2: 'laptop' })[id] ?? `#${id}`

describe('一条路的细节', () => {
  it('直连写方式：局域网、IPv6、端口可达、打洞；不认识的照实说', () => {
    for (const kind of ['lan', 'ipv6', 'reachable', 'punched', 'unknown'] as const) {
      expect(routeDetail(direct(), { direct: kind })).toBe(t.routes.directKind[kind])
    }
    expect(routeTextWithDetail(direct(), { direct: 'lan' }, name)).toBe('直连（局域网）')
  })

  it('经 relay 写第一段的传输', () => {
    expect(routeTextWithDetail(viaRelays(1), { relay_leg: 'tcp' }, name)).toBe('经 hk（第一段走 TCP）')
    expect(routeDetail(viaRelays(1), { relay_leg: 'udp' })).toBe(t.routes.relayLeg('UDP'))
  })

  it('不知道、或者旧的 controller 没写这两个字段：什么都不加', () => {
    expect(routeDetail(direct(), {})).toBeNull()
    expect(routeDetail(direct(), { direct: null, relay_leg: null })).toBeNull()
    expect(routeDetail(noRoute, { direct: 'lan' })).toBeNull()
    expect(routeTextWithDetail(viaRelays(1), {}, name)).toBe('经 hk')
  })

  it('矩阵格子的悬停文字带着细节', () => {
    const entry = makeEntry(2, 1, direct('203.0.113.7:20330'), { direct: 'punched' })
    expect(entryText(entry, name)).toMatch(/^直连（打洞），对端地址 203\.0\.113\.7:20330/)
  })
})

describe('测量值的单位', () => {
  it('微秒写成毫秒', () => {
    expect(formatMicros(120)).toBe('0.12 ms')
    expect(formatMicros(12_400)).toBe('12.4 ms')
    expect(formatMicros(150_000)).toBe('150 ms')
  })

  it('千分之几写成百分数', () => {
    expect(formatPermille(0)).toBe('0%')
    expect(formatPermille(5)).toBe('0.5%')
    expect(formatPermille(20)).toBe('2%')
  })
})
