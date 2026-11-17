import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type NodeDetail, type RelayEnabled } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { RelayTab } from './Capabilities'

const d = t.nodes.detail

function detail(extra: Partial<NodeDetail> = {}): NodeDetail {
  return {
    ...makeNode(1, 'hk', { declared: { exit: false, relay: true, forward: false } }),
    endpoints: [],
    observed_endpoint: '203.0.113.7:20330',
    mapping_varies: false,
    flows: 0,
    tx_bytes: 0,
    rx_bytes: 0,
    ...extra,
  }
}

const RELAY = { udp: ['203.0.113.7:20330'], tcp: ['203.0.113.7:20330'], region: '', bandwidth_mbps: null }

describe('relay 这一页', () => {
  it('列出连着它的节点，连着别的中继的不在里面', async () => {
    fakeApi({
      'GET /networks/1/nodes': [
        makeNode(1, 'hk', { relay: RELAY }),
        makeNode(2, 'laptop', { attached: [{ relay: 1, rtt_ms: 30 }] }),
        makeNode(3, 'nas', { attached: [{ relay: 1, rtt_ms: 12 }] }),
        makeNode(4, 'pc', { attached: [{ relay: 9, rtt_ms: 12 }] }),
      ],
    })
    renderWithApp(<RelayTab network={NETWORK} node={detail({ relay: RELAY })} onWaiting={() => undefined} />)
    const list = await screen.findByRole('list', { name: d.relayConnectedTitle })
    await waitFor(() => expect(within(list).getAllByRole('listitem')).toHaveLength(2))
    expect(list).not.toHaveTextContent('pc')
  })

  it('已连接的节点单独一张卡片：延迟、首选还是备用、指定的、它还连着哪些中继', async () => {
    fakeApi({
      'GET /networks/1/nodes': [
        makeNode(1, 'hk', { relay: RELAY }),
        makeNode(9, 'tokyo', { relay: RELAY }),
        makeNode(2, 'laptop', {
          attached: [
            { relay: 1, rtt_ms: 30 },
            { relay: 9, rtt_ms: 12 },
          ],
        }),
        makeNode(3, 'nas', { attached: [{ relay: 1, rtt_ms: 8 }], pinned_relays: [1] }),
        makeNode(4, 'pc', { pinned_relays: [1] }),
      ],
    })
    renderWithApp(<RelayTab network={NETWORK} node={detail({ relay: RELAY })} onWaiting={() => undefined} />)
    const list = await screen.findByRole('list', { name: d.relayConnectedTitle })
    const rows = within(list).getAllByRole('listitem')
    // 按延迟排：nas、laptop；只指定了、没连着的 pc 在最后。
    expect(rows.map((row) => row.querySelector('a')?.textContent)).toEqual([
      `${t.common.online}nas`,
      `${t.common.online}laptop`,
      `${t.common.online}pc`,
    ])
    expect(rows[0]).toHaveTextContent(d.relayRankFirst)
    expect(rows[0]).toHaveTextContent(d.relayPinnedTag)
    expect(rows[1]).toHaveTextContent(d.relayRankBackup)
    expect(rows[1]).toHaveTextContent(d.relayAlsoConnected(`tokyo ${t.common.ms(12)}`))
    expect(rows[2]).toHaveTextContent(d.relayNotConnected)
  })

  it('启用之后写出 controller 试连的结果：连不上是一条建议，不是错误', async () => {
    const user = userEvent.setup()
    const reply: RelayEnabled = {
      ...makeNode(1, 'hk', { relay: RELAY }),
      reachability: [
        { protocol: 'udp', addr: '203.0.113.7:20330', reachable: true, rtt_ms: 23, error: null },
        { protocol: 'tcp', addr: '203.0.113.7:20330', reachable: false, rtt_ms: null, error: 'Connection refused' },
      ],
    }
    const api = fakeApi({ 'GET /networks/1/nodes': [makeNode(1, 'hk')], 'PUT /networks/1/nodes/1/relay': reply })
    renderWithApp(<RelayTab network={NETWORK} node={detail()} onWaiting={() => undefined} />)
    await user.click(await screen.findByRole('switch', { name: d.relaySwitch('hk') }))
    await user.click(screen.getByRole('button', { name: d.relayEnable }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/nodes/1/relay')).toHaveLength(1))

    const results = await screen.findByTestId('relay-reachability')
    expect(within(results).getByText(d.reachOk('UDP', '203.0.113.7:20330', t.common.ms(23)))).toBeVisible()
    expect(within(results).getByText(d.reachFail('TCP', '203.0.113.7:20330', 'Connection refused'))).toBeVisible()
    expect(screen.getByText(d.reachSomeFail)).toBeVisible()
    // 原因是节点那边的原话（"Connection refused"），原样写在括号里。
    // 不是错误：没有红色的报错，启用成功的提示照样有。
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(await screen.findByText(d.relayEnabledToast('hk'))).toBeVisible()

    await user.click(screen.getByRole('button', { name: d.reachDismiss }))
    expect(screen.queryByTestId('relay-reachability')).not.toBeInTheDocument()
  })
})
