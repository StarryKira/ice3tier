import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'

import { type Group, type GroupRef } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { renderWithApp } from '@/test/harness'

import { GroupList, GroupPicker } from './groups'

type PickerGroup = Pick<Group, 'id' | 'name' | 'builtin' | 'nodes'>

const node = (id: number) => ({ id, name: `n${id}`, virtual_ip: `10.77.0.${id}` })
const group = (id: number, name: string, members: number[] = [], builtin = false): PickerGroup => ({
  id,
  name,
  builtin,
  nodes: members.map(node),
})

const GROUPS = [
  group(1, 'All', [1, 2, 3], true),
  group(2, 'laptops', [1, 2]),
  group(3, 'Servers', [3]),
  group(4, 'home'),
]

function Harness({ initial = [], groups = GROUPS }: { initial?: GroupRef[]; groups?: PickerGroup[] }) {
  const [value, setValue] = useState<GroupRef[]>(initial)
  return (
    <>
      <GroupPicker aria-label="groups" value={value} onChange={setValue} groups={groups} autoFocus />
      <output data-testid="value">{value.map((entry) => entry.id).join(',')}</output>
    </>
  )
}

const value = () => screen.getByTestId('value').textContent
const search = () => screen.getByRole('textbox', { name: t.groups.picker.search })
const option = (name: string) => screen.getByRole('option', { name: new RegExp(`^${name}`) })

describe('选分组', () => {
  for (const locale of ['zh-CN', 'en'] as const) {
    it(`框里总有 All、移不掉；列表里每行写着几个节点，All 勾着、勾不掉（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      const user = userEvent.setup()
      renderWithApp(<Harness />)
      expect(screen.getByText('All')).toBeVisible()
      expect(screen.queryByRole('button', { name: t.groups.picker.remove('All') })).not.toBeInTheDocument()

      await user.click(screen.getByRole('combobox', { name: 'groups' }))
      const list = screen.getByRole('listbox', { name: t.groups.picker.options })
      expect(
        within(list)
          .getAllByRole('option')
          .map((row) => row.textContent),
      ).toEqual([
        `All${t.groups.picker.nodes(3)}`,
        `laptops${t.groups.picker.nodes(2)}`,
        `Servers${t.groups.picker.nodes(1)}`,
        `home${t.groups.picker.nodes(0)}`,
      ])
      expect(option('All')).toHaveAttribute('aria-selected', 'true')
      expect(option('All')).toHaveAttribute('aria-disabled', 'true')
      await user.click(option('All'))
      expect(value()).toBe('')
    })
  }

  it('搜索只是筛选，回车勾选，不会新建；勾着的再回车取消', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness />)
    await user.keyboard('{ArrowDown}')
    expect(search()).toHaveFocus()

    await user.type(search(), 'nope')
    expect(screen.getByText(t.groups.picker.noneFound)).toBeVisible()
    await user.keyboard('{Enter}')
    expect(value()).toBe('')

    // 筛选不分大小写；回车勾的是第一个能勾的，列表不收起，接着能勾别的。
    await user.clear(search())
    await user.type(search(), 'serv{Enter}')
    expect(value()).toBe('3')
    expect(option('Servers')).toHaveAttribute('aria-selected', 'true')
    await user.clear(search())
    await user.type(search(), 'home{Enter}')
    expect(value()).toBe('4,3')
    await user.keyboard('{Enter}')
    expect(value()).toBe('3')

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()
    expect(screen.getByRole('combobox', { name: 'groups' })).toHaveFocus()
  })

  it('获得焦点时不打开列表；打开时停在第一个能勾的', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness />)
    expect(screen.getByRole('combobox', { name: 'groups' })).toHaveFocus()
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()
    await user.keyboard('{ArrowDown}')
    expect(search()).toHaveAttribute('aria-activedescendant', option('laptops').id)
  })

  it('点行勾选或取消，选中的按名字排；框里的叉移除', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness initial={[{ id: 3, name: 'Servers' }]} />)
    await user.click(screen.getByRole('combobox', { name: 'groups' }))
    await user.click(option('home'))
    expect(value()).toBe('4,3')
    await user.click(option('Servers'))
    expect(value()).toBe('4')
    await user.keyboard('{Escape}')
    await user.click(screen.getByRole('button', { name: t.groups.picker.remove('home') }))
    expect(value()).toBe('')
    // 点叉不会打开列表。
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()
  })

  it('还在保存时列表开着，但勾不了', async () => {
    const user = userEvent.setup()
    renderWithApp(<GroupPicker aria-label="groups" value={[]} onChange={vi.fn()} groups={GROUPS} busy />)
    await user.click(screen.getByRole('combobox', { name: 'groups' }))
    expect(option('home')).toHaveAttribute('aria-disabled', 'true')
  })

  it('网络里只有 All 时说还没有别的分组', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness groups={[group(1, 'All', [1], true)]} />)
    await user.click(screen.getByRole('combobox', { name: 'groups' }))
    expect(screen.getByText(t.groups.picker.noGroups)).toBeVisible()
  })
})

describe('一串分组', () => {
  it('前两个写出来，其余折成 +N，读屏读得到是哪几个', () => {
    renderWithApp(
      <GroupList
        groups={[
          { id: 2, name: 'a' },
          { id: 3, name: 'b' },
          { id: 4, name: 'c' },
          { id: 5, name: 'd' },
        ]}
      />,
    )
    expect(screen.getByText('a')).toBeVisible()
    expect(screen.queryByText('c')).not.toBeInTheDocument()
    expect(screen.getByLabelText(t.common.moreItems(2, `c${t.common.listSeparator}d`))).toHaveTextContent('+2')
  })
})
