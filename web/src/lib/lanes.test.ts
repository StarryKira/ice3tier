import { describe, expect, it } from 'vitest'

import { type LaneEffect, type LaneSide } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { en } from '@/i18n/en'
import { makeNode, makePeerLanes } from '@/test/fixtures'

import {
  handshakeParts,
  handshakeTotal,
  laneBytes,
  laneEffectView,
  lanesReported,
  pairGlance,
  pairStored,
  peerGlance,
  peerUsesLanes,
  proxyLane,
  relayCandidate,
} from './lanes'

const l = t.proxies.lane

const NODES = [
  makeNode(1, 'laptop'),
  makeNode(2, 'nas'),
  makeNode(3, 'hk', { declared: { relay: true, exit: false, forward: false } }),
]

/** 代理的两端：laptop 上监听，经 nas 出去。 */
const ENDS = { listen: 1, exit: 2 }

const effect = (extra: Partial<LaneEffect>): LaneEffect => ({
  effective: false,
  blocked: [],
  dialers: [],
  summary: 'ice3proxy 不起作用，这个代理的流都走共用的连接（见 blocked）。',
  ...extra,
})

describe('代理上的 ice3proxy', () => {
  it('比界面旧的 controller 不写这三个字段：按关着、null、[] 算', () => {
    expect(proxyLane({})).toEqual({ on: false, effect: null, sides: [] })
    expect(lanesReported({})).toBe(false)
    expect(lanesReported({ lane: true, lanes: [{ node: 1, lanes: makePeerLanes(2) }] })).toBe(true)
  })

  it('没打开（或者没有出口）时 lane_effect 是 null：什么也不说', () => {
    expect(laneEffectView(null, ENDS, [], NODES)).toEqual({ kind: 'off' })
  })

  it('生效：写出哪一边拨号，两边都能拨时说两边；有在用的连接时写在用', () => {
    expect(laneEffectView(effect({ effective: true, dialers: [1] }), ENDS, [], NODES)).toEqual({
      kind: 'effective',
      label: l.effective,
      detail: l.dials('laptop'),
    })
    const sides: LaneSide[] = [{ node: 1, lanes: makePeerLanes(2, { in_use: 2 }) }]
    expect(laneEffectView(effect({ effective: true, dialers: [1, 2] }), ENDS, sides, NODES)).toEqual({
      kind: 'effective',
      label: l.inUse,
      detail: l.bothDial,
    })
  })

  it('节点详情里的一个对端：控制连接在或者有在用的连接，就是有 ice3proxy', () => {
    expect(peerUsesLanes(makePeerLanes(2, { control: true }))).toBe(true)
    expect(peerUsesLanes(makePeerLanes(2, { control: false, in_use: 3 }))).toBe(true)
    expect(peerUsesLanes(makePeerLanes(2, { control: false }))).toBe(false)
  })

  it('两个节点都不是 relay：原因用界面自己的话写，修法是在其中一个上启用 relay', () => {
    expect(laneEffectView(effect({ blocked: [{ reason: 'no_relay' }] }), ENDS, [], NODES)).toEqual({
      kind: 'blocked',
      label: l.blocked,
      reasons: [
        {
          text: l.reason.noRelay('laptop', 'nas'),
          fix: { kind: 'enable_relay', node: 1 },
          fixLabel: l.fix.enableRelay('laptop'),
        },
      ],
    })
  })

  it('在哪个节点上启用 relay：声明过可以当 relay 的优先，其次在线的', () => {
    expect(relayCandidate(1, 3, NODES)).toBe(3)
    expect(relayCandidate(1, 2, [makeNode(1, 'a', { online: false }), makeNode(2, 'b')])).toBe(2)
    expect(relayCandidate(1, 2, NODES)).toBe(1)
  })

  it('其余的原因各有自己的修法：填 TCP 地址、更新节点、检查监听的节点能不能访问出口', () => {
    const view = laneEffectView(
      effect({
        blocked: [
          { reason: 'no_tcp_address', node: 3 },
          { reason: 'node_too_old', node: 1 },
          { reason: 'not_connected' },
        ],
      }),
      { listen: 1, exit: 3 },
      [],
      NODES,
    )
    expect(view.kind).toBe('blocked')
    if (view.kind !== 'blocked') return
    expect(view.reasons).toEqual([
      { text: l.reason.noTcpAddress('hk'), fix: { kind: 'tcp_address', node: 3 }, fixLabel: l.fix.tcpAddress('hk') },
      { text: l.reason.nodeTooOld('laptop'), fix: { kind: 'update', node: 1 }, fixLabel: l.fix.update('laptop') },
      {
        text: l.reason.notConnected('laptop', 'hk'),
        fix: { kind: 'access', from: 1, to: 3 },
        fixLabel: l.fix.access,
      },
    ])
  })

  it('不认识的原因（比界面新的 controller）：补上 controller 的原话，不给修法', () => {
    const summary = '某个以后才有的原因'
    const view = laneEffectView(
      effect({ summary, blocked: [{ reason: 'no_relay' }, { reason: 'later' } as never] }),
      ENDS,
      [],
      NODES,
    )
    expect(view.kind === 'blocked' ? view.reasons.map((reason) => [reason.text, reason.fix]) : null).toEqual([
      [l.reason.noRelay('laptop', 'nas'), { kind: 'enable_relay', node: 1 }],
      [summary, null],
    ])
  })

  it('英文：原因和修法跟着语言换', () => {
    setLocale('en', { remember: false })
    const view = laneEffectView(effect({ blocked: [{ reason: 'no_relay' }] }), ENDS, [], NODES)
    expect(view).toMatchObject({ label: en.proxies.lane.blocked })
    expect(view.kind === 'blocked' ? view.reasons[0] : null).toMatchObject({
      text: en.proxies.lane.reason.noRelay('laptop', 'nas'),
      fixLabel: en.proxies.lane.fix.enableRelay('laptop'),
    })
    expect(laneEffectView(effect({ effective: true, dialers: [1] }), ENDS, [], NODES)).toMatchObject({
      label: en.proxies.lane.effective,
      detail: en.proxies.lane.dials('laptop'),
    })
  })

  it('节点对：比 0.1.19 旧的 controller 不写 stored，按存下的算', () => {
    expect(pairStored({})).toBe(true)
    expect(pairStored({ stored: false })).toBe(false)
  })
})

