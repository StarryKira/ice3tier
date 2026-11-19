import {
  type HostReport,
  type NodeDetail,
  type NodeId,
  type PathLeg,
  type PathResult,
  type PathView,
  type Route,
  type TestItemState,
  type TestView,
} from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { formatAge } from '@/lib/time'

/**
 * 路径诊断：从两端的节点信息、两个方向现在的路、一次诊断的结果里算出界面上的指标和结论。纯函数。
 *
 * 结论按严重程度排：严重（连不上、不在线）、注意（UDP 不通、对称型 NAT、丢包、CPU 卡住……）、提示（两个方向不一样、
 * 有更快的路但差距不够换……）。诊断做完了又没有严重和注意的，给一条"没有发现问题"。
 */

const d = t.diagnosis

export type Section = 'overview' | 'route' | 'latency' | 'bandwidth' | 'hosts' | 'output'
export const SECTIONS: readonly Section[] = ['overview', 'route', 'latency', 'bandwidth', 'hosts', 'output']

export type Named = readonly { id: NodeId; name: string }[]

export interface DiagnosisInput {
  a: NodeDetail | undefined
  b: NodeDetail | undefined
  path: PathView | undefined
  test: TestView | undefined
  /** 认得的名字：节点列表里的，加上测试里带的。 */
  nodes: Named
}

const nameOf = (nodes: Named, id: NodeId) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)

/** 一条路的一句话，和拓扑、路由矩阵一样的说法："直连"、"经 h1 → h2"，没有路时"没有能走的路"。 */
export function routeName(route: Route | null | undefined, nodes: Named): string {
  if (route === null || route === undefined || route.via === 'none') return t.tests.noRoute
  if (route.via === 'direct') return t.routes.direct
  return t.routes.via(route.relays.map((id) => nameOf(nodes, id)).join(t.routes.hopSeparator))
}

/** 测了的一条路，和它是不是直连那一条（路还不知道时靠这个起名字）。 */
export interface Measured {
  result: PathResult
  direct: boolean
}

export function measuredPaths(test: TestView | undefined): Measured[] {
  if (test === undefined) return []
  return [
    ...(test.direct_quality === null ? [] : [{ result: test.direct_quality, direct: true }]),
    ...test.relay_quality.map((result) => ({ result, direct: false })),
  ]
}

/** 一条测了的路叫什么："直连"、"经 h2"。 */
export function pathLabel(path: Measured, nodes: Named): string {
  if (path.direct) return t.tests.qualityDirect
  return path.result.route === null ? t.tests.qualityRelay : routeName(path.result.route, nodes)
}

/** 平均延迟最低的、测完了的那条。 */
export function fastest(paths: readonly Measured[]): Measured | null {
  let best: Measured | null = null
  for (const path of paths) {
    const avg = path.result.latency?.avg_us
    if (path.result.state !== 'done' || avg === null || avg === undefined) continue
    const current = best?.result.latency?.avg_us
    if (best === null || current === null || current === undefined || avg < current) best = path
  }
  return best
}

/** 吞吐最高的那条：按上下行里慢的那个比。 */
export function widest(paths: readonly Measured[]): Measured | null {
  let best: Measured | null = null
  let bestRate = -1
  for (const path of paths) {
    const throughput = path.result.throughput
    if (throughput?.state !== 'done') continue
    const rate = Math.min(
      throughput.up?.bits_per_second ?? Number.POSITIVE_INFINITY,
      throughput.down?.bits_per_second ?? Number.POSITIVE_INFINITY,
    )
    if (Number.isFinite(rate) && rate > bestRate) {
      best = path
      bestRate = rate
    }
  }
  return best
}

export function hostOf(test: TestView | undefined, node: NodeId | undefined): HostReport | undefined {
  return test?.hosts?.find((host) => host.node === node)
}

/* 进度：要了的每一项现在到哪一步。 */

export type StepKey = 'direct' | 'direct_quality' | 'relay_quality' | 'route' | 'throughput' | 'host'

