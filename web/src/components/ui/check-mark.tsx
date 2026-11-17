import { Check } from 'lucide-react'

import { cn } from '@/lib/utils'

/**
 * 列表里的勾选框，只是样子：点的是整行（行是 `role="option"`，它的 `aria-selected` 才是给读屏的）。
 * `locked` 是勾不动的（内置的 All、已经选满了）。
 */
export function CheckMark({ checked, locked = false }: { checked: boolean; locked?: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        'flex size-4 shrink-0 items-center justify-center rounded-[4px] border transition-colors',
        checked ? 'border-primary bg-primary text-primary-foreground' : 'border-faint bg-card',
        locked && 'opacity-50',
      )}
    >
      {checked ? <Check className="size-3" style={{ strokeWidth: 3 }} /> : null}
    </span>
  )
}