describe('ice3proxy 的计数', () => {
  it('一个节点和一个对端之间：空闲的是停在两头、它看得到的那些', () => {
    const peer = makePeerLanes(2, { in_use: 3, idle: 2, ready: 4, parked_at_peer: 5, fallbacks: 1, dial_failures: 2 })
    expect(peerGlance(peer)).toEqual({ inUse: 3, idle: 11, fallbacks: 1, dialFailures: 2 })
  })

  it('一对节点：在用的两头都算，取大的；空闲的按停在哪一边相加；回退和拨号失败相加', () => {
    const laptop = makePeerLanes(2, { in_use: 12, ready: 4, parked_at_peer: 6, fallbacks: 1, dial_failures: 2 })
    const nas = makePeerLanes(1, { in_use: 11, idle: 6, fallbacks: 3 })
    const glance = pairGlance([
      { node: 1, lanes: laptop },
      { node: 2, lanes: nas },
    ])
    expect(glance).toEqual({ inUse: 12, idle: 10, fallbacks: 4, dialFailures: 2 })
  })

  it('只有一边报：用它看得到的；两边都没报时没有数', () => {
    const laptop = makePeerLanes(2, { in_use: 1, ready: 4, parked_at_peer: 6 })
    expect(pairGlance([{ node: 1, lanes: laptop }])).toEqual({ inUse: 1, idle: 10, fallbacks: 0, dialFailures: 0 })
    expect(pairGlance([])).toBeNull()
  })

  it('字节数三种方式相加；握手失败按原因只列不是 0 的，两种语言', () => {
    expect(laneBytes(makePeerLanes(2, { sealed_bytes: 10, passthrough_bytes: 20, spliced_bytes: 30 }))).toBe(60)
    const failures = { deadline: 0, unknown_key: 2, tls: 0, protocol: 1, busy: 0 }
    expect(handshakeTotal(failures)).toBe(3)
    expect(handshakeParts(failures)).toEqual([
      l.causeCount(l.handshakeCause.unknown_key, 2),
      l.causeCount(l.handshakeCause.protocol, 1),
    ])
    setLocale('en', { remember: false })
    const e = en.proxies.lane
    expect(handshakeParts(failures)).toEqual([
      e.causeCount(e.handshakeCause.unknown_key, 2),
      e.causeCount(e.handshakeCause.protocol, 1),
    ])
  })
})
