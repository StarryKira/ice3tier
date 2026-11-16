import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'

import { TagInput } from './tags'

function Harness() {
  const [tags, setTags] = useState<string[]>([])
  return (
    <TagInput
      aria-label={t.nodes.batchTagLabel}
      value={tags}
      onChange={setTags}
      suggestions={['home', 'laptops']}
      autoFocus
    />
  )
}

describe('标签选择器', () => {
  it('获得焦点时不打开候选列表（对话框一打开就聚焦，列表会盖住下面的按钮）；输入或按向下键才打开', async () => {
    render(<Harness />)
    const input = screen.getByRole('combobox', { name: t.nodes.batchTagLabel })
    expect(input).toHaveFocus()
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()

    await userEvent.keyboard('{ArrowDown}')
    expect(screen.getByRole('listbox', { name: t.tags.suggestions })).toBeVisible()
    // 第一下只是打开，停在第一项。
    expect(screen.getByRole('option', { name: 'home' })).toHaveAttribute('aria-selected', 'true')
    await userEvent.keyboard('{Escape}')
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument()

    await userEvent.type(input, 'lap')
    expect(screen.getByRole('option', { name: 'laptops' })).toBeVisible()
  })
})
