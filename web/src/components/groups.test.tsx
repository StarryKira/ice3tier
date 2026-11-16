import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'

import { type Group, type GroupRef } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { renderWithApp } from '@/test/harness'

import { GroupList, GroupPicker } from './groups'

const group = (id: number, name: string, builtin = false): Pick<Group, 'id' | 'name' | 'builtin'> => ({
  id,
  name,
  builtin,
})

const GROUPS = [group(1, 'All', true), group(2, 'laptops'), group(3, 'Servers'), group(4, 'home')]

function Harness({ initial = [], groups = GROUPS }: { initial?: GroupRef[]; groups?: typeof GROUPS }) {
  const [value, setValue] = useState<GroupRef[]>(initial)
  return (
    <>
      <GroupPicker aria-label="groups" value={value} onChange={setValue} groups={groups} autoFocus />
      <output data-testid="value">{value.map((entry) => entry.id).join(',')}</output>
    </>
  )
}

const value = () => screen.getByTestId('value').textContent

describe('选分组', () => {
  for (const locale of ['zh-CN', 'en'] as const) {
    it(`All 总在最前面、移不掉；输入只是筛选，回车选中，不会新建（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      const user = userEvent.setup()
      renderWithApp(<Harness />)
      const input = screen.getByRole('combobox', { name: 'groups' })
      expect(input).toHaveAttribute('placeholder', t.groups.picker.placeholder)
      expect(screen.getByText('All')).toBeVisible()
      expect(screen.queryByRole('button', { name: t.groups.picker.remove('All') })).not.toBeInTheDocument()

      // 没有这个名字的分组：什么也选不上，列表里说没有匹配的。
      await user.type(input, 'nope')
      expect(screen.getByText(t.groups.picker.noneFound)).toBeVisible()
      await user.keyboard('{Enter}')
      expect(value()).toBe('')

      // 筛选不分大小写；All 不在候选里。
      await user.clear(input)
      await user.type(input, 'serv{Enter}')
      expect(value()).toBe('3')
      await user.type(input, 'a')
      const options = within(screen.getByRole('listbox', { name: t.groups.picker.options })).getAllByRole('option')
      expect(options.map((option) => option.textContent)).toEqual(['laptops'])
    })
  }

  it('获得焦点时不打开候选列表；向下键打开，停在第一个', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness />)
    expect(screen.getByRole('combobox', { name: 'groups' })).toHaveFocus()
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()
    await user.keyboard('{ArrowDown}')
    expect(screen.getByRole('option', { name: 'laptops' })).toHaveAttribute('aria-selected', 'true')
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()
  })

  it('选中的按名字排；叉号和退格移除', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness initial={[{ id: 3, name: 'Servers' }]} />)
    const input = screen.getByRole('combobox', { name: 'groups' })
    await user.type(input, 'home{Enter}')
    expect(value()).toBe('4,3')
    await user.click(screen.getByRole('button', { name: t.groups.picker.remove('Servers') }))
    expect(value()).toBe('4')
    await user.type(input, '{Backspace}')
    expect(value()).toBe('')
  })

  it('网络里只有 All 时说还没有别的分组', async () => {
    const user = userEvent.setup()
    renderWithApp(<Harness groups={[group(1, 'All', true)]} />)
    await user.keyboard('{ArrowDown}')
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
    expect(screen.getByLabelText(t.groups.picker.more(2, `c${t.common.listSeparator}d`))).toHaveTextContent('+2')
  })
})
