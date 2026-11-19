import { type TestView } from '@/api/types'
import { StatusText } from '@/components/status'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { msText, permilleText } from '@/lib/tests'
import { cn } from '@/lib/utils'
import { type Measured, measuredPaths, pathLabel } from '@/topology/diagnosis'

import { type Named, QualityCard } from './parts'

const d = t.diagnosis

/**
 * 延迟分布：每条路一行，一根横条从最小到 P95，竖线是平均。所有的路用同一把尺子（最大的 P95 撑满），
 * 一眼看出哪条快、哪条抖。右边是丢包。
 */
function Spread({ paths, nodes }: { paths: readonly Measured[]; nodes: Named }) {
  const rows = paths.flatMap((path) => {
    const latency = path.result.latency
    if (path.result.state !== 'done' || latency === null) return []
    const { min_us: min, avg_us: avg, p95_us: p95 } = latency
    if (min === null || avg === null || p95 === null) return []
    return [{ path, min, avg, p95, loss: latency.loss_permille }]
  })
  if (rows.length === 0) return null
  const scale = Math.max(...rows.map((row) => row.p95)) * 1.08
  const at = (us: number) => `${Math.min(100, (us / scale) * 100)}%`
  return (
    <BoxCard data-testid="latency-spread">
      <BoxCardHeader title={d.rangeTitle} tip={d.rangeHint} />
      <ul className="space-y-4 px-5 py-4">
        {rows.map(({ path, min, avg, p95, loss }) => {
          const direct = path.direct || path.result.route?.via === 'direct'
          return (
            <li key={pathLabel(path, nodes)} className="grid grid-cols-[8rem_minmax(0,1fr)_4.5rem] items-center gap-4">
              <StatusText tone={direct ? 'ok' : 'relay'} className="truncate text-[13px] font-medium">
                {pathLabel(path, nodes)}
              </StatusText>
              <div>
                <div className="relative h-2.5 rounded-full bg-muted">
                  <span
                    className={cn('absolute inset-y-0 rounded-full', direct ? 'bg-ok/70' : 'bg-relay/60')}
                    style={{ left: at(min), width: `max(4px, calc(${at(p95)} - ${at(min)}))` }}
                  />
                  <span
                    aria-hidden
                    className="absolute -inset-y-1 w-0.5 -translate-x-1/2 rounded-full bg-foreground"
                    style={{ left: at(avg) }}
                  />
                </div>
                <p className="mt-1.5 text-xs text-muted-foreground tabular">
                  {d.rangeLegend(msText(min), msText(avg), msText(p95))}
                </p>
              </div>
              <span
                className={cn(
                  'text-right text-[13px] tabular',
                  loss !== null && loss >= 50
                    ? 'font-medium text-danger'
                    : loss !== null && loss >= 10
                      ? 'text-warn'
                      : 'text-muted-foreground',
                )}
              >
                {t.tests.loss} {permilleText(loss)}
              </span>
            </li>
          )
        })}
      </ul>
    </BoxCard>
  )
}

/** 延迟与丢包：各条路并排比（延迟、抖动、丢包、代价、各段），下面是分布。 */
export function LatencyTab({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  const paths = measuredPaths(test)
  if (paths.length === 0) {
    return (
      <p className="rounded-xl border border-dashed px-6 py-10 text-center text-[13px] text-muted-foreground">
        {d.latencyNone}
      </p>
    )
  }
  return (
    <div className="space-y-5">
      <QualityCard paths={paths} nodes={nodes} />
      <Spread paths={paths} nodes={nodes} />
    </div>
  )
}
