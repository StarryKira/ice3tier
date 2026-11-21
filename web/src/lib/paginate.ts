/**
 * 列表怎么分页。页码写在地址里（`?page=3`，第一页不写），浏览器的前进后退能还原。
 * 翻页的按钮是 `components/pagination.tsx`。
 */

/** 地址里的页码。没写、写得不对都当第一页。 */
export function readPage(params: URLSearchParams): number {
  const page = Number(params.get('page') ?? '1')
  return Number.isInteger(page) && page >= 1 ? page : 1
}

/** 带上页码的地址参数（另起一份，不改传进来的）。第一页不写。 */
export function withPage(params: URLSearchParams, page: number): URLSearchParams {
  const next = new URLSearchParams(params)
  if (page > 1) next.set('page', String(page))
  else next.delete('page')
  return next
}

export interface Paged<T> {
  /** 实际显示的是第几页，从 1 起。 */
  page: number
  pages: number
  rows: T[]
}

/**
 * 一页 `size` 行时，第 `page` 页是哪些行。
 * 页码超出时落在最后一页：筛选之后行少了，地址里的页码还是原来的。
 */
export function paginate<T>(rows: readonly T[], page: number, size: number): Paged<T> {
  const pages = Math.max(1, Math.ceil(rows.length / size))
  const current = Math.min(Math.max(1, page), pages)
  return { page: current, pages, rows: rows.slice((current - 1) * size, current * size) }
}
