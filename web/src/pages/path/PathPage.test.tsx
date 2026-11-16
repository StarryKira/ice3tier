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
    expect(forward).toHaveTextContent(p.source.query ?? '')
    expect(forward).toHaveTextContent('12 ms')
    expect(within(forward).getByTestId('path-forward-since')).toHaveTextContent(p.since('5 分钟前', '直连打通了'))

    const reverse = screen.getByTestId('path-reverse')
    expect(reverse).toHaveAttribute('data-source', 'report')
    expect(reverse).toHaveTextContent('经 hk')
    expect(reverse).toHaveTextContent(p.age('6 秒前'))
    // nas 不在线：说清楚它那个方向是离线之前的数据。
    expect(screen.getByText(p.offline('nas'))).toBeVisible()
  })

  it('"再问一次"重新问两端', async () => {
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
    expect(await screen.findByText(p.notPeersBody)).toBeVisible()
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
