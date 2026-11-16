import { ArrowDown, ArrowUp, ArrowUpDown } from 'lucide-react'
import { type ComponentProps, type ReactNode } from 'react'

import { t } from '@/i18n'
import { cn } from '@/lib/utils'

/** 全站一种表格：第一列是名字，操作在最后一列的菜单里，数字右对齐。 */
export function Table({ className, ...props }: ComponentProps<'table'>) {
  return (
    <div className="overflow-x-auto">
      <table className={cn('w-full border-collapse text-sm', className)} {...props} />
    </div>
  )
}

export function Th({ className, numeric = false, ...props }: ComponentProps<'th'> & { numeric?: boolean }) {
  return (
    <th
      scope="col"
      className={cn(
        'border-b bg-muted/50 px-4 py-2.5 text-xs font-medium whitespace-nowrap text-muted-foreground first:rounded-tl-lg last:rounded-tr-lg',
        numeric ? 'text-right' : 'text-left',
        className,
      )}
      {...props}
    />
  )
}

export type SortDirection = 'asc' | 'desc'

interface SortThProps {
  children: ReactNode
  active: boolean
  direction: SortDirection
  onSort: () => void
  className?: string
}

/** 可以点的表头：点一下按这一列排，再点一下反过来。 */
export function SortTh({ children, active, direction, onSort, className }: SortThProps) {
  const Icon = !active ? ArrowUpDown : direction === 'asc' ? ArrowUp : ArrowDown
  return (
    <Th aria-sort={active ? (direction === 'asc' ? 'ascending' : 'descending') : 'none'} className={className}>
      <button
        type="button"
        onClick={onSort}
        className="-mx-1 inline-flex items-center gap-1 rounded-sm px-1 hover:text-foreground"
        title={t.common.sortBy}
      >
        {children}
        <Icon className={cn('size-3', !active && 'opacity-50')} aria-hidden />
      </button>
    </Th>
  )
}

export function Tr({ className, dimmed = false, ...props }: ComponentProps<'tr'> & { dimmed?: boolean }) {
  return (
    <tr
      className={cn('border-b last:border-b-0', dimmed && '[&>td:not([data-keep])]:opacity-55', className)}
      {...props}
    />
  )
}

export function Td({ className, numeric = false, ...props }: ComponentProps<'td'> & { numeric?: boolean }) {
  return <td className={cn('px-4 py-3 align-middle', numeric && 'text-right tabular', className)} {...props} />
}
