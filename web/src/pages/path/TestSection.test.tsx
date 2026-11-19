import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type NodeRef, type PathView, type TestView } from '@/api/types'
import { t } from '@/i18n'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { direct, makeNode, makeOverview } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { PathPage } from './PathPage'

const s = t.tests
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

describe('路径诊断里的实测', () => {
  it('左边直连能不能通，右边各条路并排比：一列一条路，行是延迟、抖动、丢包、代价、各段、吞吐', async () => {
    setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5&test=12' })
    const current = await screen.findByTestId('test-current')
    expect(screen.getByTestId('test-state')).toHaveTextContent(s.state.done)

    const direct = within(current).getByTestId('test-direct')
    expect(direct).toHaveTextContent(s.directOk)
    expect(direct).toHaveTextContent(t.routes.directKind.punched)
    expect(direct).toHaveTextContent(s.directIn(640))
    expect(direct).toHaveTextContent(s.candidates(3))

    // 第一列是直连，第二列经 hk，并写出哪种选路模式会走它。
    expect(screen.getByTestId('test-path-0')).toHaveTextContent(s.qualityDirect)
    expect(screen.getByTestId('test-path-1')).toHaveTextContent(t.nodes.detail.viaRelay('hk'))
    expect(screen.getByTestId('test-path-1')).toHaveTextContent(s.pickedBy.lowest_cost)
    expect(screen.getByTestId('test-path-0-latency')).toHaveTextContent(
      s.latencyValue(t.common.ms(58), t.common.ms(61), t.common.ms(66)),
    )
    expect(screen.getByTestId('test-path-0-loss')).toHaveTextContent(s.lossSplit('1%', '2%'))
    expect(screen.getByTestId('test-path-0-throughput')).toHaveTextContent('19.1 Mbps')
    expect(screen.getByTestId('test-path-0-throughput')).toHaveTextContent(s.throughputNote('BBR', 4998))
    expect(s.throughputNote('BBR', 4998)).toContain('5')
    expect(s.throughputNote('BBR', 38)).toContain(t.common.ms(38))
    expect(screen.getByTestId('test-path-1-hops')).toHaveTextContent(`hk → nas：${t.common.ms(20)}`)
    expect(screen.getByTestId('test-path-1-throughput')).toHaveTextContent('—')
    // 选路怎么选在上面"现在怎么走"和候选路径里，这里不再列一遍。
    expect(screen.queryByTestId('test-route')).not.toBeInTheDocument()
  })

  it('没点名看哪一次：看这一对最近的一次；测过不止一次时列出来，点一行换着看', async () => {
    const user = userEvent.setup()
    const api = setup({ 'GET /networks/1/tests': [DONE, OLDER], 'GET /networks/1/tests/11': OLDER })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByTestId('test-current')).toBeVisible()
    expect(api.callsTo('GET /networks/1/tests')[0]?.query.toString()).toContain('a=2&b=5')
    expect(screen.getByTestId('test-row-12')).toHaveAttribute('aria-current', 'true')

    await user.click(screen.getByTestId('test-row-11'))
    await waitFor(() => expect(within(screen.getByTestId('test-direct')).getByText(s.directNo)).toBeVisible())
    expect(screen.getByTestId('test-row-11')).toHaveAttribute('aria-current', 'true')
  })

  it('只测过一次：不列"最近的测试"；没测过：说一句', async () => {
    setup()
    const { unmount } = renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByTestId('test-current')).toBeVisible()
    expect(screen.queryByRole('table', { name: s.historyTitle })).not.toBeInTheDocument()
    unmount()
    setup({ 'GET /networks/1/tests': [] })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByText(s.none)).toBeVisible()
  })

  it('开始测试：发出请求，接着看这个测试；进行中能取消', async () => {
    const user = userEvent.setup()
    const running: TestView = {
      ...DONE,
      id: 13,
      state: 'running',
      finished_at: null,
      direct: null,
      direct_quality: null,
      relay_quality: [],
      route: null,
    }
    const api = setup({
      'GET /networks/1/tests': [],
      'POST /networks/1/tests': running,
      'GET /networks/1/tests/13': running,
      'POST /networks/1/tests/13/cancel': { ...running, state: 'cancelled' },
    })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=5' })
    expect(await screen.findByText(s.none)).toBeVisible()
    await user.click(screen.getByRole('checkbox', { name: new RegExp(s.throughput) }))
    await user.click(screen.getByRole('button', { name: s.start }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/tests')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/tests')[0]?.body).toEqual({ a: 2, b: 5, throughput: true })
    expect(await screen.findByText(s.started('laptop', 'nas'))).toBeVisible()
    expect(await screen.findByTestId('test-state')).toHaveTextContent(s.state.running)

    await user.click(screen.getByRole('button', { name: s.cancel }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/tests/13/cancel')).toHaveLength(1))
  })

  it('整个测试做不了：写出原因和是哪个节点', async () => {
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
    expect(await screen.findByText(s.problemOf('nas', s.problems.node_offline))).toBeVisible()
  })

  it('还没选两个节点：说一句，下面是整个网络最近的测试，点一行就看那一对', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis' })
    expect(await screen.findByText(t.path.pickHint)).toBeVisible()
    const row = await screen.findByTestId('test-row-12')
    expect(row).toHaveTextContent('laptop')
    expect(row).toHaveTextContent('nas')
    await user.click(row)
    expect(await screen.findByTestId('test-current')).toBeVisible()
    expect(api.callsTo('GET /networks/1/path')[0]?.query.toString()).toBe('from=2&to=5')
  })
})
