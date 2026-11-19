import { describe, expect, it } from 'vitest'

import { type PathLeg } from '@/api/types'
import { t } from '@/i18n'
import { direct, viaRelays } from '@/test/fixtures'

import { describeCandidates, describeLeg } from './path'

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
  it('现问到的：绿色的"实时"，不写多久之前；有换路记录时写出时间，原因（节点的原话）另放', () => {
    const view = describeLeg(leg({ since: { secs_ago: 340, reason: '直连打通了' } }), name)
    expect(view).toMatchObject({
      state: 'query',
      sourceLabel: p.source.query,
      sourceTone: 'ok',
      rtt: '12 ms',
      age: null,
      since: t.time.minutesAgo(5),
      sinceReason: '直连打通了',
    })
    expect(view.route).toEqual({
      cls: 'direct',
      tone: 'ok',
      text: t.routes.direct,
      hops: ['laptop', 'nas'],
      addr: '203.0.113.7:20330',
      detail: null,
      detailHint: null,
    })
    // 旧的 controller 不写选路模式、原因和候选路径：都不显示。
    expect(view.mode).toBeNull()
    expect(view.reason).toBeNull()
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
      age: p.age(t.time.secondsAgo(6)),
      since: null,
      sinceReason: null,
    })
    expect(view.route).toMatchObject({
      cls: 'relay',
      tone: 'relay',
      text: t.routes.via('hk'),
      hops: ['laptop', 'hk', 'nas'],
    })
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

  it('不可达：红色，只有发送方自己', () => {
    const view = describeLeg(leg({ route: { via: 'none' }, rtt_ms: null }), name)
    expect(view.route).toMatchObject({ cls: 'none', tone: 'danger', text: t.routes.none, hops: ['laptop'] })
  })

  it('更新的 controller 给出不认识的来源：按"来源不明"显示，不出错', () => {
    const future = JSON.parse('"guess"') as PathLeg['source']
    expect(describeLeg(leg({ source: future }), name)).toMatchObject({
      state: 'unknown',
      sourceLabel: p.source.unknown,
      sourceHint: p.sourceHint.unknown,
    })
  })
})

describe('现问到时的细节：直连的方式、选路模式、原因', () => {
  it('直连写出方式；模式按自己的话写，原因是发送方的原话、原样给出', () => {
    const view = describeLeg(leg({ direct: 'punched', mode: 'lowest_cost', reason: '直连可用' }), name)
    expect(view.route?.detail).toBe(t.routes.directKind.punched)
    expect(view.route?.detailHint).toBe(t.routes.directKindHint.punched)
    expect(view.mode).toEqual({ label: p.mode.lowest_cost, hint: p.modeHint.lowest_cost })
    expect(view.reason).toBe('直连可用')
  })

  it('经 relay、第一段退回 TCP：写出来，悬停说明为什么', () => {
    const view = describeLeg(leg({ route: viaRelays(3), relay_leg: 'tcp' }), name)
    expect(view.route?.detail).toBe(t.routes.legTransport.tcp)
    expect(view.route?.detailHint).toBe(t.routes.relayLegTcpHint)
  })

  it('直连的方式只跟着直连走：经 relay 时不写直连的方式', () => {
    expect(describeLeg(leg({ route: viaRelays(3), direct: 'lan' }), name).route?.detail).toBeNull()
  })
})

describe('describeCandidates：考虑过的路', () => {
  it('不知道（不是现问到的、或者旧的节点）：不画这张表', () => {
    expect(describeCandidates(leg({}), name)).toEqual({ state: 'unknown' })
    expect(describeCandidates(leg({ candidates: null }), name)).toEqual({ state: 'unknown' })
  })

  it('问到了但手上还没有测量值', () => {
    expect(describeCandidates(leg({ candidates: [] }), name)).toEqual({ state: 'empty' })
  })

  it('按 controller 给的顺序；代价、每一段、负载写成人看的单位；现在走的标出来', () => {
    const view = describeCandidates(
      leg({
        candidates: [
          {
            route: direct('203.0.113.7:20330'),
            cost_us: 12_900,
            legs: [{ from: 2, to: 1, rtt_us: 12_400, jitter_us: 120, loss_permille: 0 }],
            load_percent: 0,
            current: true,
          },
          {
            route: viaRelays(3),
            cost_us: 49_500,
            legs: [
              { from: 2, to: 3, rtt_us: 21_300, jitter_us: 400, loss_permille: 2 },
              { from: 3, to: 1, rtt_us: 26_000, jitter_us: 0, loss_permille: 0 },
            ],
            load_percent: 12,
            current: false,
          },
        ],
      }),
      name,
    )
    if (view.state !== 'list') throw new Error('应该是一张表')
    expect(view.rows.map((row) => [row.text, row.cost, row.load, row.current])).toEqual([
      [t.routes.direct, '12.9 ms', '—', true],
      [t.routes.via('hk'), '49.5 ms', '12%', false],
    ])
    expect(view.rows[1]?.legs).toEqual([
      p.legLine('laptop', 'hk', '21.3 ms', '0.40 ms', '0.2%'),
      p.legLine('hk', 'nas', '26.0 ms', '0.00 ms', '0%'),
    ])
    expect(view.rows[0]?.tone).toBe('ok')
    expect(view.rows[1]?.tone).toBe('relay')
  })
})
