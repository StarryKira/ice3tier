import { screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { type BackboneView, type RouteMatrix } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { RelayLinksCard, RelayPairsCard, RelayStateValue } from './RelayCards'

const d = t.nodes.detail
const b = t.topology.backbone

const node = (id: number, name: string) => ({ id, name, virtual_ip: `10.66.0.${id}`, online: true })
const via = (relays: number[]) => ({ via: 'relay' as const, relays })
const cell = (from: number, to: number, route: RouteMatrix['entries'][number]['route'], rtt: number) => ({
  from,
  to,
  route,
  rtt_ms: rtt,
  age_secs: 3,
  stale: false,
  direct: null,
  relay_leg: null,
})

const MATRIX: RouteMatrix = {
  nodes: [node(1, 'hk'), node(2, 'laptop'), node(3, 'nas'), node(4, 'pc'), node(9, 'tokyo')],
  entries: [
    cell(2, 3, via([1]), 40),
    cell(3, 2, via([1]), 42),
    cell(4, 3, via([9, 1]), 120),
    cell(2, 4, { via: 'direct', addr: '198.51.100.4:20330' }, 9),
    cell(4, 2, via([9]), 80),
    // 一端就是 hk 自己：不算它在替别人转。
    cell(2, 1, via([1]), 30),
  ],
}

const BACKBONE: BackboneView = {
  answered: [{ id: 2, name: 'laptop', virtual_ip: '10.66.0.2' }],
  relays: [
    {
      relay: { id: 1, name: 'hk', virtual_ip: '10.66.0.1' },
      online: true,
      state: 'overloaded',
      load_percent: 97,
      age_ms: 900,
      reported_by: 2,
      links: [
        { to: 9, rtt_ms: 52, jitter_ms: 3, loss_permille: 0 },
        { to: 7, rtt_ms: null, jitter_ms: 0, loss_permille: 0 },
      ],
    },
    {
      relay: { id: 9, name: 'tokyo', virtual_ip: '10.66.0.9' },
      online: true,
      state: 'serving',
      load_percent: 0,
      age_ms: 900,
      reported_by: 2,
      links: [],
    },
    {
      relay: { id: 7, name: 'atl', virtual_ip: '10.66.0.7' },
      online: false,
      state: null,
      load_percent: null,
      age_ms: null,
      reported_by: null,
      links: [],
    },
  ],
}

describe('中继页的卡片', () => {
  it('经它中转的节点对：两个方向都经它的合成一行，路径里写出经过的每个中继，按延迟排', async () => {
    fakeApi({ 'GET /networks/1/routes': MATRIX })
    renderWithApp(<RelayPairsCard network={NETWORK} relay={1} relayName="hk" />)
    const list = await screen.findByRole('list', { name: d.relayPairsTitle('hk') })
    const rows = within(list).getAllByRole('listitem')
    // laptop ⇄ nas（40 ms），pc → nas（经 tokyo、hk，120 ms）。不经 hk 的、一端是 hk 的都不在里面。
    expect(rows).toHaveLength(2)
    // 名字前面是读屏用的"在线"。
    const on = t.common.online
    expect(rows[0]).toHaveTextContent(`${on}laptop⇄${on}nas`)
    expect(rows[0]).toHaveTextContent(t.common.ms(40))
    expect(rows[0]).not.toHaveTextContent(d.relayVia(''))
    expect(rows[1]).toHaveTextContent(`${on}pc→${on}nas`)
    expect(rows[1]).toHaveTextContent(d.relayVia('tokyo → hk'))
  })

  it('到其他中继：延迟、抖动、丢包，连不上的写出来', async () => {
    fakeApi({ 'GET /networks/1/backbone': BACKBONE, 'GET /networks/1/nodes': [makeNode(1, 'hk')] })
    renderWithApp(<RelayLinksCard network={NETWORK} relay={1} />)
    const list = await screen.findByRole('list', { name: d.relayLinksTitle })
    const rows = within(list).getAllByRole('listitem')
    expect(rows[0]).toHaveTextContent('tokyo')
    expect(rows[0]).toHaveTextContent(t.common.ms(52))
    expect(rows[0]).toHaveTextContent(d.relayLinkStats(t.common.ms(3), '0%'))
    expect(rows[1]).toHaveTextContent('atl')
    expect(rows[1]).toHaveTextContent(d.relayLinkDown)
  })

  it('状态和负载取自骨干里它那一行', async () => {
    fakeApi({ 'GET /networks/1/backbone': BACKBONE })
    renderWithApp(<RelayStateValue network={NETWORK} relay={1} />)
    expect(await screen.findByText(b.state.overloaded)).toBeVisible()
    expect(screen.getByText(b.load(97))).toBeVisible()
  })
})
