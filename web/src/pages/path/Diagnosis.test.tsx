import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type HostReport, type NodeRef, type PathView, type TestView } from '@/api/types'
import { t } from '@/i18n'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { direct, makeNode, makeOverview } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { PathPage } from './PathPage'

const s = t.tests
const d = t.diagnosis
const ref = (id: number, name: string): NodeRef => ({ id, name, virtual_ip: `10.66.0.${id}` })
const laptop = makeNode(2, 'laptop', { online: true })
const nas = makeNode(5, 'nas', { online: true })
const hk = makeNode(3, 'hk', { online: true })

const latency = (avgMs: number) => ({
  probes: 100,
  answered: 97,
  min_us: (avgMs - 3) * 1000,
  avg_us: avgMs * 1000,
  p95_us: (avgMs + 5) * 1000,
  jitter_us: 1900,
  loss_permille: 30,
  loss_up_permille: 10,
  loss_down_permille: 20,
  same_path: 97,
  interval_ms: 20,
  duration_ms: 4010,
})

/** 文档 §19 里的那个例子：打洞成功的直连、经 hk 的中继、两种模式的选择。 */
const DONE: TestView = {
  id: 12,
  network: 1,
  a: ref(2, 'laptop'),
  b: ref(5, 'nas'),
  state: 'done',
  request: { items: ['direct', 'direct_quality', 'relay_quality', 'route'], throughput: true, via: [] },
  started_at: 1_794_798_564,
  finished_at: 1_794_798_611,
  matrix: null,
  error: null,
  direct: {
    state: 'done',
    ok: true,
    kind: 'punched',
    elapsed_ms: 640,
    already: false,
    problem: null,
    sides: [
      {
        node: 2,
        ok: true,
        already: false,
        kind: 'punched',
        elapsed_ms: 640,
        failure: null,
        udp: true,
        mapping_varies: false,
        candidates: 3,
      },
      {
        node: 5,
        ok: true,
        already: false,
        kind: 'punched',
        elapsed_ms: 610,
        failure: null,
        udp: true,
        mapping_varies: false,
        candidates: 2,
      },
    ],
  },
  direct_quality: {
    state: 'done',
    route: { via: 'direct', addr: '203.0.113.7:20330' },
    picked_by: ['direct'],
    problem: null,
    latency: latency(61),
    cost_us: 74_300,
    hops: [{ from: 2, to: 5, rtt_us: 60_900, jitter_us: 1200, loss_permille: 20, source: 'measured' }],
    relay_leg: null,
    throughput: {
      state: 'done',
      problem: null,
      up: { bytes: 11_904_000, ms: 4998, bits_per_second: 19_054_000, stopped_by: 'time' },
      down: { bytes: 12_058_000, ms: 4950, bits_per_second: 19_487_000, stopped_by: 'time' },
      failure: null,
      transport: 'quic',
      congestion: 'bbr',
      limit_bytes: 20_000_000,
      limit_ms: 5000,
      pinned: true,
      shared_connection: true,
    },
  },
  relay_quality: [
    {
      state: 'done',
      route: { via: 'relay', relays: [3] },
      picked_by: ['direct_first', 'lowest_cost'],
      problem: null,
      latency: latency(52),
      cost_us: 55_200,
      hops: [
        { from: 2, to: 3, rtt_us: 30_400, jitter_us: 300, loss_permille: 0, source: 'measured' },
        { from: 3, to: 5, rtt_us: 20_000, jitter_us: null, loss_permille: null, source: 'reported' },
      ],
      relay_leg: 'udp',
      throughput: null,
    },
  ],
  route: {
    state: 'done',
    problem: null,
    forward: {
      from: 2,
      to: 5,
      mode: 'direct_first',
      current: { via: 'direct', addr: '203.0.113.7:20330' },
      direct_first: {
        route: { via: 'direct', addr: '203.0.113.7:20330' },
        reason: 'direct',
        cost_us: 74_300,
        cheaper: null,
      },
      lowest_cost: {
        route: { via: 'direct', addr: '203.0.113.7:20330' },
        reason: 'within_margin',
        cost_us: 74_300,
        cheaper: { route: { via: 'relay', relays: [3] }, cost_us: 72_100 },
      },
      candidates: [],
    },
    reverse: null,
  },
  notes: [],
}

