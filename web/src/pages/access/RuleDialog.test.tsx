import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type AccessGraph, type Rule } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { RuleDialog } from './RuleDialog'

const ref = (id: number, name: string) => ({ id, name, virtual_ip: `10.66.0.${id}` })
const ALL = { id: 1, name: 'All' }
const LAPTOPS = { id: 3, name: 'laptops' }
const SERVERS = { id: 4, name: 'servers' }
const DEFAULT: Rule = { id: 1, name: 'default', src: ALL, dst: ALL, direction: 'both', enabled: false }

const GRAPH: AccessGraph = {
  groups: [
    { ...ALL, builtin: true, nodes: [ref(1, 'gw'), ref(2, 'laptop')], rules: [DEFAULT] },
    { ...LAPTOPS, builtin: false, nodes: [ref(2, 'laptop')], rules: [] },
    { ...SERVERS, builtin: false, nodes: [ref(1, 'gw')], rules: [] },
    { id: 5, name: 'empty', builtin: false, nodes: [], rules: [] },
  ],
  rules: [DEFAULT],
  unconnected: [ref(1, 'gw'), ref(2, 'laptop')],
  unacknowledged: [],
}

async function pick(user: ReturnType<typeof userEvent.setup>, label: string, option: RegExp) {
  await user.click(screen.getByRole('combobox', { name: label }))
  await user.click(await screen.findByRole('option', { name: option }))
}

describe('规则写在两个分组之间', () => {
  for (const locale of ['zh-CN', 'en'] as const) {
    it(`只能选已有的分组，交给 controller 的是编号；句子和影响按分组的成员算（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      const user = userEvent.setup()
      const d = t.access.dialog
      const api = fakeApi({
        'POST /networks/1/rules': (call) => ({ id: 7, ...(call.body as object), src: LAPTOPS, dst: SERVERS }),
      })
      renderWithApp(<RuleDialog network={NETWORK} graph={GRAPH} rule={null} open onClose={() => undefined} />)

      // 搜一个不存在的名字：没有"新建"这一项。
      await user.click(screen.getByRole('combobox', { name: d.srcLabel }))
      await user.type(screen.getByRole('textbox', { name: t.groups.picker.search }), 'newgroup')
      expect(screen.getByText(t.groups.picker.noneFound)).toBeVisible()
      expect(screen.queryByRole('option')).not.toBeInTheDocument()
      await user.keyboard('{Escape}')

      await pick(user, d.srcLabel, /laptops/)
      await pick(user, d.dstLabel, /servers/)
      await user.click(screen.getByRole('radio', { name: d.oneWay }))
      expect(screen.getByTestId('rule-sentence')).toHaveTextContent(
        d.sentenceOneWay(d.nodesOf('laptops', 1), d.nodesOf('servers', 1)),
      )
      expect(screen.getByTestId('rule-impact')).toHaveTextContent(d.impactGain(1))

      await user.click(screen.getByRole('button', { name: d.submitCreate }))
      await waitFor(() => expect(api.callsTo('POST /networks/1/rules')).toHaveLength(1))
      expect(api.callsTo('POST /networks/1/rules')[0]?.body).toEqual({
        src: 3,
        dst: 4,
        direction: 'one_way',
        name: '',
        enabled: true,
      })
    })
  }

  it('All 写成"全部节点"；没有成员的分组提醒一句', async () => {
    const user = userEvent.setup()
    const d = t.access.dialog
    fakeApi()
    renderWithApp(<RuleDialog network={NETWORK} graph={GRAPH} rule={null} open onClose={() => undefined} />)
    await pick(user, d.srcLabel, /All/)
    await pick(user, d.dstLabel, /empty/)
    expect(screen.getByTestId('rule-sentence')).toHaveTextContent(
      d.sentenceBoth(d.nodesOfAll(2), d.nodesOf('empty', 0)),
    )
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText(d.emptyGroup)).toBeVisible()
  })

  it('编辑：分组改过名，规则里写的是现在的名字，按编号认出是哪一个', () => {
    fakeApi()
    const renamed = { ...LAPTOPS, name: 'notebooks' }
    const graph = { ...GRAPH, groups: GRAPH.groups.map((g) => (g.id === 3 ? { ...g, name: 'notebooks' } : g)) }
    const rule: Rule = { id: 2, name: 'ssh', src: renamed, dst: SERVERS, direction: 'one_way', enabled: true }
    renderWithApp(<RuleDialog network={NETWORK} graph={graph} rule={rule} open onClose={() => undefined} />)
    expect(screen.getByRole('combobox', { name: t.access.dialog.srcLabel })).toHaveTextContent('notebooks')
  })
})
