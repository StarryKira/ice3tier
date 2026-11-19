import { AlertTriangle, CheckCircle2, CircleDashed, Info, Loader2, Minus, XCircle } from 'lucide-react'
import { type ReactNode } from 'react'

import { InfoTip } from '@/components/info-tip'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Check, type CheckStatus, type ReportSection, type VerdictTone } from '@/topology/checks'

const d = t.diagnosis

/**
 * 报告的一节：一块有边框的面板，标题条里是两位的序号、名字、说明的小图标，右边放这一节自己的东西（图例）。
 * 紧凑：标题条 36 像素，内容四周 12 像素；`flush` 时内容贴边（表格自己留边）。
 */
export function ReportSectionBlock({
  id,
  index,
  title,
  hint,
  action,
  flush = false,
  className,
  children,
}: {
  id: string
  index: number
  title: string
  hint?: ReactNode
  action?: ReactNode
  flush?: boolean
  className?: string
  children: ReactNode
}) {
  return (
    <section
      id={id}
      className={cn('min-w-0 scroll-mt-4 overflow-hidden rounded-lg border bg-card', className)}
      aria-labelledby={`${id}-title`}
    >
      <header className="flex h-9 items-center justify-between gap-3 border-b bg-muted/30 px-3">
        <h2 id={`${id}-title`} className="flex min-w-0 items-center gap-2 text-[13px] font-semibold">
          <span className="text-[11px] font-medium text-muted-foreground tabular">
            {String(index).padStart(2, '0')}
          </span>
          <span className="truncate">{title}</span>
          {hint !== undefined ? <InfoTip>{hint}</InfoTip> : null}
        </h2>
        {action !== undefined ? <div className="flex shrink-0 items-center gap-3">{action}</div> : null}
      </header>
      <div className={flush ? undefined : 'px-3 py-2.5'}>{children}</div>
    </section>
  )
}

const STATUS_ICON: Record<CheckStatus, typeof CheckCircle2> = {
  pass: CheckCircle2,
  warn: AlertTriangle,
  fail: XCircle,
  info: Info,
  skip: Minus,
  pending: Loader2,
}

const STATUS_TEXT: Record<CheckStatus, string> = {
  pass: 'text-ok',
  warn: 'text-warn',
  fail: 'text-danger',
  info: 'text-primary',
  skip: 'text-muted-foreground',
  pending: 'text-relay',
}

const VERDICT: Record<VerdictTone, { icon: typeof CheckCircle2; text: string; band: string }> = {
  ok: { icon: CheckCircle2, text: 'text-ok', band: 'bg-ok' },
  warn: { icon: AlertTriangle, text: 'text-warn', band: 'bg-warn' },
  fail: { icon: XCircle, text: 'text-danger', band: 'bg-danger' },
  pending: { icon: Loader2, text: 'text-relay', band: 'bg-relay' },
  none: { icon: CircleDashed, text: 'text-muted-foreground', band: 'bg-border-strong' },
}

/** 一项的结果：形状不同的图标（读屏和悬停有字），颜色跟着结果走。 */
function StatusIcon({ status }: { status: CheckStatus }) {
  const Icon = STATUS_ICON[status]
  return (
    <Tooltip content={d.status[status]}>
      <span className={cn('mt-0.5 inline-flex shrink-0 rounded-sm', STATUS_TEXT[status])}>
        <Icon
          className={cn('size-3.5', status === 'pending' && 'motion-safe:animate-spin')}
          aria-label={d.status[status]}
        />
      </span>
    </Tooltip>
  )
}

/** 一项检查：图标、名字、测得的数（参考的范围小字跟在后面），有说明时下面一行小字。可以点的跳到讲它的那一节。 */
function CheckCell({ check, onOpen }: { check: Check; onOpen: (section: ReportSection) => void }) {
  const section = check.section
  const body = (
    <>
      <StatusIcon status={check.status} />
      <span className="w-24 shrink-0 font-medium">{d.check[check.key]}</span>
      <span className="min-w-0 flex-1">
        <span className={cn('tabular', check.status === 'skip' && 'text-muted-foreground')}>{check.value}</span>
        {check.reference !== null ? (
          <span className="ml-2 text-[11px] text-muted-foreground tabular">
            {d.colReference} {check.reference}
          </span>
        ) : null}
        {check.note !== null ? (
          <span className={cn('block text-xs', STATUS_TEXT[check.status === 'pass' ? 'skip' : check.status])}>
            {check.note}
          </span>
        ) : null}
      </span>
    </>
  )
  const className = 'flex w-full items-start gap-2 border-b border-border/70 px-3 py-1.5 text-left text-[13px]'
  return section === null ? (
    <div className={className} data-testid={`check-${check.key}`} data-status={check.status}>
      {body}
    </div>
  ) : (
    <button
      type="button"
      onClick={() => onOpen(section)}
      className={cn(className, 'hover:bg-muted/40')}
      data-testid={`check-${check.key}`}
      data-status={check.status}
    >
      {body}
    </button>
  )
}

/**
 * 诊断结果：一行总评（左边一道颜色、图标、一句话、测了几项几项通过），下面逐项检查排成两列（窄的时候一列）：
 * 图标、名字、测得（参考）、说明。没测的项按原因并成一格放在最后，不一项一项重复同一句话。
 */
export function ResultBlock({
  checks,
  verdict,
  onOpen,
}: {
  checks: readonly Check[]
  verdict: { tone: VerdictTone; title: string }
  onOpen: (section: ReportSection) => void
}) {
  const look = VERDICT[verdict.tone]
  const Icon = look.icon
  const measured = checks.filter((check) => check.status !== 'skip' && check.status !== 'pending')
  const passed = measured.filter((check) => check.status === 'pass').length
  const shown = checks.filter((check) => check.status !== 'skip')
  const skipped = new Map<string, Check[]>()
  for (const check of checks) {
    if (check.status === 'skip') skipped.set(check.value, [...(skipped.get(check.value) ?? []), check])
  }
  return (
    <div data-testid="report-result">
      <div
        className="relative flex flex-wrap items-baseline gap-x-2.5 border-b py-2 pr-3 pl-4"
        data-testid="verdict"
        data-tone={verdict.tone}
      >
        <span aria-hidden className={cn('absolute inset-y-0 left-0 w-1', look.band)} />
        <Icon
          className={cn(
            'size-4 shrink-0 self-center',
            look.text,
            verdict.tone === 'pending' && 'motion-safe:animate-spin',
          )}
          aria-hidden
        />
        <p className="text-[14px] font-semibold">{verdict.title}</p>
        <p className="text-xs text-muted-foreground">
          {verdict.tone === 'none' ? d.verdict.noneHint : d.passed(passed, measured.length)}
        </p>
      </div>
      <div className="-mb-px grid grid-cols-1 @4xl:grid-cols-2 @4xl:[&>*:nth-child(odd)]:border-r">
        {shown.map((check) => (
          <CheckCell key={check.key} check={check} onOpen={onOpen} />
        ))}
        {[...skipped].map(([reason, group]) => (
          <div
            key={`skip:${reason}`}
            className="flex items-start gap-2 border-b border-border/70 px-3 py-1.5 text-[13px] text-muted-foreground"
            data-testid={`check-skip-${group.map((check) => check.key).join('-')}`}
            data-status="skip"
          >
            <StatusIcon status="skip" />
            <span className="min-w-0 flex-1">
              <span className="font-medium text-foreground">
                {group.map((check) => d.check[check.key]).join(t.common.listSeparator)}
              </span>
              <span className="block text-xs">{reason}</span>
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}
