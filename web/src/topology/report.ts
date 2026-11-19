import { type Choice, type DecisionView, type HostReport, type NodeDetail, type PathLeg } from '@/api/types'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { formatDuration, formatFullTime } from '@/lib/time'

import { checks, verdict } from './checks'
import { type DiagnosisInput, measuredPaths, type Named, pathLabel, routeName } from './diagnosis'
import { describeLeg } from './path'

/**
 * 路径诊断的完整输出：一份纯文本的报告（贴到聊天、工单里看得懂），和一份 JSON（两端的节点、两个方向现在的路、
 * 诊断的原始结果，原样）。纯函数；时刻由调用的地方给。
 */

const d = t.diagnosis
const r = d.report
const h = d.host

const nameOf = (nodes: Named, id: number) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)

function nodeLine(label: string, node: NodeDetail): string {
  const facts = [
    node.virtual_ip,
    node.version === '' ? null : `v${node.version.replace(/^v/, '')}`,
    node.platform === '' ? null : node.platform,
    node.online ? d.online : d.offline,
  ].filter((part): part is string => part !== null)
  return r.node(label, node.name, facts.join(t.common.listSeparator))
}

function legLine(leg: PathLeg, nodes: Named): string {
  const view = describeLeg(leg, (id) => nameOf(nodes, id))
  const route =
    view.route === null
      ? t.routes.noData
      : [view.route.text, view.route.detail, view.rtt].filter((part): part is string => part !== null).join(' · ')
  return r.line(t.path.forward(leg.from.name, leg.to.name), `${route}${r.paren(view.sourceLabel)}`)
}

function choiceLine(label: string, choice: Choice, nodes: Named): string {
  const reason = Object.hasOwn(d.reason, choice.reason)
    ? d.reason[choice.reason as keyof typeof d.reason]
    : choice.reason
  const cost = choice.cost_us === null ? '' : `${r.comma}${msText(choice.cost_us)}`
  return `${label} ${routeName(choice.route, nodes)}${r.paren(`${reason}${cost}`)}`
}

function decisionLines(decision: DecisionView, nodes: Named): string[] {
  return [
    r.line(
      t.path.forward(nameOf(nodes, decision.from), nameOf(nodes, decision.to)),
      d.modeNow(t.path.mode[decision.mode]),
    ),
    `  ${choiceLine(t.path.mode.direct_first, decision.direct_first, nodes)}`,
    `  ${choiceLine(t.path.mode.lowest_cost, decision.lowest_cost, nodes)}`,
  ]
}

const pct = (value: number | null | undefined) => (value === null || value === undefined ? '—' : `${value}%`)

function hostLines(host: HostReport, nodes: Named): string[] {
  const name = nameOf(nodes, host.node)
  if (host.state !== 'done') {
    const state = d.stepState[host.state]
    return [r.line(name, host.problem === null ? state : `${state}${r.comma}${problemText(host.problem, nodes)}`)]
  }
  const lines = [r.heading(name)]
  const push = (label: string, value: string | null) => {
    if (value !== null) lines.push(`  ${r.line(label, value)}`)
  }
  const system = host.system
  push(
    h.os,
    system === null
      ? null
      : [system.os, system.kernel, system.arch, system.cores === null ? null : h.cores(system.cores)]
          .filter((part): part is string => part !== null)
          .join(' · ') || null,
  )
  push(h.cpuModel, system?.cpu_model ?? null)
  push(h.cpu, host.cpu === null ? null : h.cpuValue(pct(host.cpu.system_percent), pct(host.cpu.process_percent)))
  push(
    h.cpuPeak,
    host.cpu_peak === null ? null : h.cpuValue(pct(host.cpu_peak.system_percent), pct(host.cpu_peak.process_percent)),
  )
  push(h.load, host.load_x100 === null ? null : host.load_x100.map((value) => (value / 100).toFixed(2)).join(' '))
  push(
    h.memory,
    host.memory === null
      ? null
      : h.memoryValue(formatBytes(host.memory.available_bytes), formatBytes(host.memory.total_bytes)),
  )
  push(h.rss, host.memory?.process_rss_bytes == null ? null : formatBytes(host.memory.process_rss_bytes))
  const connections = host.connections
  if (connections !== null) {
    push(h.peers, connections.peers === null ? null : String(connections.peers))
    push(h.relays, connections.relays === null ? null : String(connections.relays))
    push(h.relayClients, connections.relay_clients === null ? null : String(connections.relay_clients))
    push(h.streams, connections.streams === null ? null : String(connections.streams))
    push(
      h.lanes,
      connections.lanes_in_use === null || connections.lanes_idle === null
        ? null
        : h.lanesValue(connections.lanes_in_use, connections.lanes_idle),
    )
  }
  const peer = host.to_peer
  if (peer !== null) {
    push(h.route, peer.route === null ? null : routeName(peer.route, nodes))
    push(h.rtt, peer.rtt_us === null ? null : msText(peer.rtt_us))
    push(h.cwnd, peer.cwnd_bytes === null ? null : formatBytes(peer.cwnd_bytes))
    push(h.mtu, peer.mtu === null ? null : String(peer.mtu))
    push(h.congestion, peer.congestion === null ? null : peer.congestion.toUpperCase())
    if (peer.sent_packets !== null && peer.lost_packets !== null) {
      const loss = peer.sent_packets === 0 ? 0 : Math.round((peer.lost_packets / peer.sent_packets) * 1000)
      push(h.packets, h.packetsValue(String(peer.sent_packets), String(peer.lost_packets), permilleText(loss)))
    }
    push(h.congestionEvents, peer.congestion_events === null ? null : String(peer.congestion_events))
    push(h.connectionAge, peer.age_secs === null ? null : formatDuration(peer.age_secs))
  }
  return lines
}

