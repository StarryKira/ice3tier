import { describe, expect, it } from 'vitest'

import { type ExitReport } from '@/api/types'
import { t } from '@/i18n'
import { makeExitView } from '@/test/fixtures'

import { exitHealthView } from './exitView'

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
