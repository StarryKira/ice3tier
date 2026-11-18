import { describe, expect, it } from 'vitest'

import { type LaneEffect, type PairView } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { en } from '@/i18n/en'
import { makeNode, makePair, makePeerLanes } from '@/test/fixtures'

import {
  handshakeParts,
  handshakeTotal,
  LANE_DEFAULT,
  laneBytes,
  laneEffectView,
  laneInUse,
  laneMode,
  lanesReported,
  pairGlance,
  pairLaneMode,
  pairStored,
  peerGlance,
  peerUsesLanes,
  relayCandidate,
} from './lanes'

const l = t.pairs.lane

const NODES = [
  makeNode(1, 'laptop'),
  makeNode(2, 'nas'),
  makeNode(3, 'hk', { declared: { relay: true, exit: false, forward: false } }),
]

const effect = (extra: Partial<LaneEffect>): LaneEffect => ({
  effective: false,
  blocked: [],
  dialers: [],
  summary: '设置不起作用，这一对节点的流都走共用的连接（见 blocked）。',
  ...extra,
})

describe('ice3proxy 起不起作用', () => {
  it('不用时什么也不说；controller 太旧不写 lane 时按不用算', () => {
    expect(laneEffectView(makePair(1, 2), NODES)).toEqual({ kind: 'off' })
    const old: PairView = { a: 1, b: 2, keep_alive: true }
    expect(laneMode(old)).toBe('off')
    expect(lanesReported(old)).toBe(false)
    expect(laneEffectView(old, NODES)).toEqual({ kind: 'off' })
  })

  it('生效：写出哪一边拨号，两边都能拨时说两边', () => {
    const one = makePair(1, 3, { lane: 'always', lane_effect: effect({ effective: true, dialers: [1] }) })
    expect(laneEffectView(one, NODES)).toEqual({
      kind: 'effective',
      label: l.effective,
      detail: l.dials('laptop'),
      standby: false,
    })
    const both = makePair(1, 3, { lane: 'always', lane_effect: effect({ effective: true, dialers: [1, 3] }) })
    expect(laneEffectView(both, NODES)).toMatchObject({ detail: l.bothDial })
  })

  it('没有 UDP 时用：有一边的新连接走它是在用；有 UDP 是待命；发现没有 UDP、还没连上是正在连接', () => {
    const fallback = (extra: Partial<LaneEffect>, lanes: PairView['lanes'] = []) =>
      makePair(1, 3, { lane: 'udp_fallback', lanes, lane_effect: effect({ effective: true, dialers: [1], ...extra }) })
    const using = fallback({ opening: [{ node: 1, udp_path: false, lanes: true }] })
    expect(laneEffectView(using, NODES)).toEqual({
      kind: 'effective',
      label: l.inUse,
      detail: l.dials('laptop'),
      standby: false,
    })
    // 两边都有 UDP：谁都不报，`opening` 是空的。
    expect(laneEffectView(fallback({ opening: [] }), NODES)).toEqual({
      kind: 'effective',
      label: l.standby,
      detail: l.hasUdp,
      standby: true,
    })
    const coming = fallback({ opening: [{ node: 1, udp_path: false, lanes: false }] })
    expect(laneEffectView(coming, NODES)).toMatchObject({ label: l.standby, detail: l.connecting })
    // UDP 回来了，还有连接没走完：在用。
    const draining = fallback({ opening: [{ node: 1, udp_path: true, lanes: false }] }, [
      { node: 3, lanes: makePeerLanes(1, { in_use: 2 }) },
    ])
    expect(laneEffectView(draining, NODES)).toMatchObject({ label: l.inUse })
    // 比 0.1.19 旧的 controller 不写 `opening`：分不出来，照旧说生效。
    expect(laneEffectView(fallback({}), NODES)).toMatchObject({ label: l.effective, standby: false })
  })

  it('没有设置的一对：默认是没有 UDP 时用', () => {
    expect(LANE_DEFAULT).toBe('udp_fallback')
    expect(pairStored(makePair(1, 2, { stored: false }))).toBe(false)
    // 比 0.1.19 旧的 controller 只答得出存下的。
    expect(pairStored(makePair(1, 2))).toBe(true)
    const stored = [makePair(1, 2, { lane: 'always' }), makePair(2, 3, { lane: 'off' })]
    expect(pairLaneMode(stored, 2, 1)).toBe('always')
    expect(pairLaneMode(stored, 3, 2)).toBe('off')
    expect(pairLaneMode(stored, 1, 3)).toBe('udp_fallback')
    expect(pairLaneMode(undefined, 1, 3)).toBe('udp_fallback')
  })

  it('一对现在用没用着：一边的新连接走它，或者一边有在用的连接', () => {
    expect(laneInUse(makePair(1, 2))).toBe(false)
    const opening = makePair(1, 2, {
      lane: 'udp_fallback',
      lane_effect: effect({ effective: true, opening: [{ node: 2, udp_path: false, lanes: true }] }),
    })
    expect(laneInUse(opening)).toBe(true)
    const busy = makePair(1, 2, { lane: 'always', lanes: [{ node: 1, lanes: makePeerLanes(2, { in_use: 1 }) }] })
    expect(laneInUse(busy)).toBe(true)
  })

  it('节点详情里的一个对端：控制连接在、没有 UDP 才走它；总是用时有 UDP 也走；有在用的连接就算', () => {
    const peer = (extra: Parameters<typeof makePeerLanes>[1]) => makePeerLanes(2, extra)
    expect(peerUsesLanes(peer({ control: true, udp_path: false }), 'udp_fallback')).toBe(true)
    // UDP 回来之后控制连接还留着：新的连接已经不走它了。
    expect(peerUsesLanes(peer({ control: true, udp_path: true }), 'udp_fallback')).toBe(false)
    expect(peerUsesLanes(peer({ control: true, udp_path: true }), 'always')).toBe(true)
    expect(peerUsesLanes(peer({ control: false, udp_path: false }), 'udp_fallback')).toBe(false)
    expect(peerUsesLanes(peer({ control: true, udp_path: false }), 'off')).toBe(false)
    expect(peerUsesLanes(peer({ control: false, in_use: 3 }), 'off')).toBe(true)
    // 旧的节点不报有没有 UDP：只看控制连接。
    expect(peerUsesLanes(peer({ control: true }), 'udp_fallback')).toBe(true)
  })

  it('两个节点都不是 relay：原因用界面自己的话写，修法是在其中一个上启用 relay', () => {
    const pair = makePair(1, 2, { lane: 'udp_fallback', lane_effect: effect({ blocked: [{ reason: 'no_relay' }] }) })
    expect(laneEffectView(pair, NODES)).toEqual({
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

  it('其余的原因各有自己的修法：填 TCP 地址、更新节点、检查访问', () => {
    const pair = makePair(1, 3, {
      lane: 'always',
      lane_effect: effect({
        blocked: [
          { reason: 'no_tcp_address', node: 3 },
          { reason: 'node_too_old', node: 1 },
          { reason: 'not_connected' },
        ],
      }),
    })
    const view = laneEffectView(pair, NODES)
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
    const pair = makePair(1, 2, {
      lane: 'always',
      lane_effect: effect({ summary, blocked: [{ reason: 'no_relay' }, { reason: 'later' } as never] }),
    })
    const view = laneEffectView(pair, NODES)
    expect(view.kind === 'blocked' ? view.reasons.map((reason) => [reason.text, reason.fix]) : null).toEqual([
      [l.reason.noRelay('laptop', 'nas'), { kind: 'enable_relay', node: 1 }],
      [summary, null],
    ])
  })

  it('英文：原因和修法跟着语言换', () => {
    setLocale('en', { remember: false })
    const pair = makePair(1, 2, { lane: 'always', lane_effect: effect({ blocked: [{ reason: 'no_relay' }] }) })
    const view = laneEffectView(pair, NODES)
    expect(view).toMatchObject({ label: en.pairs.lane.blocked })
    expect(view.kind === 'blocked' ? view.reasons[0] : null).toMatchObject({
      text: en.pairs.lane.reason.noRelay('laptop', 'nas'),
      fixLabel: en.pairs.lane.fix.enableRelay('laptop'),
    })
    const effective = makePair(1, 3, { lane: 'always', lane_effect: effect({ effective: true, dialers: [1] }) })
    expect(laneEffectView(effective, NODES)).toMatchObject({
      label: en.pairs.lane.effective,
      detail: en.pairs.lane.dials('laptop'),
    })
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
    const e = en.pairs.lane
    expect(handshakeParts(failures)).toEqual([
      e.causeCount(e.handshakeCause.unknown_key, 2),
      e.causeCount(e.handshakeCause.protocol, 1),
    ])
  })
})
