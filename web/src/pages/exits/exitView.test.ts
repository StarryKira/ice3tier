import { describe, expect, it } from 'vitest'

import { type ExitReport } from '@/api/types'
import { t } from '@/i18n'
import { makeExitView } from '@/test/fixtures'

import { crossCheck, exitHealthView, MISMATCH_FLOOR } from './exitView'

const h = t.exits.health
const MB = 1024 ** 2

const report = (extra: Partial<ExitReport> = {}): ExitReport => ({
  active: 0,
  total: 100,
  bytes_to_target: 10 * MB,
  bytes_from_target: 90 * MB,
  failed_by_target: 0,
  refused: 0,
  failed_by_exit: 0,
  aborted: 0,
  draining: false,
  quota: null,
  ...extra,
})

const seen = (bytesUp: number, bytesDown: number) => ({
  by: { id: 2, name: 'laptop', virtual_ip: '10.66.0.2' },
  exit: 1,
  flows: 10,
  bytes_up: bytesUp,
  bytes_down: bytesDown,
  failed_by_target: 0,
  refused: 0,
  failed_by_exit: 0,
  unreachable: 0,
  reported_secs_ago: 4,
})

describe('出口的健康', () => {
  it('正常：带着最近 5 分钟的数', () => {
    const view = exitHealthView(makeExitView(1, 'gw', { health: 'ok', recent_requests: 120, recent_failures: 1 }))
    expect(view).toEqual({ tone: 'ok', label: h.ok, detail: h.okDetail(120, 1) })
  })

  it('有问题：琥珀色（还能用），说是出口自己的失败', () => {
    const view = exitHealthView(makeExitView(1, 'gw', { health: 'degraded', recent_requests: 20, recent_failures: 5 }))
    expect(view.tone).toBe('warn')
    expect(view.detail).toBe(h.degradedDetail(20, 5))
  })

  it('说不上来：最近没有请求', () => {
    expect(exitHealthView(makeExitView(1, 'gw', { health: 'unknown' })).label).toBe(h.unknown)
  })

  it('离线和排空比算出来的健康更要紧', () => {
    expect(exitHealthView(makeExitView(1, 'gw', { online: false, health: 'ok' })).label).toBe(h.offline)
    const draining = exitHealthView(makeExitView(1, 'gw', { usage: report({ draining: true, active: 3 }) }))
    expect(draining).toMatchObject({ tone: 'warn', label: h.draining, detail: h.drainingDetail(3) })
  })
})

describe('出口报的和用它的节点看到的对照', () => {
  it('两边都要有数才对照', () => {
    expect(crossCheck(makeExitView(1, 'gw'))).toBeNull()
    expect(crossCheck(makeExitView(1, 'gw', { usage: report() }))).toBeNull()
  })

  it('差得不多：不提醒', () => {
    const check = crossCheck(makeExitView(1, 'gw', { usage: report(), observed: [seen(10 * MB, 88 * MB)] }))
    expect(check).toEqual({ exitBytes: 100 * MB, observedBytes: 98 * MB, mismatch: false })
  })

  it('几个节点看到的加起来比', () => {
    const check = crossCheck(
      makeExitView(1, 'gw', { usage: report(), observed: [seen(5 * MB, 45 * MB), seen(5 * MB, 45 * MB)] }),
    )
    expect(check?.observedBytes).toBe(100 * MB)
    expect(check?.mismatch).toBe(false)
  })

  it('差出四分之一以上、又超过一个下限：提醒', () => {
    const check = crossCheck(makeExitView(1, 'gw', { usage: report(), observed: [seen(5 * MB, 40 * MB)] }))
    expect(check?.mismatch).toBe(true)
  })

  it('数很小的时候差得比例大也不提醒：刚开始用，时间差就能差出这么多', () => {
    const usage = report({ bytes_to_target: 1000, bytes_from_target: 2000 })
    const check = crossCheck(makeExitView(1, 'gw', { usage, observed: [seen(0, 0)] }))
    expect(check?.mismatch).toBe(false)
    expect(MISMATCH_FLOOR).toBeGreaterThan(3000)
  })
})
