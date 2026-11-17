import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type PathView } from '@/api/types'
import { t } from '@/i18n'
import { direct, makeNode, makeOverview, viaRelays } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { PathPage } from './PathPage'

const p = t.path

const LAPTOP = { id: 2, name: 'laptop', virtual_ip: '10.66.0.2' }
const NAS = { id: 1, name: 'nas', virtual_ip: '10.66.0.1' }

const VIEW: PathView = {
  forward: {
    from: LAPTOP,
    to: NAS,
    source: 'query',
    route: direct('203.0.113.7:20330'),
    rtt_ms: 12,
    age_secs: 0,
    since: { secs_ago: 340, reason: '直连打通了' },
  },
  reverse: {
    from: NAS,
    to: LAPTOP,
    source: 'report',
    route: viaRelays(3),
    rtt_ms: 48,
    age_secs: 6,
    since: null,
  },
}

function setup(extra: Routes = {}) {
  return fakeApi({
    'GET /networks/1/nodes': [makeNode(1, 'nas', { online: false }), makeNode(2, 'laptop'), makeNode(3, 'hk')],
    'GET /networks/1/path': VIEW,
    'GET /update': makeOverview(),
    ...extra,
  })
}

describe('路径诊断', () => {
  it('两个节点从地址里来；两个方向分开写：现问到的带换路原因，上报的带多久之前', async () => {
    const api = setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=1' })
    const forward = await screen.findByTestId('path-forward')
    expect(api.callsTo('GET /networks/1/path')[0]?.query.toString()).toBe('from=2&to=1')
    expect(forward).toHaveAttribute('data-source', 'query')
    expect(forward).toHaveTextContent(p.forward('laptop', 'nas'))
    expect(forward).toHaveTextContent('12 ms')
    expect(within(forward).getByTestId('path-forward-since')).toHaveTextContent(p.since(t.time.minutesAgo(5)))
    // 换路的原因是节点的原话，原样写在下面一行。
    expect(within(forward).getByTestId('path-forward-since-reason')).toHaveTextContent('直连打通了')
    expect(within(forward).getByTestId('path-forward-source')).toHaveTextContent(p.source.query)

    const reverse = screen.getByTestId('path-reverse')
    expect(reverse).toHaveAttribute('data-source', 'report')
    expect(reverse).toHaveTextContent(t.routes.via('hk'))
    expect(reverse).toHaveTextContent(p.age(t.time.secondsAgo(6)))
    // nas 不在线：说清楚它那个方向是离线之前的数据。
    expect(screen.getByText(p.offline('nas'))).toBeVisible()
  })

  it('"重新查询"重新问两端', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=1' })
    await screen.findByTestId('path-forward')
    await user.click(screen.getByRole('button', { name: p.query }))
    await waitFor(() => expect(api.callsTo('GET /networks/1/path')).toHaveLength(2))
  })

  it('不是对端：顶上一条说明和去访问检查的入口，卡片照常显示', async () => {
    setup({
      'GET /networks/1/path': {
        forward: { ...VIEW.forward, source: 'not_peers', route: null, rtt_ms: null, age_secs: null, since: null },
        reverse: { ...VIEW.reverse, source: 'none', route: null, rtt_ms: null, age_secs: null },
      },
    })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=1' })
    expect(await screen.findByText(p.notPeersTitle)).toBeVisible()
    expect(screen.getByTestId('path-forward-source')).toHaveTextContent(p.source.not_peers)
    expect(screen.getByRole('link', { name: p.toAccess })).toHaveAttribute('href', '/n/1/access?check=1&from=2&to=1')
    expect(screen.getByTestId('path-forward')).toHaveAttribute('data-source', 'not_peers')
    expect(screen.getByTestId('path-reverse')).toHaveTextContent(t.routes.noData)
  })

  it('没选够两个、或者两边是同一个节点：不发请求，说该怎么选', async () => {
    const api = setup()
    const { unmount } = renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2' })
    expect(await screen.findByText(p.pickHint)).toBeVisible()
    unmount()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=2' })
    expect(await screen.findByText(p.same)).toBeVisible()
    expect(api.callsTo('GET /networks/1/path')).toHaveLength(0)
  })

  it('换一个节点就换地址、重新问', async () => {
    const user = userEvent.setup()
    const api = setup()
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=1' })
    await screen.findByTestId('path-forward')
    await user.click(screen.getByRole('button', { name: p.swap }))
    await waitFor(() => expect(api.callsTo('GET /networks/1/path')).toHaveLength(2))
    expect(api.callsTo('GET /networks/1/path')[1]?.query.toString()).toBe('from=1&to=2')
  })
})

