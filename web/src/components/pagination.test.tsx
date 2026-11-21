import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { createRef } from 'react'
import { describe, expect, it, vi } from 'vitest'

import { t } from '@/i18n'

import { Pagination } from './pagination'

describe('翻页', () => {
  it('写着第几页；上一页、下一页各翻一页', async () => {
    const user = userEvent.setup()
    const onPage = vi.fn()
    render(<Pagination page={2} pages={3} onPage={onPage} />)
    expect(screen.getByRole('navigation', { name: t.common.pagination })).toHaveTextContent(t.common.pageOf(2, 3))
    await user.click(screen.getByRole('button', { name: t.common.prevPage }))
    await user.click(screen.getByRole('button', { name: t.common.nextPage }))
    expect(onPage.mock.calls).toEqual([[1], [3]])
  })

  it('第一页不能再往前，最后一页不能再往后', () => {
    const { rerender } = render(<Pagination page={1} pages={3} onPage={() => undefined} />)
    expect(screen.getByRole('button', { name: t.common.prevPage })).toBeDisabled()
    expect(screen.getByRole('button', { name: t.common.nextPage })).toBeEnabled()
    rerender(<Pagination page={3} pages={3} onPage={() => undefined} />)
    expect(screen.getByRole('button', { name: t.common.prevPage })).toBeEnabled()
    expect(screen.getByRole('button', { name: t.common.nextPage })).toBeDisabled()
  })

  it('只有一页时不显示', () => {
    const { container } = render(<Pagination page={1} pages={1} onPage={() => undefined} />)
    expect(container).toBeEmptyDOMElement()
  })

  it('翻页之后把列表最上面的那一块滚到看得见的地方', async () => {
    const user = userEvent.setup()
    const top = createRef<HTMLDivElement>()
    render(
      <>
        <div ref={top} />
        <Pagination page={1} pages={2} onPage={() => undefined} top={top} />
      </>,
    )
    if (top.current === null) throw new Error('没有渲染出来')
    const scroll = vi.spyOn(top.current, 'scrollIntoView')
    await user.click(screen.getByRole('button', { name: t.common.nextPage }))
    expect(scroll).toHaveBeenCalledWith({ block: 'nearest' })
  })
})
