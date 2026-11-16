import { t } from '@/i18n'
import { cn } from '@/lib/utils'

/** 标志：三个互相连着的节点。 */
export function BrandMark({ className }: { className?: string }) {
  return (
    <div className={cn('flex items-center gap-2.5', className)}>
      <svg viewBox="0 0 32 32" className="size-7" aria-hidden>
        <rect width="32" height="32" rx="8" className="fill-foreground" />
        <g fill="none" stroke="#fff" strokeWidth="2.2" strokeLinecap="round">
          <path d="M10 11 16 21 22 11" />
          <path d="M10 11h12" />
        </g>
        <g fill="#fff">
          <circle cx="10" cy="11" r="2.6" />
          <circle cx="22" cy="11" r="2.6" />
          <circle cx="16" cy="21" r="2.6" />
        </g>
      </svg>
      <span className="text-base font-semibold tracking-tight">{t.common.appName}</span>
    </div>
  )
}
