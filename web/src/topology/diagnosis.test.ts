import { describe, expect, it } from 'vitest'

import {
  type DirectResult,
  type HostReport,
  type NodeDetail,
  type PathResult,
  type PathView,
  type TestView,
} from '@/api/types'
import { t } from '@/i18n'
import { direct, makeNode, viaRelays } from '@/test/fixtures'

import { type DiagnosisInput, fastest, findings, kpis, measuredPaths, routeName, steps, widest } from './diagnosis'
import { reportJson, reportText } from './report'

const d = t.diagnosis

function detail(id: number, name: string, extra: Partial<NodeDetail> = {}): NodeDetail {
  return {
    ...makeNode(id, name),
    endpoints: [],
    observed_endpoint: null,
    mapping_varies: false,
    flows: 0,
    tx_bytes: 0,
    rx_bytes: 0,
    reported_secs_ago: 3,
    ...extra,
  }
}

const A = detail(1, 'a')
const B = detail(2, 'b')
const NODES = [A, B, detail(3, 'r')]

const ref = (node: NodeDetail) => ({ id: node.id, name: node.name, virtual_ip: node.virtual_ip })

const PATH: PathView = {
  forward: {
    from: ref(A),
    to: ref(B),
    source: 'query',
    route: direct('203.0.113.2:1'),
    rtt_ms: 10,
    age_secs: 0,
    since: null,
  },
  reverse: {
    from: ref(B),
    to: ref(A),
    source: 'query',
    route: direct('203.0.113.1:1'),
    rtt_ms: 10,
    age_secs: 0,
    since: null,
  },
}

const latency = (avgMs: number, loss = 0) => ({
  probes: 100,
  answered: 100,
  min_us: (avgMs - 1) * 1000,
  avg_us: avgMs * 1000,
  p95_us: (avgMs + 2) * 1000,
  jitter_us: 500,
  loss_permille: loss,
  loss_up_permille: null,
  loss_down_permille: null,
  same_path: 100,
  interval_ms: 20,
  duration_ms: 2000,
})

const DIRECT: DirectResult = {
  state: 'done',
  ok: true,
  kind: 'lan',
  elapsed_ms: null,
  already: true,
  problem: null,
  sides: [],
}

const DIRECT_PATH: PathResult = {
  state: 'done',
  route: direct('203.0.113.2:1'),
  picked_by: ['direct'],
  problem: null,
  latency: latency(10),
  cost_us: 12_000,
  hops: [],
  relay_leg: null,
  throughput: null,
}

const RELAY_PATH: PathResult = {
  state: 'done',
  route: viaRelays(3),
  picked_by: ['direct_first'],
  problem: null,
  latency: latency(30),
  cost_us: 33_000,
  hops: [],
  relay_leg: 'udp',
  throughput: null,
}

const TEST: TestView = {
  id: 7,
  network: 1,
  a: ref(A),
  b: ref(B),
  state: 'done',
  request: { items: ['direct', 'direct_quality', 'relay_quality', 'route'], throughput: false, via: [] },
  started_at: 1_794_000_000,
  finished_at: 1_794_000_012,
  matrix: null,
  error: null,
  direct: DIRECT,
  direct_quality: DIRECT_PATH,
  relay_quality: [RELAY_PATH],
  route: null,
  notes: [],
}

const input = (extra: Partial<DiagnosisInput> = {}): DiagnosisInput => ({
  a: A,
  b: B,
  path: PATH,
  test: TEST,
  nodes: NODES,
  ...extra,
})

function host(node: number, extra: Partial<HostReport> = {}): HostReport {
  return {
    node,
    state: 'done',
    problem: null,
    system: { os: 'Linux', kernel: null, arch: 'x86_64', cpu_model: null, cores: 2, uptime_secs: null },
    cpu: { system_percent: 5, process_percent: 1 },
    cpu_peak: { system_percent: 20, process_percent: 30 },
    load_x100: [10, 10, 10],
    memory: { total_bytes: 1000, available_bytes: 500, process_rss_bytes: 10 },
    process: null,
    connections: null,
    to_peer: null,
    ...extra,
  }
}

const texts = (list: { text: string }[]) => list.map((item) => item.text)

