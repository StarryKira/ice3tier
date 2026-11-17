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
    // 现在：中继、经过 hk、UDP、路径长度 2、150 ms。
    expect(texts(nas).slice(1, 6)).toEqual([d.routesRelay, 'hk', 'UDP', '2', t.common.ms(150)])
    // 换成延迟优先：中继、经过 tokyo → hk、协议不知道（第一个中继换了）、长度 3、90 ms，比现在少 60 ms。
    await waitFor(() => expect(nas).toHaveTextContent('tokyo → hk'))
    expect(texts(nas).slice(6, 11)).toEqual([
      d.routesRelay,
      'tokyo → hk',
      '—',
      '3',
      `${t.common.ms(90)}${d.routesDelta(-60)}`,
    ])

    // 直连的那一行：P2P、经过一道横线、UDP、长度 1；延迟优先也是这条，照常写出来。
    const pc = screen.getByText('10.66.0.4').closest('tr')
    if (pc === null) throw new Error('没有 pc 那一行')
    expect(texts(pc).slice(1, 6)).toEqual([d.routesP2P, '—', 'UDP', '1', t.common.ms(2)])
    await waitFor(() => expect(texts(pc).slice(6, 11)).toEqual([d.routesP2P, '—', 'UDP', '1', t.common.ms(2)]))
  })
})
