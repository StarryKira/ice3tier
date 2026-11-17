import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'

import { type NodeId } from '@/api/types'
import { t } from '@/i18n'
import { MAX_PINNED_RELAYS } from '@/lib/product'
import { makeNode } from '@/test/fixtures'
import { renderWithApp } from '@/test/harness'

import { RelayPicker } from './RelayPicker'

const d = t.nodes.detail
const relay = (id: number, name: string) => makeNode(id, name, { relay: { udp: [], tcp: [], bandwidth_mbps: null } })

const NODES = [makeNode(1, 'self'), relay(2, 'hk'), relay(3, 'tokyo'), makeNode(4, 'laptop')]

function Harness({ initial = [], nodes = NODES }: { initial?: NodeId[]; nodes?: typeof NODES }) {
  const [value, setValue] = useState<NodeId[]>(initial)
  return (
    <>
      <RelayPicker aria-label="pinned" nodes={nodes} self={1} value={value} onChange={setValue} />
      <output data-testid="value">{value.join(',')}</output>
    </>
  )
}

const value = () => screen.getByTestId('value').textContent
// 行的名字前面是读屏用的"在线"、"离线"。
const option = (name: string) => screen.getByRole('option', { name: new RegExp(name) })

describe('指定的中继', () => {
  it('没有指定时框里写自动；列表只有别的中继，勾上、取消都不收起', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness />)
    const box = screen.getByRole('combobox', { name: 'pinned' })
    expect(box).toHaveTextContent(d.pinnedAuto)

    await user.click(box)
    expect(screen.getAllByRole('option').map((row) => row.textContent)).toEqual([
      `${t.common.online}hk10.66.0.2`,
      `${t.common.online}tokyo10.66.0.3`,
    ])
    await user.click(option('tokyo'))
    await user.click(option('hk'))
    expect(value()).toBe('3,2')
    expect(screen.getByRole('listbox')).toBeVisible()
    await user.click(option('tokyo'))
    expect(value()).toBe('2')

    await user.keyboard('{Escape}')
    expect(box).toHaveTextContent('hk')
  })

  it('搜索筛选，回车勾选', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness />)
    await user.click(screen.getByRole('combobox', { name: 'pinned' }))
    await user.type(screen.getByRole('textbox', { name: d.pinnedSearch }), 'tok{Enter}')
    expect(value()).toBe('3')
  })

  it('指定过但已经不是中继的排在最后、带提醒，可以取消', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness initial={[4]} />)
    expect(screen.getByLabelText(d.pinnedNotRelay)).toBeVisible()
    await user.click(screen.getByRole('combobox', { name: 'pinned' }))
    expect(screen.getAllByRole('option').at(-1)).toHaveTextContent(`laptop`)
    expect(option('laptop')).toHaveTextContent(d.pinnedNotRelay)
    await user.click(option('laptop'))
    expect(value()).toBe('')
  })

  it(`选满 ${MAX_PINNED_RELAYS} 个之后没勾的行勾不了`, async () => {
    const user = userEvent.setup()
    const many = [
      makeNode(1, 'self'),
      ...Array.from({ length: MAX_PINNED_RELAYS + 1 }, (_, i) => relay(i + 2, `r${i}`)),
    ]
    const pinned = many.slice(1, MAX_PINNED_RELAYS + 1).map((node) => node.id)
    renderWithApp(<Harness nodes={many} initial={pinned} />)
    await user.click(screen.getByRole('combobox', { name: 'pinned' }))
    const last = option(`r${MAX_PINNED_RELAYS}`)
    expect(last).toHaveAttribute('aria-disabled', 'true')
    await user.click(last)
    expect(value().split(',')).toHaveLength(MAX_PINNED_RELAYS)
  })
})