export interface Step {
  key: StepKey
  state: TestItemState
}

/** 几个结果合成一项：还有在测的就是在测；有一个成了就是成了；都没成看有没有失败的。 */
function merge(states: readonly TestItemState[]): TestItemState | null {
  if (states.length === 0) return null
  if (states.includes('pending')) return 'pending'
  if (states.includes('done')) return 'done'
  if (states.includes('failed')) return 'failed'
  return 'skipped'
}

export function steps(test: TestView): Step[] {
  const list: { key: StepKey; state: TestItemState | null }[] = [
    { key: 'direct', state: test.direct?.state ?? null },
    { key: 'direct_quality', state: test.direct_quality?.state ?? null },
    { key: 'relay_quality', state: merge(test.relay_quality.map((path) => path.state)) },
    { key: 'route', state: test.route?.state ?? null },
    {
      key: 'throughput',
      state: test.request.throughput
        ? merge(
            measuredPaths(test).flatMap(({ result }) =>
              result.throughput === null
                ? result.state === 'pending'
                  ? ['pending' as const]
                  : []
                : [result.throughput.state],
            ),
          )
        : null,
    },
    { key: 'host', state: merge((test.hosts ?? []).map((host) => host.state)) },
  ]
  return list.filter((step): step is Step => step.state !== null)
}

/* 指标：顶上一排。 */

export type KpiKey = 'direct' | 'latency' | 'loss' | 'throughput' | 'cpu'

export interface KpiValue {
  text: string
  /** 这一个数要注意（琥珀色）。 */
  warn: boolean
}

export interface Kpi {
  key: KpiKey
  /** 大字，一行或两行（吞吐上下行各一行、CPU 两端各一行）；没有是空数组。 */
  values: KpiValue[]
  sub: string | null
  tone: Tone | null
  section: Section
}

export function kpis(input: DiagnosisInput): Kpi[] {
  const { test, nodes } = input
  const paths = measuredPaths(test)
  const none = (key: KpiKey, section: Section, sub: string | null = test === undefined ? null : d.kpiNone): Kpi => ({
    key,
    values: [],
    sub,
    tone: null,
    section,
  })

  let direct = none('direct', 'overview')
  if (test?.direct?.state === 'done') {
    const ok = test.direct.ok === true
    const how =
      test.direct.kind === null
        ? null
        : test.direct.already === true
          ? `${t.routes.directKind[test.direct.kind]} · ${t.tests.directAlready}`
          : test.direct.elapsed_ms === null
            ? t.routes.directKind[test.direct.kind]
            : `${t.routes.directKind[test.direct.kind]} · ${t.tests.directIn(test.direct.elapsed_ms)}`
    direct = {
      key: 'direct',
      values: [{ text: ok ? t.tests.directOk : t.tests.directNo, warn: !ok }],
      sub: ok ? how : test.direct.problem === null ? null : problemText(test.direct.problem, nodes),
      tone: ok ? 'ok' : 'warn',
      section: 'overview',
    }
  } else if (test?.direct?.state === 'pending') direct = none('direct', 'overview', t.tests.pending)

  const best = fastest(paths)
  const latency = best?.result.latency ?? null
  const latencyKpi: Kpi =
    best === null || latency === null
      ? none('latency', 'latency', paths.some(({ result }) => result.state === 'pending') ? t.tests.pending : undefined)
      : {
          key: 'latency',
          values: [{ text: msText(latency.avg_us), warn: false }],
          sub: `${pathLabel(best, nodes)} · ${d.p95(msText(latency.p95_us))}`,
          tone: null,
          section: 'latency',
        }
  const lossValue = latency?.loss_permille ?? null
  const loss: Kpi =
    best === null || latency === null || lossValue === null
      ? none('loss', 'latency', latencyKpi.sub === t.tests.pending ? t.tests.pending : undefined)
      : {
          key: 'loss',
          values: [{ text: permilleText(lossValue), warn: lossValue >= 10 }],
          sub:
            latency.loss_up_permille !== null && latency.loss_down_permille !== null
              ? t.tests.lossSplit(permilleText(latency.loss_up_permille), permilleText(latency.loss_down_permille))
              : d.answered(latency.answered, latency.probes),
          tone: lossValue >= 50 ? 'danger' : lossValue >= 10 ? 'warn' : 'ok',
          section: 'latency',
        }

  const wide = widest(paths)
  const throughput: Kpi =
    wide?.result.throughput === null || wide?.result.throughput === undefined
      ? none(
          'throughput',
          'bandwidth',
          test === undefined ? null : test.request.throughput ? d.kpiNone : d.kpiNoThroughput,
        )
      : {
          key: 'throughput',
          values: [
            { text: `${t.tests.up} ${rateText(wide.result.throughput.up?.bits_per_second)}`, warn: false },
            { text: `${t.tests.down} ${rateText(wide.result.throughput.down?.bits_per_second)}`, warn: false },
          ],
          sub: pathLabel(wide, nodes),
          tone: null,
          section: 'bandwidth',
        }

  const peaks = (test?.hosts ?? []).flatMap((host) =>
    host.cpu_peak?.process_percent === null || host.cpu_peak?.process_percent === undefined
      ? []
      : [{ name: nameOf(nodes, host.node), percent: host.cpu_peak.process_percent }],
  )
  const cpu: Kpi =
    peaks.length === 0
      ? none(
          'cpu',
          'hosts',
          test === undefined
            ? null
            : (test.hosts ?? []).some((host) => host.state === 'pending')
              ? t.tests.pending
              : d.kpiNoHost,
        )
      : {
          key: 'cpu',
          values: peaks.map((peak) => ({ text: d.cpuSide(peak.name, peak.percent), warn: peak.percent >= 85 })),
          sub: null,
          tone: peaks.some((peak) => peak.percent >= 85) ? 'warn' : 'ok',
          section: 'hosts',
        }

  return [direct, latencyKpi, loss, throughput, cpu]
}

