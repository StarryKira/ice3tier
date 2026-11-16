import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type RouteMatrix } from '@/api/types'
import { t } from '@/i18n'
import { direct, makeEntry, makeNode, makeOverview, noRoute, viaRelays } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { TopologyPage } from './TopologyPage'

const g = t.topology
const r = t.routes
/** 矩阵一格的说法：几样事实按语言接起来。 */
const facts = (...parts: string[]) => parts.join(r.factSeparator)

const RELAY = { udp: ['203.0.113.7:20330'], tcp: [], region: 'hk', bandwidth_mbps: null }

const MATRIX: RouteMatrix = {
  nodes: [
    { id: 1, name: 'hk', virtual_ip: '10.66.0.1', online: true },
    { id: 2, name: 'laptop', virtual_ip: '10.66.0.2', online: true },
    { id: 3, name: 'nas', virtual_ip: '10.66.0.3', online: false },
  ],
  entries: [
    makeEntry(2, 3, viaRelays(1), { rtt_ms: 40 }),
    makeEntry(3, 2, direct('192.168.1.2:20330'), { rtt_ms: 5, stale: true, age_secs: 600 }),
    makeEntry(1, 2, noRoute, { rtt_ms: null }),
  ],
}

function setup(extra: Routes = {}) {
  return fakeApi({
    'GET /networks/1/routes': MATRIX,
    'GET /networks/1/nodes': [
      makeNode(1, 'hk', { relay: RELAY }),
      makeNode(2, 'laptop'),
      makeNode(3, 'nas', { online: false }),
    ],
    'GET /networks/1/tokens': [],
    'GET /networks/1/access/graph': { groups: [], rules: [], unconnected: [], unacknowledged: [] },
    'GET /update': makeOverview(),
    ...extra,
  })
}

describe('拓扑的路由矩阵', () => {
  it('行是发送方，列是接收方；格子里有字和延迟，过期的标出来，没有数据的是一个点', async () => {
    setup()
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology?view=matrix' })
    const matrix = await screen.findByTestId('route-matrix')
    const relayed = within(matrix).getByTestId('cell-2-3')
    expect(relayed).toHaveTextContent(`${t.routes.symbolRelay}40`)
    expect(relayed).toHaveAccessibleName(
      g.cellLabel('laptop', 'nas', facts(r.via('hk'), r.rtt(40), r.age(t.time.secondsAgo(4)))),
    )
    expect(within(matrix).getByTestId('cell-3-2')).toHaveAccessibleName(
      g.cellLabel(
        'nas',
        'laptop',
        facts(r.direct, r.addr('192.168.1.2:20330'), r.rtt(5), r.age(t.time.minutesAgo(10)), r.stale),
      ),
    )
    expect(within(matrix).getByTestId('cell-1-2')).toHaveTextContent(t.routes.symbolNone)
    // hk → nas 没有数据：不是按钮，读屏念"没有连接"。
    expect(within(matrix).queryByTestId('cell-1-3')).not.toBeInTheDocument()
    expect(within(matrix).getByText(g.cellLabel('hk', 'nas', r.classEmpty))).toBeInTheDocument()
  })

  it('点一格选中这一对：两个方向用文字再说一遍，入口去路径诊断', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology?view=matrix' })
    await user.click(await screen.findByTestId('cell-2-3'))
    expect(screen.getByTestId('watching')).toHaveTextContent(g.watching('laptop', 'nas'))
    const panel = screen.getByTestId('pair-panel')
    expect(within(panel).getByTestId('pair-forward')).toHaveTextContent(r.via('hk'))
    expect(within(panel).getByTestId('pair-forward')).toHaveTextContent(['laptop', 'hk', 'nas'].join(r.hopSeparator))
    expect(within(panel).getByTestId('pair-reverse')).toHaveTextContent(g.legStale(t.time.minutesAgo(10)))
    expect(within(panel).getByRole('link', { name: g.toPath })).toHaveAttribute('href', '/n/1/diagnosis?a=2&b=3')
    expect(screen.getByTestId('cell-2-3')).toHaveAttribute('aria-pressed', 'true')
    expect(screen.getByTestId('cell-3-2')).toHaveAttribute('aria-pressed', 'true')

    await user.click(screen.getByRole('button', { name: g.clearPair }))
    expect(screen.queryByTestId('pair-panel')).not.toBeInTheDocument()
  })

  it('地址里带着一对，矩阵里没有这个方向的数据时照实说', async () => {
    setup()
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology?view=matrix&a=1&b=3' })
    const panel = await screen.findByTestId('pair-panel')
    expect(within(panel).getByTestId('pair-forward')).toHaveTextContent(g.legNoData)
  })

  it('还没有节点上报路由：说一句，节点照样画出来', async () => {
    setup({ 'GET /networks/1/routes': { ...MATRIX, entries: [] } })
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology' })
    expect(await screen.findByText(g.noRoutesTitle)).toBeVisible()
    expect(screen.getByTestId('topology-graph')).toBeInTheDocument()
  })

  it('节点不足两个：引导去添加节点', async () => {
    setup({ 'GET /networks/1/routes': { nodes: [MATRIX.nodes[0]], entries: [] } })
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology' })
    expect(await screen.findByText(g.emptyTitle)).toBeVisible()
    expect(screen.getByRole('button', { name: g.addNode })).toBeVisible()
  })
})

