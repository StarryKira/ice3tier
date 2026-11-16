import { TriangleAlert } from 'lucide-react'
import { Link } from 'react-router'

import { type Status } from '@/api/types'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { WEB_VERSION } from '@/lib/version'

const v = t.version

/**
 * 侧栏底部的版本：controller 的和界面自己的。两者不是同一次构建时直接写出来；
 * 已知有比 controller 新的正式版时，跟一个不显眼的提示，点了去「节点更新」。
 * `status` 还没取到时只写界面的版本。
 */
export function VersionFooter({ status }: { status: Status | undefined }) {
  const mismatch = status !== undefined && status.version !== WEB_VERSION
  return (
    <div className="mt-2 space-y-0.5 px-2.5 text-xs text-muted-foreground tabular" data-testid="version-footer">
      {/* 平时只占一行：侧栏在矮一些的窗口里本来就挤。 */}
      <p>
        {status !== undefined ? <>{v.footerController(status.version)} · </> : null}
        <span data-testid="web-version">{v.footerWeb(WEB_VERSION)}</span>
      </p>
      {mismatch ? (
        <p>
          <Tooltip content={v.mismatch(WEB_VERSION, status.version)} side="right">
            <span tabIndex={0} className="inline-flex items-center gap-1 rounded-sm text-warn">
              <TriangleAlert className="size-3" aria-hidden />
              {v.mismatchShort}
            </span>
          </Tooltip>
        </p>
      ) : null}
      {status?.newer_version != null ? (
        <Tooltip content={v.newerHint(status.newer_version)} side="right">
          <Link
            to="/settings/updates"
            className="inline-flex items-center gap-1.5 rounded-sm text-foreground/80 hover:text-foreground hover:underline"
            data-testid="newer-version"
          >
            <span aria-hidden className="size-1.5 rounded-full bg-warn-dot" />
            {v.newer(status.newer_version)}
          </Link>
        </Tooltip>
      ) : null}
    </div>
  )
}