/* 结论。 */

export type FindingTone = 'danger' | 'warn' | 'info' | 'ok'

export interface Finding {
  /** 同一条结论只出现一次（两个来源都说了 UDP 不通时）。 */
  key: string
  tone: FindingTone
  text: string
  section: Section
}

const ORDER: Record<FindingTone, number> = { danger: 0, warn: 1, info: 2, ok: 3 }

const legLabel = (leg: PathLeg) => t.path.forward(leg.from.name, leg.to.name)

function sameRoute(a: Route | null, b: Route | null): boolean {
  if (a === null || b === null) return a === b
  if (a.via !== b.via) return false
  if (a.via === 'relay' && b.via === 'relay') return a.relays.join('.') === [...b.relays].reverse().join('.')
  return true
}

/** 一个方向现在走的路，一句话："直连"、"经 h2"。 */
function legRoute(leg: PathLeg, nodes: Named): string {
  return routeName(leg.route, nodes)
}

export function findings(input: DiagnosisInput): Finding[] {
  const { a, b, path, test, nodes } = input
  const list: Finding[] = []
  const add = (finding: Finding) => {
    if (!list.some((other) => other.key === finding.key)) list.push(finding)
  }
  const sides = [a, b].filter((node): node is NodeDetail => node !== undefined)

  for (const node of sides) {
    if (!node.online)
      add({ key: `offline:${node.id}`, tone: 'danger', text: d.find.offline(node.name), section: 'hosts' })
  }
  if (path !== undefined && [path.forward, path.reverse].some((leg) => leg.source === 'not_peers')) {
    add({ key: 'not_peers', tone: 'danger', text: d.find.notPeers, section: 'route' })
  }
  if (test?.error !== null && test?.error !== undefined) {
    add({
      key: 'test_error',
      tone: 'danger',
      text: d.find.testFailed(problemText(test.error, nodes)),
      section: 'overview',
    })
  }
  for (const [index, note] of (test?.notes ?? []).entries()) {
    add({ key: `note:${index}:${note.code}`, tone: 'warn', text: problemText(note, nodes), section: 'overview' })
  }

  // 直连：能不能、能的话现在走没走。
  const direct = test?.direct
  if (direct?.state === 'done') {
    if (direct.ok === true) {
      const how =
        direct.kind === null
          ? t.tests.directOk
          : direct.already === true
            ? `${t.routes.directKind[direct.kind]} · ${t.tests.directAlready}`
            : direct.elapsed_ms === null
              ? t.routes.directKind[direct.kind]
              : `${t.routes.directKind[direct.kind]} · ${t.tests.directIn(direct.elapsed_ms)}`
      add({ key: 'direct_ok', tone: 'ok', text: d.find.directOk(how), section: 'overview' })
      for (const leg of path === undefined ? [] : [path.forward, path.reverse]) {
        if (leg.source === 'query' && leg.route?.via === 'relay') {
          add({
            key: `direct_unused:${leg.from.id}`,
            tone: 'warn',
            text: d.find.directUnused(legLabel(leg), legRoute(leg, nodes)),
            section: 'route',
          })
        }
      }
    } else {
      add({
        key: 'no_direct',
        tone: 'warn',
        text: d.find.cannotDirect(direct.problem === null ? '' : problemText(direct.problem, nodes)),
        section: 'overview',
      })
    }
    for (const side of direct.sides) {
      const name = nameOf(nodes, side.node)
      if (side.udp === false)
        add({ key: `udp:${side.node}`, tone: 'warn', text: d.find.udpBlocked(name), section: 'overview' })
      if (side.mapping_varies === true) {
        add({ key: `nat:${side.node}`, tone: 'warn', text: d.find.symmetricNat(name), section: 'hosts' })
      }
    }
  }
  for (const node of sides) {
    if (node.no_udp === true)
      add({ key: `no_udp:${node.id}`, tone: 'warn', text: d.find.noUdp(node.name), section: 'hosts' })
    if (node.mapping_varies === true) {
      add({ key: `nat:${node.id}`, tone: 'warn', text: d.find.symmetricNat(node.name), section: 'hosts' })
    }
  }

  // 路：到中继走 TCP、两个方向不一样、中继忙。
  if (path !== undefined) {
    for (const leg of [path.forward, path.reverse]) {
      if (leg.route?.via === 'relay' && leg.relay_leg === 'tcp') {
        add({ key: `tcp:${leg.from.id}`, tone: 'warn', text: d.find.tcpLeg(legLabel(leg)), section: 'route' })
      }
      const current = leg.candidates?.find((candidate) => candidate.current)
      if (current !== undefined && current.load_percent >= 90) {
        add({
          key: `busy:${leg.from.id}`,
          tone: 'warn',
          text: d.find.relayBusy(routeName(current.route, nodes), current.load_percent),
          section: 'route',
        })
      }
    }
    if (
      path.forward.route !== null &&
      path.reverse.route !== null &&
      path.forward.route.via !== 'none' &&
      path.reverse.route.via !== 'none' &&
      !sameRoute(path.forward.route, path.reverse.route)
    ) {
      add({
        key: 'asymmetric',
        tone: 'info',
        text: d.find.asymmetric(
          `${legLabel(path.forward)} ${legRoute(path.forward, nodes)}`,
          `${legLabel(path.reverse)} ${legRoute(path.reverse, nodes)}`,
        ),
        section: 'route',
      })
    }
  }

  // 各条路的丢包、吞吐。
  for (const measured of measuredPaths(test)) {
    const { result } = measured
    if (result.state !== 'done') continue
    const label = pathLabel(measured, nodes)
    const latency = result.latency
    if (latency !== null && latency.probes > 0 && latency.answered === 0) {
      add({ key: `no_answer:${label}`, tone: 'danger', text: d.find.noAnswer(label), section: 'latency' })
    } else if (latency?.loss_permille !== null && latency?.loss_permille !== undefined && latency.loss_permille >= 10) {
      add({
        key: `loss:${label}`,
        tone: latency.loss_permille >= 50 ? 'danger' : 'warn',
        text: d.find.loss(label, permilleText(latency.loss_permille)),
        section: 'latency',
      })
    }
    const up = result.throughput?.up?.bits_per_second
    const down = result.throughput?.down?.bits_per_second
    if (up !== undefined && down !== undefined && up > 0 && down > 0 && Math.max(up, down) / Math.min(up, down) >= 3) {
      add({ key: `skew:${label}`, tone: 'info', text: d.find.throughputSkewed(label), section: 'bandwidth' })
    }
  }

  // 两种模式：延迟优先有更快的路，但差距不够换。
  for (const decision of [test?.route?.forward, test?.route?.reverse]) {
    const choice = decision?.lowest_cost
    if (decision === null || decision === undefined || choice?.reason !== 'within_margin') continue
    const cheaper = choice.cheaper
    if (cheaper === null || cheaper === undefined || choice.cost_us === null) continue
    add({
      key: `cheaper:${decision.from}`,
      tone: 'info',
      text: d.find.cheaper(
        t.path.forward(nameOf(nodes, decision.from), nameOf(nodes, decision.to)),
        routeName(cheaper.route, nodes),
        msText(cheaper.cost_us),
        msText(choice.cost_us),
      ),
      section: 'route',
    })
  }

  // 两端主机。
  for (const host of test?.hosts ?? []) {
    if (host.state !== 'done') continue
    const name = nameOf(nodes, host.node)
    const process = host.cpu_peak?.process_percent
    if (process !== null && process !== undefined && process >= 85) {
      add({ key: `cpu:${host.node}`, tone: 'warn', text: d.find.cpuBound(name, process), section: 'hosts' })
    }
    const system = host.cpu_peak?.system_percent
    if (system !== null && system !== undefined && system >= 90) {
      add({ key: `system:${host.node}`, tone: 'warn', text: d.find.systemBusy(name, system), section: 'hosts' })
    }
    const memory = host.memory
    if (memory !== null && memory.total_bytes > 0 && memory.available_bytes / memory.total_bytes < 0.1) {
      add({
        key: `memory:${host.node}`,
        tone: 'warn',
        text: d.find.lowMemory(name, formatBytes(memory.available_bytes)),
        section: 'hosts',
      })
    }
    const load = host.load_x100?.[0]
    const cores = host.system?.cores
    if (load !== undefined && cores !== null && cores !== undefined && cores > 0 && load / 100 > cores) {
      add({
        key: `load:${host.node}`,
        tone: 'warn',
        text: d.find.highLoad(name, (load / 100).toFixed(2), cores),
        section: 'hosts',
      })
    }
    const sent = host.to_peer?.sent_packets
    const lost = host.to_peer?.lost_packets
    if (
      sent !== null &&
      sent !== undefined &&
      lost !== null &&
      lost !== undefined &&
      sent >= 100 &&
      lost / sent >= 0.02
    ) {
      add({
        key: `conn_loss:${host.node}`,
        tone: 'warn',
        text: d.find.connectionLoss(name, permilleText(Math.round((lost / sent) * 1000))),
        section: 'hosts',
      })
    }
  }

  if (a !== undefined && b !== undefined && a.version !== '' && b.version !== '' && a.version !== b.version) {
    add({
      key: 'versions',
      tone: 'info',
      text: d.find.versions(a.name, a.version, b.name, b.version),
      section: 'hosts',
    })
  }
  for (const node of sides) {
    const age = node.reported_secs_ago
    if (node.online && age !== null && age !== undefined && age > 60) {
      add({
        key: `stale:${node.id}`,
        tone: 'info',
        text: d.find.staleReport(node.name, formatAge(age)),
        section: 'hosts',
      })
    }
  }

  if (test?.state === 'done' && !list.some((finding) => finding.tone === 'danger' || finding.tone === 'warn')) {
    add({ key: 'all_good', tone: 'ok', text: d.find.allGood, section: 'overview' })
  }
  return list.sort((x, y) => ORDER[x.tone] - ORDER[y.tone])
}