export function reportText(input: DiagnosisInput, at: number): string {
  const { a, b, path, test, nodes } = input
  const lines: string[] = [r.title, r.generated(formatFullTime(at))]
  if (a !== undefined) lines.push(nodeLine(d.nodeA, a))
  if (b !== undefined) lines.push(nodeLine(d.nodeB, b))

  // 诊断结果：总评，然后逐项：结果、检查项、测得（参考）、说明。
  const results = checks(input)
  lines.push('', r.section(d.sections.result), verdict(results, input.test, nodes).title)
  for (const item of results) {
    const reference = item.reference === null ? '' : r.paren(`${d.colReference} ${item.reference}`)
    const note = item.note === null ? '' : ` — ${item.note}`
    lines.push(`- [${d.status[item.status]}] ${r.line(d.check[item.key], `${item.value}${reference}`)}${note}`)
  }
  // 建议：要注意、异常的项该怎么办，严重的在前。
  const rank = { fail: 0, warn: 1, info: 2, pass: 3, pending: 4, skip: 5 } as const
  const advice = results
    .filter((item) => item.advice !== null && item.status !== 'pass')
    .sort((x, y) => rank[x.status] - rank[y.status])
  if (advice.length > 0) {
    lines.push('', r.section(d.adviceTitle))
    advice.forEach((item, index) => lines.push(`${index + 1}. ${r.line(d.check[item.key], item.advice ?? '')}`))
  }

  if (path !== undefined) {
    lines.push('', r.section(r.now), legLine(path.forward, nodes), legLine(path.reverse, nodes))
  }

  if (test === undefined) {
    lines.push('', r.noTest)
    return lines.join('\n')
  }
  lines.push('', r.section(r.test(test.id, t.tests.state[test.state])), r.started(formatFullTime(test.started_at)))
  if (test.finished_at !== null) lines.push(t.tests.tookSecs(test.finished_at - test.started_at))
  if (test.error !== null) lines.push(d.find.testFailed(problemText(test.error, nodes)))

  const direct = test.direct
  if (direct !== null) {
    lines.push('', r.heading(r.direct))
    if (direct.state !== 'done') {
      lines.push(
        `  ${d.stepState[direct.state]}${direct.problem === null ? '' : `${r.comma}${problemText(direct.problem, nodes)}`}`,
      )
    } else {
      const how =
        direct.ok === true
          ? [
              direct.kind === null ? null : t.routes.directKind[direct.kind],
              direct.already === true
                ? t.tests.directAlready
                : direct.elapsed_ms === null
                  ? null
                  : t.tests.directIn(direct.elapsed_ms),
            ]
              .filter((part): part is string => part !== null)
              .join(' · ')
          : direct.problem === null
            ? ''
            : problemText(direct.problem, nodes)
      lines.push(`  ${direct.ok === true ? t.tests.directOk : t.tests.directNo}${how === '' ? '' : r.paren(how)}`)
      for (const side of direct.sides) {
        const facts = [
          side.udp === true ? t.tests.udpOk : side.udp === false ? t.tests.udpBlocked : t.tests.udpUnknown,
          side.mapping_varies === true ? t.tests.natVaries : null,
          side.candidates === null ? null : t.tests.candidates(side.candidates),
        ].filter((part): part is string => part !== null)
        lines.push(`  ${r.line(nameOf(nodes, side.node), facts.join(' · '))}`)
      }
    }
  }

  const paths = measuredPaths(test)
  if (paths.length > 0) {
    lines.push('', r.heading(r.paths))
    for (const measured of paths) {
      const { result } = measured
      const label = pathLabel(measured, nodes)
      if (result.state !== 'done') {
        lines.push(
          `  ${r.line(label, `${d.stepState[result.state]}${result.problem === null ? '' : `${r.comma}${problemText(result.problem, nodes)}`}`)}`,
        )
        continue
      }
      lines.push(`  ${r.heading(`${label}${result.relay_leg === 'tcp' ? ` · ${t.routes.legTransport.tcp}` : ''}`)}`)
      const latency = result.latency
      if (latency !== null) {
        lines.push(
          `    ${r.line(t.tests.latency, t.tests.latencyValue(msText(latency.min_us), msText(latency.avg_us), msText(latency.p95_us)))}${r.paren(t.tests.answered(latency.answered, latency.probes))}`,
          `    ${r.line(t.tests.jitter, msText(latency.jitter_us))}`,
          `    ${r.line(
            t.tests.loss,
            latency.loss_up_permille !== null && latency.loss_down_permille !== null
              ? `${permilleText(latency.loss_permille)}${r.paren(t.tests.lossSplit(permilleText(latency.loss_up_permille), permilleText(latency.loss_down_permille)))}`
              : permilleText(latency.loss_permille),
          )}`,
        )
      }
      if (result.cost_us !== null) lines.push(`    ${r.line(t.tests.cost, msText(result.cost_us))}`)
      if (result.hops.length > 1) {
        lines.push(
          `    ${r.line(
            t.tests.hops,
            result.hops
              .map(
                (hop) =>
                  `${nameOf(nodes, hop.from)} → ${nameOf(nodes, hop.to)} ${msText(hop.rtt_us)}${r.paren(t.tests.hopSource[hop.source])}`,
              )
              .join(r.semicolon),
          )}`,
        )
      }
      const throughput = result.throughput
      if (throughput !== null) {
        if (throughput.state === 'done') {
          const ms = Math.max(throughput.up?.ms ?? 0, throughput.down?.ms ?? 0)
          lines.push(
            `    ${r.line(
              t.tests.throughputTitle,
              `${t.tests.up} ${rateText(throughput.up?.bits_per_second)} · ${t.tests.down} ${rateText(throughput.down?.bits_per_second)}${r.paren(t.tests.throughputNote((throughput.congestion ?? '').toUpperCase(), ms))}`,
            )}`,
          )
        } else {
          lines.push(
            `    ${r.line(t.tests.throughputTitle, `${d.stepState[throughput.state]}${throughput.problem === null ? '' : `${r.comma}${problemText(throughput.problem, nodes)}`}`)}`,
          )
        }
      }
    }
  }

  const decisions = [test.route?.forward, test.route?.reverse].filter(
    (decision): decision is DecisionView => decision !== null && decision !== undefined,
  )
  if (decisions.length > 0) {
    lines.push('', r.heading(r.decisions))
    for (const decision of decisions) lines.push(...decisionLines(decision, nodes).map((line) => `  ${line}`))
  }

  const hosts = test.hosts ?? []
  if (hosts.length > 0) {
    lines.push('', r.heading(r.hosts))
    for (const host of hosts) lines.push(...hostLines(host, nodes).map((line) => `  ${line}`))
  }
  return lines.join('\n')
}

/** JSON：原样的数据，加上生成的时刻。 */
export function reportJson(input: DiagnosisInput, at: number): string {
  return JSON.stringify(
    {
      generated_at: new Date(at * 1000).toISOString(),
      a: input.a ?? null,
      b: input.b ?? null,
      path: input.path ?? null,
      test: input.test ?? null,
    },
    null,
    2,
  )
}
