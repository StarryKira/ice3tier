import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type ForcedUpdates, type UpdateOverview } from '@/api/types'
import { t } from '@/i18n'
import { makeNode, makeUpdate } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { UpdatesTab } from './UpdatesTab'

const u = t.updates

const OVERVIEW: UpdateOverview = {
  auto_update: true,
  source: 'https://github.com/fuqiuluo/ice3tier/releases',
  source_is_default: true,
  controller_version: '0.2.0',
  release: {
    version: '0.3.0',
    published: 1_794_000_000,
    min_controller: '0.2.0',
    controller_too_old: false,
    newer_than_controller: true,
    packages: [
      { platform: 'linux-aarch64', file: 'i3t-linux-aarch64', size: 17_100_288, state: 'not_needed', error: null },
      { platform: 'linux-x86_64', file: 'i3t-linux-x86_64', size: 18_234_880, state: 'failed', error: '大小对不上' },
    ],
  },
  checked_at: 1_794_000_100,
  check_error: null,
  paused: [
    {
      version: '0.3.0',
      node: { id: 2, name: 'laptop', virtual_ip: '10.66.0.2' },
      network: 1,
      reason: '自检没有通过：退出码 1',
      at: 1_794_000_200,
    },
  ],
  nodes: { total: 3, updating: 0, failed: 1, manual: 1, outdated: 1, up_to_date: 0 },
}

const NODES = [
  makeNode(1, 'nas', { update: makeUpdate({ available: '0.3.0' }) }),
  makeNode(2, 'laptop', {
    update: makeUpdate({
      available: '0.3.0',
      failure: { version: '0.3.0', kind: 'version_rejected', reason: '自检没有通过：退出码 1', at: 1_794_000_200 },
    }),
  }),
  makeNode(3, 'box', {
    update: makeUpdate({ available: '0.3.0', self_update: { able: false, blocker: 'container' } }),
  }),
]

function setup(extra: Routes = {}) {
  return fakeApi({
    'GET /update': OVERVIEW,
    'GET /networks': [{ ...NETWORK, nodes: 3 }],
    'GET /networks/1/nodes': NODES,
    'GET /status': {},
    'GET /log': [],
    ...extra,
  })
}

