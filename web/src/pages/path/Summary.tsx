import { AlertTriangle, CheckCircle2, ChevronRight, Info, XCircle } from 'lucide-react'

import { type TestView } from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Finding, type FindingTone, type Kpi, type Section } from '@/topology/diagnosis'

const d = t.diagnosis

const DOT_TONE = {
  ok: 'bg-ok-dot',
  warn: 'bg-warn',
  danger: 'bg-danger',
  relay: 'bg-relay',
  idle: 'bg-idle-dot',
} as const

/**
 * 顶上一排指标：直连、最低延迟、丢包、吞吐、CPU 峰值。大字是数，下面一行小字是哪条路、补充的数；
 * 点一格去对应的标签页。没测的一道横线。
 */
export function KpiStrip({ kpis, onOpen }: { kpis: readonly Kpi[]; onOpen: (section: Section) => void }) {
  return (
    <div className="grid grid-cols-2 gap-3 @2xl:grid-cols-3 @4xl:grid-cols-5" data-testid="kpis">
      {kpis.map((kpi) => (
        <button
          key={kpi.key}
          type="button"
          onClick={() => onOpen(kpi.section)}
          data-testid={`kpi-${kpi.key}`}
          className="group flex min-w-0 flex-col items-start rounded-xl border bg-card px-4 py-3 text-left transition-[box-shadow,border-color] hover:border-border-strong hover:shadow-card"
        >
          <span className="flex w-full items-center gap-1.5 text-xs text-muted-foreground">
            {kpi.tone !== null ? (
              <span aria-hidden className={cn('size-1.5 rounded-full', DOT_TONE[kpi.tone])} />
            ) : null}
            {d.kpi[kpi.key]}
            {kpi.key === 'cpu' ? <InfoTip>{d.cpuHint}</InfoTip> : null}
            <ChevronRight
              aria-hidden
              className="ml-auto size-3.5 opacity-0 transition-opacity group-hover:opacity-100"
            />
          </span>
          {kpi.values.length === 0 ? (
            <span className="mt-1.5 text-xl leading-7 font-semibold text-faint">—</span>
          ) : (
            kpi.values.map((value) => (
              <span
                key={value.text}
                className={cn(
                  'mt-1 max-w-full truncate leading-7 font-semibold tracking-tight tabular',
                  kpi.values.length > 1 ? 'text-base leading-6' : 'text-xl',
                  value.warn && (kpi.tone === 'danger' ? 'text-danger' : 'text-warn'),
                )}
              >
                {value.text}
              </span>
            ))
          )}
          <span className="mt-0.5 max-w-full truncate text-xs text-muted-foreground">{kpi.sub ?? ' '}</span>
        </button>
      ))}
    </div>
  )
}

const ICON = { danger: XCircle, warn: AlertTriangle, info: Info, ok: CheckCircle2 } as const
const ICON_TONE: Record<FindingTone, string> = {
  danger: 'text-danger',
  warn: 'text-warn',
  info: 'text-primary',
  ok: 'text-ok',
}

/** 结论：严重的在前，一条一行，右边"查看"去对应的标签页。 */
export function Findings({
  findings,
  test,
  onOpen,
}: {
  findings: readonly Finding[]
  test: TestView | undefined
  onOpen: (section: Section) => void
}) {
  const hint = test === undefined ? d.findingsIdle : test.state === 'running' ? d.findingsRunning : null
  return (
    <BoxCard data-testid="findings">
      <BoxCardHeader title={d.findingsTitle} />
      {hint !== null ? <p className="border-b px-5 py-2.5 text-[13px] text-muted-foreground">{hint}</p> : null}
      {findings.length === 0 ? null : (
        <ul className="divide-y">
          {findings.map((finding) => {
            const Icon = ICON[finding.tone]
            return (
              <li key={finding.key} className="flex items-start gap-3 px-5 py-3" data-tone={finding.tone}>
                <Icon
                  className={cn('mt-0.5 size-4 shrink-0', ICON_TONE[finding.tone])}
                  aria-label={d.tone[finding.tone]}
                />
                <p className="min-w-0 flex-1 text-[13px] leading-5">{finding.text}</p>
                {finding.section !== 'overview' ? (
                  <button
                    type="button"
                    onClick={() => onOpen(finding.section)}
                    className="inline-flex shrink-0 items-center gap-0.5 rounded-sm text-xs text-muted-foreground hover:text-foreground"
                  >
                    {d.open} {d.tabs[finding.section]}
                    <ChevronRight className="size-3.5" aria-hidden />
                  </button>
                ) : null}
              </li>
            )
          })}
        </ul>
      )}
    </BoxCard>
  )
}
