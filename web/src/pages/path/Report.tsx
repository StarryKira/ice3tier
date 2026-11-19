import { AlertTriangle, CheckCircle2, ChevronRight, CircleDashed, Info, Loader2, Minus, XCircle } from 'lucide-react'
import { type ReactNode } from 'react'

import { InfoTip } from '@/components/info-tip'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Check, type CheckStatus, type ReportSection, type VerdictTone } from '@/topology/checks'

const d = t.diagnosis

/** 报告的一节：标题前面一个两位的序号，下面一块有边框的内容；`bare` 时内容自己画边框（表格贴边）。 */
export function ReportSectionBlock({
  id,
  index,
  title,
  hint,
  action,
  bare = false,
  children,
}: {
  id: string
  index: number
  title: string
  hint?: ReactNode
  action?: ReactNode
  bare?: boolean
  children: ReactNode
}) {
  return (
    <section id={id} className="scroll-mt-4" aria-labelledby={`${id}-title`}>
      <header className="mb-2 flex items-center justify-between gap-3">
        <h2 id={`${id}-title`} className="flex items-baseline gap-2 text-[15px] font-semibold">
          <span className="text-xs font-medium text-muted-foreground tabular">{String(index).padStart(2, '0')}</span>
          {title}
          {hint !== undefined ? <InfoTip className="self-center">{hint}</InfoTip> : null}
        </h2>
        {action !== undefined ? <div className="flex items-center gap-3">{action}</div> : null}
      </header>
      {bare ? children : <div className="rounded-lg border bg-card px-4 py-3">{children}</div>}
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

/** 一项的结果：图标和字，颜色跟着结果走（图标和字一起，不只靠颜色）。 */
function Status({ status }: { status: CheckStatus }) {
  const Icon = STATUS_ICON[status]
  return (
    <span className={cn('inline-flex items-center gap-1.5 font-medium whitespace-nowrap', STATUS_TEXT[status])}>
      <Icon className={cn('size-3.5', status === 'pending' && 'motion-safe:animate-spin')} aria-hidden />
      {d.status[status]}
    </span>
  )
}

/**
 * 诊断结果：顶上一条总评（左边一道颜色、图标、一句话，下面是这一次测了几项、几项通过），
 * 下面是逐项检查的表：检查项、结果、测得、参考、说明。有讲它的那一节时整行可以点，跳过去。
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
  // 没测的项按原因并成一行（"直连、延迟、丢包…：还没有诊断"），放在最后，不一行一行重复同一句话。
  const shown = checks.filter((check) => check.status !== 'skip')
  const skipped = new Map<string, Check[]>()
  for (const check of checks) {
    if (check.status === 'skip') skipped.set(check.value, [...(skipped.get(check.value) ?? []), check])
  }
  return (
    <div className="overflow-hidden rounded-lg border bg-card" data-testid="report-result">
      <div
        className="relative flex items-center gap-3 border-b py-3 pr-4 pl-5"
        data-testid="verdict"
        data-tone={verdict.tone}
      >
        <span aria-hidden className={cn('absolute inset-y-0 left-0 w-1', look.band)} />
        <Icon
          className={cn('size-5 shrink-0', look.text, verdict.tone === 'pending' && 'motion-safe:animate-spin')}
          aria-hidden
        />
        <div className="min-w-0">
          <p className="text-[15px] font-semibold">{verdict.title}</p>
          <p className="text-xs text-muted-foreground">
            {verdict.tone === 'none' ? d.verdict.noneHint : d.passed(passed, measured.length)}
          </p>
        </div>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full min-w-[46rem] text-[13px]" aria-label={d.sections.result}>
          <thead>
            <tr className="border-b text-left text-xs text-muted-foreground">
              <th className="w-36 py-2 pl-5 font-normal">{d.colCheck}</th>
              <th className="w-24 py-2 font-normal">{d.colStatus}</th>
              <th className="py-2 font-normal">{d.colMeasured}</th>
              <th className="w-28 py-2 font-normal">{d.colReference}</th>
              <th className="w-[30%] py-2 pr-4 font-normal">{d.colNote}</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border/70">
            {shown.map((check) => {
              const section = check.section
              return (
                <tr
                  key={check.key}
                  data-testid={`check-${check.key}`}
                  data-status={check.status}
                  className={cn(
                    'align-top',
                    section !== null && 'cursor-pointer hover:bg-muted/40',
                    check.status === 'skip' && 'text-muted-foreground',
                  )}
                  onClick={section === null ? undefined : () => onOpen(section)}
                >
                  <th scope="row" className="py-2 pl-5 text-left font-medium">
                    {d.check[check.key]}
                  </th>
                  <td className="py-2">
                    <Status status={check.status} />
                  </td>
                  <td className="py-2 pr-4 tabular">{check.value}</td>
                  <td className="py-2 pr-4 text-xs text-muted-foreground tabular">{check.reference ?? '—'}</td>
                  <td className="py-2 pr-4 text-xs text-muted-foreground">
                    <span className="flex items-start justify-between gap-2">
                      <span>{check.note ?? ''}</span>
                      {section !== null ? (
                        <ChevronRight className="mt-0.5 size-3.5 shrink-0 text-faint" aria-hidden />
                      ) : null}
                    </span>
                  </td>
                </tr>
              )
            })}
            {[...skipped].map(([reason, group]) => (
              <tr
                key={`skip:${reason}`}
                className="align-top text-muted-foreground"
                data-testid={`check-skip-${group.map((check) => check.key).join('-')}`}
                data-status="skip"
              >
                <th scope="row" className="py-2 pl-5 text-left font-medium">
                  {group.map((check) => d.check[check.key]).join(t.common.listSeparator)}
                </th>
                <td className="py-2">
                  <Status status="skip" />
                </td>
                <td colSpan={3} className="py-2 pr-4">
                  {reason}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}
