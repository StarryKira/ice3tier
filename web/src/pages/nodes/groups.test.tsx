import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type Group } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { NodeGroupsDialog } from './actions'
import { NodesPage } from './NodesPage'

const LAPTOPS = { id: 3, name: 'laptops' }
const SERVERS = { id: 4, name: 'servers' }
const gw = makeNode(1, 'gw', { groups: [SERVERS] })
const laptop = makeNode(2, 'laptop', { groups: [LAPTOPS] })
const phone = makeNode(5, 'phone')

const group = (ref: { id: number; name: string }, builtin = false): Group => ({ ...ref, builtin, nodes: [], rules: [] })
const GROUPS = [group({ id: 1, name: 'All' }, true), group(LAPTOPS), group(SERVERS)]

function setup() {
  return fakeApi({
    'GET /networks/1/nodes': [gw, laptop, phone],
    'GET /networks/1/groups': GROUPS,
    'PATCH /networks/1/groups/4/nodes': group(SERVERS),
    'PATCH /networks/1/groups/3/nodes': group(LAPTOPS),
  })
}

async function select(user: ReturnType<typeof userEvent.setup>, ...names: string[]) {
  for (const name of names) await user.click(await screen.findByRole('checkbox', { name: t.nodes.selectOne(name) }))
}

describe('节点列表上的分组', () => {
  for (const locale of ['zh-CN', 'en'] as const) {
    it(`一列写着节点的分组；按分组筛选写的是编号（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      setup()
      renderWithApp(<NodesPage />, { network: NETWORK, route: '/n/1/nodes?group=3' })
      expect(await screen.findByTestId('node-row-laptop')).toHaveTextContent('laptops')
      expect(screen.queryByTestId('node-row-gw')).not.toBeInTheDocument()
      expect(await screen.findByText(t.nodes.chipGroup('laptops'))).toBeVisible()
    })
  }

  it('选中的节点一起添加到一个已有的分组：一个请求', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<NodesPage />, { network: NETWORK, route: '/n/1/nodes' })
    await select(user, 'laptop', 'phone')
    await user.click(screen.getByRole('button', { name: t.nodes.batchAddGroup }))
    const dialog = await screen.findByRole('dialog', { name: t.nodes.batchAddTitle(2) })
    // All 不在里面：每个节点本来就在。
    await user.click(within(dialog).getByRole('combobox', { name: t.nodes.batchGroupLabel }))
    expect(screen.queryByRole('option', { name: /All/ })).not.toBeInTheDocument()
    await user.click(screen.getByRole('option', { name: /servers/ }))
    await user.click(within(dialog).getByRole('button', { name: t.nodes.batchAddSubmit }))
    await waitFor(() => expect(api.callsTo('PATCH /networks/1/groups/4/nodes')).toHaveLength(1))
    expect(api.callsTo('PATCH /networks/1/groups/4/nodes')[0]?.body).toEqual({ add: [2, 5] })
    expect(await screen.findByText(t.nodes.batchAdded(2, 'servers'))).toBeVisible()
  })

  it('从分组移除：只列选中的节点里有人在的分组', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<NodesPage />, { network: NETWORK, route: '/n/1/nodes' })
    await select(user, 'laptop', 'phone')
    await user.click(screen.getByRole('button', { name: t.nodes.batchRemoveGroup }))
    const dialog = await screen.findByRole('dialog', { name: t.nodes.batchRemoveTitle(2) })
    expect(within(dialog).getByText(t.nodes.batchRemoveNote)).toBeVisible()
    await user.click(within(dialog).getByRole('combobox', { name: t.nodes.batchGroupLabel }))
    expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual([
      `laptops${t.access.graph.members(0)}`,
    ])
    await user.click(screen.getByRole('option', { name: /laptops/ }))
    await user.click(within(dialog).getByRole('button', { name: t.nodes.batchRemoveSubmit }))
    await waitFor(() => expect(api.callsTo('PATCH /networks/1/groups/3/nodes')[0]?.body).toEqual({ remove: [2, 5] }))
  })

  it('选中的节点都不在别的分组里：没有可移除的，说一句', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<NodesPage />, { network: NETWORK, route: '/n/1/nodes' })
    await select(user, 'phone')
    await user.click(screen.getByRole('button', { name: t.nodes.batchRemoveGroup }))
    const dialog = await screen.findByRole('dialog', { name: t.nodes.batchRemoveTitle(1) })
    expect(await within(dialog).findByText(t.nodes.batchNotInGroups)).toBeVisible()
    expect(within(dialog).getByRole('button', { name: t.nodes.batchRemoveSubmit })).toBeDisabled()
  })
})

describe('节点的分组', () => {
  for (const locale of ['zh-CN', 'en'] as const) {
    it(`选一个已有的分组，立即保存，交给 controller 的是编号（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      const user = userEvent.setup()
      const api = setup()
      api.use({ 'PATCH /networks/1/nodes/2': { ...laptop, groups: [LAPTOPS, SERVERS] } })
      renderWithApp(<NodeGroupsDialog network={NETWORK} node={laptop} open onClose={() => undefined} />)
      const dialog = await screen.findByRole('dialog', { name: t.nodes.groupsTitle('laptop') })
      const input = within(dialog).getByRole('combobox')
      await user.type(input, 'serv{Enter}')
      await waitFor(() => expect(api.callsTo('PATCH /networks/1/nodes/2')).toHaveLength(1))
      expect(api.callsTo('PATCH /networks/1/nodes/2')[0]?.body).toEqual({ groups: [3, 4] })
      expect(await screen.findByText(t.nodes.groupsSaved)).toBeVisible()
      expect(within(dialog).getByRole('link', { name: t.groups.picker.manage })).toHaveAttribute('href', '/n/1/groups')
    })
  }
})
