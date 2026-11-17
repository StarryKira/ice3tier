import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type LaneEffect, type PairView } from '@/api/types'
import { t } from '@/i18n'
import { makeNode, makePair, makePeerLanes } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { PairsPage } from './PairsPage'

const p = t.pairs
const l = t.pairs.lane

const HK = makeNode(1, 'hk', {
  declared: { relay: true, exit: false, forward: false },
  relay: { udp: ['203.0.113.7:20330'], tcp: ['203.0.113.7:20330'], region: 'hk', bandwidth_mbps: null },
})
const LAPTOP = makeNode(2, 'laptop')
const NAS = makeNode(3, 'nas')

const ON_LANES: PairView = makePair(1, 2, {
  keep_alive: false,
  lane: 'always',
  lane_effect: { effective: true, blocked: [], dialers: [2], summary: '由 #2 拨对方的 TCP 端口' },
  lanes: [
    {
      node: 1,
      lanes: makePeerLanes(2, { in_use: 12, idle: 6, fallbacks: 1, claims: 40, claims_waited: 3, claim_wait_ms: 412 }),
    },
    {
      node: 2,
      lanes: makePeerLanes(1, { dials: true, in_use: 12, ready: 2, parked_at_peer: 6, dial_failures: 2 }),
    },
  ],
})

const BLOCKED: LaneEffect = {
  effective: false,
  blocked: [{ reason: 'no_relay' }],
  dialers: [],
  summary: '设置不起作用，这一对节点的流都走共用的连接（见 blocked）。',
}

function setup(pairs: PairView[], extra: Routes = {}) {
  return fakeApi({
    'GET /networks/1/pairs': pairs,
    'GET /networks/1/nodes': [HK, LAPTOP, NAS],
    'GET /networks/1/nodes/1': { ...HK, lanes: null },
    'GET /networks/1/nodes/2': {
      ...LAPTOP,
      lanes: {
        total: makePeerLanes(0),
        handshake_failures: { deadline: 0, unknown_key: 2, tls: 0, protocol: 1, busy: 0 },
        peers: [],
      },
    },
    ...extra,
  })
}

describe('节点对页', () => {
  it('独占 TCP 生效：写出谁拨号和一眼要看的几个数，两边报的合起来', async () => {
    setup([ON_LANES])
    renderWithApp(<PairsPage />, { network: NETWORK })
    const row = await screen.findByTestId('pair-row-hk-laptop')
    expect(within(row).getByRole('combobox', { name: l.select('hk', 'laptop') })).toHaveValue('always')
    expect(within(row).getByTestId('lane-effect-hk-laptop')).toHaveTextContent(l.effective)
    expect(within(row).getByTestId('lane-effect-hk-laptop')).toHaveTextContent(l.dials('laptop'))
    // 在用取两边里大的，空闲按停在哪一边相加（6 + 2），回退 1，拨号失败 2。
    const cells = within(row)
      .getAllByRole('cell')
      .map((cell) => cell.textContent)
    expect(cells.slice(4, 8)).toEqual(['12', '8', '1', '2'])
    // 开着独占 TCP 时说一次：共用的连接和路由照常保留。
    expect(screen.getByText(l.routeNote)).toBeVisible()
  })

  it('详情：其余的计数两边并排，握手失败取自节点详情', async () => {
    setup([ON_LANES])
    renderWithApp(<PairsPage />, { network: NETWORK })
    await userEvent.click(await screen.findByRole('button', { name: l.detailsFor('hk', 'laptop') }))
    const table = screen.getByRole('table', { name: l.detailsFor('hk', 'laptop') })
    expect(within(table).getByRole('columnheader', { name: 'hk' })).toBeVisible()
    expect(within(table).getByRole('columnheader', { name: 'laptop' })).toBeVisible()
    expect(within(table).getByText(l.row.waitedValue(3, t.common.ms(412)))).toBeVisible()
    await waitFor(() =>
      expect(
        within(table).getByText(
          [l.causeCount(l.handshakeCause.unknown_key, 2), l.causeCount(l.handshakeCause.protocol, 1)].join(' · '),
        ),
      ).toBeVisible(),
    )
  })

  it('不生效：下面一行是原因和修好它的那一步，用的是原因码而不是 controller 的原话', async () => {
    setup([makePair(2, 3, { lane: 'udp_fallback', lane_effect: BLOCKED })])
    renderWithApp(<PairsPage />, { network: NETWORK })
    const effect = await screen.findByTestId('lane-effect-laptop-nas')
    expect(effect).toHaveTextContent(l.blocked)
    const blocked = screen.getByTestId('lane-blocked-laptop-nas')
    expect(blocked).toHaveTextContent(l.reason.noRelay('laptop', 'nas'))
    expect(blocked).not.toHaveTextContent(BLOCKED.summary)
    expect(within(blocked).getByRole('link', { name: l.fix.enableRelay('laptop') })).toHaveAttribute(
      'href',
      '/n/1/nodes/2?tab=relay',
    )
    // 没有节点报上来，就没有数可看：不画计数的几列，也没有详情。
    expect(screen.queryByRole('columnheader', { name: l.colInUse })).toBeNull()
    expect(screen.queryByRole('button', { name: l.detailsFor('laptop', 'nas') })).toBeNull()
  })

  it('改独占 TCP 只发这一项；拨连接常驻的开关也只发那一项', async () => {
    const api = setup([makePair(1, 2)], {
      'PUT /networks/1/pairs/1/2': (call) => ({ ...makePair(1, 2), ...(call.body as object) }),
    })
    renderWithApp(<PairsPage />, { network: NETWORK })
    // 没有哪一对开着独占 TCP 时，不画计数的几列。
    await screen.findByTestId('pair-row-hk-laptop')
    expect(screen.queryByRole('columnheader', { name: l.colInUse })).toBeNull()

    await userEvent.selectOptions(screen.getByRole('combobox', { name: l.select('hk', 'laptop') }), 'udp_fallback')
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/1/2')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/1/2')[0]?.body).toEqual({ lane: 'udp_fallback' })
    expect(await screen.findByText(l.changed('hk', 'laptop', l.modes.udp_fallback))).toBeVisible()

    await userEvent.click(screen.getByRole('switch', { name: p.toggle('hk', 'laptop') }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/1/2')).toHaveLength(2))
    expect(api.callsTo('PUT /networks/1/pairs/1/2')[1]?.body).toEqual({ keep_alive: false })
  })

  it('从节点详情过来（a、b 在地址里）：那一对的详情已经展开', async () => {
    setup([ON_LANES])
    renderWithApp(<PairsPage />, { network: NETWORK, route: '/n/1/pairs?a=2&b=1' })
    expect(await screen.findByRole('table', { name: l.detailsFor('hk', 'laptop') })).toBeVisible()
  })
})