describe('诊断的结论', () => {
  it('都好：只有能直连和"没有发现问题"', () => {
    expect(texts(findings(input()))).toEqual([
      d.find.directOk(`${t.routes.directKind.lan} · ${t.tests.directAlready}`),
      d.find.allGood,
    ])
  })

  it('还没诊断：只说现在知道的，不说"没有发现问题"', () => {
    expect(findings(input({ test: undefined }))).toEqual([])
  })

  it('严重的在前：不在线、不是对端', () => {
    const list = findings(
      input({
        a: { ...A, online: false },
        path: { ...PATH, forward: { ...PATH.forward, source: 'not_peers', route: null } },
      }),
    )
    expect(list[0]).toMatchObject({ tone: 'danger', text: d.find.offline('a') })
    expect(list[1]).toMatchObject({ tone: 'danger', text: d.find.notPeers })
    expect(texts(list)).not.toContain(d.find.allGood)
  })

  it('能直连但现在经中继：注意；到中继走 TCP：注意；两个方向不一样：提示', () => {
    const list = findings(
      input({
        path: { ...PATH, reverse: { ...PATH.reverse, route: viaRelays(3), relay_leg: 'tcp' } },
      }),
    )
    expect(texts(list)).toContain(d.find.directUnused(t.path.forward('b', 'a'), t.routes.via('r')))
    expect(texts(list)).toContain(d.find.tcpLeg(t.path.forward('b', 'a')))
    expect(texts(list)).toContain(
      d.find.asymmetric(
        `${t.path.forward('a', 'b')} ${t.routes.direct}`,
        `${t.path.forward('b', 'a')} ${t.routes.via('r')}`,
      ),
    )
  })

  it('同一条路两个方向倒过来写是一样的', () => {
    const list = findings(
      input({
        path: {
          forward: { ...PATH.forward, route: viaRelays(3, 4) },
          reverse: { ...PATH.reverse, route: viaRelays(4, 3) },
        },
      }),
    )
    expect(list.some((finding) => finding.key === 'asymmetric')).toBe(false)
  })

  it('UDP 不通、对称型 NAT 两个来源都说了只写一次', () => {
    const test: TestView = {
      ...TEST,
      direct: {
        ...DIRECT,
        ok: false,
        kind: null,
        problem: { code: 'mapping_varies', node: 2, message: '' },
        sides: [
          {
            node: 2,
            ok: false,
            already: false,
            kind: null,
            elapsed_ms: null,
            failure: null,
            udp: false,
            mapping_varies: true,
            candidates: 1,
          },
        ],
      },
    }
    const list = findings(input({ test, b: { ...B, mapping_varies: true } }))
    expect(list.filter((finding) => finding.key === 'nat:2')).toHaveLength(1)
    expect(texts(list)).toContain(d.find.udpBlocked('b'))
    expect(texts(list)).toContain(d.find.cannotDirect(t.tests.problemOf('b', t.tests.problems.mapping_varies)))
  })

  it('丢包到 1% 注意，到 5% 严重；一个都没回答是严重', () => {
    const test = (loss: number, answered = 100): TestView => ({
      ...TEST,
      direct_quality: { ...DIRECT_PATH, latency: { ...latency(10, loss), answered } },
    })
    expect(findings(input({ test: test(10) })).find((finding) => finding.key.startsWith('loss'))?.tone).toBe('warn')
    expect(findings(input({ test: test(60) })).find((finding) => finding.key.startsWith('loss'))?.tone).toBe('danger')
    expect(texts(findings(input({ test: test(1000, 0) })))).toContain(d.find.noAnswer(t.tests.qualityDirect))
  })

  it('两端主机：i3t 占满一个核、内存不够、负载超过核数、到对端的连接丢包', () => {
    const test: TestView = {
      ...TEST,
      hosts: [
        host(1, {
          cpu_peak: { system_percent: 95, process_percent: 99 },
          memory: { total_bytes: 1000, available_bytes: 50, process_rss_bytes: null },
        }),
        host(2, {
          load_x100: [450, 300, 200],
          to_peer: {
            route: direct('203.0.113.1:1'),
            rtt_us: 10_000,
            cwnd_bytes: 1,
            mtu: 1420,
            congestion: 'bbr',
            sent_packets: 1000,
            lost_packets: 50,
            lost_bytes: 0,
            congestion_events: 0,
            bytes_sent: 0,
            bytes_received: 0,
            age_secs: 1,
          },
        }),
      ],
    }
    const list = texts(findings(input({ test })))
    expect(list).toContain(d.find.cpuBound('a', 99))
    expect(list).toContain(d.find.systemBusy('a', 95))
    expect(list).toContain(d.find.highLoad('b', '4.50', 2))
    expect(list).toContain(d.find.connectionLoss('b', '5%'))
    expect(list.some((text) => text.startsWith(d.find.lowMemory('a', '').slice(0, 4)))).toBe(true)
  })

  it('版本不同、上报过时：提示', () => {
    const list = texts(
      findings(input({ a: { ...A, version: '0.1.22' }, b: { ...B, version: '0.1.23', reported_secs_ago: 300 } })),
    )
    expect(list).toContain(d.find.versions('a', '0.1.22', 'b', '0.1.23'))
    expect(list).toContain(d.find.staleReport('b', t.time.minutesAgo(5)))
  })
})

