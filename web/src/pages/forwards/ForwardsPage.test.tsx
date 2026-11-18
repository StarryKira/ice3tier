import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { setLocale, t } from '@/i18n'
import { makeForward, makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { ForwardsPage } from './ForwardsPage'

const f = t.forwards
const NODES = [
  makeNode(1, 'laptop'),
  makeNode(2, 'gw-home', { declared: { exit: false, relay: false, forward: true } }),
]
const traffic = { connections: 2, bytes_up: 512, bytes_down: 8192 }

describe('端口转发的监听状态', () => {
  it('端口写 0：列表里写节点挑定的端口，复制的也是它', async () => {
    fakeApi({
      'GET /networks/1/forwards': [
        makeForward(4, 'db', {
          bind: '127.0.0.1:0',
          listen: { state: 'listening', local: '127.0.0.1:41873', ...traffic },
        }),
      ],
      'GET /networks/1/nodes': NODES,
    })
    renderWithApp(<ForwardsPage />, { network: NETWORK })
    const picked = await screen.findByTestId('forward-picked-4')
    expect(picked).toHaveTextContent('127.0.0.1:41873')
    expect(picked).toHaveTextContent(f.picked(41873))
    const row = screen.getByTestId('forward-row-db')
    expect(row).toHaveTextContent(t.listen.connections(2))
    expect(row).toHaveTextContent(t.common.trafficUpDown('512 B', '8.0 KB'))
  })

  it('端口写 0、还没挑定：说在等', async () => {
    fakeApi({
      'GET /networks/1/forwards': [makeForward(4, 'db', { bind: '127.0.0.1:0', listen: null })],
      'GET /networks/1/nodes': NODES,
    })
    renderWithApp(<ForwardsPage />, { network: NETWORK })
    expect(await screen.findByText(f.pickedPending, { exact: false })).toBeVisible()
    expect(within(screen.getByTestId('forward-row-db')).getByText(t.listen.pending)).toBeVisible()
  })

  it('端口被占用：红色，下面一行写是哪台机器上的哪个端口，节点的原话在它下面', async () => {
    fakeApi({
      'GET /networks/1/forwards': [
        makeForward(5, 'web', { listen: { state: 'port_in_use', detail: 'Address already in use', ...traffic } }),
      ],
      'GET /networks/1/nodes': NODES,
    })
    renderWithApp(<ForwardsPage />, { network: NETWORK })
    const row = await screen.findByTestId('forward-row-web')
    expect(within(row).getByText(t.listen.portInUse)).toBeVisible()
    expect(screen.getByText(t.listen.portInUseDetail('laptop', '8080'))).toBeVisible()
    expect(screen.getByText('Address already in use')).toBeVisible()
  })

  it('开关的时候把服务等级原样带上（PUT 是整个替换）', async () => {
    const user = userEvent.setup()
    const forward = makeForward(6, 'web', {
      class: 'bulk',
      listen: { state: 'listening', local: '127.0.0.1:8080', ...traffic },
    })
    const api = fakeApi({
      'GET /networks/1/forwards': [forward],
      'GET /networks/1/nodes': NODES,
      'PUT /networks/1/forwards/6': () => ({ ...forward, enabled: false, status: 'disabled' }),
    })
    renderWithApp(<ForwardsPage />, { network: NETWORK })
    await user.click(await screen.findByRole('switch', { name: f.toggle('web') }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/forwards/6')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/forwards/6')[0]?.body).toMatchObject({ enabled: false, class: 'bulk' })
  })

  it('新建时端口留空：交给 controller 的端口是 0；监听在虚拟 IP 上时不行', async () => {
    const user = userEvent.setup()
    const api = fakeApi({
      'GET /networks/1/forwards': [makeForward(1, 'old')],
      'GET /networks/1/nodes': NODES,
      'GET /networks/1/access/check': { from: NODES[0], to: NODES[1], allowed: true, rules: [] },
      'POST /networks/1/forwards': (call) => makeForward(9, 'new', call.body as object),
    })
    renderWithApp(<ForwardsPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: f.create }))
    const dialog = await screen.findByRole('dialog', { name: f.dialog.createTitle })
    const listenSide = within(dialog).getByRole('group', { name: f.dialog.listenSide })
    const targetSide = within(dialog).getByRole('group', { name: f.dialog.targetSide })
    await user.click(within(listenSide).getByRole('combobox', { name: f.dialog.node }))
    await user.click(await screen.findByRole('option', { name: /laptop/ }))
    await waitFor(() => expect(screen.queryByRole('listbox')).not.toBeInTheDocument())
    await user.click(within(targetSide).getByRole('combobox', { name: f.dialog.node }))
    await user.click(await screen.findByRole('option', { name: /gw-home/ }))
    await user.type(within(targetSide).getByRole('textbox', { name: f.dialog.targetPort }), '80')
    expect(within(listenSide).getByRole('textbox', { name: f.dialog.port })).toHaveAttribute(
      'placeholder',
      f.dialog.portPlaceholder,
    )

    await user.click(within(listenSide).getByRole('radio', { name: new RegExp(f.dialog.whoMesh) }))
    await user.click(within(dialog).getByRole('button', { name: f.dialog.submitCreate }))
    expect(within(listenSide).getByText(f.dialog.portMeshRequired)).toBeVisible()
    expect(api.callsTo('POST /networks/1/forwards')).toHaveLength(0)

    await user.click(within(listenSide).getByRole('radio', { name: new RegExp(f.dialog.whoSelf) }))
    await user.click(within(dialog).getByRole('button', { name: f.dialog.submitCreate }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/forwards')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/forwards')[0]?.body).toMatchObject({ bind: '127.0.0.1:0', class: null })
  })
})

describe('英文', () => {
  it('转发列表里没有中文，节点的原话照样显示', async () => {
    setLocale('en', { remember: false })
    fakeApi({
      'GET /networks/1/forwards': [
        makeForward(5, 'web', { listen: { state: 'port_in_use', detail: 'Address already in use', ...traffic } }),
        makeForward(6, 'db', { status: 'access_denied' }),
        makeForward(7, 'off', { status: 'disabled', enabled: false }),
      ],
      'GET /networks/1/nodes': NODES,
    })
    renderWithApp(<ForwardsPage />, { network: NETWORK })
    await screen.findByTestId('forward-row-web')
    expect(screen.getByText(t.listen.portInUseDetail('laptop', '8080'))).toBeVisible()
    expect(screen.getByText('Address already in use')).toBeVisible()
    expect(document.body.textContent).not.toMatch(/\p{Script=Han}/u)
  })
})
