import { type RefObject } from 'react'

import { Button } from '@/components/ui/button'
import { t } from '@/i18n'

interface PaginationProps {
  /** 当前第几页，从 1 起。 */
  page: number
  pages: number
  onPage: (page: number) => void
  /** 列表最上面的那一块（工具栏）：翻页之后它要看得见，不然人还停在上一页的末尾。 */
  top?: RefObject<HTMLElement | null>
}

/**
 * 翻页：上一页、第几页、下一页。放在 `TableFooter` 的右端；只有一页时不显示。
 * 哪些行在哪一页、页码怎么写进地址，见 `lib/paginate.ts`。
 */
export function Pagination({ page, pages, onPage, top }: PaginationProps) {
  if (pages <= 1) return null
  const go = (next: number) => {
    onPage(next)
    top?.current?.scrollIntoView({ block: 'nearest' })
  }
  return (
    <nav aria-label={t.common.pagination} className="flex items-center gap-2">
      <Button size="sm" disabled={page <= 1} onClick={() => go(page - 1)}>
        {t.common.prevPage}
      </Button>
      <span className="tabular">{t.common.pageOf(page, pages)}</span>
      <Button size="sm" disabled={page >= pages} onClick={() => go(page + 1)}>
        {t.common.nextPage}
      </Button>
    </nav>
  )
}
