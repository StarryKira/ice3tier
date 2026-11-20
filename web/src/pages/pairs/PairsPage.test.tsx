import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type PairView } from '@/api/types'
import { t } from '@/i18n'
import { allPairs } from '@/lib/pairs'
import { makeNode, makePair } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { PairsPage } from './PairsPage'

const p = t.pairs

const HK = makeNode(1, 'hk')
const LAPTOP = makeNode(2, 'laptop')
const NAS = makeNode(3, 'nas')

/** 0.1.28 起的接口：每一对都给，没有设置的 stored 是 false。 */
const ALL: PairView[] = [
  makePair(1, 2, { keep_alive: true, congestion: 'cubic', stored: true }),
  makePair(1, 3, { keep_alive: false, congestion: null, stored: false }),
  makePair(2, 3, { keep_alive: false, congestion: null, stored: false }),
]

function setup(pairs: PairView[] = ALL, extra: Routes = {}) {
  return fakeApi({
    'GET /networks/1/pairs': pairs,
    'GET /networks/1/nodes': [HK, LAPTOP, NAS],
    ...extra,
  })
}

describe('补齐每一对', () => {
  it('接口给了的照用；没给的（比 0.1.28 旧的 controller）补成默认；按 a、b 排', () => {
    const rows = allPairs([NAS, HK, LAPTOP], [makePair(3, 1, { keep_alive: true })])
    expect(rows.map((row) => [row.a, row.b, row.keep_alive, row.stored])).toEqual([
      [1, 2, false, false],
      [3, 1, true, undefined],
      [2, 3, false, false],
    ])
  })
})

describe('节点对页', () => {
  it('默认列出每一对：没有特殊设置的写默认，有的写自定义、能恢复默认；没有 ice3proxy', async () => {
    setup()
    renderWithApp(<PairsPage />, { network: NETWORK })
    const custom = await screen.findByTestId('pair-row-hk-laptop')
    expect(within(custom).getByRole('switch', { name: p.toggle('hk', 'laptop') })).toBeChecked()
    expect(within(custom).getByRole('combobox', { name: p.congestionSelect('hk', 'laptop') })).toHaveValue('cubic')
    expect(screen.getByTestId('pair-setting-hk-laptop')).toHaveTextContent(p.settingCustom)
    expect(within(custom).getByRole('button', { name: p.removeFor('hk', 'laptop') })).toBeVisible()

    const plain = screen.getByTestId('pair-row-laptop-nas')
    expect(screen.getByTestId('pair-setting-laptop-nas')).toHaveTextContent(p.settingDefault)
    expect(within(plain).queryByRole('button', { name: p.removeFor('laptop', 'nas') })).toBeNull()
    expect(screen.getAllByTestId(/^pair-row-/)).toHaveLength(3)
    expect(screen.queryByText('ice3proxy')).toBeNull()
    // 不用先添加：没有添加的按钮。
    expect(screen.queryByRole('button', { name: /添加/ })).toBeNull()
  })

  it('比 0.1.28 旧的 controller 只给有设置的：用节点列表补齐，其余写默认', async () => {
    setup([{ a: 1, b: 2, keep_alive: true }])
    renderWithApp(<PairsPage />, { network: NETWORK })
    expect(await screen.findByTestId('pair-row-hk-nas')).toBeVisible()
    expect(screen.getByTestId('pair-setting-hk-nas')).toHaveTextContent(p.settingDefault)
    expect(screen.getByTestId('pair-setting-hk-laptop')).toHaveTextContent(p.settingCustom)
    // 不写拥塞控制的是默认。
    expect(screen.getByRole('combobox', { name: p.congestionSelect('hk', 'laptop') })).toHaveValue('')
  })

  it('默认的一对直接改：只发改的那一项', async () => {
    const api = setup(ALL, {
      'PUT /networks/1/pairs/2/3': (call) => ({ ...makePair(2, 3), stored: true, ...(call.body as object) }),
    })
    renderWithApp(<PairsPage />, { network: NETWORK })
    await userEvent.selectOptions(
      await screen.findByRole('combobox', { name: p.congestionSelect('laptop', 'nas') }),
      'bbr',
    )
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/2/3')[0]?.body).toEqual({ congestion: 'bbr' })
    expect(await screen.findByText(p.congestionChanged('laptop', 'nas', 'BBR'))).toBeVisible()

    await userEvent.click(screen.getByRole('switch', { name: p.toggle('laptop', 'nas') }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(2))
    expect(api.callsTo('PUT /networks/1/pairs/2/3')[1]?.body).toEqual({ keep_alive: true })
  })

  it('改回默认值：拥塞控制写 null', async () => {
    const api = setup(ALL, { 'PUT /networks/1/pairs/1/2': (call) => ({ ...ALL[0], ...(call.body as object) }) })
    renderWithApp(<PairsPage />, { network: NETWORK })
    await userEvent.selectOptions(await screen.findByRole('combobox', { name: p.congestionSelect('hk', 'laptop') }), '')
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/1/2')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/1/2')[0]?.body).toEqual({ congestion: null })
  })

  it('恢复默认：删掉这一对的设置', async () => {
    const api = setup(ALL, { 'DELETE /networks/1/pairs/1/2': undefined })
    renderWithApp(<PairsPage />, { network: NETWORK })
    await userEvent.click(await screen.findByRole('button', { name: p.removeFor('hk', 'laptop') }))
    await waitFor(() => expect(api.callsTo('DELETE /networks/1/pairs/1/2')).toHaveLength(1))
    expect(await screen.findByText(p.removed('hk', 'laptop'))).toBeVisible()
  })

  it('按节点名搜索，或者只看有设置的；一对也不剩时能清除', async () => {
    setup()
    renderWithApp(<PairsPage />, { network: NETWORK })
    await screen.findByTestId('pair-row-hk-laptop')
    await userEvent.type(screen.getByRole('searchbox', { name: p.searchPlaceholder }), 'nas')
    expect(screen.getAllByTestId(/^pair-row-/).map((row) => row.dataset.testid)).toEqual([
      'pair-row-hk-nas',
      'pair-row-laptop-nas',
    ])
    await userEvent.click(screen.getByRole('radio', { name: new RegExp(p.showCustom) }))
    expect(screen.queryAllByTestId(/^pair-row-/)).toHaveLength(0)
    await userEvent.click(screen.getByRole('button', { name: t.common.clearFilters }))
    expect(screen.getAllByTestId(/^pair-row-/)).toHaveLength(3)
  })

  it('从节点详情过来（a、b 在地址里）：那一对亮出来，没有设置也在列表里', async () => {
    setup()
    renderWithApp(<PairsPage />, { network: NETWORK, route: '/n/1/pairs?a=3&b=2' })
    const row = await screen.findByTestId('pair-row-laptop-nas')
    expect(row.className).toMatch(/bg-/)
  })

  it('不到两个节点：说一句', async () => {
    fakeApi({ 'GET /networks/1/pairs': [], 'GET /networks/1/nodes': [HK] })
    renderWithApp(<PairsPage />, { network: NETWORK })
    expect(await screen.findByText(p.emptyTitle)).toBeVisible()
  })
})
