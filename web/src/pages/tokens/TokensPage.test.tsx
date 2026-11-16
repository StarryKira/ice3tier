import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { nowSeconds } from '@/lib/time'
import { makeNode, makeToken } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { TokensPage } from './TokensPage'

const k = t.tokens

function setup() {
  const now = nowSeconds()
  const pending = makeToken(3, { prefix: 'i3t-eyJzIjoiQUJD', expires_at: now + 3600 })
  const used = makeToken(2, {
    prefix: 'i3t-eyJzIjoiREVG',
    state: 'used',
    used_at: now - 600,
    used_by: { id: 4, name: 'nas', virtual_ip: '10.66.0.4' },
  })
  const expired = makeToken(1, { prefix: '', state: 'expired', expires_at: now - 86_400 })
  return fakeApi({
    'GET /networks/1/tokens': (call) => (call.query.get('all') === 'true' ? [expired, used, pending] : [pending]),
    'GET /networks/1/nodes': [makeNode(4, 'nas')],
    'DELETE /networks/1/tokens/2': undefined,
    'DELETE /networks/1/tokens/3': undefined,
  })
}

describe('注册令牌', () => {
  it('默认只列还能用的，认令牌靠它文本的开头', async () => {
    const api = setup()
    renderWithApp(<TokensPage />, { network: NETWORK })
    const row = await screen.findByTestId('token-row-3')
    expect(within(row).getByText('i3t-eyJzIjoiQUJD…')).toBeVisible()
    expect(within(row).getByText(new RegExp(k.unused('')))).toBeVisible()
    expect(screen.queryByTestId('token-row-2')).not.toBeInTheDocument()
    expect(api.callsTo('GET /networks/1/tokens')[0]?.query.get('all')).toBeNull()
  })

  it('切到"全部"：连用掉的和过期的也列出，写着是哪个节点用的；地址里记着', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<TokensPage />, { network: NETWORK })
    await user.click(await screen.findByRole('radio', { name: k.showAll }))
    const used = await screen.findByTestId('token-row-2')
    expect(api.callsTo('GET /networks/1/tokens').some((call) => call.query.get('all') === 'true')).toBe(true)
    expect(within(used).getByText(k.used)).toBeVisible()
    expect(within(used).getByRole('link', { name: /nas/ })).toHaveAttribute('href', '/n/1/nodes/4')
    expect(within(screen.getByTestId('token-row-1')).getByText(k.expired)).toBeVisible()
    // 以前的令牌没有前缀，用编号认。
    expect(within(screen.getByTestId('token-row-1')).getByText('#1')).toBeVisible()
    // 还能用的排在最前面。
    const rows = screen.getAllByTestId(/^token-row-/)
    expect(rows.map((row) => row.dataset.testid)).toEqual(['token-row-3', 'token-row-2', 'token-row-1'])
  })

  it('收回还能用的是危险确认；删掉用过的只是删记录', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<TokensPage />, { network: NETWORK, route: '/n/1/tokens?show=all' })
    const label = 'i3t-eyJzIjoiQUJD…'
    await user.click(await screen.findByRole('button', { name: k.revokeFor(label) }))
    const revoke = await screen.findByRole('alertdialog', { name: k.revokeTitle(label) })
    expect(within(revoke).getByRole('button', { name: t.common.cancel })).toHaveFocus()
    await user.click(within(revoke).getByRole('button', { name: k.revokeConfirm }))
    await waitFor(() => expect(api.callsTo('DELETE /networks/1/tokens/3')).toHaveLength(1))

    const usedLabel = 'i3t-eyJzIjoiREVG…'
    await user.click(screen.getByRole('button', { name: k.forgetFor(usedLabel) }))
    const forget = await screen.findByRole('alertdialog', { name: k.forgetTitle(usedLabel) })
    expect(within(forget).getByText(k.forgetBody)).toBeVisible()
    await user.click(within(forget).getByRole('button', { name: k.forgetConfirm }))
    await waitFor(() => expect(api.callsTo('DELETE /networks/1/tokens/2')).toHaveLength(1))
  })

  it('从操作记录点过来、令牌已经被清掉了：说一句', async () => {
    setup()
    renderWithApp(<TokensPage />, { network: NETWORK, route: '/n/1/tokens?show=all&focus=42' })
    expect(await screen.findByText(k.focusMissing(42))).toBeVisible()
  })
})
