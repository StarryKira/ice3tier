import { Check } from 'lucide-react'
import { type ReactNode } from 'react'

import {
  type Choice,
  type DecisionView,
  type HostReport,
  type NodeId,
  type PathLeg,
  type PathSegment,
  type Route,
  type PathView,
  type TestView,
  type Transfer,
} from '@/api/types'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { niceRange, niceScale, type Scale } from '@/lib/scale'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { cn } from '@/lib/utils'
import { measuredPaths, type Named, pathLabel, routeName } from '@/topology/diagnosis'
import { describeLeg } from '@/topology/path'

import { dimmed, routeKey, useFocus } from './focus'
import { ItemState } from './parts'
import { Bar, Dot, Empty, Legend, Plot, type PlotRow } from './viz'

const d = t.diagnosis
const c = t.diagnosis.chart
const s = t.tests

const KIND = {
  direct: { dot: 'bg-ok-dot', soft: 'bg-ok-dot/35', line: 'bg-ok-dot' },
  relay: { dot: 'bg-relay-dot', soft: 'bg-relay-dot/35', line: 'bg-relay-dot' },
  none: { dot: 'bg-idle-dot', soft: 'bg-idle-dot/35', line: 'bg-border-strong' },
} as const

type Kind = keyof typeof KIND

function PathName({ kind, children }: { kind: Kind; children: ReactNode }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <span aria-hidden className={cn('size-1.5 shrink-0 rounded-full', KIND[kind].dot)} />
      <span className="truncate">{children}</span>
    </span>
  )
}

const nameOf = (nodes: Named, id: NodeId) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)
const kindOf = (via: string | undefined): Kind => (via === 'direct' ? 'direct' : via === 'relay' ? 'relay' : 'none')

/** 表头的一格：小号灰字。 */
function Head({ children, right = false, className }: { children?: ReactNode; right?: boolean; className?: string }) {
  return (
    <th
      scope="col"
      className={cn(
        'px-2 pb-1.5 text-xs font-normal whitespace-nowrap text-muted-foreground first:pl-0 last:pr-0',
        right ? 'text-right' : 'text-left',
        className,
      )}
    >
      {children}
    </th>
  )
}

/* 两个方向的路径：一张表，像 mtr 一样一段一行，最后一列是瀑布条，看延迟在哪一段累起来。 */

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
    ({ result }) => forward && result.route !== null && routeKey(result.route) === routeKey(route),
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

