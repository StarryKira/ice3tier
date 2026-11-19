import { type HostReport, type NodeDetail, type PathLeg, type TestView } from '@/api/types'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { formatAge } from '@/lib/time'

import { type DiagnosisInput, fastest, measuredPaths, type Named, pathLabel, routeName, widest } from './diagnosis'

/**
 * 诊断报告的逐项检查：一项一行，结果（通过、注意、异常、提示、未测、进行中）、测得的数、参考的范围、一句说明。
 * 量不到的项也列出来，写为什么没测，报告里不留空白。纯函数。
 *
 * 门槛按常见的经验：抖动 10 ms、丢包 1%（5% 算异常）、i3t 进程最忙时 85% 一个核、可用内存 10%、
 * 到对端的连接丢包 2%、节点上报 60 秒。
 */

const d = t.diagnosis
const v = d.value

export type CheckStatus = 'pass' | 'warn' | 'fail' | 'info' | 'skip' | 'pending'

export type CheckKey = keyof typeof d.check

/** 报告里讲这一项的那一节，点了跳过去；没有就是 `null`。 */
export type ReportSection = 'path' | 'latency' | 'throughput' | 'routing' | 'hosts'

/** 检查分三组：连通与路由、链路质量、两端主机。 */
export type CheckGroup = keyof typeof d.groups

export interface Check {
  key: CheckKey
  group: CheckGroup
  status: CheckStatus
  value: string
  reference: string | null
  note: string | null
  /** 要注意、异常时该怎么办，一句话；没有就是 `null`。 */
  advice: string | null
  section: ReportSection | null
}

const GROUP: Record<CheckKey, CheckGroup> = {
  connectivity: 'link',
  direct: 'link',
  routing: 'link',
  transport: 'link',
  nat: 'link',
  latency: 'quality',
  jitter: 'quality',
  loss: 'quality',
  throughput: 'quality',
  connection: 'quality',
  cpu: 'host',
  memory: 'host',
  version: 'host',
  report: 'host',
}

const a_ = d.advice

const nameOf = (nodes: Named, id: number) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)
const legLabel = (leg: PathLeg) => t.path.forward(leg.from.name, leg.to.name)
const join = (parts: readonly string[]) => parts.join(' · ')

function check(
  key: CheckKey,
  status: CheckStatus,
  value: string,
  extra: {
    reference?: string | null
    note?: string | null
    advice?: string | null
    section?: ReportSection | null
  } = {},
): Check {
  return {
    key,
    group: GROUP[key],
    status,
    value,
    reference: extra.reference ?? null,
    note: extra.note ?? null,
    advice: extra.advice ?? null,
    section: extra.section ?? null,
  }
}

/** 主机快照没有时为什么：还没诊断、controller 太旧、在测、节点太旧、没报。 */
function hostMissing(test: TestView | undefined, nodes: Named): { status: CheckStatus; value: string } {
  if (test === undefined) return { status: 'skip', value: d.note.notTested }
  const hosts = test.hosts ?? null
  if (hosts === null) return { status: 'skip', value: d.hostNotReported }
  if (hosts.some((host) => host.state === 'pending')) return { status: 'pending', value: t.tests.pending }
  if (hosts.every((host) => host.problem?.code === 'not_supported')) return { status: 'skip', value: d.hostTooOld }
  const problem = hosts.find((host) => host.problem !== null)?.problem ?? null
  return { status: 'skip', value: problem === null ? d.hostNotReported : problemText(problem, nodes) }
}

function connectivity({ a, b, path, nodes }: DiagnosisInput): Check {
  const offline = [a, b].filter((node): node is NodeDetail => node !== undefined && !node.online)
  if (offline.length > 0) {
    return check('connectivity', 'fail', join(offline.map((node) => v.offline(node.name))), {
      note: t.path.offline(offline[0]?.name ?? ''),
      advice: a_.offline(offline[0]?.name ?? ''),
      section: 'path',
    })
  }
  if (path === undefined) return check('connectivity', 'pending', t.path.querying, { section: 'path' })
  const legs = [path.forward, path.reverse]
  if (legs.some((leg) => leg.source === 'not_peers')) {
    return check('connectivity', 'fail', v.notPeers, {
      note: t.path.notPeersTitle,
      advice: a_.notPeers,
      section: 'path',
    })
  }
  const broken = legs.filter((leg) => leg.route === null || leg.route.via === 'none')
  if (broken.length > 0) {
    return check('connectivity', 'fail', join(broken.map((leg) => v.noRoute(legLabel(leg)))), {
      advice: a_.noRoute,
      section: 'path',
    })
  }
  return check(
    'connectivity',
    'pass',
    join(
      legs.map((leg) =>
        v.leg(legLabel(leg), routeName(leg.route, nodes), leg.rtt_ms === null ? '' : t.common.ms(leg.rtt_ms)).trim(),
      ),
    ),
    { section: 'path' },
  )
}