describe('现问到时的细节：方式、模式、原因、候选路径', () => {
  const DETAILED: PathView = {
    forward: {
      ...VIEW.forward,
      direct: 'punched',
      relay_leg: null,
      mode: 'direct_first',
      reason: '直连可用',
      candidates: [
        {
          route: direct('203.0.113.7:20330'),
          cost_us: 12_900,
          legs: [{ from: 2, to: 1, rtt_us: 12_400, jitter_us: 120, loss_permille: 0 }],
          load_percent: 0,
          current: true,
        },
        {
          route: viaRelays(3),
          cost_us: 49_500,
          legs: [
            { from: 2, to: 3, rtt_us: 21_300, jitter_us: 400, loss_permille: 2 },
            { from: 3, to: 1, rtt_us: 26_000, jitter_us: 0, loss_permille: 0 },
          ],
          load_percent: 12,
          current: false,
        },
      ],
    },
    reverse: { ...VIEW.reverse, direct: null, relay_leg: 'tcp', mode: null, reason: null, candidates: null },
  }

  it('每个方向写直连的方式、第一段的传输、选路模式和原因；候选路径一张表，现在走的标出来', async () => {
    setup({ 'GET /networks/1/path': DETAILED })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=1' })
    const forward = await screen.findByTestId('path-forward')
    expect(within(forward).getByTestId('path-forward-detail')).toHaveTextContent(t.routes.directKind.punched)
    // 原因是节点的原话：照原样，前面一个"原因"。
    expect(within(forward).getByTestId('path-forward-reason')).toHaveTextContent(`${p.reasonLabel} · 直连可用`)
    expect(forward).toHaveTextContent(p.modeLabel(p.mode.direct_first))
    expect(within(screen.getByTestId('path-reverse')).getByTestId('path-reverse-detail')).toHaveTextContent(
      t.routes.legTransport.tcp,
    )

    const table = within(screen.getByTestId('candidates-forward')).getByRole('table', {
      name: p.candidatesTitle('laptop', 'nas'),
    })
    const rows = within(table).getAllByRole('row').slice(1)
    expect(rows).toHaveLength(2)
    expect(rows[0]).toHaveAttribute('aria-current', 'true')
    expect(rows[0]).toHaveTextContent(p.current)
    expect(rows[0]).toHaveTextContent('12.9 ms')
    expect(rows[1]).toHaveTextContent(p.legLine('laptop', 'hk', '21.3 ms', '0.40 ms', '0.2%'))
    expect(rows[1]).toHaveTextContent('12%')
    // 上报来的方向不知道考虑过哪些路：没有这张表。
    expect(screen.queryByTestId('candidates-reverse')).not.toBeInTheDocument()
  })

  it('问到了但发送方手上还没有测量值：照实说', async () => {
    setup({ 'GET /networks/1/path': { ...DETAILED, forward: { ...DETAILED.forward, candidates: [] } } })
    renderWithApp(<PathPage />, { network: NETWORK, route: '/n/1/diagnosis?a=2&b=1' })
    expect(await screen.findByText(p.candidatesEmpty)).toBeVisible()
  })
})
