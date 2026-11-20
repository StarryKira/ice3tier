import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type PairView } from '@/api/types'
import { t } from '@/i18n'
import { makeNode, makePair } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { PairsPage } from './PairsPage'

const p = t.pairs

const HK = makeNode(1, 'hk')
const LAPTOP = makeNode(2, 'laptop')
const NAS = makeNode(3, 'nas')

function setup(pairs: PairView[], extra: Routes = {}) {
  return fakeApi({
    'GET /networks/1/pairs': pairs,
    'GET /networks/1/nodes': [HK, LAPTOP, NAS],
    ...extra,
  })
}

describe('节点对页', () => {
  it('一对一行：连接常驻和拥塞控制，没有 ice3proxy（它在代理上）', async () => {
    setup([makePair(1, 2, { congestion: 'cubic' })])
    renderWithApp(<PairsPage />, { network: NETWORK })
    const row = await screen.findByTestId('pair-row-hk-laptop')
    expect(within(row).getByRole('switch', { name: p.toggle('hk', 'laptop') })).toBeChecked()
    expect(within(row).getByRole('combobox', { name: p.congestionSelect('hk', 'laptop') })).toHaveValue('cubic')
    expect(screen.queryByText('ice3proxy')).toBeNull()
    expect(screen.getByText(p.defaultNote)).toBeVisible()
  })

  it('比界面旧的 controller 不写拥塞控制：是默认', async () => {
    setup([{ a: 1, b: 2, keep_alive: true }])
    renderWithApp(<PairsPage />, { network: NETWORK })
    const row = await screen.findByTestId('pair-row-hk-laptop')
    expect(within(row).getByRole('combobox', { name: p.congestionSelect('hk', 'laptop') })).toHaveValue('')
  })

  it('改拥塞控制只发这一项，默认是 null；拨连接常驻的开关也只发那一项', async () => {
    const api = setup([makePair(1, 2, { congestion: 'cubic' })], {
      'PUT /networks/1/pairs/1/2': (call) => ({ ...makePair(1, 2), ...(call.body as object) }),
    })
    renderWithApp(<PairsPage />, { network: NETWORK })
    const select = await screen.findByRole('combobox', { name: p.congestionSelect('hk', 'laptop') })
    await userEvent.selectOptions(select, '')
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/1/2')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/1/2')[0]?.body).toEqual({ congestion: null })
    expect(await screen.findByText(p.congestionChanged('hk', 'laptop', p.congestion.default))).toBeVisible()

    await userEvent.click(screen.getByRole('switch', { name: p.toggle('hk', 'laptop') }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/1/2')).toHaveLength(2))
    expect(api.callsTo('PUT /networks/1/pairs/1/2')[1]?.body).toEqual({ keep_alive: false })
  })

  it('从节点详情过来的那一对没有设置：单独取来，标着默认，没有恢复默认；改一项就存下来', async () => {
    const unset = makePair(2, 3, { stored: false, keep_alive: false })
    const api = setup([], {
      'GET /networks/1/pairs/3/2': unset,
      'PUT /networks/1/pairs/2/3': (call) => ({ ...unset, stored: true, ...(call.body as object) }),
    })
    renderWithApp(<PairsPage />, { network: NETWORK, route: '/n/1/pairs?a=3&b=2' })
    const row = await screen.findByTestId('pair-row-laptop-nas')
    expect(within(row).getByText(p.defaultBadge)).toBeVisible()
    expect(within(row).queryByRole('button', { name: p.removeFor('laptop', 'nas') })).toBeNull()

    await userEvent.selectOptions(
      within(row).getByRole('combobox', { name: p.congestionSelect('laptop', 'nas') }),
      'bbr',
    )
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/2/3')[0]?.body).toEqual({ congestion: 'bbr' })
  })
})

describe('添加节点对', () => {
  async function pick(dialog: HTMLElement) {
    await userEvent.click(within(dialog).getByRole('combobox', { name: p.nodeA }))
    await userEvent.click(screen.getByRole('option', { name: /laptop/ }))
    await userEvent.click(within(dialog).getByRole('combobox', { name: p.nodeB }))
    await userEvent.click(screen.getByRole('option', { name: /nas/ }))
  }

  it('两个节点和两项设置一起提交：连接常驻默认开着，拥塞控制默认', async () => {
    const api = setup([], { 'PUT /networks/1/pairs/2/3': makePair(2, 3) })
    renderWithApp(<PairsPage />, { network: NETWORK })
    await screen.findByText(p.emptyTitle)
    expect(screen.getByText(p.defaultNote)).toBeVisible()
    await userEvent.click(screen.getByRole('button', { name: p.add }))
    const dialog = screen.getByRole('dialog', { name: p.dialogTitle })
    await pick(dialog)
    await userEvent.selectOptions(within(dialog).getByRole('combobox', { name: p.colCongestion }), 'cubic')
    await userEvent.click(within(dialog).getByRole('button', { name: p.submit }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/2/3')[0]?.body).toEqual({ keep_alive: true, congestion: 'cubic' })
  })

  it('两项都和默认一样时不提交，说出来', async () => {
    const api = setup([])
    renderWithApp(<PairsPage />, { network: NETWORK })
    await screen.findByText(p.emptyTitle)
    await userEvent.click(screen.getByRole('button', { name: p.add }))
    const dialog = screen.getByRole('dialog', { name: p.dialogTitle })
    await pick(dialog)
    await userEvent.click(within(dialog).getByRole('switch', { name: p.colKeepAlive }))
    await userEvent.click(within(dialog).getByRole('button', { name: p.submit }))
    expect(within(dialog).getByText(p.nothingSet)).toBeVisible()
    expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(0)
  })
})
