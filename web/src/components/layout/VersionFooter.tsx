import { TriangleAlert } from 'lucide-react'
import { Link } from 'react-router'

import { type Status } from '@/api/types'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { WEB_VERSION } from '@/lib/version'

const v = t.version

const BADGE =
  'relative inline-flex h-5 items-center gap-1 rounded-md px-1.5 text-[11px] leading-none tabular transition-colors'

/**
 * 顶栏上标志后面的版本号（界面自己的版本）。悬停写出 controller 和界面各是哪个版本。
 * 两边不是同一次构建时变成琥珀色、带一个提醒的图标；已知有比 controller 新的正式版时，右上角一个小点，
 * 点了去「节点更新」。`status` 还没取到时只写界面的版本。
 */
export function VersionBadge({ status }: { status: Status | undefined }) {
  const mismatch = status !== undefined && status.version !== WEB_VERSION
  const newer = status?.newer_version ?? null
  const both =
    status === undefined
      ? v.footerWeb(WEB_VERSION)
      : `${v.footerController(status.version)} · ${v.footerWeb(WEB_VERSION)}`
  const tip = mismatch ? v.mismatch(WEB_VERSION, status.version) : newer !== null ? v.newerHint(newer) : both
  const tone = mismatch ? 'bg-warn-soft text-warn' : 'bg-foreground/[0.05] text-muted-foreground hover:text-foreground'

  const content = (
    <>
      {mismatch ? <TriangleAlert className="size-3" aria-hidden /> : null}
      <span data-testid="web-version">v{WEB_VERSION}</span>
      {newer !== null ? (
        <span aria-hidden className="absolute -top-0.5 -right-0.5 size-1.5 rounded-full bg-warn-dot" />
      ) : null}
    </>
  )

  return (
    <Tooltip content={tip} side="bottom">
      {newer !== null ? (
        <Link
          to="/settings/updates"
          aria-label={`${mismatch ? tip : both}. ${v.newer(newer)}`}
          data-testid="version-badge"
          data-mismatch={mismatch || undefined}
          className={cn(BADGE, tone)}
        >
          {content}
        </Link>
      ) : (
        <span
          tabIndex={0}
          aria-label={mismatch ? tip : both}
          data-testid="version-badge"
          data-mismatch={mismatch || undefined}
          className={cn(BADGE, tone)}
        >
          {content}
        </span>
      )}
    </Tooltip>
  )
}
