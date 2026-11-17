import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type NodeDetail } from '@/api/types'
import { t } from '@/i18n'
import { makeNode, makeOverview, makeUpdate } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { AdvancedCard } from './AdvancedCard'
import { VersionRow } from './NodeVersion'

const u = t.updates
const d = t.nodes.detail

const detail = (extra: Partial<NodeDetail> = {}): NodeDetail => ({
  ...makeNode(1, 'laptop', { version: '0.1.10', online: true }),
  endpoints: [],
  observed_endpoint: null,
  mapping_varies: false,
  flows: 0,
  tx_bytes: 0,
  rx_bytes: 0,
  ...extra,
})

describe('概览上的版本', () => {
  it('没有新版本时只有版本号，没有按钮', async () => {
    fakeApi({ 'GET /update': makeOverview() })
    renderWithApp(<VersionRow node={detail()} />)
    expect(await screen.findByText('0.1.10')).toBeVisible()
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('有新版本时版本右边是"升级到"，平常不是红的', async () => {
    fakeApi({ 'GET /update': makeOverview(), 'GET /networks/1/nodes/1': detail() })
    renderWithApp(<VersionRow node={detail({ update: makeUpdate({ available: '0.1.11' }) })} />)
    const button = await screen.findByRole('button', { name: u.detail.upgrade('0.1.11') })
    expect(button).toBeEnabled()
    expect(button.className).not.toMatch(/bg-destructive/)
    await userEvent.click(button)
    expect(await screen.findByRole('dialog', { name: u.force.title('laptop', '0.1.11') })).toBeVisible()
  })

  it('安全更新的按钮是红的，悬停说明', async () => {
    fakeApi({ 'GET /update': makeOverview() })
    renderWithApp(<VersionRow node={detail({ update: makeUpdate({ available: '0.1.11', security: true }) })} />)
    const button = await screen.findByRole('button', { name: u.detail.upgrade('0.1.11') })
    expect(button.className).toMatch(/bg-destructive/)
    await userEvent.hover(button)
    expect((await screen.findAllByText(u.detail.securityTip)).length).toBeGreaterThan(0)
  })

  it('不能自己更新时按钮照样能点，点开是手动更新的做法', async () => {
    fakeApi({ 'GET /update': makeOverview() })
    const update = makeUpdate({ available: '0.1.11', self_update: { able: false, blocker: 'read_only_dir' } })
    renderWithApp(<VersionRow node={detail({ update })} />)
    await userEvent.click(await screen.findByRole('button', { name: u.detail.upgrade('0.1.11') }))
    const dialog = await screen.findByRole('dialog', { name: u.detail.manualTitle('laptop', '0.1.11') })
    expect(dialog).toHaveTextContent(u.blocker.read_only_dir)
  })

  it('离线时置灰', async () => {
    fakeApi({ 'GET /update': makeOverview() })
    renderWithApp(<VersionRow node={detail({ online: false, update: makeUpdate({ available: '0.1.11' }) })} />)
    expect(await screen.findByRole('button', { name: u.detail.upgrade('0.1.11') })).toBeDisabled()
  })

  it('上次更新失败时版本下面写出来', async () => {
    fakeApi({ 'GET /update': makeOverview() })
    const update = makeUpdate({
      available: '0.1.11',
      status: { state: 'failed', kind: 'not_installed', version: '0.1.11', reason: '磁盘满了' },
    })
    renderWithApp(<VersionRow node={detail({ update })} />)
    expect(await screen.findByTestId('node-update-state')).toHaveTextContent(u.state.failed)
  })
})

describe('高级里的自动更新', () => {
  it('controller 没有这个字段时不显示', async () => {
    fakeApi({ 'GET /update': makeOverview(), 'GET /networks/1/nodes': [] })
    renderWithApp(<AdvancedCard network={NETWORK} node={detail()} />)
    expect(await screen.findByText(d.routingLabel)).toBeVisible()
    expect(screen.queryByRole('switch', { name: d.autoUpdate })).toBeNull()
  })

  it('点开关就改这个节点的自动更新', async () => {
    const api = fakeApi({
      'GET /update': makeOverview(),
      'GET /networks/1/nodes': [],
      'PATCH /networks/1/nodes/1': detail({ auto_update: false }),
    })
    renderWithApp(<AdvancedCard network={NETWORK} node={detail({ auto_update: true })} />)
    const toggle = await screen.findByRole('switch', { name: d.autoUpdate })
    expect(toggle).toBeChecked()
    expect(screen.getByText(d.autoUpdateHint)).toBeVisible()
    await userEvent.click(toggle)
    await waitFor(() => expect(api.callsTo('PATCH /networks/1/nodes/1')).toHaveLength(1))
    expect(api.callsTo('PATCH /networks/1/nodes/1')[0]?.body).toEqual({ auto_update: false })
  })

  it('全局关着时说明里写出来，给去设置的链接', async () => {
    fakeApi({ 'GET /update': makeOverview({ auto_update: false }), 'GET /networks/1/nodes': [] })
    renderWithApp(<AdvancedCard network={NETWORK} node={detail({ auto_update: true })} />)
    expect(await screen.findByRole('link', { name: d.autoUpdateSettings })).toHaveAttribute('href', '/settings/updates')
    expect(screen.getByText(d.autoUpdateGlobalOff, { exact: false })).toBeVisible()
  })
})
