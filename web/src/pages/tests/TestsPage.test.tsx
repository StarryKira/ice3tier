import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type MatrixRun, type NodeRef, type TestView } from '@/api/types'
import { t } from '@/i18n'
import { msText, permilleText, problemText, rateText } from '@/lib/tests'
import { makeNode } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { TestsPage } from './TestsPage'

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

function setup(extra: Record<string, unknown> = {}) {
  return fakeApi({
    'GET /networks/1/nodes': [laptop, hk, nas],
    'GET /networks/1/tests': [DONE],
    'GET /networks/1/tests/12': DONE,
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

describe('连通性测试', () => {
  it('打开一个测试：直连、选路、直连质量和中继质量各一张卡片', async () => {
    setup()
    renderWithApp(<TestsPage />, { network: NETWORK, route: '/n/1/tests?test=12&a=2&b=5' })
    const current = await screen.findByTestId('test-current')
    expect(await within(current).findByTestId('test-state')).toHaveTextContent(s.state.done)

    const direct = within(current).getByTestId('test-direct')
    expect(direct).toHaveTextContent(s.directOk)
    expect(direct).toHaveTextContent(t.routes.directKind.punched)
    expect(direct).toHaveTextContent(s.directIn(640))
    expect(direct).toHaveTextContent(s.candidates(3))

    const [directPath, relayPath] = within(current).getAllByTestId('test-path')
    expect(directPath).toHaveTextContent(s.latencyValue(t.common.ms(58), t.common.ms(61), t.common.ms(66)))
    expect(directPath).toHaveTextContent(s.lossSplit('1%', '2%'))
    expect(directPath).toHaveTextContent('19.1 Mbps')
    expect(directPath).toHaveTextContent(s.throughputNote('BBR', '5'))
    expect(relayPath).toHaveTextContent(t.nodes.detail.viaRelay('hk'))
    expect(relayPath).toHaveTextContent(s.pickedBy.lowest_cost)
    expect(relayPath).toHaveTextContent(`hk → nas：${t.common.ms(20)}`)

    // 延迟优先留在现在的路上：写出那条更便宜的差多少。
    const route = within(current).getByTestId('test-route')
    expect(route).toHaveTextContent(s.reasons.within_margin)
    expect(route).toHaveTextContent(s.withinMargin(t.nodes.detail.viaRelay('hk'), t.common.ms(2.2)))
  })

  it('选好两个节点开始：发出请求，接着看这个测试；同一个节点不能选两次', async () => {
    const user = userEvent.setup()
    const api = setup({
      'POST /networks/1/tests': {
        ...DONE,
        id: 13,
        state: 'running',
        direct: null,
        direct_quality: null,
        relay_quality: [],
        route: null,
      },
      'GET /networks/1/tests/13': { ...DONE, id: 13 },
    })
    renderWithApp(<TestsPage />, { network: NETWORK, route: '/n/1/tests?a=2&b=2' })
    expect(await screen.findByText(s.pickTwo)).toBeVisible()
    expect(screen.getByRole('button', { name: s.start })).toBeDisabled()

    await user.click(screen.getByRole('combobox', { name: s.nodeB }))
    await user.click(await screen.findByRole('option', { name: /nas/ }))
    await user.click(screen.getByRole('checkbox', { name: new RegExp(s.throughput) }))
    await user.click(screen.getByRole('button', { name: s.start }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/tests')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/tests')[0]?.body).toEqual({ a: 2, b: 5, throughput: true })
    expect(await screen.findByText(s.started('laptop', 'nas'))).toBeVisible()
    await waitFor(() => expect(api.callsTo('GET /networks/1/tests/13').length).toBeGreaterThan(0))
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
    renderWithApp(<TestsPage />, { network: NETWORK, route: '/n/1/tests?test=12' })
    expect(await screen.findByText(s.problemOf('nas', s.problems.node_offline))).toBeVisible()
  })

  it('全网测试：一张节点 × 节点的表，格子里是 P2P 或中继和平均延迟，点开看这一对', async () => {
    const user = userEvent.setup()
    const run: MatrixRun = {
      id: 3,
      network: 1,
      state: 'done',
      started_at: 1_794_798_564,
      finished_at: 1_794_798_600,
      concurrency: 4,
      total: 3,
      finished: 3,
      nodes: [ref(2, 'laptop'), ref(3, 'hk'), ref(5, 'nas')],
      cells: [
        {
          a: 2,
          b: 3,
          state: 'done',
          test: 40,
          direct_ok: true,
          direct_kind: 'punched',
          direct_elapsed_ms: 640,
          route: { via: 'direct', addr: 'x' },
          latency: latency(61),
          problem: null,
        },
        {
          a: 2,
          b: 5,
          state: 'done',
          test: 12,
          direct_ok: false,
          direct_kind: null,
          direct_elapsed_ms: null,
          route: { via: 'relay', relays: [3] },
          latency: latency(52),
          problem: { code: 'mapping_varies', node: 5, message: '' },
        },
        {
          a: 3,
          b: 5,
          state: 'skipped',
          test: null,
          direct_ok: null,
          direct_kind: null,
          direct_elapsed_ms: null,
          route: null,
          latency: null,
          problem: { code: 'node_offline', node: 5, message: '' },
        },
      ],
    }
    setup({ 'GET /networks/1/tests/matrix': run })
    renderWithApp(<TestsPage />, { network: NETWORK, route: '/n/1/tests?tab=matrix' })
    expect(await screen.findByTestId('matrix-progress')).toHaveTextContent(s.matrixProgress(3, 3))
    // 两半对称。
    expect(screen.getByTestId('matrix-cell-2-3')).toHaveTextContent(`${s.matrixCellP2P}${t.common.ms(61)}`)
    expect(screen.getByTestId('matrix-cell-3-2')).toHaveTextContent(`${s.matrixCellP2P}${t.common.ms(61)}`)
    expect(screen.getByTestId('matrix-cell-2-5')).toHaveTextContent(`${s.matrixCellRelay}${t.common.ms(52)}`)
    expect(screen.getByTestId('matrix-cell-3-5')).toHaveTextContent('—')

    await user.click(screen.getByRole('button', { name: s.matrixCellOpen('laptop', 'nas') }))
    expect(await screen.findByTestId('test-current')).toBeVisible()
  })

  it('还没跑过全网测试：controller 回 404 时是一句话和开始的按钮', async () => {
    setup({
      'GET /networks/1/tests/matrix': () =>
        new Response(JSON.stringify({ error: { code: 'not_found', message: '' } }), {
          status: 404,
          headers: { 'Content-Type': 'application/json' },
        }),
    })
    renderWithApp(<TestsPage />, { network: NETWORK, route: '/n/1/tests?tab=matrix' })
    expect(await screen.findByText(s.matrixEmpty)).toBeVisible()
    expect(screen.getByRole('button', { name: s.matrixStart })).toBeEnabled()
  })
})
