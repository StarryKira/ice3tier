import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type RouteMatrix } from '@/api/types'
import { t } from '@/i18n'
import { direct, makeEntry, makeNode, makeOverview, noRoute, viaRelays } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { TopologyPage } from './TopologyPage'

const g = t.topology

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
    'GET /networks/1/access/graph': { tags: [], rules: [], unconnected: [], unacknowledged: [] },
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
    expect(relayed).toHaveAccessibleName(g.cellLabel('laptop', 'nas', '经 hk，40 ms，4 秒前的上报'))
    expect(within(matrix).getByTestId('cell-3-2')).toHaveAccessibleName(
      g.cellLabel('nas', 'laptop', '直连，对端地址 192.168.1.2:20330，5 ms，10 分钟前的上报，过期'),
    )
    expect(within(matrix).getByTestId('cell-1-2')).toHaveTextContent(t.routes.symbolNone)
    // hk → nas 没有数据：不是按钮，读屏念"没有连接"。
    expect(within(matrix).queryByTestId('cell-1-3')).not.toBeInTheDocument()
    expect(within(matrix).getByText(g.cellLabel('hk', 'nas', g.cellEmpty))).toBeInTheDocument()
  })

  it('点一格选中这一对：两个方向用文字再说一遍，入口去路径诊断', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<TopologyPage />, { network: NETWORK, route: '/n/1/topology?view=matrix' })
    await user.click(await screen.findByTestId('cell-2-3'))
    expect(screen.getByTestId('watching')).toHaveTextContent(g.watching('laptop', 'nas'))
    const panel = screen.getByTestId('pair-panel')
    expect(within(panel).getByTestId('pair-forward')).toHaveTextContent('经 hk')
    expect(within(panel).getByTestId('pair-forward')).toHaveTextContent('laptop → hk → nas')
    expect(within(panel).getByTestId('pair-reverse')).toHaveTextContent(g.legStale('10 分钟前'))
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

  it('还没有节点上报路由：说明为什么是空的，节点照样画出来', async () => {
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
