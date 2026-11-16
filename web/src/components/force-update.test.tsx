import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { type Node, type NodeDetail, type RouteMatrix } from '@/api/types'
import { t } from '@/i18n'
import { makeEntry, makeNode, makeOverview, makeUpdate, viaRelays } from '@/test/fixtures'
import { apiFailure, fakeApi, renderWithApp, type Routes } from '@/test/harness'

import { ForceUpdateDialog } from './force-update'
import { VersionCell } from './update'

const f = t.updates.force

const RELAY = { udp: ['203.0.113.7:20330'], tcp: [], region: 'hk', bandwidth_mbps: null }

function detail(node: Node, flows: number): NodeDetail {
  return {
    ...node,
    endpoints: [],
    observed_endpoint: null,
    attached: [],
    mapping_varies: false,
    flows,
    tx_bytes: 0,
    rx_bytes: 0,
  }
}

const NAS = makeNode(2, 'nas', { update: makeUpdate({ available: '0.3.0' }) })

function setup(node: Node, flows: number, extra: Routes = {}) {
  return fakeApi({
    [`GET /networks/1/nodes/${node.id}`]: detail(node, flows),
    'GET /networks/1/nodes': [node],
    'GET /update': makeOverview(),
    'GET /status': {},
    'GET /log': [],
    'GET /networks': [],
    ...extra,
  })
}

describe('立即更新的确认框', () => {
  it('有流时让人选：马上（写出会断几条）还是排空后；选了排空，发的就是排空', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    const api = setup(NAS, 3, {
      'POST /networks/1/nodes/2/update': makeUpdate({
        available: '0.3.0',
        ordered: { version: '0.3.0', timing: 'after_drain' },
      }),
    })
    renderWithApp(<ForceUpdateDialog node={NAS} onClose={onClose} />)
    const dialog = await screen.findByRole('dialog', { name: f.title('nas', '0.3.0') })
    expect(await within(dialog).findByText(f.nowFlows(3))).toBeVisible()
    expect(within(dialog).getByText(f.afterDrainHint)).toBeVisible()
    await user.click(within(dialog).getByRole('radio', { name: new RegExp(f.afterDrain) }))
    await user.click(within(dialog).getByRole('button', { name: f.confirmDrain }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/nodes/2/update')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/nodes/2/update')[0]?.body).toEqual({ timing: 'after_drain' })
    await waitFor(() => expect(onClose).toHaveBeenCalled())
  })

  it('没有流时没有可选的，只是一个普通的确认，发的是马上', async () => {
    const user = userEvent.setup()
    const api = setup(NAS, 0, { 'POST /networks/1/nodes/2/update': makeUpdate({ waiting: 'package' }) })
    renderWithApp(<ForceUpdateDialog node={NAS} onClose={() => undefined} />)
    const dialog = await screen.findByRole('dialog')
    expect(await within(dialog).findByText(f.idleBody)).toBeVisible()
    expect(within(dialog).queryByRole('radio')).not.toBeInTheDocument()
    await user.click(within(dialog).getByRole('button', { name: f.confirmNow }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/nodes/2/update')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/nodes/2/update')[0]?.body).toEqual({ timing: 'now' })
  })

  it('controller 拒绝时原因写在框里，框不关', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    setup(NAS, 0, {
      'POST /networks/1/nodes/2/update': () =>
        apiFailure(409, 'node_offline', '节点 nas 不在线。让节点立即更新要它连着 controller'),
    })
    renderWithApp(<ForceUpdateDialog node={NAS} onClose={onClose} />)
    const dialog = await screen.findByRole('dialog')
    await within(dialog).findByText(f.idleBody)
    await user.click(within(dialog).getByRole('button', { name: f.confirmNow }))
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('节点 nas 不在线')
    expect(onClose).not.toHaveBeenCalled()
  })

  it('唯一的 relay：写出重启会断开经它中转的几对连接', async () => {
    const hk = makeNode(1, 'hk', { relay: RELAY, update: makeUpdate({ available: '0.3.0' }) })
    const matrix: RouteMatrix = {
      nodes: [],
      entries: [makeEntry(2, 3, viaRelays(1)), makeEntry(3, 2, viaRelays(1)), makeEntry(2, 4, viaRelays(1))],
    }
    setup(hk, 0, { 'GET /networks/1/routes': matrix })
    renderWithApp(<ForceUpdateDialog node={hk} onClose={() => undefined} />)
    expect(await screen.findByText(f.relayOnly(2))).toBeVisible()
  })

  it('还有别的 relay 时说会通知经它中转的节点换路', async () => {
    const hk = makeNode(1, 'hk', { relay: RELAY, update: makeUpdate({ available: '0.3.0' }) })
    const sg = makeNode(5, 'sg', { relay: RELAY })
    setup(hk, 0, { 'GET /networks/1/nodes': [hk, sg], 'GET /networks/1/routes': { nodes: [], entries: [] } })
    renderWithApp(<ForceUpdateDialog node={hk} onClose={() => undefined} />)
    expect(await screen.findByText(f.relayOthers(1))).toBeVisible()
  })
})

describe('节点列表的版本一列', () => {
  it('有新版本时版本号旁边一个图标，悬停写出版本和下一步', async () => {
    fakeApi({ 'GET /update': makeOverview() })
    renderWithApp(<VersionCell node={NAS} />)
    const mark = await screen.findByTestId('update-mark-available')
    expect(mark).toHaveAccessibleName(new RegExp(t.updates.state.available('0.3.0')))
  })

  it('已是最新时不标', () => {
    fakeApi({ 'GET /update': makeOverview() })
    renderWithApp(<VersionCell node={makeNode(1, 'a')} />)
    expect(screen.getByText('0.1.0')).toBeVisible()
    expect(screen.queryByRole('img')).not.toBeInTheDocument()
  })
})
