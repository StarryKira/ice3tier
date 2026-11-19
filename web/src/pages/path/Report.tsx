import { type CSSProperties, type ReactNode } from 'react'

import { StatusDot, type Tone } from '@/components/status'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Check, type CheckGroup, type CheckStatus, type ReportSection, type VerdictTone } from '@/topology/checks'

const d = t.diagnosis

/**
 * 报告的一节：一块有边框的面板，标题一行（名字，右边放这一节的图例），下面是内容，四周 16 像素。
 * 面板撑满它那一格的高度，内容在里面往下排；和旁边的面板一样高。
 */
export function ReportSectionBlock({
  id,
  title,
  action,
  className,
  children,
}: {
  id: string
  title: string
  action?: ReactNode
  className?: string
  children: ReactNode
}) {
  return (
    <section
      id={id}
      className={cn('flex min-w-0 scroll-mt-4 flex-col rounded-lg border bg-card', className)}
      aria-labelledby={`${id}-title`}
    >
      <header className="flex min-h-10 flex-wrap items-center justify-between gap-x-4 gap-y-1 px-4 pt-2.5 pb-1">
        <h2 id={`${id}-title`} className="text-[13px] font-semibold">
          {title}
        </h2>
        {action !== undefined ? <div className="flex items-center gap-3">{action}</div> : null}
      </header>
      <div className="min-w-0 flex-1 px-4 pb-3">{children}</div>
    </section>
  )
}

/** 结果和全站的状态圆点一样：通过绿、注意琥珀、异常红，提示和进行中是中继的紫，没测是空心的灰圈。 */
const TONE: Record<CheckStatus, Tone> = {
  pass: 'ok',
  warn: 'warn',
  fail: 'danger',
  info: 'relay',
  pending: 'relay',
  skip: 'idle',
}

const VERDICT: Record<VerdictTone, { tone: Tone; text: string }> = {
  ok: { tone: 'ok', text: 'text-foreground' },
  warn: { tone: 'warn', text: 'text-foreground' },
  fail: { tone: 'danger', text: 'text-danger' },
  pending: { tone: 'relay', text: 'text-foreground' },
  none: { tone: 'idle', text: 'text-muted-foreground' },
}

const NOTE: Record<CheckStatus, string> = {
  pass: 'text-muted-foreground',
  warn: 'text-warn',
  fail: 'text-danger',
  info: 'text-muted-foreground',
  skip: 'text-muted-foreground',
  pending: 'text-muted-foreground',
}

/** 一项的结果：圆点（读屏和悬停有字）。 */
function Mark({ status }: { status: CheckStatus }) {
  return (
    <span title={d.status[status]} className="inline-flex h-5 items-center">
      <StatusDot
        tone={TONE[status]}
        label={d.status[status]}
        className={status === 'pending' ? 'motion-safe:animate-pulse' : undefined}
      />
    </span>
  )
}

const ROW = 'grid grid-cols-[minmax(0,6.5rem)_minmax(0,1fr)] items-start gap-x-3 rounded-sm px-1.5 py-1 text-[13px]'

/** 一项检查：圆点和名字，测得的数（参考的范围小字跟在后面），有说明时下面一行小字。可以点的跳到讲它的那一节。 */
function CheckRow({ check, onOpen }: { check: Check; onOpen: (section: ReportSection) => void }) {
  const section = check.section
  const body = (
    <>
      <span className="flex items-start gap-2 font-medium">
        <Mark status={check.status} />
        <span className="leading-5">{d.check[check.key]}</span>
      </span>
      <span className="min-w-0 leading-5">
        <span className="tabular">{check.value}</span>
        {check.reference !== null ? (
          <span className="ml-2 text-[11px] text-muted-foreground tabular">
            {d.colReference} {check.reference}
          </span>
        ) : null}
        {check.note !== null ? <span className={cn('block text-xs', NOTE[check.status])}>{check.note}</span> : null}
      </span>
    </>
  )
  return section === null ? (
    <div className={ROW} data-testid={`check-${check.key}`} data-status={check.status}>
      {body}
    </div>
  ) : (
    <button
      type="button"
      onClick={() => onOpen(section)}
      className={cn(ROW, 'w-full text-left hover:bg-muted/50')}
      data-testid={`check-${check.key}`}
      data-status={check.status}
    >
      {body}
    </button>
  )
}

const SUMMARY_ORDER: readonly CheckStatus[] = ['fail', 'warn', 'info', 'pass', 'pending', 'skip']

const BAR: Record<CheckStatus, string> = {
  pass: 'bg-ok-dot',
  warn: 'bg-warn-dot',
  fail: 'bg-danger-dot',
  info: 'bg-relay-dot',
  pending: 'bg-relay-dot',
  skip: 'bg-border-strong',
}

/** 一根细的分段条：各种结果各占多少，后面一行数（"9 通过 · 4 注意"），没有的不写。 */
function SummaryBar({ checks }: { checks: readonly Check[] }) {
  const counts = SUMMARY_ORDER.map((status) => ({
    status,
    count: checks.filter((check) => check.status === status).length,
  })).filter(({ count }) => count > 0)
  return (
    <div className="flex min-w-0 items-center gap-3" data-testid="verdict-summary">
      <div className="flex h-1.5 w-32 gap-0.5" aria-hidden>
        {counts.map(({ status, count }) => (
          <span
            key={status}
            className={cn('h-full first:rounded-l-full last:rounded-r-full', BAR[status])}
            style={{ flexGrow: count }}
          />
        ))}
      </div>
      <p className="text-xs whitespace-nowrap text-muted-foreground tabular">
        {counts.map(({ status, count }) => `${count} ${d.summary[status]}`).join(' · ')}
      </p>
    </div>
  )
}

