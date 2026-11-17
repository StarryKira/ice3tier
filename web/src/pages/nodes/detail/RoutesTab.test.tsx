import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type LaneCounts, type NodeDetail, type NodeRoute, type PathView, type RouteTable } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { RoutesTab } from './RoutesTab'

const d = t.nodes.detail
const ref = (id: number, name: string) => ({ id, name, virtual_ip: `10.66.0.${id}` })

const node: NodeDetail = {
  ...makeNode(1, 'laptop'),
  endpoints: [],
  observed_endpoint: null,
  mapping_varies: false,
  flows: 0,
  tx_bytes: 0,
  rx_bytes: 0,
}

const TO_NAS_ROUTE: NodeRoute = {
  peer: ref(2, 'nas'),
  route: { via: 'relay', relays: [3] },
  rtt_ms: 150,
  age_secs: 2,
  since_secs: 600,
  direct: null,
  relay_leg: 'udp',
  tx_bytes: 1024,
  rx_bytes: 2048,
}

const TO_PC_ROUTE: NodeRoute = {
  peer: ref(4, 'pc'),
  route: { via: 'direct', addr: '192.168.1.4:20330' },
  rtt_ms: 2,
  age_secs: 2,
  since_secs: 600,
  direct: 'lan',
  relay_leg: null,
}

const ROUTES: RouteTable = {
  node: ref(1, 'laptop'),
  online: true,
  routes: [TO_NAS_ROUTE, TO_PC_ROUTE],
}

const LANE_COUNTS: LaneCounts = {
  in_use: 1,
  dialling: 0,
  claims: 0,
  claims_waited: 0,
  claim_wait_ms: 0,
  claims_failed: 0,
  fallbacks: 0,
  dial_failures: 0,
  refused: 0,
  sealed_bytes: 0,
  passthrough_bytes: 0,
  spliced_bytes: 0,
}

const leg = (from: number, to: number, rttMs: number) => ({
  from,
  to,
  rtt_us: rttMs * 1000,
  jitter_us: 0,
  loss_permille: 0,
})

/** 到 nas：现在经 hk（150 ms）；代价最低的是经 tokyo、hk 两个中继（90 ms）。 */
const pathTo = (to: number, candidates: NonNullable<PathView['forward']['candidates']>): PathView => {
  const side = {
    from: ref(1, 'laptop'),
    to: ref(to, 'x'),
    source: 'query' as const,
    route: null,
    rtt_ms: null,
    age_secs: 0,
    since: null,
    direct: null,
    relay_leg: null,
  }
  return { forward: { ...side, candidates }, reverse: side }
}

const TO_NAS = pathTo(2, [
  {
    route: { via: 'relay', relays: [5, 3] },
    cost_us: 92_000,
    legs: [leg(1, 5, 40), leg(5, 3, 30), leg(3, 2, 20)],
    load_percent: 0,
    current: false,
  },
  {
    route: { via: 'relay', relays: [3] },
    cost_us: 151_000,
    legs: [leg(1, 3, 100), leg(3, 2, 50)],
    load_percent: 0,
    current: true,
  },
])

const TO_PC = pathTo(4, [
  {
    route: { via: 'direct', addr: '192.168.1.4:20330' },
    cost_us: 2_000,
    legs: [leg(1, 4, 2)],
    load_percent: 0,
    current: true,
  },
])