/** 一个方向在表里的几行：第一格跨这几行，写方向、路、延迟和数据的来源，悬停看什么时候换的、为什么、选路模式。 */
function DirectionRows({
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
  const { focus, setFocus } = useFocus()
  const view = describeLeg(leg, (id) => nameOf(nodes, id))
  const kind = kindOf(leg.route?.via)
  const key = routeKey(leg.route)
  const sinceReason = view.reason !== null && view.sinceReason !== view.reason ? view.sinceReason : null
  const meta = [
    view.since === null
      ? null
      : `${t.path.sinceLabel} ${view.since}${sinceReason === null ? '' : d.report.paren(sinceReason)}`,
    view.reason ?? view.sinceReason,
    view.mode?.label ?? null,
  ].filter((part): part is string => part !== null)
  const rows: readonly (Hop | null)[] = hops.length === 0 ? [null] : hops
  const starts = hops.map((_, index) => hops.slice(0, index).reduce((sum, hop) => sum + (hop.rtt_us ?? 0), 0))
  const at = (us: number) => `${total === 0 ? 0 : (us / total) * 100}%`
  return (
    <tbody
      className={cn('border-t first:border-t-0 transition-opacity', dimmed(focus, key) && 'opacity-30')}
      data-testid={testId}
      data-source={view.state}
      onMouseEnter={() => setFocus(key)}
      onMouseLeave={() => setFocus(null)}
    >
      {rows.map((hop, index) => (
        <tr key={hop === null ? 'none' : `${hop.from}>${hop.to}`} className="align-top">
          {index === 0 ? (
            <th scope="rowgroup" rowSpan={rows.length} className="w-[34%] py-2 pr-3 text-left font-normal">
              <span className="flex flex-wrap items-baseline gap-x-2">
                <span className="font-semibold">{t.path.forward(leg.from.name, leg.to.name)}</span>
                <span className="ml-auto font-semibold tabular">{view.rtt ?? '—'}</span>
              </span>
              <span className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-xs">
                <PathName kind={kind}>{view.route?.text ?? t.routes.noData}</PathName>
                {view.route?.detail !== null && view.route?.detail !== undefined ? (
                  <span className={leg.relay_leg === 'tcp' ? 'font-medium text-warn' : 'text-muted-foreground'}>
                    {view.route.detail}
                  </span>
                ) : null}
                {view.state !== 'query' ? (
                  <Tooltip content={view.sourceHint}>
                    <span tabIndex={0} className="rounded-sm text-warn">
                      {view.sourceLabel}
                      {view.age !== null ? ` · ${view.age}` : null}
                    </span>
                  </Tooltip>
                ) : (
                  <span className="sr-only">{view.sourceLabel}</span>
                )}
              </span>
              {meta.length > 0 ? (
                <span className="mt-0.5 block text-xs text-muted-foreground">{meta.join(' · ')}</span>
              ) : null}
            </th>
          ) : null}
          {hop === null ? (
            <td colSpan={5} className="py-2 text-muted-foreground">
              {t.routes.noData}
            </td>
          ) : (
            <>
              <td className="py-2 pr-2 whitespace-nowrap">
                {hop.from} → {hop.to}
                {index === 0 && leg.relay_leg === 'tcp' ? (
                  <span className="ml-1.5 text-xs font-medium text-warn">TCP</span>
                ) : null}
              </td>
              <td className="px-2 py-2 text-right font-medium whitespace-nowrap tabular">{msText(hop.rtt_us)}</td>
              <td className="px-2 py-2 text-right whitespace-nowrap text-muted-foreground tabular">
                {msText(hop.jitter_us)}
              </td>
              <td
                className={cn(
                  'px-2 py-2 text-right whitespace-nowrap tabular',
                  hop.loss_permille !== null && hop.loss_permille >= 10
                    ? 'font-medium text-warn'
                    : 'text-muted-foreground',
                )}
              >
                {permilleText(hop.loss_permille)}
              </td>
              <td className="w-[30%] py-2 pl-2">
                {/* 和这一行的字在同一条线上：一行字高，条在中间。 */}
                <span className="flex h-5 items-center">
                  <span className="relative block h-1.5 flex-1 rounded-full bg-muted">
                    {hop.rtt_us === null ? null : (
                      <span
                        className={cn('absolute inset-y-0 rounded-full', KIND[kind].line)}
                        style={{ left: at(starts[index] ?? 0), width: `max(3px, ${at(hop.rtt_us)})` }}
                      />
                    )}
                  </span>
                </span>
              </td>
            </>
          )}
        </tr>
      ))}
    </tbody>
  )
}

/** 两个方向一张表：方向、段、延迟、抖动、丢包、累计；两个方向的瀑布条用同一把尺子。 */
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
    <div className="overflow-x-auto">
      <table className="w-full min-w-[40rem] border-collapse text-[13px] leading-5" aria-label={d.sections.path}>
        <thead className="border-b">
          <tr>
            <Head>{c.direction}</Head>
            <Head>{c.hop}</Head>
            <Head right>{s.latency}</Head>
            <Head right>{s.jitter}</Head>
            <Head right>{s.loss}</Head>
            <Head>{c.cumulative}</Head>
          </tr>
        </thead>
        <DirectionRows leg={path.forward} hops={forward} total={total} nodes={nodes} testId="leg-forward" />
        <DirectionRows leg={path.reverse} hops={reverse} total={total} nodes={nodes} testId="leg-reverse" />
      </table>
    </div>
  )
}