describe('设置里的「节点更新」', () => {
  it('版本、各平台的包、按更新情况数出来的数；controller 比发布旧时提醒它要手动更新', async () => {
    setup()
    renderWithApp(<UpdatesTab />)
    expect(await screen.findByTestId('release-version')).toHaveTextContent(u.release.latest('0.3.0'))
    expect(screen.getByText(u.release.controllerNewer('0.2.0', '0.3.0'))).toBeVisible()
    const pkg = screen.getByTestId('package-linux-x86_64')
    expect(pkg).toHaveTextContent(u.release.packageState.failed ?? '')
    expect(pkg).toHaveTextContent('大小对不上')
    expect(screen.getByTestId('update-summary')).toHaveTextContent(u.nodes.summary(OVERVIEW.nodes))
  })

  it('被暂停的版本：横幅写出版本和节点，原因点开才看；继续要确认，确认之后才发请求', async () => {
    const user = userEvent.setup()
    const api = setup({ 'POST /update/resume': { ...OVERVIEW, paused: [] } })
    renderWithApp(<UpdatesTab />)
    expect(await screen.findByText(u.paused.title('0.3.0', 'laptop'))).toBeVisible()
    const reason = screen.getByTestId('paused-reason-0.3.0')
    expect(reason).not.toBeVisible()
    await user.click(screen.getByRole('button', { name: u.paused.reason }))
    expect(reason).toBeVisible()
    expect(reason).toHaveTextContent('自检没有通过：退出码 1')

    await user.click(screen.getByRole('button', { name: u.paused.resume }))
    const question = await screen.findByRole('alertdialog', { name: u.paused.resumeTitle('0.3.0') })
    expect(question).toHaveTextContent(u.paused.resumeBody('0.3.0', 'laptop'))
    expect(api.callsTo('POST /update/resume')).toHaveLength(0)
    await user.click(within(question).getByRole('button', { name: u.paused.resumeConfirm }))
    await waitFor(() => expect(api.callsTo('POST /update/resume')).toHaveLength(1))
    expect(api.callsTo('POST /update/resume')[0]?.body).toEqual({ version: '0.3.0' })
  })

  it('自动更新的开关：改了立即生效，说明随开关变', async () => {
    const user = userEvent.setup()
    const api = setup({ 'PATCH /update': { ...OVERVIEW, auto_update: false } })
    renderWithApp(<UpdatesTab />)
    expect(await screen.findByText(u.auto.on)).toBeVisible()
    await user.click(screen.getByRole('switch', { name: u.auto.label }))
    await waitFor(() => expect(api.callsTo('PATCH /update')).toHaveLength(1))
    expect(api.callsTo('PATCH /update')[0]?.body).toEqual({ auto_update: false })
  })

  it('发布源：目录要写绝对路径；写对了才能保存，存的是去掉空白的路径', async () => {
    const user = userEvent.setup()
    const api = setup({ 'PATCH /update': OVERVIEW })
    renderWithApp(<UpdatesTab />)
    await screen.findByTestId('release-version')
    await user.click(screen.getByText(u.source.title))
    const save = screen.getByRole('button', { name: u.source.save })
    expect(save).toBeDisabled()

    await user.click(screen.getByRole('radio', { name: new RegExp(u.source.kindDir) }))
    const input = screen.getByLabelText(u.source.dirLabel)
    await user.type(input, 'releases')
    expect(screen.getByText(u.source.dirInvalid)).toBeVisible()
    expect(save).toBeDisabled()

    await user.clear(input)
    await user.type(input, ' /srv/ice3tier/releases ')
    expect(save).toBeEnabled()
    await user.click(save)
    await waitFor(() => expect(api.callsTo('PATCH /update')).toHaveLength(1))
    expect(api.callsTo('PATCH /update')[0]?.body).toEqual({ source: '/srv/ice3tier/releases' })
  })

  it('改回默认的发布源时写 null', async () => {
    const user = userEvent.setup()
    const api = setup({
      'GET /update': { ...OVERVIEW, source: 'https://mirror.example.com/r', source_is_default: false },
      'PATCH /update': OVERVIEW,
    })
    renderWithApp(<UpdatesTab />)
    expect(await screen.findByTestId('source-summary')).toHaveTextContent(
      u.source.summaryUrl('https://mirror.example.com/r'),
    )
    await user.click(screen.getByText(u.source.title))
    await user.click(screen.getByRole('radio', { name: new RegExp(u.source.kindDefault) }))
    await user.click(screen.getByRole('button', { name: u.source.save }))
    await waitFor(() => expect(api.callsTo('PATCH /update')).toHaveLength(1))
    expect(api.callsTo('PATCH /update')[0]?.body).toEqual({ source: null })
  })

  it('立即检查：查完就地说结果；查不到时原因常驻，发布源自己展开', async () => {
    const user = userEvent.setup()
    const failed = { ...OVERVIEW, check_error: '清单的签名不对' }
    const api = setup({ 'POST /update/check': OVERVIEW })
    renderWithApp(<UpdatesTab />)
    await user.click(await screen.findByRole('button', { name: u.release.check }))
    expect(await screen.findByTestId('check-result')).toHaveTextContent(u.release.found('0.3.0', 3))
    expect(api.callsTo('POST /update/check')).toHaveLength(1)

    api.use({ 'POST /update/check': failed, 'GET /update': failed })
    await user.click(screen.getByRole('button', { name: u.release.check }))
    expect(await screen.findByText(u.release.checkFailed('清单的签名不对'))).toBeVisible()
    expect(screen.queryByTestId('check-result')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: u.source.save })).toBeVisible()
  })

  it('每个节点的更新状态：能点的给"立即更新"，不能的置灰；失败过的写原因', async () => {
    setup()
    renderWithApp(<UpdatesTab />)
    const nas = await screen.findByTestId('update-row-nas')
    expect(within(nas).getByRole('button', { name: `${u.force.button} nas` })).toBeEnabled()
    const box = screen.getByTestId('update-row-box')
    expect(box).toHaveTextContent(u.state.cannotSelf)
    expect(within(box).getByRole('button', { name: `${u.force.button} box` })).toBeDisabled()
    expect(screen.getByTestId('update-row-laptop')).toHaveTextContent(u.state.lastFailed)
  })

  it('全部更新：确认框里选时机，结果按已下发、稍后、跳过列出来', async () => {
    const user = userEvent.setup()
    const result: ForcedUpdates = {
      version: '0.3.0',
      ordered: [{ id: 1, name: 'nas', virtual_ip: '10.66.0.1' }],
      queued: [],
      skipped: [{ node: { id: 3, name: 'box', virtual_ip: '10.66.0.3' }, reason: '节点 box 不能自己更新' }],
    }
    const api = setup({ 'POST /update/force': result })
    renderWithApp(<UpdatesTab />)
    await user.click(await screen.findByRole('button', { name: u.nodes.updateAll }))
    const dialog = await screen.findByRole('dialog', { name: u.forceAll.title('0.3.0') })
    expect(dialog).toHaveTextContent(u.forceAll.body(2))
    expect(dialog).toHaveTextContent(u.forceAll.flowsUnknown)
    await user.click(within(dialog).getByRole('radio', { name: new RegExp(u.force.afterDrain) }))
    await user.click(within(dialog).getByRole('button', { name: u.forceAll.confirmDrain }))
    await waitFor(() => expect(api.callsTo('POST /update/force')).toHaveLength(1))
    expect(api.callsTo('POST /update/force')[0]?.body).toEqual({ timing: 'after_drain' })
    const shown = await screen.findByTestId('force-all-result')
    expect(shown).toHaveTextContent('nas')
    expect(shown).toHaveTextContent('节点 box 不能自己更新')
  })

  it('没有能立即更新的节点时"全部更新"是灰的', async () => {
    setup({ 'GET /update': { ...OVERVIEW, nodes: { ...OVERVIEW.nodes, outdated: 0, failed: 0 } } })
    renderWithApp(<UpdatesTab />)
    expect(await screen.findByRole('button', { name: u.nodes.updateAll })).toBeDisabled()
  })
})
