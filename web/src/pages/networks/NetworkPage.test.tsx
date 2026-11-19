import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type Network } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { NetworkPage } from './NetworkPage'

const n = t.networks

const LAB: Network = { ...NETWORK, id: 2, name: 'lab', domain: 'lab.i3t', cidr: '10.67.0.0/16', nodes: 3 }

function setup(networks: Network[] = [NETWORK, LAB]) {
  return fakeApi({
    'GET /networks': networks,
    'GET /networks/1/nodes': [makeNode(1, 'nas', { routing: 'lowest_cost' })],
    'GET /networks/1/overview': { rules: 1, proxies: 0, pending_tokens: 0 },
    'PATCH /networks/1': (call) => ({ ...NETWORK, ...(call.body as object) }),
  })
}

describe('网络', () => {
  it('基本信息一张卡片；其他网络一行一个，点了切过去', async () => {
    setup()
    renderWithApp(<NetworkPage />, { network: NETWORK })
    const basics = screen.getByRole('heading', { name: n.basics }).closest('section')
    if (basics === null) throw new Error('没有基本信息卡片')
    expect(basics).toHaveTextContent('home.i3t')
    expect(basics).toHaveTextContent('10.66.0.0/16')
    expect(within(basics).getByRole('button', { name: n.expand })).toBeVisible()
    expect(await screen.findByRole('link', { name: n.switchTo('lab') })).toHaveAttribute('href', '/n/2/network')
  })

  it('只有这一个网络：没有其他网络那张卡片', async () => {
    setup([NETWORK])
    renderWithApp(<NetworkPage />, { network: NETWORK })
    await screen.findByRole('heading', { name: n.routing })
    await waitFor(() => expect(screen.queryByRole('heading', { name: n.othersTitle })).not.toBeInTheDocument())
  })

  it('选路：说明跟着选的那一项变；改了才能保存，一次提交全部', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<NetworkPage />, { network: NETWORK })
    const save = screen.getByRole('button', { name: n.saveSettings })
    expect(save).toBeDisabled()
    expect(screen.getByText(n.routingDirectFirstDesc)).toBeVisible()
    // 单独设了选路模式的节点有几个，点过去看。
    expect(await screen.findByRole('link', { name: n.routingOverridden(1) })).toHaveAttribute('href', '/n/1/nodes')

    await user.selectOptions(screen.getByLabelText(n.routingLabel), 'lowest_cost')
    await user.selectOptions(screen.getByLabelText(n.reachLabel), 'direct_only')
    expect(screen.getByText(n.routingLowestCostDesc)).toBeVisible()
    expect(screen.getByText(n.reachDirectOnlyDesc)).toBeVisible()
    await user.click(save)
    await waitFor(() => expect(api.callsTo('PATCH /networks/1')).toHaveLength(1))
    expect(api.callsTo('PATCH /networks/1')[0]?.body).toEqual({
      routing: 'lowest_cost',
      max_relay_hops: 2,
      reach: 'direct_only',
      class: 'auto',
    })
  })

  it('取消回到原来的设置', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<NetworkPage />, { network: NETWORK })
    const hops = screen.getByLabelText(n.hopsLabel)
    await user.selectOptions(hops, '3')
    await user.click(screen.getByRole('button', { name: t.common.cancel }))
    expect(hops).toHaveValue('2')
    expect(screen.getByRole('button', { name: n.saveSettings })).toBeDisabled()
  })

  it('网络里还有节点：删除是灰的，写着要先删节点', () => {
    setup()
    renderWithApp(<NetworkPage />, { network: { ...NETWORK, nodes: 2 } })
    expect(screen.getByRole('button', { name: n.deleteButton })).toBeDisabled()
    expect(screen.getByText(n.dangerNotEmpty(2), { exact: false })).toBeVisible()
  })
})