/* 各条路：一条路一行。延迟（最小到 P95 的条、平均的点）和吞吐（上行、下行两根条）画在格子里，刻度写在表头，
   数字一列一列对齐。悬停一行，别的图里同一条路也亮着。 */

/** 画图那一列的表头：名字下面一排刻度，和格子里的图用同一把尺子。 */
function AxisHead({
  label,
  scale,
  format,
  width,
}: {
  label: ReactNode
  scale: Scale
  format: (tick: number, last: boolean) => string
  /** 这一列占表宽的多少：图要有地方画。 */
  width: string
}) {
  const span = scale.max - scale.min || 1
  return (
    <th
      scope="col"
      className="px-2 pb-1.5 text-left align-bottom text-xs font-normal text-muted-foreground"
      style={{ width }}
    >
      {label}
      <span aria-hidden className="relative mt-0.5 block h-3.5 text-[10px] tabular">
        {scale.ticks.map((tick, index) => (
          <span
            key={tick}
            className={cn(
              'absolute top-0 whitespace-nowrap',
              index === 0 ? '' : index === scale.ticks.length - 1 ? '-translate-x-full' : '-translate-x-1/2',
            )}
            style={{ left: `${((tick - scale.min) / span) * 100}%` }}
          >
            {format(tick, index === scale.ticks.length - 1)}
          </span>
        ))}
      </span>
    </th>
  )
}

export const PATHS_LEGEND = () => [
  { label: c.rangeKey, className: 'bg-foreground/25' },
  { label: c.avgKey, className: 'bg-foreground', shape: 'dot' as const },
  { label: s.up, className: 'bg-viz-1' },
  { label: s.down, className: 'bg-viz-2' },
]

/** 图例：有吞吐时才有后两个。 */
export function PathsLegend({ test }: { test: TestView | undefined }) {
  const legend = PATHS_LEGEND()
  const throughput = measuredPaths(test).some(({ result }) => result.throughput !== null)
  return <Legend items={throughput ? legend : legend.slice(0, 2)} />
}

function transferLine(label: (rate: string) => string, transfer: Transfer | null) {
  if (transfer === null) return null
  const time = transfer.ms >= 1000 ? `${Math.round(transfer.ms / 100) / 10} s` : `${transfer.ms} ms`
  return (
    <span key={label('')} className="block">
      {label(rateText(transfer.bits_per_second))} · {formatBytes(transfer.bytes)} · {time}
    </span>
  )
}

