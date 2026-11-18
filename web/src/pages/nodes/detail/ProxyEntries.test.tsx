import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type NodeDetail } from '@/api/types'
import { t } from '@/i18n'
import { exitOf } from '@/lib/proxy'
import { makeListener, makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { ProxyEntriesCard } from './ProxyEntries'

const p = t.nodes.detail.proxy
const tokyo = makeNode(3, 'tokyo', { online: true })
const nas = makeNode(2, 'nas', { online: true })

const exitNode: NodeDetail = {
  ...tokyo,
  endpoints: [],
  observed_endpoint: null,
  mapping_varies: false,
  flows: 0,
  tx_bytes: 0,
  rx_bytes: 0,
}

const traffic = { connections: 2, bytes_up: 1024, bytes_down: 4096 }

describe('哪个出口的代理', () => {
  it('固定出口就是它；没固定、默认走某个出口的是那个出口；别的不算', () => {
    expect(exitOf({ fixed_exit: 3, default_action: null })).toBe(3)
    expect(exitOf({ fixed_exit: null, default_action: { action: 'exit', node: 4 } })).toBe(4)
    expect(exitOf({ fixed_exit: null, default_action: { action: 'local' } })).toBeNull()
  })
})

describe('代理入口', () => {
  it('只列经这个出口出去的代理：开在哪台、协议、地址、在不在监听', async () => {
    fakeApi({
      'GET /networks/1/nodes': [nas, tokyo],
      'GET /networks/1/listeners': [
        makeListener(1, 2, 'socks5-1080', {
          bind: '0.0.0.0:1080',
          fixed_exit: 3,
          require_credential: true,
          status: { state: 'listening', local: '0.0.0.0:1080', ...traffic },
        }),
        makeListener(2, 2, 'other', { fixed_exit: 9 }),
      ],
    })
    renderWithApp(<ProxyEntriesCard network={NETWORK} node={exitNode} />)
    const row = await screen.findByTestId('proxy-entry-1')
    expect(row).toHaveTextContent('nas')
    expect(row).toHaveTextContent('SOCKS5')
    expect(row).toHaveTextContent('0.0.0.0:1080')
    expect(row).toHaveTextContent(t.listen.listening)
    expect(row).toHaveTextContent(p.connections(2))
    expect(row).toHaveTextContent(p.withCredential)
    expect(screen.queryByTestId('proxy-entry-2')).toBeNull()
  })

  it('添加：选节点、协议、地址、端口；固定从这个出口出去；开在本机以外时要凭据', async () => {
    const user = userEvent.setup()
    const api = fakeApi({
      'GET /networks/1/nodes': [nas, tokyo],
      'GET /networks/1/listeners': [],
      'POST /networks/1/listeners': makeListener(5, 2, 'http-8080', { fixed_exit: 3 }),
    })
    renderWithApp(<ProxyEntriesCard network={NETWORK} node={exitNode} />)
    expect(await screen.findByText(p.empty('tokyo'))).toBeVisible()
    await user.click(screen.getByRole('button', { name: p.add }))
    const dialog = await screen.findByRole('dialog', { name: p.addTitle('tokyo') })

    await user.click(within(dialog).getByRole('combobox', { name: p.node }))
    await user.click(await screen.findByRole('option', { name: /nas/ }))
    await user.click(within(dialog).getByRole('radio', { name: 'HTTP' }))
    await user.click(within(dialog).getByRole('radio', { name: p.whereAll }))
    expect(within(dialog).getByText(p.needsCredential, { exact: false })).toBeVisible()
    const port = within(dialog).getByRole('textbox', { name: p.port })
    await user.clear(port)
    await user.type(port, '8080')
    await user.click(within(dialog).getByRole('button', { name: p.add }))

    await waitFor(() => expect(api.callsTo('POST /networks/1/listeners')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/listeners')[0]?.body).toEqual({
      node: 2,
      name: 'http-8080',
      bind: '0.0.0.0:8080',
      protocol: 'http',
      allow_from: [],
      require_credential: true,
      default_action: null,
      fixed_exit: 3,
      class: null,
      optimistic: null,
    })
  })
})
