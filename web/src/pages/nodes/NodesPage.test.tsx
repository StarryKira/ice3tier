import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { NodesPage } from './NodesPage'

describe('节点列表', () => {
  it('controller 认定用不了 UDP 的节点名字后面标着"没有 UDP"，悬停说它的 TCP 连接走 ice3proxy', async () => {
    fakeApi({
      'GET /networks/1/nodes': [makeNode(1, 'suq', { no_udp: true }), makeNode(2, 'laptop'), makeNode(3, 'nas')],
      'GET /networks/1/groups': [],
    })
    renderWithApp(<NodesPage />, { network: NETWORK, route: '/n/1/nodes' })
    const badge = await screen.findByTestId('no-udp-badge-suq')
    expect(badge).toHaveTextContent(t.nodes.noUdp)
    // 比 0.1.19 旧的 controller 不写这个字段：不标。
    expect(within(await screen.findByTestId('node-row-laptop')).queryByText(t.nodes.noUdp)).toBeNull()
    await userEvent.hover(badge)
    expect((await screen.findAllByText(t.nodes.noUdpTip)).length).toBeGreaterThan(0)
  })
})
