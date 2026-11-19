import { type ReactNode } from 'react'

import { type HostReport, type NodeId, type PathLeg, type PathView, type TestView } from '@/api/types'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { cn } from '@/lib/utils'
import { measuredPaths, type Named, pathLabel, routeName } from '@/topology/diagnosis'
import { describeLeg } from '@/topology/path'

import { niceRange, niceScale } from '@/lib/scale'

import { Bar, Dot, Empty, Plot, type PlotRow } from './viz'

const d = t.diagnosis
const c = t.diagnosis.chart
const s = t.tests

const KIND = {
  direct: { dot: 'bg-ok-dot', soft: 'bg-ok-dot/35', line: 'bg-ok-dot' },
  relay: { dot: 'bg-relay-dot', soft: 'bg-relay-dot/35', line: 'bg-relay-dot' },
  none: { dot: 'bg-idle-dot', soft: 'bg-idle-dot/35', line: 'bg-border-strong' },
} as const

function PathName({ kind, children }: { kind: keyof typeof KIND; children: ReactNode }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <span aria-hidden className={cn('size-1.5 shrink-0 rounded-full', KIND[kind].dot)} />
      <span className="truncate">{children}</span>
    </span>
  )
}

const nameOf = (nodes: Named, id: NodeId) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)

/* 两个方向的路径：像 mtr 一样一段一行，后面一根瀑布条看延迟在哪一段累起来。 */

interface Hop {
  from: string
  to: string
  rtt_us: number | null
  jitter_us: number | null
  loss_permille: number | null
}

/**
 * 一个方向现在走的路分成几段：发送方的候选路径里现在那条有各段的测量值就用它；没有时用诊断里测这条路时的各段；
 * 再没有就按经过的节点列出来，数是空的（直连时整段就是这个方向的延迟）。
 */
function hopsOf(leg: PathLeg, test: TestView | undefined, nodes: Named): Hop[] {
  const route = leg.route
  if (route === null || route.via === 'none') return []
  const current = leg.candidates?.find((candidate) => candidate.current)
  if (current !== undefined && current.legs.length > 0) {
    return current.legs.map((segment) => ({
      from: nameOf(nodes, segment.from),
      to: nameOf(nodes, segment.to),
      rtt_us: segment.rtt_us,
      jitter_us: segment.jitter_us,
      loss_permille: segment.loss_permille,
    }))
  }
  const forward = test?.a.id === leg.from.id
  const tested = measuredPaths(test).find(
    ({ result }) =>
      forward &&
      result.route !== null &&
      result.route.via === route.via &&
      (route.via !== 'relay' ||
        (result.route.via === 'relay' && result.route.relays.join('.') === route.relays.join('.'))),
  )
  if (tested !== undefined && tested.result.hops.length > 0) {
    return tested.result.hops.map((hop) => ({
      from: nameOf(nodes, hop.from),
      to: nameOf(nodes, hop.to),
      rtt_us: hop.rtt_us,
      jitter_us: hop.jitter_us,
      loss_permille: hop.loss_permille,
    }))
  }
  const names = [
    leg.from.name,
    ...(route.via === 'relay' ? route.relays.map((id) => nameOf(nodes, id)) : []),
    leg.to.name,
  ]
  return names.slice(1).map((to, index) => ({
    from: names[index] ?? '',
    to,
    rtt_us: route.via === 'direct' && leg.rtt_ms !== null ? leg.rtt_ms * 1000 : null,
    jitter_us: null,
    loss_permille: null,
  }))
}

/**
 * 一个方向：标题行是方向、路、直连的方式或到中继的传输、延迟和这份数据的来源，下面一张表一段一行（延迟、抖动、丢包），
 * 最后一列是瀑布条：这一段从前面几段累计的地方开始，长短是它的延迟。两个方向用同一把尺子。下面一行灰字是什么时候换的、为什么、选路模式。
 */
