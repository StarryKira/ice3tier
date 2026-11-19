import { ArrowDown, ArrowUp } from 'lucide-react'

import { type HostReport, type TestView, type ThroughputResult, type Transfer } from '@/api/types'
import { StatusText } from '@/components/status'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { rateText } from '@/lib/tests'
import { cn } from '@/lib/utils'
import { type Measured, measuredPaths, pathLabel } from '@/topology/diagnosis'

import { ItemState, type Named } from './parts'

const d = t.diagnosis
const s = t.tests

const seconds = (ms: number) => (ms >= 1000 ? `${Math.round(ms / 100) / 10} s` : `${ms} ms`)

/** 一个方向：箭头、速率（大字）、一根按同一把尺子画的条，下面是搬了多少、用了多久、为什么停。 */
function Direction({ up, transfer, scale }: { up: boolean; transfer: Transfer | null; scale: number }) {
  const Icon = up ? ArrowUp : ArrowDown
  const rate = transfer?.bits_per_second ?? null
  return (
    <div className="space-y-2">
      <div className="flex items-baseline justify-between gap-3">
        <span className="inline-flex items-center gap-1.5 text-[13px] text-muted-foreground">
          <Icon className="size-3.5" aria-hidden />
          {up ? s.up : s.down}
        </span>
        <span className="text-xl font-semibold tracking-tight tabular">{rateText(rate)}</span>
      </div>
      <div className="h-2 rounded-full bg-muted">
        <div
          className={cn('h-full rounded-full', up ? 'bg-primary' : 'bg-primary/60')}
          style={{ width: rate === null || scale === 0 ? 0 : `${Math.max(2, (rate / scale) * 100)}%` }}
        />
      </div>
      {transfer !== null ? (
        <p className="text-xs text-muted-foreground tabular">
          {d.transferred(formatBytes(transfer.bytes), seconds(transfer.ms))}
          {' · '}
          {Object.hasOwn(d.stoppedBy, transfer.stopped_by)
            ? d.stoppedBy[transfer.stopped_by as keyof typeof d.stoppedBy]
            : transfer.stopped_by}
        </p>
      ) : null}
    </div>
  )
}

function PathThroughput({
  path,
  throughput,
  scale,
  nodes,
}: {
  path: Measured
  throughput: ThroughputResult
  scale: number
  nodes: Named
}) {
  const direct = path.direct || path.result.route?.via === 'direct'
  return (
    <BoxCard data-testid={`bandwidth-${path.direct ? 'direct' : pathLabel(path, nodes)}`}>
      <BoxCardHeader
        title={
          <StatusText tone={direct ? 'ok' : 'relay'} className="font-semibold">
            {pathLabel(path, nodes)}
          </StatusText>
        }
        action={
          throughput.congestion !== null ? (
            <span className="text-xs text-muted-foreground">
              {d.congestion} {throughput.congestion.toUpperCase()}
            </span>
          ) : null
        }
      />
      {throughput.state !== 'done' ? (
        <div className="px-5 py-4">
          <ItemState state={throughput.state} problem={throughput.problem} nodes={nodes} />
        </div>
      ) : (
        <div className="space-y-5 px-5 py-4">
          <Direction up transfer={throughput.up} scale={scale} />
          <Direction up={false} transfer={throughput.down} scale={scale} />
          <ul className="space-y-1 border-t pt-3 text-xs text-muted-foreground">
            <li>{d.limit(formatBytes(throughput.limit_bytes), Math.round(throughput.limit_ms / 1000))}</li>
            {throughput.pinned ? <li>{d.pinned}</li> : null}
            {throughput.shared_connection ? <li>{d.shared}</li> : null}
            <li>{s.throughputCaveat}</li>
          </ul>
        </div>
      )}
    </BoxCard>
  )
}

/** 量吞吐时两端的 CPU：整机和 i3t 进程，开始那一秒和最忙的一秒。 */
function CpuDuring({ hosts, nodes }: { hosts: readonly HostReport[]; nodes: Named }) {
  const rows = hosts.filter((host) => host.cpu_peak !== null)
  if (rows.length === 0) return null
  const name = (id: number) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)
  const pct = (value: number | null) => (value === null ? '—' : `${value}%`)
  return (
    <BoxCard data-testid="bandwidth-cpu">
      <BoxCardHeader title={d.cpuDuring} tip={d.cpuHint} />
      <ul className="divide-y">
        {rows.map((host) => {
          const process = host.cpu_peak?.process_percent ?? null
          return (
            <li key={host.node} className="grid grid-cols-[8rem_minmax(0,1fr)] items-center gap-4 px-5 py-3">
              <span className="truncate text-[13px] font-medium">{name(host.node)}</span>
              <div className="space-y-1.5">
                <div className="h-2 rounded-full bg-muted">
                  <div
                    className={cn('h-full rounded-full', process !== null && process >= 85 ? 'bg-warn' : 'bg-ok/70')}
                    style={{ width: `${Math.min(100, process ?? 0)}%` }}
                  />
                </div>
                <p className="text-xs text-muted-foreground tabular">
                  {d.host.cpuValue(pct(host.cpu_peak?.system_percent ?? null), pct(process))}
                </p>
              </div>
            </li>
          )
        })}
      </ul>
    </BoxCard>
  )
}

/** 带宽：每条量了吞吐的路一张卡片，上下行用同一把尺子；下面是两端 CPU 最忙的时候。 */
export function BandwidthTab({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  if (test === undefined) {
    return (
      <p className="rounded-xl border border-dashed px-6 py-10 text-center text-[13px] text-muted-foreground">
        {d.bandwidthIdle}
      </p>
    )
  }
  const paths = measuredPaths(test).flatMap((path) =>
    path.result.throughput === null ? [] : [{ path, throughput: path.result.throughput }],
  )
  const scale = Math.max(
    0,
    ...paths.flatMap(({ throughput }) => [throughput.up?.bits_per_second ?? 0, throughput.down?.bits_per_second ?? 0]),
  )
  return (
    <div className="space-y-5">
      {paths.length === 0 ? (
        <p className="rounded-xl border border-dashed px-6 py-10 text-center text-[13px] text-muted-foreground">
          {d.bandwidthNone}
        </p>
      ) : (
        <div className="grid grid-cols-1 items-start gap-5 @3xl:grid-cols-2">
          {paths.map(({ path, throughput }) => (
            <PathThroughput
              key={pathLabel(path, nodes)}
              path={path}
              throughput={throughput}
              scale={scale}
              nodes={nodes}
            />
          ))}
        </div>
      )}
      <CpuDuring hosts={test.hosts ?? []} nodes={nodes} />
    </div>
  )
}
