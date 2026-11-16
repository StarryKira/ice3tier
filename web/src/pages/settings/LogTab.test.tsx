import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type LogEntry } from '@/api/types'
import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { type ApiCall, fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { LogTab } from './LogTab'

const s = t.settings.log

/** 150 条记录，编号从 150 往下：一页一百条，要翻一次。 */
const ENTRIES: LogEntry[] = Array.from({ length: 150 }, (_, index) => {
  const id = 150 - index
  if (id === 150)
    return {
      id,
      at: 1_794_000_000,
      network: 1,
      action: 'credential.revoke',
      detail: '凭据 #5 phone 已吊销',
      refs: [
        { kind: 'credential', id: 5 },
        { kind: 'node', id: 2 },
        { kind: 'gadget', id: 1 },
      ],
    }
  return { id, at: 1_794_000_000 - index, network: 1, action: 'node.update', detail: `第 ${id} 条`, refs: [] }
})

/** 按 controller 的规则筛：before、network、action（前缀）、ref。 */
function serve(call: ApiCall): LogEntry[] {
  const before = Number(call.query.get('before') ?? Infinity)
  const action = call.query.get('action')
  const ref = call.query.get('ref')
  const limit = Number(call.query.get('limit') ?? 100)
  return ENTRIES.filter((entry) => entry.id < before)
    .filter((entry) => action === null || entry.action === action || entry.action.startsWith(`${action}.`))
    .filter((entry) => ref === null || entry.refs.some((item) => `${item.kind}:${item.id}` === ref))
    .slice(0, limit)
}

function setup() {
  return fakeApi({
    'GET /log': serve,
    'GET /networks': [NETWORK],
    'GET /networks/1/nodes': [makeNode(2, 'laptop')],
  })
}

describe('操作记录', () => {
  it('一页一页往前翻：下一页从上一页最后一条的编号往前取', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<LogTab />, { route: '/settings/log' })
    expect(await screen.findByTestId('log-row-150')).toBeVisible()
    expect(screen.queryByTestId('log-row-50')).not.toBeInTheDocument()
    expect(screen.getByTestId('log-count')).toHaveTextContent(s.showing(100, 100))

    await user.click(screen.getByRole('button', { name: s.loadMore }))
    expect(await screen.findByTestId('log-row-50')).toBeVisible()
    const calls = api.callsTo('GET /log')
    expect(calls.at(-1)?.query.get('before')).toBe('51')
    expect(screen.getByText(s.noMore)).toBeVisible()
    expect(screen.queryByRole('button', { name: s.loadMore })).not.toBeInTheDocument()
  })

  it('记录说的东西做成链接；节点写名字；不认识的种类不显示', async () => {
    setup()
    renderWithApp(<LogTab />, { route: '/settings/log' })
    const row = await screen.findByTestId('log-row-150')
    expect(within(row).getByRole('link', { name: s.refLabels.credential(5) })).toHaveAttribute(
      'href',
      '/n/1/credentials?focus=5',
    )
    expect(await within(row).findByRole('link', { name: s.refLabels.node(2, 'laptop') })).toHaveAttribute(
      'href',
      '/n/1/nodes/2',
    )
    expect(within(row).queryByText(/gadget/)).not.toBeInTheDocument()
  })

  it('按操作筛：交给 controller，一类操作写前缀', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<LogTab />, { route: '/settings/log' })
    await screen.findByTestId('log-row-150')
    await user.selectOptions(screen.getByRole('combobox', { name: s.filterAction }), 'credential')
    await waitFor(() => expect(api.callsTo('GET /log').at(-1)?.query.get('action')).toBe('credential'))
    expect(await screen.findByTestId('log-count')).toHaveTextContent(s.showing(1, 1))
    expect(screen.queryByTestId('log-row-149')).not.toBeInTheDocument()
  })

  it('只看和某样东西有关的：限定在它的网络里；条件可以单独去掉', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<LogTab />, { route: '/settings/log' })
    const row = await screen.findByTestId('log-row-150')
    await user.click(within(row).getByRole('button', { name: s.filterByRef(s.refLabels.credential(5)) }))
    await waitFor(() => expect(api.callsTo('GET /log').at(-1)?.query.get('ref')).toBe('credential:5'))
    expect(api.callsTo('GET /log').at(-1)?.query.get('network')).toBe('1')
    expect(screen.getByTestId('log-ref-filter')).toHaveTextContent(s.refChip(s.refLabels.credential(5)))
    expect(await screen.findByTestId('log-count')).toHaveTextContent(s.showing(1, 1))

    await user.click(screen.getByRole('button', { name: s.refRemove(s.refLabels.credential(5)) }))
    await waitFor(() => expect(api.callsTo('GET /log').at(-1)?.query.get('ref')).toBeNull())
    expect(await screen.findByTestId('log-row-149')).toBeVisible()
  })

  it('地址里带来的条件直接生效；什么都筛不出来时可以一键清除', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<LogTab />, { route: '/settings/log?action=tag&q=x' })
    expect(await screen.findByText(s.noMatchesServer)).toBeVisible()
    expect(api.callsTo('GET /log')[0]?.query.get('action')).toBe('tag')
    expect(screen.getByRole('searchbox', { name: s.searchPlaceholder })).toHaveValue('x')
    const [clear] = screen.getAllByRole('button', { name: t.common.clearFilters })
    if (clear === undefined) throw new Error('没有"清除筛选"')
    await user.click(clear)
    expect(await screen.findByTestId('log-row-150')).toBeVisible()
  })

  it('搜索只在取回的记录里找', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<LogTab />, { route: '/settings/log' })
    await screen.findByTestId('log-row-150')
    await user.type(screen.getByRole('searchbox', { name: s.searchPlaceholder }), 'phone')
    expect(screen.getByTestId('log-count')).toHaveTextContent(s.showing(1, 100))
  })
})
