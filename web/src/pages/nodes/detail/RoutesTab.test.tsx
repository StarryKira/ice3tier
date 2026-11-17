import { screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { type NodeDetail, type PathView, type RouteTable } from '@/api/types'
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

const ROUTES: RouteTable = {
  node: ref(1, 'laptop'),
  online: true,
  routes: [
    {
      peer: ref(2, 'nas'),
      route: { via: 'relay', relays: [3] },
      rtt_ms: 150,
      age_secs: 2,
      since_secs: 600,
      direct: null,
      relay_leg: 'udp',
      tx_bytes: 1024,
      rx_bytes: 2048,
    },
    {
      peer: ref(4, 'pc'),
      route: { via: 'direct', addr: '192.168.1.4:20330' },
      rtt_ms: 2,
      age_secs: 2,
      since_secs: 600,
      direct: 'lan',
      relay_leg: null,
    },
  ],
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
  it('写清楚 P2P 还是中继、下一跳和跳数；右边是延迟优先会怎么走', async () => {
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
    expect(await screen.findByText(d.routesModeNow(d.routingDirectFirst))).toBeVisible()

    const nas = (await screen.findByText('10.66.0.2')).closest('tr')
    if (nas === null) throw new Error('没有 nas 那一行')
    expect(within(nas).getByText(d.routesRelay)).toBeVisible()
    expect(nas).toHaveTextContent(t.routes.relayLeg('UDP'))
    // 现在：下一跳 hk、两跳、150 ms；延迟优先：下一跳 tokyo、三跳、90 ms。
    await waitFor(() => expect(nas).toHaveTextContent('tokyo'))
    const cells = within(nas)
      .getAllByRole('cell')
      .map((cell) => cell.textContent)
    expect(cells.slice(2, 8)).toEqual(['hk', '2', t.common.ms(150), 'tokyo', '3', t.common.ms(90)])

    const pc = screen.getByText('10.66.0.4').closest('tr')
    if (pc === null) throw new Error('没有 pc 那一行')
    expect(within(pc).getByText(d.routesP2P)).toBeVisible()
    expect(within(pc).getAllByText(d.routesDirectHop)).toHaveLength(2)
  })
})