/** 现在怎么走：这几个测试不看它，给一个最简单的。 */
const PATH: PathView = {
  forward: {
    from: ref(2, 'laptop'),
    to: ref(5, 'nas'),
    source: 'query',
    route: direct('203.0.113.7:20330'),
    rtt_ms: 61,
    age_secs: 0,
    since: null,
  },
  reverse: {
    from: ref(5, 'nas'),
    to: ref(2, 'laptop'),
    source: 'query',
    route: direct('198.51.100.4:20330'),
    rtt_ms: 61,
    age_secs: 0,
    since: null,
  },
}

/** 同一对早一些的一次：只测了直连，不能直连。 */
const OLDER: TestView = {
  ...DONE,
  id: 11,
  started_at: DONE.started_at - 3600,
  finished_at: DONE.started_at - 3590,
  direct: { state: 'done', ok: false, kind: null, elapsed_ms: null, already: false, problem: null, sides: [] },
  direct_quality: null,
  relay_quality: [],
  route: null,
}

/** 两端主机（0.1.24 的 `host` 项）：laptop 量吞吐时 i3t 占满了一个核。 */
const HOSTS: HostReport[] = [
  {
    node: 2,
    state: 'done',
    problem: null,
    system: {
      os: 'Debian GNU/Linux 13 (trixie)',
      kernel: '6.12.48+deb13-amd64',
      arch: 'x86_64',
      cpu_model: 'AMD EPYC 7B13',
      cores: 2,
      uptime_secs: 86_400 * 3,
    },
    cpu: { system_percent: 7, process_percent: 2 },
    cpu_peak: { system_percent: 61, process_percent: 98 },
    load_x100: [35, 22, 18],
    memory: { total_bytes: 2_147_483_648, available_bytes: 1_073_741_824, process_rss_bytes: 28_311_552 },
    process: { uptime_secs: 5400, threads: 6 },
    connections: { peers: 4, relays: 2, relay_clients: 5, streams: 3, lanes_in_use: 0, lanes_idle: 4 },
    to_peer: {
      route: { via: 'direct', addr: '203.0.113.7:20330' },
      rtt_us: 61_400,
      cwnd_bytes: 482_000,
      mtu: 1420,
      congestion: 'bbr',
      sent_packets: 18_230,
      lost_packets: 41,
      lost_bytes: 52_480,
      congestion_events: 3,
      bytes_sent: 20_971_520,
      bytes_received: 20_985_000,
      age_secs: 120,
    },
  },
  {
    node: 5,
    state: 'skipped',
    problem: { code: 'not_supported', node: 5, message: '' },
    system: null,
    cpu: null,
    cpu_peak: null,
    load_x100: null,
    memory: null,
    process: null,
    connections: null,
    to_peer: null,
  },
]

const WITH_HOSTS: TestView = {
  ...DONE,
  request: { ...DONE.request, items: [...DONE.request.items, 'host'] },
  hosts: HOSTS,
}

function setup(extra: Record<string, unknown> = {}) {
  return fakeApi({
    'GET /networks/1/nodes': [laptop, hk, nas],
    'GET /networks/1/path': PATH,
    'GET /networks/1/tests': [DONE],
    'GET /networks/1/tests/12': DONE,
    'GET /update': makeOverview(),
    ...extra,
  })
}

