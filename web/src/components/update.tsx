import { CircleAlert, CircleArrowUp, RefreshCw, Wrench } from 'lucide-react'
import { type ReactNode } from 'react'

import { type Node } from '@/api/types'
import { CodeBlock } from '@/components/copy'
import { IceStack } from '@/components/layout/BrandMark'
import { OsIcon } from '@/components/os-icon'
import { StatusText } from '@/components/status'
import { Button, type ButtonProps } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { downloadCommands } from '@/lib/commands'
import { type SelfUpdateBlock, type UpdateMark, updateMark, type UpdateView } from '@/lib/nodeUpdate'
import { PLATFORMS, type PlatformId } from '@/lib/product'
import { useUpdateView } from '@/lib/useUpdateView'
import { platformLabel } from '@/lib/platform'
import { cn } from '@/lib/utils'

const u = t.updates

/** 状态的短语，下面一行是说明，下载时再加一条进度。 */
export function UpdateStateView({ view, compact = false }: { view: UpdateView; compact?: boolean }) {
  return (
    <div className="min-w-0">
      <StatusText tone={view.tone} className={cn(view.tone === 'idle' && 'font-normal')}>
        {view.label}
      </StatusText>
      {view.progress !== null ? (
        <div
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.floor(view.progress * 100)}
          aria-label={view.label}
          className="mt-1.5 h-1 w-40 max-w-full overflow-hidden rounded-full bg-border"
        >
          <div className="h-full rounded-full bg-relay-dot" style={{ width: `${view.progress * 100}%` }} />
        </div>
      ) : null}
      {view.detail !== null && !compact ? (
        <p className="mt-0.5 text-[13px] leading-snug text-muted-foreground">{view.detail}</p>
      ) : null}
      {view.reason !== null && !compact ? (
        <p className="mt-0.5 font-mono text-xs leading-snug break-words text-muted-foreground">{view.reason}</p>
      ) : null}
    </div>
  )
}

const MARKS: Record<UpdateMark, { Icon: typeof CircleArrowUp; className: string }> = {
  available: { Icon: CircleArrowUp, className: 'text-warn-dot' },
  updating: { Icon: RefreshCw, className: 'text-relay' },
  failed: { Icon: CircleAlert, className: 'text-danger' },
  manual: { Icon: Wrench, className: 'text-faint' },
  behind: { Icon: CircleArrowUp, className: 'text-faint' },
}

/** 节点列表"版本"一列：版本号和平台；有新版本、正在更新、更新失败、要手动更新时跟一个图标，悬停说明。 */
/**
 * 节点列表里"版本"那一格，两行：上面是 i3t 的标志和版本号（有新版本、要手动更新时后面一个标记），
 * 下面是系统的图标和平台。
 */
export function VersionCell({ node }: { node: Node }) {
  const view = useUpdateView(node)
  const mark = updateMark(view.kind)
  const style = mark === null ? null : MARKS[mark]
  const tip = view.detail === null ? view.label : t.common.sentences(view.label, view.detail)
  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2 text-[13px] tabular">
        <IceStack className="size-4 shrink-0" />
        {node.version === '' ? '—' : node.version}
        {style !== null ? (
          <Tooltip content={tip}>
            <span
              tabIndex={0}
              role="img"
              aria-label={tip}
              data-testid={`update-mark-${mark ?? ''}`}
              className={cn('inline-flex rounded-sm', style.className)}
            >
              <style.Icon className="size-3.5" aria-hidden />
            </span>
          </Tooltip>
        ) : null}
      </div>
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span className="flex size-4 shrink-0 items-center justify-center">
          <OsIcon platform={node.platform} />
        </span>
        {platformLabel(node.platform)}
      </div>
    </div>
  )
}

interface ReasonButtonProps extends ButtonProps {
  /** 不能点的原因。给了就置灰，悬停或聚焦时显示它。 */
  reason: string | null
}

/** 可能不能点的按钮：不能点时仍然能用键盘聚焦，好让人看到原因。 */
export function ReasonButton({ reason, children, ...props }: ReasonButtonProps) {
  if (reason === null) return <Button {...props}>{children}</Button>
  return (
    <Tooltip content={reason}>
      <span tabIndex={0} className="inline-flex rounded-md">
        <Button {...props} disabled onClick={undefined}>
          {children}
        </Button>
      </span>
    </Tooltip>
  )
}

const isPlatform = (id: string): id is PlatformId => PLATFORMS.some((platform) => platform.id === id)

/** 不能自己更新时：原因、做法，需要时给出在那台机器上手动替换程序的命令。 */
export function ManualUpdate({ block, platform }: { block: SelfUpdateBlock; platform: string }): ReactNode {
  return (
    <div className="space-y-2">
      <p className="text-sm font-medium">{block.reason}</p>
      <p className="text-[13px] text-muted-foreground">{block.howTo}</p>
      {block.commands ? (
        isPlatform(platform) ? (
          <>
            <p className="text-[13px] text-muted-foreground">{u.detail.manualDownload(platform)}</p>
            <CodeBlock copy={downloadCommands(platform).join('\n')}>{downloadCommands(platform).join('\n')}</CodeBlock>
          </>
        ) : (
          <p className="text-[13px] text-muted-foreground">{u.detail.manualNoPlatform}</p>
        )
      ) : null}
    </div>
  )
}