function direct({ test, nodes }: DiagnosisInput): Check {
  const result = test?.direct ?? null
  if (test === undefined || result === null) return check('direct', 'skip', d.note.notTested)
  if (result.state === 'pending') return check('direct', 'pending', t.tests.pending)
  if (result.state !== 'done') {
    return check('direct', 'skip', result.problem === null ? t.tests.skipped : problemText(result.problem, nodes))
  }
  if (result.ok !== true) {
    return check('direct', 'warn', t.tests.directNo, {
      note: result.problem === null ? d.note.directNo : problemText(result.problem, nodes),
      advice: a_.direct,
    })
  }
  const how = [
    result.kind === null ? null : t.routes.directKind[result.kind],
    result.already === true
      ? t.tests.directAlready
      : result.elapsed_ms === null
        ? null
        : t.tests.directIn(result.elapsed_ms),
  ].filter((part): part is string => part !== null)
  return check('direct', 'pass', join([t.tests.directOk, ...how]))
}

function routing({ path, test, nodes }: DiagnosisInput): Check {
  if (path === undefined) return check('routing', 'pending', t.path.querying, { section: 'routing' })
  if (test?.direct?.state === 'done' && test.direct.ok === true) {
    const unused = [path.forward, path.reverse].filter((leg) => leg.source === 'query' && leg.route?.via === 'relay')
    if (unused.length > 0) {
      return check(
        'routing',
        'warn',
        join(unused.map((leg) => v.directUnused(legLabel(leg), routeName(leg.route, nodes)))),
        {
          note: unused.map((leg) => leg.reason ?? leg.since?.reason ?? null).find((reason) => reason !== null) ?? null,
          advice: a_.directUnused,
          section: 'routing',
        },
      )
    }
  }
  for (const decision of [test?.route?.forward, test?.route?.reverse]) {
    const choice = decision?.lowest_cost
    if (decision === null || decision === undefined || choice?.reason !== 'within_margin') continue
    if (choice.cheaper === null || choice.cheaper === undefined || choice.cost_us === null) continue
    return check(
      'routing',
      'info',
      d.find.cheaper(
        t.path.forward(nameOf(nodes, decision.from), nameOf(nodes, decision.to)),
        routeName(choice.cheaper.route, nodes),
        msText(choice.cheaper.cost_us),
        msText(choice.cost_us),
      ),
      { section: 'routing' },
    )
  }
  return check('routing', 'pass', v.routingOk, { section: 'routing' })
}

function quality({ test, nodes }: DiagnosisInput): Check[] {
  const paths = measuredPaths(test)
  const done = paths.filter(({ result }) => result.state === 'done' && result.latency !== null)
  if (test === undefined || paths.length === 0) {
    return (['latency', 'jitter', 'loss'] as const).map((key) => check(key, 'skip', d.note.notTested))
  }
  if (done.length === 0) {
    const status: CheckStatus = paths.some(({ result }) => result.state === 'pending') ? 'pending' : 'skip'
    const value = status === 'pending' ? t.tests.pending : t.tests.failed
    return (['latency', 'jitter', 'loss'] as const).map((key) => check(key, status, value, { section: 'latency' }))
  }
  const best = fastest(done)
  const latency = best?.result.latency ?? null
  const label = best === null ? '' : pathLabel(best, nodes)
  const jitter = latency?.jitter_us ?? null
  const losses = done.map((path) => ({
    label: pathLabel(path, nodes),
    loss: path.result.latency?.loss_permille ?? null,
  }))
  const worst = Math.max(...losses.map(({ loss }) => loss ?? 0))
  return [
    check('latency', 'pass', v.latency(msText(latency?.avg_us), label, msText(latency?.p95_us)), {
      section: 'latency',
    }),
    check(
      'jitter',
      jitter !== null && jitter >= 30_000 ? 'fail' : jitter !== null && jitter >= 10_000 ? 'warn' : 'pass',
      v.best(msText(jitter), label),
      {
        reference: d.reference.jitter,
        note: jitter !== null && jitter >= 10_000 ? d.note.jitter : null,
        advice: jitter !== null && jitter >= 10_000 ? a_.jitter : null,
        section: 'latency',
      },
    ),
    check(
      'loss',
      worst >= 50 ? 'fail' : worst >= 10 ? 'warn' : 'pass',
      join(losses.map(({ label: path, loss }) => v.side(path, permilleText(loss)))),
      {
        reference: d.reference.loss,
        note: worst >= 10 ? d.note.loss : null,
        advice: worst >= 10 ? a_.loss : null,
        section: 'latency',
      },
    ),
  ]
}