describe('骨干：relay 之间的延迟', () => {
  const BACKBONE = {
    relays: [
      {
        relay: { id: 1, name: 'hk', virtual_ip: '10.66.0.1' },
        online: true,
        state: 'overloaded',
        load_percent: 97,
        age_ms: 1800,
        reported_by: 2,
        links: [{ to: 4, rtt_ms: 150, jitter_ms: 2, loss_permille: 5 }],
      },
      {
        relay: { id: 4, name: 'la', virtual_ip: '10.66.0.4' },
        online: false,
        state: null,
        load_percent: null,
        age_ms: null,
        reported_by: null,
        links: [],
      },
    ],
    answered: [{ id: 2, name: 'laptop', virtual_ip: '10.66.0.2' }],
  }

  it('路由矩阵旁边一个标签页：行是测的 relay，带状态和负载；格子里是延迟', async () => {
    const user = userEvent.setup()
    const api = setup({ 'GET /networks/1/backbone': BACKBONE })
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology' })
    await user.click(await screen.findByRole('radio', { name: g.viewBackbone }))
    const row = await screen.findByTestId('backbone-row-1')
    expect(row).toHaveTextContent(g.backbone.state.overloaded)
    expect(row).toHaveTextContent(g.backbone.load(97))
    expect(row).toHaveTextContent(g.backbone.reportedBy('laptop'))
    expect(screen.getByTestId('backbone-cell-1-4')).toHaveAccessibleName(
      g.cellLabel('hk', 'la', g.backbone.cell(150, 2, '0.5%')),
    )
    expect(screen.getByTestId('backbone-row-4')).toHaveTextContent(g.backbone.noRow)
    expect(screen.getByText(g.backbone.answered('laptop'))).toBeVisible()
    // 选节点对的那一栏和骨干无关，不出现。
    expect(screen.queryByRole('combobox', { name: g.pickA })).not.toBeInTheDocument()

    // 每次都要现问：不轮询，有"重新查询"。
    expect(api.callsTo('GET /networks/1/backbone')).toHaveLength(1)
    await user.click(screen.getByRole('button', { name: g.backbone.query }))
    expect(api.callsTo('GET /networks/1/backbone')).toHaveLength(2)
  })

  it('没有 relay：照实说', async () => {
    setup({ 'GET /networks/1/backbone': { relays: [], answered: [] } })
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology?view=backbone' })
    expect(await screen.findByText(g.backbone.noRelaysTitle)).toBeVisible()
  })

  it('格子里写出到 relay 退回了 TCP；直连的方式在悬停文字里', async () => {
    setup({
      'GET /networks/1/routes': {
        ...MATRIX,
        entries: [
          makeEntry(2, 3, viaRelays(1), { rtt_ms: 40, relay_leg: 'tcp' }),
          makeEntry(3, 2, direct('192.168.1.2:20330'), { rtt_ms: 5, direct: 'lan' }),
        ],
      },
    })
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology?view=matrix' })
    const relayed = await screen.findByTestId('cell-2-3')
    expect(relayed).toHaveTextContent('TCP')
    expect((relayed.getAttribute('aria-label') ?? '').includes(r.withDetail(r.via('hk'), r.relayLeg('TCP')))).toBe(true)
    const lan = screen.getByTestId('cell-3-2').getAttribute('aria-label') ?? ''
    expect(lan.includes(r.withDetail(r.direct, r.directKind.lan))).toBe(true)
  })
})