describe('指标和进度', () => {
  it('最低延迟取平均最低的那条；吞吐取上下行里慢的那个最高的', () => {
    const paths = measuredPaths(TEST)
    expect(fastest(paths)?.direct).toBe(true)
    expect(widest(paths)).toBeNull()
  })

  it('没诊断时每个指标都是一道横线、没有说明', () => {
    for (const kpi of kpis(input({ test: undefined }))) {
      expect(kpi.values).toEqual([])
      expect(kpi.sub).toBeNull()
    }
  })

  it('CPU 峰值两端各一行，超过 85% 的那一个标出来', () => {
    const cpu = kpis(
      input({
        test: { ...TEST, hosts: [host(1, { cpu_peak: { system_percent: 50, process_percent: 90 } }), host(2)] },
      }),
    ).find((kpi) => kpi.key === 'cpu')
    expect(cpu?.values).toEqual([
      { text: d.cpuSide('a', 90), warn: true },
      { text: d.cpuSide('b', 30), warn: false },
    ])
  })

  it('进度：只列要了的项；几条中继路径合成一项，还有在测的就是在测', () => {
    const running: TestView = {
      ...TEST,
      state: 'running',
      relay_quality: [RELAY_PATH, { ...RELAY_PATH, state: 'pending' }],
      hosts: [host(1), host(2, { state: 'pending' })],
    }
    expect(steps(running)).toEqual([
      { key: 'direct', state: 'done' },
      { key: 'direct_quality', state: 'done' },
      { key: 'relay_quality', state: 'pending' },
      { key: 'host', state: 'pending' },
    ])
  })

  it('路的说法和拓扑一样：直连、经 r', () => {
    expect(routeName(direct('1.2.3.4:5'), NODES)).toBe(t.routes.direct)
    expect(routeName(viaRelays(3), NODES)).toBe(t.routes.via('r'))
    expect(routeName(null, NODES)).toBe(t.tests.noRoute)
  })
})

describe('完整输出', () => {
  it('文本报告：两端、结论、现在怎么走、这次诊断的每一项', () => {
    const text = reportText(input({ test: { ...TEST, hosts: [host(1), host(2)] } }), 1_794_000_100)
    expect(text).toContain(d.report.title)
    expect(text).toContain(d.find.allGood)
    expect(text).toContain(t.path.forward('a', 'b'))
    expect(text).toContain(d.report.test(7, t.tests.state.done))
    expect(text).toContain(t.routes.via('r'))
    expect(text).toContain(d.host.cpuValue('20%', '30%'))
  })

  it('没诊断过：报告里照实说', () => {
    expect(reportText(input({ test: undefined }), 1_794_000_100)).toContain(d.report.noTest)
  })

  it('JSON 原样带上两端、路和诊断', () => {
    const json = JSON.parse(reportJson(input(), 1_794_000_100)) as Record<string, unknown>
    expect(Object.keys(json)).toEqual(['generated_at', 'a', 'b', 'path', 'test'])
    expect((json.test as TestView).id).toBe(7)
  })
})
