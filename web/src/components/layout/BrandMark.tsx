import { t } from '@/i18n'
import { cn } from '@/lib/utils'

/** 标志：三个互相连着的节点。`markOnly` 时只画图形（登录页左上角），名字作为它的无障碍名称。 */
export function BrandMark({ className, markOnly = false }: { className?: string; markOnly?: boolean }) {
  return (
    <div className={cn('flex items-center gap-2.5', className)}>
      <svg
        viewBox="0 0 32 32"
        className="size-7"
        role={markOnly ? 'img' : undefined}
        aria-label={markOnly ? t.common.appName : undefined}
        aria-hidden={markOnly ? undefined : true}
      >
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
      {markOnly ? null : <span className="text-base font-semibold tracking-tight">{t.common.appName}</span>}
    </div>
  )
}