/**
 * 一组检查：小标题（组名，右边通过了几项），下面一项一行；没测的按原因并成一行放在组的最后。
 * 宽的时候三组并排，各组的第几行放在同一个网格行里，一样高、对齐；窄的时候一组接一组。
 */
function Group({
  group,
  column,
  checks,
  onOpen,
}: {
  group: CheckGroup
  column: number
  checks: readonly Check[]
  onOpen: (section: ReportSection) => void
}) {
  const measured = checks.filter((check) => check.status !== 'skip' && check.status !== 'pending')
  const passed = measured.filter((check) => check.status === 'pass').length
  const skipped = new Map<string, Check[]>()
  for (const check of checks) {
    if (check.status === 'skip') skipped.set(check.value, [...(skipped.get(check.value) ?? []), check])
  }
  const items: ReactNode[] = [
    <p key="head" className="flex items-baseline justify-between px-1.5 pb-0.5 text-xs text-muted-foreground">
      {d.groups[group]}
      {measured.length > 0 ? <span className="tabular">{d.groupCount(passed, measured.length)}</span> : null}
    </p>,
    ...checks
      .filter((check) => check.status !== 'skip')
      .map((check) => <CheckRow key={check.key} check={check} onOpen={onOpen} />),
    ...[...skipped].map(([reason, entries]) => (
      <div
        key={`skip:${reason}`}
        className={cn(ROW, 'text-muted-foreground')}
        data-testid={`check-skip-${entries.map((check) => check.key).join('-')}`}
        data-status="skip"
      >
        <span className="flex items-start gap-2 font-medium">
          <Mark status="skip" />
          <span className="leading-5">{entries.map((check) => d.check[check.key]).join(t.common.listSeparator)}</span>
        </span>
        <span className="text-xs leading-5">{reason}</span>
      </div>
    )),
  ]
  return (
    <div className="contents" data-testid={`check-group-${group}`}>
      {items.map((item, row) => (
        <div
          key={row}
          className={cn(
            'min-w-0 @5xl:[grid-column:var(--column)] @5xl:[grid-row:var(--row)]',
            row === 0 && column > 1 && 'mt-3 @5xl:mt-0',
          )}
          style={{ '--column': column, '--row': row + 1 } as CSSProperties}
        >
          {item}
        </div>
      ))}
    </div>
  )
}

const GROUPS: readonly CheckGroup[] = ['link', 'quality', 'host']

const RANK: Record<CheckStatus, number> = { fail: 0, warn: 1, info: 2, pass: 3, pending: 4, skip: 5 }

/**
 * 诊断结果：一句总评（圆点和一句话，后面是测了几项几项通过；右边一根分段条看各种结果各占多少），
 * 下面是建议（要注意、异常的项该怎么办，严重的在前），再下面检查按三组排开：连通与路由、链路质量、两端主机。
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
  const measured = checks.filter((check) => check.status !== 'skip' && check.status !== 'pending')
  const passed = measured.filter((check) => check.status === 'pass').length
  const advice = checks
    .filter((check) => check.advice !== null && check.status !== 'pass')
    .sort((x, y) => RANK[x.status] - RANK[y.status])
  return (
    <div className="space-y-3" data-testid="report-result">
      <div
        className="flex flex-wrap items-center justify-between gap-x-6 gap-y-1.5"
        data-testid="verdict"
        data-tone={verdict.tone}
      >
        <p className="flex items-baseline gap-2.5">
          <span className="inline-flex self-center">
            <StatusDot
              tone={look.tone}
              className={verdict.tone === 'pending' ? 'motion-safe:animate-pulse' : undefined}
            />
          </span>
          <span className={cn('text-[15px] font-semibold', look.text)}>{verdict.title}</span>
          <span className="text-xs text-muted-foreground">
            {verdict.tone === 'none' ? d.verdict.noneHint : d.passed(passed, measured.length)}
          </span>
        </p>
        {measured.length > 0 ? <SummaryBar checks={checks} /> : null}
      </div>
      {advice.length > 0 ? (
        <div className="rounded-md bg-muted/40 px-1.5 py-1.5" data-testid="advice">
          <p className="px-1.5 pb-0.5 text-xs text-muted-foreground">{d.adviceTitle}</p>
          <ul>
            {advice.map((check) => (
              <li key={check.key} className={ROW}>
                <span className="flex items-start gap-2 font-medium">
                  <Mark status={check.status} />
                  <span className="leading-5">{d.check[check.key]}</span>
                </span>
                <span className="leading-5">{check.advice}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <div className="grid grid-cols-1 gap-x-8 @5xl:grid-cols-3">
        {GROUPS.map((group, index) => (
          <Group
            key={group}
            group={group}
            column={index + 1}
            checks={checks.filter((check) => check.group === group)}
            onOpen={onOpen}
          />
        ))}
      </div>
    </div>
  )
}