describe('写法', () => {
  it('延迟 10 ms 以下留一位小数，丢包写成百分比，速率按量级', () => {
    expect(msText(640)).toBe(t.common.ms(0.6))
    expect(msText(61_240)).toBe(t.common.ms(61))
    expect(permilleText(30)).toBe('3%')
    expect(permilleText(5)).toBe('0.5%')
    expect(rateText(19_054_000)).toBe('19.1 Mbps')
    expect(rateText(1_200_000_000)).toBe('1.20 Gbps')
  })

  it('原因按码写，带上是哪个节点的；认不出的码用原话', () => {
    const nodes = [laptop, nas]
    expect(problemText({ code: 'mapping_varies', node: 5, message: 'x' }, nodes)).toBe(
      s.problemOf('nas', s.problems.mapping_varies),
    )
    expect(problemText({ code: 'brand_new', node: null, message: '原话' }, nodes)).toBe('原话')
  })
})

describe('路径诊断：一次诊断', () => {
  it('顶上一排指标：直连、最低延迟、丢包、吞吐；概览里是结论和直连', async () => {
    setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByTestId('test-state')).toHaveTextContent(s.state.done)
    expect(screen.getByTestId('kpi-direct')).toHaveTextContent(s.directOk)
    expect(screen.getByTestId('kpi-direct')).toHaveTextContent(s.directIn(640))
    // 最低延迟是经 hk 的那条（52 ms），丢包也是它的。
    expect(screen.getByTestId('kpi-latency')).toHaveTextContent(t.common.ms(52))
    expect(screen.getByTestId('kpi-latency')).toHaveTextContent(t.routes.via('hk'))
    expect(screen.getByTestId('kpi-loss')).toHaveTextContent('3%')
    expect(screen.getByTestId('kpi-throughput')).toHaveTextContent('19.1 Mbps')
    expect(screen.getByTestId('kpi-cpu')).toHaveTextContent(d.kpiNoHost)

    const findings = screen.getByTestId('findings')
    expect(findings).toHaveTextContent(d.find.directOk(`${t.routes.directKind.punched} · ${s.directIn(640)}`))
    expect(findings).toHaveTextContent(d.find.loss(s.qualityDirect, '3%'))
    expect(findings).toHaveTextContent(
      d.find.cheaper(t.path.forward('laptop', 'nas'), t.routes.via('hk'), t.common.ms(72), t.common.ms(74)),
    )
    expect(within(screen.getByTestId('test-direct')).getByText(s.directOk)).toBeVisible()
    // 每一项一个进度的小块，都做完了。
    expect(screen.getByTestId('step-direct')).toHaveAttribute('data-state', 'done')
    expect(screen.getByTestId('step-throughput')).toHaveAttribute('data-state', 'done')
    expect(screen.queryByTestId('step-host')).not.toBeInTheDocument()
  })

  it('点一个指标去对应的标签页，地址里记着', async () => {
    const user = userEvent.setup()
    setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    await user.click(await screen.findByTestId('kpi-latency'))
    expect(await screen.findByTestId('test-quality')).toBeVisible()
    expect(screen.getByRole('tab', { name: d.tabs.latency })).toHaveAttribute('aria-selected', 'true')
  })

  it('延迟与丢包：各条路并排比，一列一条路；下面是延迟分布', async () => {
    setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&tab=latency' })
    expect(await screen.findByTestId('test-path-0')).toHaveTextContent(s.qualityDirect)
    expect(screen.getByTestId('test-path-1')).toHaveTextContent(t.routes.via('hk'))
    expect(screen.getByTestId('test-path-1')).toHaveTextContent(s.pickedBy.lowest_cost)
    expect(screen.getByTestId('test-path-0-latency')).toHaveTextContent(
      s.latencyValue(t.common.ms(58), t.common.ms(61), t.common.ms(66)),
    )
    expect(screen.getByTestId('test-path-0-loss')).toHaveTextContent(s.lossSplit('1%', '2%'))
    expect(screen.getByTestId('test-path-1-hops')).toHaveTextContent(`hk → nas：${t.common.ms(20)}`)
    // 吞吐在带宽那一页，不在这张表里。
    expect(screen.queryByTestId('test-path-0-throughput')).not.toBeInTheDocument()
    expect(screen.getByTestId('latency-spread')).toHaveTextContent(
      d.rangeLegend(t.common.ms(58), t.common.ms(61), t.common.ms(66)),
    )
  })

  it('带宽：量了吞吐的路一张卡片，上下行、搬了多少、为什么停', async () => {
    setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&tab=bandwidth' })
    const card = await screen.findByTestId('bandwidth-direct')
    expect(card).toHaveTextContent('19.1 Mbps')
    expect(card).toHaveTextContent('19.5 Mbps')
    expect(card).toHaveTextContent(d.stoppedBy.time)
    expect(card).toHaveTextContent(`${d.congestion} BBR`)
  })

  it('没有测吞吐：带宽那一页说怎么测', async () => {
    const plain: TestView = {
      ...DONE,
      request: { ...DONE.request, throughput: false },
      direct_quality: DONE.direct_quality === null ? null : { ...DONE.direct_quality, throughput: null },
    }
    setup({ 'GET /networks/1/tests': [plain], 'GET /networks/1/tests/12': plain })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&tab=bandwidth' })
    expect(await screen.findByText(d.bandwidthNone)).toBeVisible()
    expect(screen.getByTestId('kpi-throughput')).toHaveTextContent(d.kpiNoThroughput)
  })

  it('路由：两个方向的卡片、候选路径，和两种模式各会怎么选', async () => {
    setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&tab=route' })
    expect(await screen.findByTestId('leg-forward')).toHaveTextContent(t.path.forward('laptop', 'nas'))
    const lowest = await screen.findByTestId('decision-2-lowest_cost')
    expect(lowest).toHaveTextContent(d.reason.within_margin)
    expect(lowest).toHaveTextContent(d.cheaperLine(t.routes.via('hk'), t.common.ms(72)))
    expect(screen.getByTestId('decision-2-direct_first')).toHaveTextContent(t.path.current)
  })

  it('两端主机：诊断时的系统、CPU、内存、连接，到对端的连接；旧版本的节点说不报', async () => {
    setup({ 'GET /networks/1/tests': [WITH_HOSTS], 'GET /networks/1/tests/12': WITH_HOSTS })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&tab=hosts' })
    const a = await screen.findByTestId('host-a')
    await waitFor(() => expect(a).toHaveTextContent('Debian GNU/Linux 13 (trixie)'))
    expect(a).toHaveTextContent(d.host.cpuValue('61%', '98%'))
    expect(a).toHaveTextContent('0.35 · 0.22 · 0.18')
    expect(a).toHaveTextContent(d.host.lanesValue(0, 4))
    expect(a).toHaveTextContent(d.host.packetsValue('18230', '41', '0.2%'))
    expect(a).toHaveTextContent('1420')
    expect(screen.getByTestId('host-b')).toHaveTextContent(d.hostTooOld)
    // CPU 卡住是一条结论，指标里也标出来。
    expect(screen.getByTestId('kpi-cpu')).toHaveTextContent(d.cpuSide('laptop', 98))
    expect(screen.getByTestId('step-host')).toHaveAttribute('data-state', 'done')
  })

  it('完整输出：文本报告和 JSON，都能复制、下载', async () => {
    const user = userEvent.setup()
    setup({ 'GET /networks/1/tests': [WITH_HOSTS], 'GET /networks/1/tests/12': WITH_HOSTS })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&tab=output' })
    const text = await screen.findByTestId('diagnosis-output-text')
    await waitFor(() => expect(text).toHaveTextContent(d.find.cpuBound('laptop', 98)))
    expect(text).toHaveTextContent(d.report.title)
    expect(text).toHaveTextContent(d.find.cpuBound('laptop', 98))
    expect(text).toHaveTextContent('Debian GNU/Linux 13 (trixie)')
    await user.click(screen.getByRole('radio', { name: d.outputJson }))
    const json = JSON.parse(screen.getByTestId('diagnosis-output-json').textContent) as { test: { id: number } }
    expect(json.test.id).toBe(12)
    expect(screen.getByRole('button', { name: d.download })).toBeEnabled()
  })

  it('没点名看哪一次：看这一对最近的一次', async () => {
    const api = setup({ 'GET /networks/1/tests': [DONE, OLDER], 'GET /networks/1/tests/11': OLDER })
    const { unmount } = renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByTestId('test-state')).toHaveTextContent(s.state.done)
    expect(api.callsTo('GET /networks/1/tests')[0]?.query.toString()).toContain('a=2&b=5')
    expect(api.callsTo('GET /networks/1/tests/12')).toHaveLength(1)
    expect(api.callsTo('GET /networks/1/tests/11')).toHaveLength(0)
    unmount()

    // 地址里点名了以前的那一次：看它。
    setup({ 'GET /networks/1/tests': [DONE, OLDER], 'GET /networks/1/tests/11': OLDER })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&test=11' })
    await waitFor(() => expect(screen.getByTestId('kpi-direct')).toHaveTextContent(s.directNo))
  })

  it('没诊断过：说一句，按钮是"开始诊断"', async () => {
    setup({ 'GET /networks/1/tests': [] })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByText(d.never)).toBeVisible()
    expect(screen.getByRole('button', { name: d.run })).toBeEnabled()
    expect(screen.getByTestId('findings')).toHaveTextContent(d.findingsIdle)
  })

  it('开始诊断：发出请求，接着看这一次；进行中每一项到哪一步看得见，能停', async () => {
    const user = userEvent.setup()
    const running: TestView = {
      ...DONE,
      id: 13,
      state: 'running',
      finished_at: null,
      direct_quality: null,
      relay_quality: DONE.relay_quality.map((path) => ({ ...path, state: 'pending' as const, latency: null })),
      route: { state: 'pending', problem: null, forward: null, reverse: null },
    }
    const api = setup({
      'GET /networks/1/tests': [],
      'POST /networks/1/tests': running,
      'GET /networks/1/tests/13': running,
      'POST /networks/1/tests/13/cancel': { ...running, state: 'cancelled' },
    })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByText(d.never)).toBeVisible()
    await user.click(screen.getByRole('checkbox', { name: new RegExp(s.throughput) }))
    await user.click(screen.getByRole('button', { name: d.run }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/tests')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/tests')[0]?.body).toEqual({ a: 2, b: 5, throughput: true })
    expect(await screen.findByText(d.started('laptop', 'nas'))).toBeVisible()
    expect(await screen.findByTestId('test-state')).toHaveTextContent(s.state.running)
    expect(screen.getByTestId('step-direct')).toHaveAttribute('data-state', 'done')
    expect(screen.getByTestId('step-relay_quality')).toHaveAttribute('data-state', 'pending')
    expect(screen.getByTestId('findings')).toHaveTextContent(d.findingsRunning)

    await user.click(screen.getByRole('button', { name: d.stop }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/tests/13/cancel')).toHaveLength(1))
  })

  it('整个诊断做不了：结论里写出原因和是哪个节点', async () => {
    setup({
      'GET /networks/1/tests/12': {
        ...DONE,
        state: 'failed',
        error: { code: 'node_offline', node: 5, message: '' },
        direct: null,
        direct_quality: null,
        relay_quality: [],
        route: null,
      },
    })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&test=12' })
    expect(await screen.findByText(d.find.testFailed(s.problemOf('nas', s.problems.node_offline)))).toBeVisible()
  })

  it('还没选两个节点：只说一句，不问测试', async () => {
    const api = setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis' })
    expect(await screen.findByText(t.path.pickHint)).toBeVisible()
    expect(api.callsTo('GET /networks/1/tests')).toHaveLength(0)
  })
})
