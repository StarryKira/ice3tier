import { describe, expect, it } from 'vitest'

import { type BackboneRelay } from '@/api/types'
import { t } from '@/i18n'

import { linkView, relayRowView } from './backbone'

const b = t.topology.backbone
const name = (id: number) => ({ 2: 'laptop', 3: 'hk', 5: 'la' })[id] ?? `#${id}`

function relay(extra: Partial<BackboneRelay> = {}): BackboneRelay {
  return {
    relay: { id: 3, name: 'hk', virtual_ip: '10.66.0.3' },
    online: true,
    state: 'serving',
    load_percent: 12,
    age_ms: 1800,
    reported_by: 2,
    links: [
      { to: 5, rtt_ms: 150, jitter_ms: 2, loss_permille: 5 },
      { to: 7, rtt_ms: null, jitter_ms: 0, loss_permille: 0 },
      { to: 8, rtt_ms: 40, jitter_ms: 1, loss_permille: 30 },
    ],
    ...extra,
  }
}

describe('骨干矩阵一行的表头', () => {
  it('状态、负载、多新、从谁那里问来', () => {
    expect(relayRowView(relay(), name)).toEqual({
      state: { label: b.state.serving, tone: 'ok', hint: b.stateHint.serving },
      load: b.load(12),
      age: b.rowAge(t.time.secondsAgo(2)),
      reportedBy: b.reportedBy('laptop'),
    })
  })

  it('满载、即将下线是琥珀色：它还在，只是节点会改走别的', () => {
    expect(relayRowView(relay({ state: 'overloaded' }), name).state.tone).toBe('warn')
    expect(relayRowView(relay({ state: 'draining' }), name).state.label).toBe(b.state.draining)
  })

  it('比界面新的 controller 报了不认识的状态：按"状态不明"写，灰色', () => {
    const future = JSON.parse('"resting"') as BackboneRelay['state']
    expect(relayRowView(relay({ state: future }), name).state).toEqual({
      label: b.state.unknown,
      tone: 'idle',
      hint: b.stateHint.unknown,
    })
  })

  it('问到的节点手上都没有这一行', () => {
    const view = relayRowView(
      relay({ state: null, load_percent: null, age_ms: null, reported_by: null, links: [] }),
      name,
    )
    expect(view).toEqual({
      state: { label: b.noRow, tone: 'idle', hint: b.noRowHint },
      load: null,
      age: null,
      reportedBy: null,
    })
  })
})

describe('两个 relay 之间的一格', () => {
  it('连得上：大字是延迟，第二行抖动和丢包', () => {
    expect(linkView(relay(), 5)).toEqual({
      rtt: 150,
      detail: b.detail(2, '0.5%'),
      lossy: false,
      tone: 'ok',
      text: b.cell(150, 2, '0.5%'),
    })
  })

  it('连不上：红色的 ✕，第二行写不可达', () => {
    expect(linkView(relay(), 7)).toMatchObject({ rtt: null, detail: b.unreachable, tone: 'danger' })
  })

  it('丢包到了百分之一：琥珀色', () => {
    expect(linkView(relay(), 8)).toMatchObject({ tone: 'warn', lossy: true })
  })

  it('这一行里没有到它的链路：没有数据', () => {
    expect(linkView(relay(), 9)).toBeNull()
  })
})
