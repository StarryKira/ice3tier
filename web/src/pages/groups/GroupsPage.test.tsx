import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type Group, type NodeRef, type Rule } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { GroupsPage } from './GroupsPage'

const gw = makeNode(1, 'gw')
const laptop = makeNode(2, 'laptop')
const ref = (node: typeof gw): NodeRef => ({ id: node.id, name: node.name, virtual_ip: node.virtual_ip })

const ssh: Rule = {
  id: 2,
  name: 'ssh',
  src: { id: 3, name: 'laptops' },
  dst: { id: 4, name: 'servers' },
  direction: 'one_way',
  enabled: true,
}

const ALL: Group = { id: 1, name: 'All', builtin: true, nodes: [ref(gw), ref(laptop)], rules: [] }
const LAPTOPS: Group = { id: 3, name: 'laptops', builtin: false, nodes: [ref(laptop)], rules: [ssh] }
const SERVERS: Group = { id: 4, name: 'servers', builtin: false, nodes: [], rules: [ssh] }

/** controller 拒绝时的应答，带着 `what` 和 `field`（文档 §2、§18.1）。 */
function refusal(status: number, code: string, what: string, field: string | null): Response {
  return new Response(JSON.stringify({ error: { code, message: '原话', what, field } }), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function setup() {
  return fakeApi({
    'GET /networks/1/groups': [ALL, LAPTOPS, SERVERS],
    'GET /networks/1/nodes': [gw, laptop],
  })
}

function render(route = '/n/1/groups') {
  return renderWithApp(<GroupsPage />, { network: NETWORK, route })
}

const g = () => t.groups

describe('分组页', () => {
  for (const locale of ['zh-CN', 'en'] as const) {
    it(`一行一个分组：节点数、规则数；All 在最前面，标着内置，没有改名、删除（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      setup()
      render()
      const all = await screen.findByTestId('group-row-All')
      expect(all).toHaveTextContent(g().builtin)
      const cells = within(screen.getByTestId('group-row-laptops')).getAllByRole('cell')
      expect(cells.slice(0, 3).map((cell) => cell.textContent)).toEqual(['laptops', '1', '1'])
      expect(screen.queryByRole('button', { name: t.common.actionsFor('All') })).not.toBeInTheDocument()
      expect(screen.getByRole('button', { name: t.common.actionsFor('laptops') })).toBeVisible()
      const rows = screen.getAllByTestId(/^group-row-/)
      expect(rows[0]).toBe(all)
    })
  }

  it('新建：写法不对的在名字下面拦下；controller 说重名时也写在名字下面；建好之后展开它', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({
      'POST /networks/1/groups': (call) => {
        const { name } = call.body as { name: string }
        if (name.toLowerCase() === 'servers') return refusal(409, 'name_taken', 'group', 'name')
        return { id: 9, name, builtin: false, nodes: [], rules: [] }
      },
    })
    render()
    await user.click(await screen.findByRole('button', { name: g().create }))
    const dialog = await screen.findByRole('dialog', { name: g().createTitle })
    const input = within(dialog).getByRole('textbox', { name: g().nameLabel })

    await user.type(input, 'my group')
    await user.click(within(dialog).getByRole('button', { name: g().createSubmit }))
    expect(within(dialog).getByText(t.validate.groupCharset)).toBeVisible()
    expect(api.callsTo('POST /networks/1/groups')).toHaveLength(0)

    await user.clear(input)
    await user.type(input, 'Servers')
    await user.click(within(dialog).getByRole('button', { name: g().createSubmit }))
    await waitFor(() => expect(input).toHaveAccessibleDescription(t.errors.group.nameTaken))

    await user.clear(input)
    await user.type(input, 'Büro')
    api.use({ 'GET /networks/1/groups': [ALL, { id: 9, name: 'Büro', builtin: false, nodes: [], rules: [] }] })
    await user.click(within(dialog).getByRole('button', { name: g().createSubmit }))
    expect(await screen.findByText(g().created('Büro'))).toBeVisible()
    expect(api.callsTo('POST /networks/1/groups').at(-1)?.body).toEqual({ name: 'Büro' })
    expect(await screen.findByTestId('group-members-Büro')).toHaveTextContent(g().noMembers)
  })

  it('改名：只发名字', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({ 'PATCH /networks/1/groups/3': (call) => ({ ...LAPTOPS, name: (call.body as { name: string }).name }) })
    render()
    await user.click(await screen.findByRole('button', { name: t.common.actionsFor('laptops') }))
    await user.click(await screen.findByRole('menuitem', { name: g().menuRename }))
    const dialog = await screen.findByRole('dialog', { name: g().renameTitle('laptops') })
    const input = within(dialog).getByRole('textbox', { name: g().nameLabel })
    await user.clear(input)
    await user.type(input, 'Notebooks')
    await user.click(within(dialog).getByRole('button', { name: g().renameSubmit }))
    expect(await screen.findByText(g().renamed('laptops', 'Notebooks'))).toBeVisible()
    expect(api.callsTo('PATCH /networks/1/groups/3')[0]?.body).toEqual({ name: 'Notebooks' })
  })

  it('controller 按 what 说名字太长：写在名字下面，用和提交之前的校验一样的话', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({ 'POST /networks/1/groups': () => refusal(400, 'invalid_request', 'too_long', 'name') })
    render()
    await user.click(await screen.findByRole('button', { name: g().create }))
    const dialog = await screen.findByRole('dialog', { name: g().createTitle })
    const input = within(dialog).getByRole('textbox', { name: g().nameLabel })
    await user.type(input, 'ok')
    await user.click(within(dialog).getByRole('button', { name: g().createSubmit }))
    await waitFor(() => expect(input).toHaveAccessibleDescription(t.validate.groupTooLong(32)))
  })

  for (const locale of ['zh-CN', 'en'] as const) {
    it(`删除：先取这个分组，确认框里写它从几个节点上移除、几条规则一起删掉（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      const user = userEvent.setup()
      const api = setup()
      // 列表是旧的：分组自己的应答里已经多了一个成员。
      const fresh = { ...LAPTOPS, nodes: [ref(laptop), ref(gw)] }
      api.use({
        'GET /networks/1/groups/3': fresh,
        'DELETE /networks/1/groups/3': { id: 3, name: 'laptops', nodes: fresh.nodes, rules: [ssh], tokens: [] },
      })
      render()
      await user.click(await screen.findByRole('button', { name: t.common.actionsFor('laptops') }))
      await user.click(await screen.findByRole('menuitem', { name: g().menuDelete }))
      const question = await screen.findByRole('alertdialog', { name: g().deleteTitle('laptops') })
      expect(question).toHaveTextContent(g().deleteImpact(2, 1) ?? '')
      await user.click(within(question).getByRole('button', { name: g().deleteConfirm }))
      await waitFor(() => expect(api.callsTo('DELETE /networks/1/groups/3')).toHaveLength(1))
      expect(await screen.findByText(g().deleted('laptops'))).toBeVisible()
    })
  }

  it('没有成员、也没有规则用到的分组：确认框只有标题', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({ 'GET /networks/1/groups/4': { ...SERVERS, rules: [] } })
    render()
    await user.click(await screen.findByRole('button', { name: t.common.actionsFor('servers') }))
    await user.click(await screen.findByRole('menuitem', { name: g().menuDelete }))
    const question = await screen.findByRole('alertdialog', { name: g().deleteTitle('servers') })
    expect(question).not.toHaveTextContent(g().deleteImpact(0, 1) ?? '')
  })

  it('分组在这期间被删掉了：不弹确认框，说分组不存在', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({ 'GET /networks/1/groups/4': () => refusal(404, 'not_found', 'group', null) })
    render()
    await user.click(await screen.findByRole('button', { name: t.common.actionsFor('servers') }))
    await user.click(await screen.findByRole('menuitem', { name: g().menuDelete }))
    expect(await screen.findByText(t.errors.group.notFound)).toBeVisible()
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
  })

  it('成员：展开看，叉号移除，选一个节点添加；All 的成员只能看', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({
      'PUT /networks/1/groups/3/nodes/1': { ...LAPTOPS, nodes: [ref(gw), ref(laptop)] },
      'DELETE /networks/1/groups/3/nodes/2': { ...LAPTOPS, nodes: [] },
    })
    render()
    await user.click(await screen.findByRole('button', { name: g().membersOf('laptops') }))
    const members = await screen.findByTestId('group-members-laptops')
    await user.click(within(members).getByRole('button', { name: g().removeMember('laptop', 'laptops') }))
    await waitFor(() => expect(api.callsTo('DELETE /networks/1/groups/3/nodes/2')).toHaveLength(1))
    expect(await screen.findByText(g().removed('laptop', 'laptops'))).toBeVisible()

    await user.click(within(members).getByRole('combobox', { name: g().addMemberTo('laptops') }))
    await user.click(await screen.findByRole('option', { name: /gw/ }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/groups/3/nodes/1')).toHaveLength(1))

    await user.click(screen.getByRole('button', { name: g().membersOf('All') }))
    const all = await screen.findByTestId('group-members-All')
    expect(all).toHaveTextContent(g().allMembers)
    expect(within(all).queryByRole('button')).not.toBeInTheDocument()
    expect(within(all).queryByRole('combobox')).not.toBeInTheDocument()
  })

  it('从操作记录点过来（?focus=）：那一行展开；已经删掉了的说一句', async () => {
    setup()
    const { unmount } = render('/n/1/groups?focus=3')
    expect(await screen.findByTestId('group-members-laptops')).toBeVisible()
    unmount()
    setup()
    render('/n/1/groups?focus=77')
    expect(await screen.findByText(g().focusMissing(77))).toBeVisible()
  })
})
