import { CircleAlert, type LucideIcon, SearchX } from 'lucide-react'
import { type ReactNode } from 'react'

import { errorMessage } from '@/api/client'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'

interface EmptyStateProps {
  icon?: LucideIcon
  /** 一句话：现在是什么情况。 */
  title: string
  /** 一句解释：这是什么、为什么要它。 */
  description?: ReactNode
  /** 下一步做什么。通常只有一个主按钮。 */
  action?: ReactNode
  children?: ReactNode
  className?: string
}

/** "还没有数据"的样子：它本身就是入口。和"筛选之后没有结果"是两回事，后者用 {@link NoMatches}。 */
export function EmptyState({ icon: Icon, title, description, action, children, className }: EmptyStateProps) {
  return (
    <div className={cn('flex flex-col items-center px-6 py-12 text-center', className)}>
      {Icon !== undefined ? (
        <span className="mb-4 flex size-11 items-center justify-center rounded-full bg-accent text-accent-foreground">
          <Icon className="size-5" aria-hidden />
        </span>
      ) : null}
      <h2 className="text-[15px] font-semibold">{title}</h2>
      {description !== undefined ? (
        <p className="mt-1.5 max-w-lg text-sm text-muted-foreground">{description}</p>
      ) : null}
      {action !== undefined ? (
        <div className="mt-5 flex flex-wrap items-center justify-center gap-2">{action}</div>
      ) : null}
      {children}
    </div>
  )
}

/** 筛选或搜索之后一行也不剩。 */
export function NoMatches({ message, onClear }: { message: string; onClear: () => void }) {
  return (
    <div className="flex flex-col items-center px-6 py-10 text-center">
      <SearchX className="mb-3 size-6 text-muted-foreground" aria-hidden />
      <p className="text-sm text-muted-foreground">{message}</p>
      <Button variant="outline" size="sm" className="mt-3" onClick={onClear}>
        {t.common.clearFilters}
      </Button>
    </div>
  )
}

interface ErrorStateProps {
  /** 哪一步出了错，例如"节点列表没有取到"。 */
  title: string
  error: unknown
  onRetry?: () => void
  className?: string
}

/** 取数失败。服务器给的说明原样显示。 */
export function ErrorState({ title, error, onRetry, className }: ErrorStateProps) {
  return (
    <div role="alert" className={cn('flex flex-col items-center px-6 py-10 text-center', className)}>
      <span className="mb-3 flex size-10 items-center justify-center rounded-full bg-danger-soft text-danger">
        <CircleAlert className="size-5" aria-hidden />
      </span>
      <h2 className="text-[15px] font-semibold">{title}</h2>
      <p className="mt-1.5 max-w-lg text-sm text-muted-foreground">{errorMessage(error)}</p>
      {onRetry !== undefined ? (
        <Button variant="outline" size="sm" className="mt-4" onClick={onRetry}>
          {t.common.retry}
        </Button>
      ) : null}
    </div>
  )
}

/** 表格第一次加载时的骨架行，形状和将要出现的行一致。 */
export function SkeletonRows({ rows = 4, columns }: { rows?: number; columns: number }) {
  return (
    <>
      {Array.from({ length: rows }, (_, row) => (
        <tr key={row} className="border-b last:border-b-0">
          {Array.from({ length: columns }, (_, column) => (
            <td key={column} className="px-4 py-3.5">
              <Skeleton className={cn('h-3.5', column === 0 ? 'w-36' : 'w-20')} />
            </td>
          ))}
        </tr>
      ))}
    </>
  )
}

export function SkeletonBlock({ lines = 3, className }: { lines?: number; className?: string }) {
  return (
    <div className={cn('space-y-3 p-5', className)} aria-busy="true" aria-label={t.common.loading}>
      {Array.from({ length: lines }, (_, line) => (
        <Skeleton key={line} className={cn('h-3.5', line % 3 === 0 ? 'w-2/5' : line % 3 === 1 ? 'w-3/5' : 'w-1/3')} />
      ))}
    </div>
  )
}
