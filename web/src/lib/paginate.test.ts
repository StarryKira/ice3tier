import { describe, expect, it } from 'vitest'

import { paginate, readPage, withPage } from './paginate'

const ROWS = Array.from({ length: 60 }, (_, index) => index + 1)

describe('分页', () => {
  it('一页 25 行：60 行是三页，最后一页不满', () => {
    expect(paginate(ROWS, 1, 25)).toEqual({ page: 1, pages: 3, rows: ROWS.slice(0, 25) })
    expect(paginate(ROWS, 3, 25)).toEqual({ page: 3, pages: 3, rows: ROWS.slice(50) })
  })

  it('页码超出时落在最后一页；没有行也算一页', () => {
    expect(paginate(ROWS, 9, 25).page).toBe(3)
    expect(paginate(ROWS, 0, 25).page).toBe(1)
    expect(paginate([], 4, 25)).toEqual({ page: 1, pages: 1, rows: [] })
  })

  it('刚好整页时不多出一页空的', () => {
    expect(paginate(ROWS.slice(0, 50), 2, 25)).toEqual({ page: 2, pages: 2, rows: ROWS.slice(25, 50) })
  })

  it('地址里的页码：没写、写得不对都当第一页', () => {
    expect(readPage(new URLSearchParams('page=3'))).toBe(3)
    expect(readPage(new URLSearchParams(''))).toBe(1)
    for (const bad of ['0', '-2', '2.5', 'x', '']) expect(readPage(new URLSearchParams({ page: bad }))).toBe(1)
  })

  it('写进地址：第一页不写，别的参数留着，不改传进来的那一份', () => {
    const params = new URLSearchParams('q=a&page=4')
    expect(withPage(params, 2).toString()).toBe('q=a&page=2')
    expect(withPage(params, 1).toString()).toBe('q=a')
    expect(params.toString()).toBe('q=a&page=4')
  })
})
