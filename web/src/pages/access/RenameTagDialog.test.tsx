import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type AccessGraph } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { apiFailure, fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { TagPanel } from './TagPanel'

const p = t.access.tagsPanel
const laptop = makeNode(2, 'laptop', { tags: ['laptops'] })
const phone = makeNode(3, 'phone', { tags: ['phones'] })
const ref = (node: typeof laptop) => ({ id: node.id, name: node.name, virtual_ip: node.virtual_ip })

const GRAPH: AccessGraph = {
  tags: [
    { tag: 'all', nodes: [ref(laptop), ref(phone)] },
    { tag: 'laptops', nodes: [ref(laptop)] },
    { tag: 'phones', nodes: [ref(phone)] },
  ],
  rules: [{ id: 2, name: 'ssh', src: 'laptops', dst: 'all', direction: 'one_way', enabled: true }],
  unconnected: [],
  unacknowledged: [],
}

function setup() {
  return fakeApi({
    'GET /networks/1/nodes': [laptop, phone],
    'POST /networks/1/tags/rename': (call) => {
      const body = call.body as { from: string; to: string }
      return { ...body, nodes: 1, rules: 1, tokens: 0 }
    },
  })
}

async function openRename(user: ReturnType<typeof userEvent.setup>, tag: string) {
  renderWithApp(<TagPanel network={NETWORK} graph={GRAPH} open={tag} onOpenChange={() => undefined} />)
  await user.click(await screen.findByRole('button', { name: p.renameFor(tag) }))
  return screen.findByRole('dialog', { name: p.renameTitle(tag) })
}

describe('标签改名', () => {
  it('整个网络里一起改；新名字没人用时说谁能访问谁不变', async () => {
    const user = userEvent.setup()
    const api = setup()
    const dialog = await openRename(user, 'laptops')
    expect(within(dialog).getByText(p.renameUses(1, 1))).toBeVisible()
    const input = within(dialog).getByRole('textbox', { name: p.renameLabel })
    await user.clear(input)
    await user.type(input, 'Notebooks')
    expect(within(dialog).getByText(p.renameNoEffect)).toBeVisible()
    await user.click(within(dialog).getByRole('button', { name: p.renameSubmit }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/tags/rename')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/tags/rename')[0]?.body).toEqual({ from: 'laptops', to: 'notebooks' })
    expect(await screen.findByText(p.renamed('laptops', 'notebooks', 1, 1, 0))).toBeVisible()
  })

  it('改成已经有人用的名字：两组合成一组，保存之前说出来', async () => {
    const user = userEvent.setup()
    setup()
    const dialog = await openRename(user, 'laptops')
    const input = within(dialog).getByRole('textbox', { name: p.renameLabel })
    await user.clear(input)
    await user.type(input, 'phones')
    expect(within(dialog).getByText(p.renameMerge('phones', 1))).toBeVisible()
  })

  it('不能改成 all，不能和原来一样，写法不对的在这里就拦下', async () => {
    const user = userEvent.setup()
    const api = setup()
    const dialog = await openRename(user, 'laptops')
    const input = within(dialog).getByRole('textbox', { name: p.renameLabel })
    await user.clear(input)
    await user.type(input, 'all')
    expect(within(dialog).getByText(p.renameAll)).toBeVisible()
    await user.clear(input)
    await user.type(input, 'my.tag')
    expect(within(dialog).getByText(t.validate.tagCharset)).toBeVisible()
    await user.click(within(dialog).getByRole('button', { name: p.renameSubmit }))
    expect(api.callsTo('POST /networks/1/tags/rename')).toHaveLength(0)
  })

  it('controller 不答应时把它的话写在输入框下面', async () => {
    const user = userEvent.setup()
    const api = setup()
    api.use({ 'POST /networks/1/tags/rename': () => apiFailure(400, 'invalid_request', '标签的写法不对') })
    const dialog = await openRename(user, 'laptops')
    const input = within(dialog).getByRole('textbox', { name: p.renameLabel })
    await user.clear(input)
    await user.type(input, 'x')
    await user.click(within(dialog).getByRole('button', { name: p.renameSubmit }))
    await waitFor(() => expect(input).toHaveAccessibleDescription(/标签的写法不对/))
  })

  it('内置的 all 没有改名的按钮', async () => {
    setup()
    renderWithApp(<TagPanel network={NETWORK} graph={GRAPH} open="all" onOpenChange={() => undefined} />)
    expect(await screen.findByText(p.allMembers)).toBeVisible()
    expect(screen.queryByRole('button', { name: p.renameFor('all') })).not.toBeInTheDocument()
  })
})