function throughput({ test, nodes }: DiagnosisInput): Check {
  if (test === undefined) return check('throughput', 'skip', d.note.notTested)
  if (!test.request.throughput) return check('throughput', 'skip', d.note.noThroughput)
  const paths = measuredPaths(test)
  const wide = widest(paths)
  if (wide?.result.throughput !== null && wide?.result.throughput !== undefined) {
    return check(
      'throughput',
      'pass',
      v.throughput(
        rateText(wide.result.throughput.up?.bits_per_second),
        rateText(wide.result.throughput.down?.bits_per_second),
        pathLabel(wide, nodes),
      ),
      { section: 'throughput' },
    )
  }
  const states = paths.map(({ result }) => result.throughput?.state ?? result.state)
  if (states.includes('pending')) return check('throughput', 'pending', t.tests.pending, { section: 'throughput' })
  const problem = paths.find(({ result }) => result.throughput?.problem != null)?.result.throughput?.problem ?? null
  return check('throughput', 'warn', problem === null ? t.tests.failed : problemText(problem, nodes))
}

function transport({ path }: DiagnosisInput): Check {
  if (path === undefined) return check('transport', 'pending', t.path.querying, { section: 'path' })
  const relayed = [path.forward, path.reverse].filter((leg) => leg.route?.via === 'relay')
  if (relayed.length === 0) return check('transport', 'pass', v.noRelay, { section: 'path' })
  const tcp = relayed.filter((leg) => leg.relay_leg === 'tcp')
  if (tcp.length > 0) {
    return check('transport', 'warn', join(tcp.map((leg) => v.tcp(legLabel(leg)))), {
      note: d.note.tcp,
      advice: a_.tcp(tcp[0]?.from.name ?? ''),
      section: 'path',
    })
  }
  return check('transport', 'pass', v.udp, { section: 'path' })
}

function nat({ a, b, test, nodes }: DiagnosisInput): Check {
  const problems: string[] = []
  let advice: string | null = null
  let known = false
  for (const node of [a, b]) {
    if (node === undefined) continue
    const side = test?.direct?.sides.find((candidate) => candidate.node === node.id)
    const udp = side?.udp ?? (node.no_udp === true ? false : node.online ? true : null)
    const varies = side?.mapping_varies ?? node.mapping_varies
    if (udp !== null || varies !== null) known = true
    const name = nameOf(nodes, node.id)
    if (udp === false) {
      problems.push(v.udpBlocked(name))
      advice ??= a_.udp(name)
    }
    if (varies === true) {
      problems.push(v.symmetric(name))
      advice ??= a_.nat(name)
    }
  }
  if (problems.length > 0) return check('nat', 'warn', join(problems), { advice, section: 'hosts' })
  return known ? check('nat', 'pass', v.natOk, { section: 'hosts' }) : check('nat', 'skip', d.note.notTested)
}

