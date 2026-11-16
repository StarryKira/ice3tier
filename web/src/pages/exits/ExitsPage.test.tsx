import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { setLocale, t } from '@/i18n'
import { makeExitSettings, makeExitView, makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { ExitsPage } from './ExitsPage'

const e = t.exits
const GB = 1024 ** 3
const MB = 1024 ** 2
const DEC_1 = Date.UTC(2026, 11, 1) / 1000

const LAPTOP = { id: 2, name: 'laptop', virtual_ip: '10.66.0.2' }

const GW = makeExitView(1, 'gw', {
  settings: makeExitSettings({ quota_bytes: 100 * GB, quota_reset_day: 1, family: 'ipv4', connects_per_minute: 120 }),
  health: 'degraded',
  recent_requests: 40,
  recent_failures: 6,
  usage: {
    active: 2,
    total: 40,
    bytes_to_target: 2 * GB,
    bytes_from_target: 90 * GB,
    failed_by_target: 3,
    refused: 1,
    failed_by_exit: 6,
    aborted: 0,
    draining: false,
    quota: { limit_bytes: 100 * GB, used_bytes: 92 * GB, period_start: DEC_1 - 30 * 86400, period_end: DEC_1 },
  },
  reported_secs_ago: 4,
  observed: [
    {
      by: LAPTOP,
      exit: 1,
      flows: 38,
      bytes_up: 100 * MB,
      bytes_down: 2 * GB,
      failed_by_target: 3,
      refused: 1,
      failed_by_exit: 6,
      unreachable: 2,
      reported_secs_ago: 7,
    },
  ],
})

const JP = makeExitView(3, 'jp', {
  usage: {
    active: 0,
    total: 5,
    bytes_to_target: 10 * MB,
    bytes_from_target: 50 * MB,
    failed_by_target: 0,
    refused: 0,
    failed_by_exit: 0,
    aborted: 0,
    draining: true,
    quota: null,
  },
  reported_secs_ago: 3,
})

function setup() {
  return fakeApi({
    'GET /networks/1/exits': [GW, JP],
    'GET /networks/1/nodes': [makeNode(1, 'gw'), makeNode(2, 'laptop'), makeNode(3, 'jp')],
    'GET /networks/1/nodes/1/access': { node: GW.node, can_reach: [], reachable_by: [], acknowledged: true },
    'GET /networks/1/nodes/3/access': { node: JP.node, can_reach: [], reachable_by: [], acknowledged: true },
  })
}

describe('出口页', () => {
  it('限制写成一行：配额、地址族、建连预算都在', async () => {
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    const row = await screen.findByTestId('exit-row-gw')
    expect(row).toHaveTextContent(t.nodes.detail.exitQuota('100 GB', 1))
    expect(row).toHaveTextContent(t.nodes.detail.exitFamilyIpv4)
    expect(row).toHaveTextContent(t.nodes.detail.exitConnects(120))
  })

  it('配额：进度条、已用多少、哪天重置', async () => {
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    const usage = await screen.findByTestId('exit-usage-gw')
    expect(within(usage).getByRole('meter', { name: e.quotaLabel('gw') })).toHaveAttribute('aria-valuenow', '92')
    expect(within(usage).getByText('92.0 GB / 100 GB')).toBeVisible()
    expect(within(usage).getByText(e.quotaResets('12月1日'))).toBeVisible()
  })

  it('健康：有问题时写出最近 5 分钟里出口自己的失败；排空中直说', async () => {
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    const health = await screen.findByTestId('exit-health-gw')
    expect(within(health).getByText(e.health.degraded)).toBeVisible()
    expect(within(health).getByText(e.health.degradedShort(40, 6))).toBeVisible()
    expect(within(screen.getByTestId('exit-health-jp')).getByText(e.health.draining)).toBeVisible()
  })

  it('没配配额：写这次运行以来搬运的量', async () => {
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    const usage = await screen.findByTestId('exit-usage-jp')
    expect(within(usage).getByText(t.common.trafficUpDown('10.0 MB', '50.0 MB'))).toBeVisible()
    expect(within(usage).queryByRole('meter')).not.toBeInTheDocument()
  })

  it('出口报的和发起端看到的差得多：带一个记号，说两个数各是多少', async () => {
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    const usage = await screen.findByTestId('exit-usage-gw')
    expect(within(usage).getByRole('img', { name: e.mismatchTip('92.0 GB', '2.1 GB') })).toBeVisible()
  })

  it('对照：出口自己报的和用它的节点看到的并排放着', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: e.detailsFor('gw') }))
    const table = screen.getByRole('table', { name: e.detailsFor('gw') })
    expect(within(table).getByRole('columnheader', { name: e.exitReported })).toBeVisible()
    expect(within(table).getByRole('columnheader', { name: 'laptop' })).toBeVisible()
    const unreachable = within(table).getByRole('row', { name: new RegExp(e.rowUnreachable) })
    expect(
      within(unreachable)
        .getAllByRole('cell')
        .map((cell) => cell.textContent),
    ).toEqual([e.notApplicable, '2'])
    const failed = within(table).getByRole('row', { name: new RegExp(e.rowFailedExit) })
    expect(
      within(failed)
        .getAllByRole('cell')
        .map((cell) => cell.textContent),
    ).toEqual(['6', '6'])
  })
})

describe('英文', () => {
  it('出口列表和对照表里没有中文', async () => {
    const user = userEvent.setup()
    setLocale('en', { remember: false })
    setup()
    renderWithApp(<ExitsPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: e.detailsFor('gw') }))
    expect(within(screen.getByTestId('exit-usage-gw')).getByText('Resets Dec 1')).toBeVisible()
    expect(document.body.textContent).not.toMatch(/\p{Script=Han}/u)
  })
})