describe('路由', () => {
  it('现在和延迟优先各一组：方式、经过、协议、路径长度、延迟；延迟优先写差多少，一样时也照常写出来', async () => {
    fakeApi({
      'GET /networks/1/nodes/1/routes': ROUTES,
      'GET /networks/1/nodes': [
        makeNode(1, 'laptop'),
        makeNode(2, 'nas'),
        makeNode(3, 'hk'),
        makeNode(4, 'pc'),
        makeNode(5, 'tokyo'),
      ],
      // 一个接口，按查询参数里的对端分开回答。
      'GET /networks/1/path': (call) => (call.query.get('to') === '2' ? TO_NAS : TO_PC),
    })
    renderWithApp(<RoutesTab network={NETWORK} node={node} />)
    expect(await screen.findByTestId('routes-mode')).toHaveTextContent(d.routingDirectFirst)

    const nas = (await screen.findByText('10.66.0.2')).closest('tr')
    if (nas === null) throw new Error('没有 nas 那一行')
    const texts = (row: HTMLElement) =>
      within(row)
        .getAllByRole('cell')
        .map((cell) => cell.textContent)
    // 现在：中继、下一跳 hk 和它的虚拟 IP、QUIC/UDP、路径长度 2、150 ms。
    expect(texts(nas).slice(1, 7)).toEqual([d.routesRelay, 'hk', '10.66.0.3', 'QUIC/UDP', '2', t.common.ms(150)])
    // 换成延迟优先：中继、经过 tokyo → hk、底下的传输不知道（第一个中继换了）、长度 3、90 ms，比现在少 60 ms。
    // 下一跳是 tokyo（经 tokyo → hk，长度 3）。
    await waitFor(() => expect(nas).toHaveTextContent('10.66.0.5'))
    expect(texts(nas).slice(7, 13)).toEqual([
      d.routesRelay,
      'tokyo',
      '10.66.0.5',
      'QUIC',
      '3',
      `${t.common.ms(90)}${d.routesDelta(-60)}`,
    ])

    // 直连的那一行：P2P、经过一道横线、UDP、长度 1；延迟优先也是这条，照常写出来。
    const pc = screen.getByText('10.66.0.4').closest('tr')
    if (pc === null) throw new Error('没有 pc 那一行')
    expect(texts(pc).slice(1, 7)).toEqual([d.routesP2P, '—', '—', 'QUIC/UDP', '1', t.common.ms(2)])
    await waitFor(() =>
      expect(texts(pc).slice(7, 13)).toEqual([d.routesP2P, '—', '—', 'QUIC/UDP', '1', t.common.ms(2)]),
    )
  })

  it('0.1.11 起节点自己报另一种模式和直连的情况：不再去问路径，中继上悬停说直连为什么没通', async () => {
    const api = fakeApi({
      'GET /networks/1/nodes/1/routes': {
        ...ROUTES,
        routes: [
          {
            ...TO_NAS_ROUTE,
            relay_leg: 'tcp',
            direct_status: { state: 'failed', kind: null, cause: { code: 'no_answer' }, attempted_at: 1 },
            alternative: { route: { via: 'direct', addr: '198.51.100.2:20330' }, rtt_ms: 40 },
          },
          {
            ...TO_PC_ROUTE,
            direct_status: { state: 'verified', kind: 'lan', cause: null, attempted_at: 1 },
            alternative: null,
          },
        ],
      } satisfies RouteTable,
      'GET /networks/1/nodes': [makeNode(1, 'laptop'), makeNode(2, 'nas'), makeNode(3, 'hk'), makeNode(4, 'pc')],
    })
    const withLane: NodeDetail = {
      ...node,
      lanes: {
        total: LANE_COUNTS,
        handshake_failures: { deadline: 0, unknown_key: 0, tls: 0, protocol: 0, busy: 0 },
        peers: [{ ...LANE_COUNTS, peer: 4, dials: true, control: true, idle: 0, ready: 0, parked_at_peer: 0 }],
      },
    }
    renderWithApp(<RoutesTab network={NETWORK} node={withLane} />)

    const nas = (await screen.findByText('10.66.0.2')).closest('tr')
    if (nas === null) throw new Error('没有 nas 那一行')
    const texts = (row: HTMLElement) =>
      within(row)
        .getAllByRole('cell')
        .map((cell) => cell.textContent)
    // 到中继 UDP 不通，现在走的是 QUIC/TCP；延迟优先会直连，40 ms，少 110 ms。
    expect(texts(nas).slice(1, 13)).toEqual([
      d.routesRelay,
      'hk',
      '10.66.0.3',
      'QUIC/TCP',
      '2',
      t.common.ms(150),
      d.routesP2P,
      '—',
      '—',
      'QUIC/UDP',
      '1',
      `${t.common.ms(40)}${d.routesDelta(-110)}`,
    ])
    await userEvent.hover(within(nas).getByText(d.routesRelay))
    expect(
      (await screen.findAllByText(t.routes.directState.failed(t.routes.directCause.no_answer))).length,
    ).toBeGreaterThan(0)

    // 报了 null：和现在一样，照常写出来。这一对开着 ice3proxy，协议下面多一行。
    const pc = screen.getByText('10.66.0.4').closest('tr')
    if (pc === null) throw new Error('没有 pc 那一行')
    const lane = `QUIC/UDP${t.routes.protocol.over(t.routes.protocol.lane, 'TCP')}`
    expect(texts(pc).slice(1, 13)).toEqual([
      d.routesP2P,
      '—',
      '—',
      lane,
      '1',
      t.common.ms(2),
      d.routesP2P,
      '—',
      '—',
      lane,
      '1',
      t.common.ms(2),
    ])
    expect(api.callsTo('GET /networks/1/path')).toHaveLength(0)
    expect(screen.queryByRole('button', { name: d.routesRecompute(d.routingLowestCost) })).toBeNull()
  })
})
