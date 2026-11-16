import { describe, expect, it } from 'vitest'

import { type PathLeg } from '@/api/types'
import { t } from '@/i18n'
import { direct, viaRelays } from '@/test/fixtures'

import { describeLeg } from './path'

const p = t.path

const LAPTOP = { id: 2, name: 'laptop', virtual_ip: '10.66.0.2' }
const NAS = { id: 1, name: 'nas', virtual_ip: '10.66.0.1' }
const names: Record<number, string> = { 1: 'nas', 2: 'laptop', 3: 'hk' }
const name = (id: number) => names[id] ?? `#${id}`

function leg(extra: Partial<PathLeg>): PathLeg {
  return {
    from: LAPTOP,
    to: NAS,
    source: 'query',
    route: direct('203.0.113.7:20330'),
    rtt_ms: 12,
    age_secs: 0,
    since: null,
    ...extra,
  }
}

describe('describeLeg：路径诊断里一个方向怎么说', () => {
  it('现问到的：绿色的"刚刚问到"，不写多久之前；有换路记录时写出时间和原因', () => {
    const view = describeLeg(leg({ since: { secs_ago: 340, reason: '直连打通了' } }), name)
    expect(view).toMatchObject({
      state: 'query',
      sourceLabel: p.source.query,
      sourceTone: 'ok',
      rtt: '12 ms',
      age: null,
      since: p.since('5 分钟前', '直连打通了'),
    })
    expect(view.route).toEqual({
      cls: 'direct',
      tone: 'ok',
      text: '直连',
      hops: ['laptop', 'nas'],
      addr: '203.0.113.7:20330',
    })
  })

  it('现问到、这次启动以来没换过路：照实说', () => {
    expect(describeLeg(leg({}), name).since).toBe(p.sinceUnknown)
  })

  it('问不到、用的是上报：灰色，标出多久之前；换路的原因只有现问才有', () => {
    const view = describeLeg(leg({ source: 'report', route: viaRelays(3), rtt_ms: 48, age_secs: 6 }), name)
    expect(view).toMatchObject({
      state: 'report',
      sourceLabel: p.source.report,
      sourceTone: 'idle',
      age: p.age('6 秒前'),
      since: null,
    })
    expect(view.route).toMatchObject({ cls: 'relay', tone: 'relay', text: '经 hk', hops: ['laptop', 'hk', 'nas'] })
  })

  it('不是对端：琥珀色，没有路', () => {
    const view = describeLeg(leg({ source: 'not_peers', route: null, rtt_ms: null, age_secs: null }), name)
    expect(view).toMatchObject({ state: 'not_peers', sourceTone: 'warn', route: null, rtt: null, age: null })
    expect(view.sourceHint).toBe(p.sourceHint.not_peers)
  })

  it('没有数据：灰色，什么都不编', () => {
    const view = describeLeg(leg({ source: 'none', route: null, rtt_ms: null, age_secs: null }), name)
    expect(view).toMatchObject({ state: 'none', sourceLabel: p.source.none, route: null, since: null })
  })

  it('无路可走：红色，只有发送方自己', () => {
    const view = describeLeg(leg({ route: { via: 'none' }, rtt_ms: null }), name)
    expect(view.route).toMatchObject({ cls: 'none', tone: 'danger', text: t.routes.none, hops: ['laptop'] })
  })

  it('更新的 controller 给出不认识的来源：按"来源不明"显示，不出错', () => {
    const future = JSON.parse('"guess"') as PathLeg['source']
    expect(describeLeg(leg({ source: future }), name)).toMatchObject({
      state: 'unknown',
      sourceLabel: p.source.unknown,
    })
  })
})