export function PathsTable({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  const { focus, setFocus } = useFocus()
  const paths = measuredPaths(test)
  if (paths.length === 0) return <Empty>{test === undefined ? d.latencyNone : c.notMeasured}</Empty>
  const timed = paths.flatMap(({ result }) =>
    result.state === 'done' && result.latency !== null ? [result.latency] : [],
  )
  // 点和区间不必从 0 量：轴只盖住数据，几毫秒的差别也看得出。
  const top = Math.max(0, ...timed.map((latency) => latency.p95_us ?? 0)) / 1000
  const bottom = Math.min(top * 1000, ...timed.map((latency) => latency.min_us ?? top * 1000)) / 1000
  const pad = Math.max((top - bottom) * 0.1, top * 0.02, 0.5)
  const latencyScale = niceRange(Math.max(0, bottom - pad), top + pad, 3)
  const withThroughput = paths.some(({ result }) => result.throughput !== null)
  const rates = paths.flatMap(({ result }) => [
    result.throughput?.up?.bits_per_second ?? 0,
    result.throughput?.down?.bits_per_second ?? 0,
  ])
  // 吞吐按 Mbps 量，上了 1 Gbps 按 Gbps。
  const giga = Math.max(...rates) >= 1e9
  const unit = giga ? 1e9 : 1e6
  const rateScale = niceScale(Math.max(...rates) / unit, 2)
  const xOf = (scale: Scale) => (value: number) =>
    `${Math.max(0, Math.min(100, ((value - scale.min) / (scale.max - scale.min || 1)) * 100))}%`
  const xl = xOf(latencyScale)
  const xr = xOf(rateScale)
  const columns = withThroughput ? 8 : 6
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[46rem] border-collapse text-[13px]" data-testid="test-quality">
        <thead className="border-b">
          <tr>
            <Head className="align-bottom">{c.colPath}</Head>
            <AxisHead
              label={s.latency}
              width={withThroughput ? '24%' : '36%'}
              scale={latencyScale}
              format={(tick, last) => (last ? t.common.ms(tick) : String(tick))}
            />
            <Head right className="align-bottom">
              {c.avgKey}
            </Head>
            <Head right className="align-bottom">
              {s.jitter}
            </Head>
            <Head right className="align-bottom">
              {s.loss}
            </Head>
            {withThroughput ? (
              <>
                <AxisHead
                  label={s.throughputTitle}
                  width="18%"
                  scale={rateScale}
                  format={(tick, last) => (last ? `${tick} ${giga ? 'Gbps' : 'Mbps'}` : String(tick))}
                />
                <Head right className="align-bottom">{`${s.up} / ${s.down}`}</Head>
              </>
            ) : null}
            <Head right className="align-bottom">
              {s.cost}
            </Head>
          </tr>
        </thead>
        <tbody>
          {paths.map((path, index) => {
            const { result } = path
            const key = path.direct ? 'direct' : routeKey(result.route)
            const kind = path.direct ? 'direct' : kindOf(result.route?.via)
            const latency = result.latency
            const throughput = result.throughput
            const done = result.state === 'done'
            const loss = latency?.loss_permille ?? null
            const label = pathLabel(path, nodes)
            const why = result.picked_by.filter((reason) => reason !== 'direct')
            return (
              <tr
                key={`${key}-${index}`}
                className={cn('transition-opacity hover:bg-muted/40', dimmed(focus, key) && 'opacity-30')}
                onMouseEnter={() => setFocus(key)}
                onMouseLeave={() => setFocus(null)}
              >
                <th scope="row" className="py-1.5 pr-2 text-left font-normal" data-testid={`test-path-${index}`}>
                  <span className="flex items-center gap-1.5">
                    <PathName kind={kind}>{label}</PathName>
                    {result.relay_leg === 'tcp' ? (
                      <span className="text-xs font-medium text-warn">{t.routes.legTransport.tcp}</span>
                    ) : null}
                  </span>
                  {why.length > 0 ? (
                    <span className="block pl-3 text-[11px] text-muted-foreground">
                      {why.map((reason) => s.pickedBy[reason]).join(t.common.listSeparator)}
                    </span>
                  ) : null}
                </th>
                {done ? (
                  <>
                    <td className="px-2 py-1.5" data-testid={`latency-${path.direct ? 'direct' : label}`}>
                      <Tooltip
                        content={
                          latency === null ? null : (
                            <span className="block space-y-0.5">
                              <span className="block">
                                {s.latencyValue(msText(latency.min_us), msText(latency.avg_us), msText(latency.p95_us))}
                              </span>
                              <span className="block">{s.answered(latency.answered, latency.probes)}</span>
                              {result.hops.length > 1
                                ? result.hops.map((hop) => (
                                    <span key={`${hop.from}>${hop.to}`} className="block">
                                      {t.path.legLine(
                                        nameOf(nodes, hop.from),
                                        nameOf(nodes, hop.to),
                                        msText(hop.rtt_us),
                                        msText(hop.jitter_us),
                                        permilleText(hop.loss_permille),
                                      )}
                                    </span>
                                  ))
                                : null}
                            </span>
                          )
                        }
                      >
                        <span tabIndex={0} className="relative block h-5 rounded-sm outline-none">
                          {latency?.min_us == null || latency.avg_us === null || latency.p95_us === null ? null : (
                            <>
                              <Bar
                                from={xl(latency.min_us / 1000)}
                                to={xl(latency.p95_us / 1000)}
                                className={KIND[kind].soft}
                              />
                              <Dot at={xl(latency.avg_us / 1000)} className={KIND[kind].dot} />
                            </>
                          )}
                        </span>
                      </Tooltip>
                    </td>
                    <td
                      className="px-2 py-1.5 text-right font-medium whitespace-nowrap tabular"
                      data-testid={`test-path-${index}-latency`}
                    >
                      {msText(latency?.avg_us)}
                    </td>
                    <td className="px-2 py-1.5 text-right whitespace-nowrap text-muted-foreground tabular">
                      {msText(latency?.jitter_us)}
                    </td>
                    <td
                      className={cn(
                        'px-2 py-1.5 text-right whitespace-nowrap tabular',
                        loss !== null && loss >= 10 ? 'font-medium text-warn' : 'text-muted-foreground',
                      )}
                      data-testid={`test-path-${index}-loss`}
                    >
                      {permilleText(loss)}
                      {latency?.loss_up_permille != null && latency.loss_down_permille !== null ? (
                        <span className="block text-[11px] font-normal text-muted-foreground">
                          {s.lossSplit(
                            permilleText(latency.loss_up_permille),
                            permilleText(latency.loss_down_permille),
                          )}
                        </span>
                      ) : null}
                    </td>
                    {withThroughput ? (
                      throughput?.state === 'done' ? (
                        <>
                          <td className="px-2 py-1.5" data-testid={`bandwidth-${path.direct ? 'direct' : label}`}>
                            <Tooltip
                              content={
                                <span className="block space-y-0.5">
                                  {transferLine(c.up, throughput.up)}
                                  {transferLine(c.down, throughput.down)}
                                  <span className="block">
                                    {d.congestion} {throughput.congestion?.toUpperCase() ?? '—'}
                                  </span>
                                </span>
                              }
                            >
                              <span tabIndex={0} className="relative block h-5 rounded-sm outline-none">
                                {throughput.up !== null ? (
                                  <Bar
                                    from="0%"
                                    to={xr(throughput.up.bits_per_second / unit)}
                                    className="bg-viz-1"
                                    top="28%"
                                    height="h-1.5"
                                  />
                                ) : null}
                                {throughput.down !== null ? (
                                  <Bar
                                    from="0%"
                                    to={xr(throughput.down.bits_per_second / unit)}
                                    className="bg-viz-2"
                                    top="72%"
                                    height="h-1.5"
                                  />
                                ) : null}
                              </span>
                            </Tooltip>
                          </td>
                          <td
                            className="px-2 py-1.5 text-right text-xs leading-tight whitespace-nowrap tabular"
                            data-testid={`test-path-${index}-throughput`}
                          >
                            <span className="block">{rateText(throughput.up?.bits_per_second ?? null)}</span>
                            <span className="block">{rateText(throughput.down?.bits_per_second ?? null)}</span>
                          </td>
                        </>
                      ) : (
                        <td
                          colSpan={2}
                          className="px-2 py-1.5 text-xs text-muted-foreground"
                          data-testid={`test-path-${index}-throughput`}
                        >
                          {throughput === null ? (
                            c.notMeasured
                          ) : throughput.problem !== null ? (
                            problemText(throughput.problem, nodes)
                          ) : (
                            <ItemState state={throughput.state} problem={null} nodes={nodes} />
                          )}
                        </td>
                      )
                    ) : null}
                    <td
                      className="py-1.5 pl-2 text-right whitespace-nowrap text-muted-foreground tabular"
                      data-testid={`test-path-${index}-cost`}
                    >
                      {msText(result.cost_us)}
                    </td>
                  </>
                ) : (
                  <td colSpan={columns - 1} className="px-2 py-1.5" data-testid={`test-path-${index}-state`}>
                    <ItemState state={result.state} problem={result.problem} nodes={nodes} />
                  </td>
                )}
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

/* 候选路径：一个方向一组，一条一行：代价的条（现在走的是主色），两种选路模式各会不会走它。 */

function reasonText(reason: string): string {
  return Object.hasOwn(d.reason, reason) ? d.reason[reason as keyof typeof d.reason] : reason
}

/** "会走"那一格：会走就是一个勾，悬停看为什么（有更便宜却没换时写是哪条）；不会走是空的。 */
function Pick({
  choice,
  candidate,
  nodes,
  testId,
}: {
  choice: Choice | undefined
  candidate: string
  nodes: Named
  testId: string
}) {
  if (choice === undefined || routeKey(choice.route) !== candidate) return null
  const cheaper =
    choice.cheaper === null || choice.cheaper === undefined
      ? null
      : d.cheaperLine(routeName(choice.cheaper.route, nodes), msText(choice.cheaper.cost_us))
  return (
    <Tooltip
      content={
        <span className="block space-y-0.5">
          <span className="block">{reasonText(choice.reason)}</span>
          {cheaper !== null ? <span className="block">{cheaper}</span> : null}
        </span>
      }
    >
      <span
        tabIndex={0}
        className="inline-flex rounded-sm text-foreground"
        aria-label={[reasonText(choice.reason), cheaper].filter((part) => part !== null).join(t.common.listSeparator)}
        data-testid={testId}
      >
        <Check className="size-3.5" aria-hidden />
      </span>
    </Tooltip>
  )
}

interface CandidateRow {
  route: Route
  cost_us: number | null
  legs: PathSegment[]
  load_percent: number | null
  current: boolean
}

/**
 * 一个方向的候选路径：发送方现在手上的；没有时用诊断那一刻它考虑过的；再没有就是两种模式选的路和那条更便宜的
 * （只有代价，没有各段）。
 */
function candidatesOf(leg: PathLeg, decision: DecisionView | undefined): CandidateRow[] {
  const live = leg.candidates ?? []
  if (live.length > 0) return live
  if (decision === undefined) return []
  if (decision.candidates.length > 0) return decision.candidates
  const rows: CandidateRow[] = []
  const add = (route: Route | null | undefined, cost: number | null) => {
    if (route === null || route === undefined || route.via === 'none') return
    if (rows.some((row) => routeKey(row.route) === routeKey(route))) return
    rows.push({
      route,
      cost_us: cost,
      legs: [],
      load_percent: null,
      current: routeKey(route) === routeKey(decision.current),
    })
  }
  add(decision.direct_first.route, decision.direct_first.cost_us)
  add(decision.lowest_cost.route, decision.lowest_cost.cost_us)
  add(decision.lowest_cost.cheaper?.route, decision.lowest_cost.cheaper?.cost_us ?? null)
  add(decision.direct_first.cheaper?.route, decision.direct_first.cheaper?.cost_us ?? null)
  return rows.sort((x, y) => (x.cost_us ?? Infinity) - (y.cost_us ?? Infinity))
}

export function CandidatesTable({
  path,
  test,
  nodes,
}: {
  path: PathView | undefined
  test: TestView | undefined
  nodes: Named
}) {
  const { focus, setFocus } = useFocus()
  const decisions = [test?.route?.forward, test?.route?.reverse].filter(
    (decision): decision is DecisionView => decision !== null && decision !== undefined,
  )
  const decisionOf = (leg: PathLeg) => decisions.find((decision) => decision.from === leg.from.id)
  const legs = (path === undefined ? [] : [path.forward, path.reverse])
    .map((leg) => ({ leg, rows: candidatesOf(leg, decisionOf(leg)) }))
    .filter(({ rows }) => rows.length > 0)
  if (legs.length === 0) return <Empty>{path === undefined ? t.path.querying : t.path.candidatesEmpty}</Empty>
  const top = Math.max(...legs.flatMap(({ rows }) => rows.map((candidate) => candidate.cost_us ?? 0)))
  const withPicks = decisions.length > 0
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[34rem] border-collapse text-[13px]" aria-label={t.path.candidatesTitle}>
        <thead className="border-b">
          <tr>
            <Head>{c.colPath}</Head>
            <Head className="w-[38%]">{c.colCost}</Head>
            <Head right>{c.colLoad}</Head>
            {withPicks ? (
              <>
                <Head className="text-center">{c.pickDefault}</Head>
                <Head className="text-center">{c.pickLowest}</Head>
              </>
            ) : null}
          </tr>
        </thead>
        {legs.map(({ leg, rows }) => {
          const decision = decisionOf(leg)
          return (
            <tbody key={leg.from.id} data-testid={`candidates-${leg === path?.forward ? 'forward' : 'reverse'}`}>
              <tr>
                <th
                  colSpan={withPicks ? 5 : 3}
                  scope="colgroup"
                  className="pt-2 pb-1 text-left text-xs font-medium text-muted-foreground"
                >
                  {t.path.forward(leg.from.name, leg.to.name)}
                </th>
              </tr>
              {rows.slice(0, 6).map((candidate) => {
                const key = routeKey(candidate.route)
                const kind = kindOf(candidate.route.via)
                return (
                  <Tooltip
                    key={key}
                    content={
                      candidate.legs.length === 0 ? null : (
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
                        </span>
                      )
                    }
                  >
                    <tr
                      tabIndex={0}
                      className={cn(
                        'outline-none transition-opacity focus-visible:bg-muted/40',
                        dimmed(focus, key) && 'opacity-30',
                      )}
                      data-testid={candidate.current ? 'candidate-current' : undefined}
                      onMouseEnter={() => setFocus(key)}
                      onMouseLeave={() => setFocus(null)}
                    >
                      <td className="py-1 pr-2 whitespace-nowrap">
                        <PathName kind={kind}>{routeName(candidate.route, nodes)}</PathName>
                        {candidate.current ? (
                          <span className="ml-1.5 text-xs text-muted-foreground">{c.current}</span>
                        ) : null}
                      </td>
                      <td className="px-2 py-1">
                        <span className="flex items-center gap-2">
                          <span className="relative h-1.5 flex-1">
                            <span
                              className={cn(
                                'absolute inset-y-0 left-0 rounded-full',
                                candidate.current ? 'bg-primary' : 'bg-border-strong',
                              )}
                              style={{
                                width: `max(3px, ${top === 0 || candidate.cost_us === null ? 0 : (candidate.cost_us / top) * 100}%)`,
                              }}
                            />
                          </span>
                          <span className={cn('w-14 text-right text-xs tabular', candidate.current && 'font-medium')}>
                            {msText(candidate.cost_us)}
                          </span>
                        </span>
                      </td>
                      <td className="px-2 py-1 text-right text-xs text-muted-foreground tabular">
                        {candidate.route.via === 'relay' && candidate.load_percent !== null
                          ? `${candidate.load_percent}%`
                          : '—'}
                      </td>
                      {withPicks ? (
                        <>
                          <td className="px-2 py-1 text-center">
                            <Pick
                              choice={decision?.direct_first}
                              candidate={key}
                              nodes={nodes}
                              testId={`pick-${leg.from.id}-direct_first`}
                            />
                          </td>
                          <td className="py-1 pl-2 text-center">
                            <Pick
                              choice={decision?.lowest_cost}
                              candidate={key}
                              nodes={nodes}
                              testId={`pick-${leg.from.id}-lowest_cost`}
                            />
                          </td>
                        </>
                      ) : null}
                    </tr>
                  </Tooltip>
                )
              })}
            </tbody>
          )
        })}
      </table>
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
                  <span aria-hidden className="absolute inset-x-0 top-1/2 h-px bg-border" />
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
      scale={niceScale(top, 2)}
      format={(tick, last) => (last ? `${tick}%` : String(tick))}
      minRow="min-h-7"
      labelWidth="7rem"
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
    <div className="flex h-full flex-col justify-around gap-3">
      {rows.map(({ host, memory }) => {
        const total = memory.total_bytes
        const rss = Math.min(memory.process_rss_bytes ?? 0, total)
        const used = Math.max(0, total - memory.available_bytes - rss)
        const pct = (bytes: number) => `${(bytes / total) * 100}%`
        return (
          <div key={host.node} className="text-[13px]" data-testid={`memory-${host.node}`}>
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
          </div>
        )
      })}
    </div>
  )
}
