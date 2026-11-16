import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type NodeDetail, type RelayEnabled } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { RelayRow } from './Capabilities'

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

describe('relay 这一行', () => {
  it('写出有几个节点挂靠在它上面', async () => {
    fakeApi({
      'GET /networks/1/nodes': [
        makeNode(1, 'hk', { relay: RELAY }),
        makeNode(2, 'laptop', { attached: [{ relay: 1, rtt_ms: 30 }] }),
        makeNode(3, 'nas', { attached: [{ relay: 1, rtt_ms: 12 }] }),
        makeNode(4, 'pc', { attached: [{ relay: 9, rtt_ms: 12 }] }),
      ],
    })
    renderWithApp(<RelayRow network={NETWORK} node={detail({ relay: RELAY })} onWaiting={() => undefined} />)
    await waitFor(() => expect(screen.getByTestId('relay-attached')).toHaveTextContent(d.relayAttached(2)))
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
    renderWithApp(<RelayRow network={NETWORK} node={detail()} onWaiting={() => undefined} />)
    await user.click(await screen.findByRole('switch', { name: d.relaySwitch('hk') }))
    await user.click(screen.getByRole('button', { name: d.relayEnable }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/nodes/1/relay')).toHaveLength(1))

    const results = await screen.findByTestId('relay-reachability')
    expect(within(results).getByText(d.reachOk('UDP', '203.0.113.7:20330', t.common.ms(23)))).toBeVisible()
    expect(within(results).getByText(d.reachFail('TCP', '203.0.113.7:20330', 'Connection refused'))).toBeVisible()
    expect(screen.getByText(d.reachSomeFail)).toBeVisible()
    // 不是错误：没有红色的报错，启用成功的提示照样有。
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(await screen.findByText(d.relayEnabledToast('hk'))).toBeVisible()

    await user.click(screen.getByRole('button', { name: d.reachDismiss }))
    expect(screen.queryByTestId('relay-reachability')).not.toBeInTheDocument()
  })
})