function hostChecks({ test, nodes }: DiagnosisInput): Check[] {
  const hosts = (test?.hosts ?? []).filter((host): host is HostReport => host.state === 'done')
  if (hosts.length === 0) {
    const missing = hostMissing(test, nodes)
    return (['cpu', 'memory', 'connection'] as const).map((key) => check(key, missing.status, missing.value))
  }
  const name = (host: HostReport | undefined) => (host === undefined ? '' : nameOf(nodes, host.node))
  const peaks = hosts.flatMap((host) =>
    host.cpu_peak?.process_percent == null ? [] : [{ host, percent: host.cpu_peak.process_percent }],
  )
  const busy = peaks.some(({ percent }) => percent >= 85)
  const memories = hosts.flatMap((host) => (host.memory === null ? [] : [{ host, memory: host.memory }]))
  const low = memories.some(({ memory }) => memory.total_bytes > 0 && memory.available_bytes / memory.total_bytes < 0.1)
  const links = hosts.flatMap((host) => {
    const sent = host.to_peer?.sent_packets ?? null
    const lost = host.to_peer?.lost_packets ?? null
    if (sent === null || lost === null || sent === 0) return []
    return [{ host, loss: Math.round((lost / sent) * 1000) }]
  })
  // 计数是每一端、每个方向的：哪一端的丢包高，说明从它发出的那个方向不好。
  const lossy = links.some(({ loss }) => loss >= 20)
  // 丢得最多的那一端：从它发出去的方向。
  const worst = links.reduce<(typeof links)[number] | undefined>(
    (top, link) => (top === undefined || link.loss > top.loss ? link : top),
    undefined,
  )
  const peerOf = (id: number) => (test?.a.id === id ? test.b.name : test?.b.id === id ? test.a.name : '')
  const worstLink =
    worst === undefined
      ? undefined
      : { loss: worst.loss, direction: t.path.forward(name(worst.host), peerOf(worst.host.node)) }
  const severe = links.some(({ loss }) => loss >= 100)
  return [
    peaks.length === 0
      ? check('cpu', 'skip', d.hostNotReported)
      : check(
          'cpu',
          busy ? 'warn' : 'pass',
          join(peaks.map(({ host, percent }) => v.side(name(host), `${percent}%`))),
          {
            reference: d.reference.cpu,
            note: busy ? d.note.cpu : null,
            advice: busy ? a_.cpu(name(peaks.find(({ percent }) => percent >= 85)?.host ?? hosts[0])) : null,
            section: 'hosts',
          },
        ),
    memories.length === 0
      ? check('memory', 'skip', d.hostNotReported)
      : check(
          'memory',
          low ? 'warn' : 'pass',
          join(
            memories.map(({ host, memory }) =>
              v.memory(name(host), formatBytes(memory.available_bytes), formatBytes(memory.total_bytes)),
            ),
          ),
          {
            reference: d.reference.memory,
            note: low ? d.note.memory : null,
            advice: low
              ? a_.memory(
                  name(
                    memories.find(({ memory }) => memory.available_bytes / memory.total_bytes < 0.1)?.host ?? hosts[0],
                  ),
                )
              : null,
            section: 'hosts',
          },
        ),
    links.length === 0
      ? check('connection', 'skip', d.hostNotReported)
      : check(
          'connection',
          severe ? 'fail' : lossy ? 'warn' : 'pass',
          join(links.map(({ host, loss }) => v.sent(name(host), permilleText(loss)))),
          {
            reference: d.reference.connection,
            note: lossy ? d.note.connection : null,
            advice:
              worstLink === undefined || !lossy
                ? null
                : a_.connection(worstLink.direction, permilleText(worstLink.loss)),
            section: 'hosts',
          },
        ),
  ]
}

function version({ a, b }: DiagnosisInput): Check {
  if (a === undefined || b === undefined || a.version === '' || b.version === '') {
    return check('version', 'skip', t.path.querying)
  }
  if (a.version === b.version) return check('version', 'pass', v.versionSame(a.version), { section: 'hosts' })
  return check('version', 'info', v.versionDiffer(a.name, a.version, b.name, b.version), {
    note: d.note.version,
    advice: a_.version,
    section: 'hosts',
  })
}

function report({ a, b }: DiagnosisInput): Check {
  const ages = [a, b].flatMap((node) =>
    node === undefined || !node.online || node.reported_secs_ago === null || node.reported_secs_ago === undefined
      ? []
      : [{ name: node.name, age: node.reported_secs_ago }],
  )
  if (ages.length === 0) return check('report', 'skip', d.note.notTested)
  const slowest = ages.reduce((worst, item) => (item.age > worst.age ? item : worst))
  const stale = slowest.age > 60
  return check('report', stale ? 'warn' : 'pass', v.reported(formatAge(slowest.age)), {
    reference: d.reference.report,
    note: stale ? d.note.stale : null,
    advice: stale ? a_.stale(slowest.name) : null,
    section: 'hosts',
  })
}

/** 全部检查，按报告里的顺序：先连没连上、怎么连的，再质量，再两端主机。 */
export function checks(input: DiagnosisInput): Check[] {
  return [
    connectivity(input),
    direct(input),
    routing(input),
    ...quality(input),
    throughput(input),
    transport(input),
    nat(input),
    ...hostChecks(input),
    version(input),
    report(input),
  ]
}

export type VerdictTone = 'ok' | 'warn' | 'fail' | 'pending' | 'none'

/**
 * 总评：整个诊断没做成就写原因；有异常就是异常，有要注意的就是注意；诊断在跑是进行中；
 * 还没诊断时只看现在就知道的（连通性这几项）。
 */
export function verdict(
  list: readonly Check[],
  test: TestView | undefined,
  nodes: Named = [],
): { tone: VerdictTone; title: string } {
  if (test?.error !== null && test?.error !== undefined) {
    return { tone: 'fail', title: d.find.testFailed(problemText(test.error, nodes)) }
  }
  const fails = list.filter((item) => item.status === 'fail').length
  const warns = list.filter((item) => item.status === 'warn').length
  if (fails > 0) return { tone: 'fail', title: d.verdict.fail(fails) }
  if (test?.state === 'running') return { tone: 'pending', title: d.verdict.pending }
  if (test === undefined) return { tone: 'none', title: d.verdict.none }
  if (warns > 0) return { tone: 'warn', title: d.verdict.warn(warns) }
  return { tone: 'ok', title: d.verdict.ok }
}