describe('添加节点对', () => {
  it('两个节点和两项设置一起提交', async () => {
    const api = setup([], { 'PUT /networks/1/pairs/2/3': makePair(2, 3) })
    renderWithApp(<PairsPage />, { network: NETWORK })
    await screen.findByText(p.emptyTitle)
    await userEvent.click(screen.getByRole('button', { name: p.add }))
    const dialog = screen.getByRole('dialog', { name: p.dialogTitle })
    await userEvent.click(within(dialog).getByRole('combobox', { name: p.nodeA }))
    await userEvent.click(screen.getByRole('option', { name: /laptop/ }))
    await userEvent.click(within(dialog).getByRole('combobox', { name: p.nodeB }))
    await userEvent.click(screen.getByRole('option', { name: /nas/ }))
    await userEvent.click(within(dialog).getByRole('radio', { name: l.modes.always }))
    expect(within(dialog).getByText(l.hint)).toBeVisible()
    await userEvent.click(within(dialog).getByRole('button', { name: p.submit }))
    await waitFor(() => expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(1))
    expect(api.callsTo('PUT /networks/1/pairs/2/3')[0]?.body).toEqual({ keep_alive: true, lane: 'always' })
  })

  it('两项都是默认时不提交，说要至少开一项', async () => {
    const api = setup([])
    renderWithApp(<PairsPage />, { network: NETWORK })
    await screen.findByText(p.emptyTitle)
    await userEvent.click(screen.getByRole('button', { name: p.add }))
    const dialog = screen.getByRole('dialog', { name: p.dialogTitle })
    await userEvent.click(within(dialog).getByRole('combobox', { name: p.nodeA }))
    await userEvent.click(screen.getByRole('option', { name: /laptop/ }))
    await userEvent.click(within(dialog).getByRole('combobox', { name: p.nodeB }))
    await userEvent.click(screen.getByRole('option', { name: /nas/ }))
    await userEvent.click(within(dialog).getByRole('switch', { name: p.colKeepAlive }))
    await userEvent.click(within(dialog).getByRole('button', { name: p.submit }))
    expect(within(dialog).getByText(p.nothingSet)).toBeVisible()
    expect(api.callsTo('PUT /networks/1/pairs/2/3')).toHaveLength(0)
  })
})