function HopTable({
  leg,
  hops,
  total,
  nodes,
  testId,
}: {
  leg: PathLeg
  hops: readonly Hop[]
  total: number
  nodes: Named
  testId: string
}) {
  const view = describeLeg(leg, (id) => nameOf(nodes, id))
  const kind = leg.route?.via === 'direct' ? 'direct' : leg.route?.via === 'relay' ? 'relay' : 'none'
  const sinceReason = view.reason !== null && view.sinceReason !== view.reason ? view.sinceReason : null
  const meta = [
    view.since === null
      ? null
      : `${t.path.sinceLabel} ${view.since}${sinceReason === null ? '' : d.report.paren(sinceReason)}`,
    view.reason ?? view.sinceReason,
    view.mode?.label ?? null,
  ].filter((part): part is string => part !== null)
  // 每一段从前面几段累计的地方开始。
  const starts = hops.map((_, index) => hops.slice(0, index).reduce((sum, hop) => sum + (hop.rtt_us ?? 0), 0))
  return (
    <div data-testid={testId} data-source={view.state}>
      <p className="flex flex-wrap items-baseline gap-x-2 text-[13px]">
        <span className="font-semibold">{t.path.forward(leg.from.name, leg.to.name)}</span>
        <PathName kind={kind}>{view.route?.text ?? t.routes.noData}</PathName>
        {view.route?.detail !== null && view.route?.detail !== undefined ? (
          <span className={cn('text-xs', leg.relay_leg === 'tcp' ? 'font-medium text-warn' : 'text-muted-foreground')}>
            {view.route.detail}
          </span>
        ) : null}
        {view.state !== 'query' ? (
          <Tooltip content={view.sourceHint}>
            <span tabIndex={0} className="rounded-sm text-xs text-warn">
              {view.sourceLabel}
              {view.age !== null ? ` · ${view.age}` : null}
            </span>
          </Tooltip>
        ) : (
          <span className="sr-only">{view.sourceLabel}</span>
        )}
        <span className="ml-auto font-semibold tabular">{view.rtt ?? '—'}</span>
      </p>
      {hops.length > 0 ? (
        <table className="mt-1.5 w-full table-fixed text-[13px]">
          <colgroup>
            <col className="w-8" />
            <col className="w-[34%]" />
            <col className="w-20" />
            <col className="w-20" />
            <col className="w-16" />
            <col />
          </colgroup>
          <thead>
            <tr className="border-b text-left text-xs text-muted-foreground">
              <th className="py-1.5 font-normal">#</th>
              <th className="py-1.5 font-normal">{c.hop}</th>
              <th className="py-1.5 text-right font-normal">{s.latency}</th>
              <th className="py-1.5 text-right font-normal">{s.jitter}</th>
              <th className="py-1.5 text-right font-normal">{s.loss}</th>
              <th className="py-1.5 pl-4 font-normal">
                <span className="sr-only">{c.waterfall}</span>
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border/60">
            {hops.map((hop, index) => {
              const from = starts[index] ?? 0
              const at = (us: number) => `${total === 0 ? 0 : (us / total) * 100}%`
              return (
                <tr key={`${hop.from}>${hop.to}`}>
                  <td className="py-1.5 text-muted-foreground tabular">{index + 1}</td>
                  <td className="truncate py-1.5">
                    {hop.from} → {hop.to}
                    {index === 0 && leg.relay_leg === 'tcp' ? (
                      <span className="ml-1.5 text-xs font-medium text-warn">TCP</span>
                    ) : null}
                  </td>
                  <td className="py-1.5 text-right font-medium tabular">{msText(hop.rtt_us)}</td>
                  <td className="py-1.5 text-right text-muted-foreground tabular">{msText(hop.jitter_us)}</td>
                  <td
                    className={cn(
                      'py-1.5 text-right tabular',
                      hop.loss_permille !== null && hop.loss_permille >= 10
                        ? 'font-medium text-warn'
                        : 'text-muted-foreground',
                    )}
                  >
                    {permilleText(hop.loss_permille)}
                  </td>
                  <td className="py-1.5 pl-4">
                    <span className="relative block h-2 rounded-full bg-muted">
                      {hop.rtt_us === null ? null : (
                        <span
                          className={cn('absolute inset-y-0 rounded-full', KIND[kind].line)}
                          style={{ left: at(from), width: `max(3px, ${at(hop.rtt_us)})` }}
                        />
                      )}
                    </span>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      ) : null}
      {meta.length > 0 ? <p className="mt-1.5 text-xs text-muted-foreground">{meta.join(' · ')}</p> : null}
    </div>
  )
}

export function PathChart({
  path,
  test,
  nodes,
}: {
  path: PathView | undefined
  test: TestView | undefined
  nodes: Named
}) {
  if (path === undefined) return <Empty>{t.path.querying}</Empty>
  const forward = hopsOf(path.forward, test, nodes)
  const reverse = hopsOf(path.reverse, test, nodes)
  const sum = (hops: readonly Hop[]) => hops.reduce((total, hop) => total + (hop.rtt_us ?? 0), 0)
  const total = Math.max(sum(forward), sum(reverse))
  return (
    <div className="grid grid-cols-1 gap-x-8 gap-y-5 @4xl:grid-cols-2">
      <HopTable leg={path.forward} hops={forward} total={total} nodes={nodes} testId="leg-forward" />
      <HopTable leg={path.reverse} hops={reverse} total={total} nodes={nodes} testId="leg-reverse" />
    </div>
  )
}

/* 各条路的延迟：最小到 P95 的条，平均是点。 */

export function LatencyChart({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  const paths = measuredPaths(test).filter(({ result }) => result.state === 'done' && result.latency !== null)
  if (paths.length === 0) return <Empty>{test === undefined ? d.latencyNone : c.notMeasured}</Empty>
  // 点和区间不必从 0 量：轴只盖住数据，几毫秒的差别也看得出。
  const top = Math.max(...paths.map(({ result }) => result.latency?.p95_us ?? 0)) / 1000
  const bottom = Math.min(...paths.map(({ result }) => result.latency?.min_us ?? top * 1000)) / 1000
  const pad = Math.max((top - bottom) * 0.1, top * 0.02, 0.5)
  const scale = niceRange(bottom - pad, top + pad)
  const rows: PlotRow[] = paths.map((path) => {
    const latency = path.result.latency
    const kind = path.direct || path.result.route?.via === 'direct' ? 'direct' : 'relay'
    const loss = latency?.loss_permille ?? null
    return {
      key: pathLabel(path, nodes),
      testId: `latency-${path.direct ? 'direct' : pathLabel(path, nodes)}`,
      label: <PathName kind={kind}>{pathLabel(path, nodes)}</PathName>,
      marks: (x) => {
        const min = latency?.min_us ?? null
        const avg = latency?.avg_us ?? null
        const p95 = latency?.p95_us ?? null
        return min === null || avg === null || p95 === null ? null : (
          <>
            <Bar from={x(min / 1000)} to={x(p95 / 1000)} className={KIND[kind].soft} />
            <Dot at={x(avg / 1000)} className={KIND[kind].dot} />
          </>
        )
      },
      value: (
        <>
          <span className="block font-medium">{msText(latency?.avg_us)}</span>
          <span
            className={cn(
              'block text-[11px]',
              loss !== null && loss >= 10 ? 'font-medium text-warn' : 'text-muted-foreground',
            )}
          >
            {c.loss(permilleText(loss))}
          </span>
        </>
      ),
      tip:
        latency === null ? undefined : (
          <span className="block space-y-0.5">
            <span className="block">
              {s.latency}：{s.latencyValue(msText(latency.min_us), msText(latency.avg_us), msText(latency.p95_us))}
            </span>
            <span className="block">
              {s.jitter}：{msText(latency.jitter_us)} · {s.loss}：{permilleText(latency.loss_permille)}
            </span>
            <span className="block">
              {s.cost}：{msText(path.result.cost_us)} · {s.answered(latency.answered, latency.probes)}
            </span>
          </span>
        ),
    }
  })
  return (
    <Plot
      label={c.latencyTitle}
      rows={rows}
      scale={scale}
      format={(tick, last) => (last ? t.common.ms(tick) : String(tick))}
      rowClassName="h-10"
    />
  )
}

/* 吞吐：一条路两根条，上行、下行。 */

export const THROUGHPUT_LEGEND = () => [
  { label: s.up, className: 'bg-viz-1' },
  { label: s.down, className: 'bg-viz-2' },
]

export function ThroughputChart({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  if (test === undefined) return <Empty>{d.bandwidthIdle}</Empty>
  const paths = measuredPaths(test).filter(({ result }) => result.throughput !== null)
  if (paths.length === 0) return <Empty>{d.bandwidthNone}</Empty>
  const rates = paths.flatMap(({ result }) => [
    result.throughput?.up?.bits_per_second ?? 0,
    result.throughput?.down?.bits_per_second ?? 0,
  ])
  const giga = Math.max(...rates) >= 1e9
  const unit = giga ? 1e9 : 1e6
  // 面板窄：刻度少一些，免得挤在一起。
  const scale = niceScale(Math.max(...rates) / unit, 2)
  const rows: PlotRow[] = paths.map((path) => {
    const throughput = path.result.throughput
    const kind = path.direct || path.result.route?.via === 'direct' ? 'direct' : 'relay'
    const up = throughput?.up?.bits_per_second ?? null
    const down = throughput?.down?.bits_per_second ?? null
    const done = throughput?.state === 'done'
    return {
      key: pathLabel(path, nodes),
      testId: `bandwidth-${path.direct ? 'direct' : pathLabel(path, nodes)}`,
      label: <PathName kind={kind}>{pathLabel(path, nodes)}</PathName>,
      marks: (x) =>
        done ? (
          <>
            {up !== null ? <Bar from="0%" to={x(up / unit)} className="bg-viz-1" top="33%" height="h-1.5" /> : null}
            {down !== null ? <Bar from="0%" to={x(down / unit)} className="bg-viz-2" top="70%" height="h-1.5" /> : null}
          </>
        ) : null,
      value: done ? (
        <>
          <span className="block text-xs font-medium">{rateText(up)}</span>
          <span className="block text-xs font-medium">{rateText(down)}</span>
        </>
      ) : (
        <span className="text-xs text-muted-foreground">{c.notMeasured}</span>
      ),
      tip:
        throughput === null ? undefined : done ? (
          <span className="block space-y-0.5">
            {[
              { label: c.up, transfer: throughput.up },
              { label: c.down, transfer: throughput.down },
            ].map(({ label, transfer }) =>
              transfer === null ? null : (
                <span key={label(rateText(transfer.bits_per_second))} className="block">
                  {label(rateText(transfer.bits_per_second))} · {formatBytes(transfer.bytes)} ·{' '}
                  {transfer.ms >= 1000 ? `${Math.round(transfer.ms / 100) / 10} s` : `${transfer.ms} ms`}
                </span>
              ),
            )}
            <span className="block">
              {d.congestion} {throughput.congestion?.toUpperCase() ?? '—'}
            </span>
          </span>
        ) : throughput.problem !== null ? (
          problemText(throughput.problem, nodes)
        ) : undefined,
    }
  })
  return (
    <Plot
      label={s.throughputTitle}
      rows={rows}
      scale={scale}
      format={(tick, last) => (last ? `${tick} ${giga ? 'Gbps' : 'Mbps'}` : String(tick))}
      rowClassName="h-11"
    />
  )
}

/* 候选路径的代价：一个方向一小张，现在走的那条是主色，别的是灰的。 */

export function CostChart({ path, nodes }: { path: PathView | undefined; nodes: Named }) {
  const legs = path === undefined ? [] : [path.forward, path.reverse].filter((leg) => (leg.candidates ?? []).length > 0)
  if (legs.length === 0) return <Empty>{path === undefined ? t.path.querying : t.path.candidatesEmpty}</Empty>
  const top = Math.max(...legs.flatMap((leg) => (leg.candidates ?? []).map((candidate) => candidate.cost_us)))
  const scale = niceScale(top / 1000)
  return (
    <div className="space-y-4">
      {legs.map((leg) => (
        <div key={leg.from.id} data-testid={`candidates-${leg === path?.forward ? 'forward' : 'reverse'}`}>
          <p className="mb-1 text-xs text-muted-foreground">{t.path.forward(leg.from.name, leg.to.name)}</p>
          <Plot
            label={`${t.path.candidatesTitle} ${t.path.forward(leg.from.name, leg.to.name)}`}
            scale={scale}
            format={(tick, last) => (last ? t.common.ms(tick) : String(tick))}
            rowClassName="h-7"
            rows={(leg.candidates ?? []).slice(0, 6).map((candidate) => {
              const kind = candidate.route.via === 'direct' ? 'direct' : 'relay'
              return {
                key: JSON.stringify(candidate.route),
                testId: candidate.current ? 'candidate-current' : undefined,
                label: (
                  <span className="inline-flex min-w-0 items-center gap-1.5">
                    <PathName kind={kind}>{routeName(candidate.route, nodes)}</PathName>
                    {candidate.current ? <span className="text-xs text-muted-foreground">{c.current}</span> : null}
                  </span>
                ),
                marks: (x) => (
                  <Bar
                    from="0%"
                    to={x(candidate.cost_us / 1000)}
                    className={candidate.current ? 'bg-primary' : 'bg-border-strong'}
                    height="h-1.5"
                  />
                ),
                value: (
                  <span className={cn('text-xs', candidate.current && 'font-medium')}>{msText(candidate.cost_us)}</span>
                ),
                tip: (
                  <span className="block space-y-0.5">
                    {candidate.legs.map((segment) => (
                      <span key={`${segment.from}>${segment.to}`} className="block">
                        {t.path.legLine(
                          nameOf(nodes, segment.from),
                          nameOf(nodes, segment.to),
                          msText(segment.rtt_us),
                          msText(segment.jitter_us),
                          permilleText(segment.loss_permille),
                        )}
                      </span>
                    ))}
                    {candidate.route.via === 'relay' ? (
                      <span className="block">
                        {t.path.colLoad} {candidate.load_percent}%
                      </span>
                    ) : null}
                  </span>
                ),
              }
            })}
          />
        </div>
      ))}
    </div>
  )
}

/* 两端资源：CPU（开始那一秒到最忙的一秒，哑铃），内存（i3t、其它、可用，一根分段的细条）。 */

export const CPU_LEGEND = () => [
  { label: c.cpuStart, className: 'bg-viz-1/40', shape: 'dot' as const },
  { label: c.cpuPeak, className: 'bg-viz-1', shape: 'dot' as const },
]

export function CpuChart({ hosts, nodes, empty }: { hosts: readonly HostReport[]; nodes: Named; empty: string }) {
  const rows: PlotRow[] = hosts.flatMap((host) => {
    if (host.state !== 'done' || host.cpu_peak === null) return []
    const name = nameOf(nodes, host.node)
    return (
      [
        {
          key: 'system',
          label: c.cpuSystem(name),
          start: host.cpu?.system_percent ?? null,
          peak: host.cpu_peak.system_percent,
        },
        {
          key: 'process',
          label: c.cpuProcess(name),
          start: host.cpu?.process_percent ?? null,
          peak: host.cpu_peak.process_percent,
        },
      ] as const
    ).flatMap((row) =>
      row.peak === null
        ? []
        : [
            {
              key: `${host.node}-${row.key}`,
              testId: `cpu-${host.node}-${row.key}`,
              label: row.label,
              marks: (x: (value: number) => string) => (
                <>
                  {row.start !== null ? (
                    <Bar
                      from={x(Math.min(row.start, row.peak ?? 0))}
                      to={x(Math.max(row.start, row.peak ?? 0))}
                      className="bg-viz-1/30"
                      height="h-0.5"
                    />
                  ) : null}
                  {row.start !== null ? <Dot at={x(row.start)} className="bg-viz-1/40" /> : null}
                  <Dot at={x(row.peak ?? 0)} className="bg-viz-1" />
                </>
              ),
              value: (
                <span className={cn('text-xs font-medium', row.key === 'process' && row.peak >= 85 && 'text-warn')}>
                  {row.peak}%
                </span>
              ),
              tip: `${c.cpuStart} ${row.start === null ? '—' : `${row.start}%`} · ${c.cpuPeak} ${row.peak}%`,
            },
          ],
    )
  })
  if (rows.length === 0) return <Empty>{empty}</Empty>
  const top = Math.max(
    100,
    ...hosts.flatMap((host) => [host.cpu_peak?.process_percent ?? 0, host.cpu_peak?.system_percent ?? 0]),
  )
  return (
    <Plot
      label={c.cpuTitle}
      rows={rows}
      scale={niceScale(top)}
      format={(tick, last) => (last ? `${tick}%` : String(tick))}
      rowClassName="h-7"
      labelWidth="7.5rem"
      valueWidth="3rem"
    />
  )
}

export const MEMORY_LEGEND = () => [
  { label: c.memoryI3t, className: 'bg-viz-1' },
  { label: c.memoryOther, className: 'bg-border-strong' },
  { label: c.memoryFree, className: 'bg-muted' },
]

export function MemoryChart({ hosts, nodes, empty }: { hosts: readonly HostReport[]; nodes: Named; empty: string }) {
  const rows = hosts.flatMap((host) =>
    host.state !== 'done' || host.memory === null || host.memory.total_bytes <= 0
      ? []
      : [{ host, memory: host.memory }],
  )
  if (rows.length === 0) return <Empty>{empty}</Empty>
  return (
    <ul className="space-y-3">
      {rows.map(({ host, memory }) => {
        const total = memory.total_bytes
        const rss = Math.min(memory.process_rss_bytes ?? 0, total)
        const used = Math.max(0, total - memory.available_bytes - rss)
        const pct = (bytes: number) => `${(bytes / total) * 100}%`
        return (
          <li key={host.node} className="text-[13px]" data-testid={`memory-${host.node}`}>
            <div className="flex items-baseline justify-between gap-3">
              <span>{nameOf(nodes, host.node)}</span>
              <span className="text-xs text-muted-foreground tabular">
                {c.memoryFree} {c.memoryValue(formatBytes(memory.available_bytes), formatBytes(total))}
              </span>
            </div>
            <Tooltip
              content={`${c.memoryI3t} ${formatBytes(rss)} · ${c.memoryOther} ${formatBytes(used)} · ${c.memoryFree} ${formatBytes(memory.available_bytes)}`}
            >
              <div tabIndex={0} className="mt-1.5 flex h-2 gap-0.5 rounded-full outline-none" aria-hidden>
                <span className="h-full min-w-[3px] rounded-l-full bg-viz-1" style={{ width: pct(rss) }} />
                <span className="h-full bg-border-strong" style={{ width: pct(used) }} />
                <span className="h-full flex-1 rounded-r-full bg-muted" />
              </div>
            </Tooltip>
          </li>
        )
      })}
    </ul>
  )
}
