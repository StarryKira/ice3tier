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

import { type Check, checks, verdict } from './checks'
import { type DiagnosisInput, fastest, measuredPaths, routeName, steps, widest } from './diagnosis'
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

describe('逐项检查', () => {
  const status = (list: Check[], key: Check['key']) => list.find((item) => item.key === key)?.status
  const value = (list: Check[], key: Check['key']) => list.find((item) => item.key === key)?.value

  it('都好：每一项都有结果，总评正常；没要主机数据的三项写为什么', () => {
    const list = checks(input())
    expect(status(list, 'connectivity')).toBe('pass')
    expect(value(list, 'connectivity')).toContain(t.path.forward('a', 'b'))
    expect(status(list, 'direct')).toBe('pass')
    expect(status(list, 'latency')).toBe('pass')
    expect(value(list, 'latency')).toBe(d.value.latency(t.common.ms(10), t.tests.qualityDirect, t.common.ms(12)))
    expect(status(list, 'loss')).toBe('pass')
    expect(status(list, 'throughput')).toBe('skip')
    expect(value(list, 'throughput')).toBe(d.note.noThroughput)
    expect(status(list, 'cpu')).toBe('skip')
    expect(value(list, 'cpu')).toBe(d.hostNotReported)
    expect(verdict(list, TEST)).toEqual({ tone: 'ok', title: d.verdict.ok })
  })

  it('还没诊断：现在就知道的照常有结果，要量的写"还没有诊断"，总评是还没有诊断', () => {
    const list = checks(input({ test: undefined }))
    expect(status(list, 'connectivity')).toBe('pass')
    expect(status(list, 'direct')).toBe('skip')
    expect(value(list, 'latency')).toBe(d.note.notTested)
    expect(verdict(list, undefined).tone).toBe('none')
  })

  it('不在线、不是对端：连通性异常，总评异常', () => {
    const offline = checks(input({ a: { ...A, online: false } }))
    expect(status(offline, 'connectivity')).toBe('fail')
    expect(value(offline, 'connectivity')).toBe(d.value.offline('a'))
    const notPeers = checks(
      input({ path: { ...PATH, forward: { ...PATH.forward, source: 'not_peers', route: null } } }),
    )
    expect(value(notPeers, 'connectivity')).toBe(d.value.notPeers)
    expect(verdict(notPeers, TEST)).toEqual({ tone: 'fail', title: d.verdict.fail(1) })
  })

  it('能直连但现在经中继：路由选择要注意；到中继走 TCP：传输要注意', () => {
    const list = checks(
      input({
        path: { ...PATH, reverse: { ...PATH.reverse, route: viaRelays(3), relay_leg: 'tcp', reason: '中继更近' } },
      }),
    )
    expect(status(list, 'routing')).toBe('warn')
    expect(value(list, 'routing')).toBe(d.value.directUnused(t.path.forward('b', 'a'), t.routes.via('r')))
    expect(list.find((item) => item.key === 'routing')?.note).toBe('中继更近')
    expect(status(list, 'transport')).toBe('warn')
    expect(value(list, 'transport')).toBe(d.value.tcp(t.path.forward('b', 'a')))
    expect(verdict(list, TEST)).toEqual({ tone: 'warn', title: d.verdict.warn(2) })
  })

  it('不能直连：直连要注意，写原因；UDP 不通、对称型 NAT：NAT 与 UDP 要注意', () => {
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
    const list = checks(input({ test }))
    expect(status(list, 'direct')).toBe('warn')
    expect(list.find((item) => item.key === 'direct')?.note).toBe(
      t.tests.problemOf('b', t.tests.problems.mapping_varies),
    )
    expect(status(list, 'nat')).toBe('warn')
    expect(value(list, 'nat')).toBe([d.value.udpBlocked('b'), d.value.symmetric('b')].join(' · '))
  })

  it('丢包到 1% 注意，到 5% 异常；抖动到 10 ms 注意', () => {
    const test = (loss: number, jitter = 500): TestView => ({
      ...TEST,
      direct_quality: { ...DIRECT_PATH, latency: { ...latency(10, loss), jitter_us: jitter } },
    })
    expect(status(checks(input({ test: test(10) })), 'loss')).toBe('warn')
    expect(status(checks(input({ test: test(60) })), 'loss')).toBe('fail')
    expect(status(checks(input({ test: test(0, 12_000) })), 'jitter')).toBe('warn')
  })

  it('两端主机：i3t 占满一个核、内存不够、到对端的连接丢包', () => {
    const test: TestView = {
      ...TEST,
      hosts: [
        host(1, {
          cpu_peak: { system_percent: 95, process_percent: 99 },
          memory: { total_bytes: 1000, available_bytes: 50, process_rss_bytes: null },
        }),
        host(2, {
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
    const list = checks(input({ test }))
    expect(status(list, 'cpu')).toBe('warn')
    expect(value(list, 'cpu')).toBe([d.value.side('a', '99%'), d.value.side('b', '30%')].join(' · '))
    expect(status(list, 'memory')).toBe('warn')
    // 计数是每一端发出的：b 发出的丢了 5%，是 b → a 这个方向不好。
    expect(status(list, 'connection')).toBe('warn')
    expect(value(list, 'connection')).toBe(d.value.sent('b', '5%'))
    expect(list.find((item) => item.key === 'connection')?.advice).toBe(
      d.advice.connection(t.path.forward('b', 'a'), '5%'),
    )
    // 要注意、异常的项都有建议。
    expect(list.find((item) => item.key === 'cpu')?.advice).toBe(d.advice.cpu('a'))
  })

  it('主机数据：旧版本的节点不报、诊断还在跑，各写各的', () => {
    const old = checks(
      input({
        test: {
          ...TEST,
          hosts: [
            host(1, { state: 'skipped', problem: { code: 'not_supported', node: 1, message: '' } }),
            host(2, { state: 'skipped', problem: { code: 'not_supported', node: 2, message: '' } }),
          ],
        },
      }),
    )
    expect(value(old, 'cpu')).toBe(d.hostTooOld)
    const running = checks(input({ test: { ...TEST, hosts: [host(1, { state: 'pending' }), host(2)] } }))
    expect(status(running, 'memory')).not.toBe('skip')
  })

  it('版本不同：提示；上报过时：注意', () => {
    const list = checks(
      input({ a: { ...A, version: '0.1.22' }, b: { ...B, version: '0.1.23', reported_secs_ago: 300 } }),
    )
    expect(status(list, 'version')).toBe('info')
    expect(value(list, 'version')).toBe(d.value.versionDiffer('a', '0.1.22', 'b', '0.1.23'))
    expect(status(list, 'report')).toBe('warn')
  })
})

describe('测了的路和进度', () => {
  it('最低延迟取平均最低的那条；吞吐取上下行里慢的那个最高的', () => {
    const paths = measuredPaths(TEST)
    expect(fastest(paths)?.direct).toBe(true)
    expect(widest(paths)).toBeNull()
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
    expect(text).toContain(d.verdict.ok)
    expect(text).toContain(t.path.forward('a', 'b'))
    expect(text).toContain(d.report.test(7, t.tests.state.done))
    expect(text).toContain(t.routes.via('r'))
    expect(text).toContain(d.host.cpuValue('20%', '30%'))
  })

  it('有要注意的项时报告里有建议', () => {
    const text = reportText(
      input({ path: { ...PATH, reverse: { ...PATH.reverse, route: viaRelays(3), relay_leg: 'tcp' } } }),
      1_794_000_100,
    )
    expect(text).toContain(d.adviceTitle)
    expect(text).toContain(d.advice.tcp('b'))
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
