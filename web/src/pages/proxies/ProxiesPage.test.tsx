import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type Proxy } from '@/api/types'
import { t } from '@/i18n'
import { proxyUrl } from '@/lib/proxy'
import { makeNode, makePeerLanes } from '@/test/fixtures'
import { fakeApi, NETWORK, renderWithApp } from '@/test/harness'

import { ProxiesPage } from './ProxiesPage'

const s = t.proxies
const d = s.dialog

const nas = makeNode(2, 'nas', { online: true })
const tokyo = makeNode(3, 'tokyo', {
  online: true,
  exit: {
    ports: null,
    lan: false,
    upstream_socks5: null,
    quota_bytes: null,
    quota_reset_day: 1,
    family: 'any',
    connects_per_minute: null,
    class: null,
  },
})

const proxy = (extra: Partial<Proxy> = {}): Proxy => ({
  id: 1,
  name: 'phone',
  node: 2,
  bind: '0.0.0.0:1080',
  protocol: 'mixed',
  exit: 3,
  username: 'u1',
  password: 'secret',
  enabled: true,
  allow_from: [],
  class: null,
  optimistic: null,
  status: { code: 'listening', local: '0.0.0.0:1080' },
  connections: 2,
  bytes_up: 1024,
  bytes_down: 4096,
  ...extra,
})

describe('代理地址', () => {
  it('自动识别的写成 SOCKS5 的；用户名密码转义；IPv6 加方括号；没有密码不写那一段', () => {
    expect(proxyUrl({ protocol: 'mixed', host: '192.168.1.5', port: 1080, username: 'u', password: 'p@ss' })).toBe(
      'socks5://u:p%40ss@192.168.1.5:1080',
    )
    expect(proxyUrl({ protocol: 'http', host: '::1', port: 8080, username: '', password: '' })).toBe(
      'http://[::1]:8080',
    )
  })
})

describe('代理页', () => {
  it('没有代理：一行图说明怎么走，一个新建的按钮', async () => {
    fakeApi({ 'GET /networks/1/proxies': [], 'GET /networks/1/nodes': [nas, tokyo] })
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    expect(await screen.findByText(s.emptyTitle)).toBeVisible()
    expect(screen.getByText(s.flow.exit)).toBeVisible()
    expect(screen.getByRole('button', { name: s.create })).toBeVisible()
  })

  it('一行一个代理：入口、出口、认证、状态、用量；状态码写成界面自己的话', async () => {
    fakeApi({
      'GET /networks/1/proxies': [
        proxy(),
        proxy({ id: 2, name: 'tv', status: { code: 'access_denied' }, username: '' }),
        proxy({ id: 3, name: 'old', enabled: false, status: { code: 'disabled' } }),
      ],
      'GET /networks/1/nodes': [nas, tokyo],
    })
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    const row = await screen.findByTestId('proxy-row-phone')
    expect(row).toHaveTextContent('nas')
    expect(row).toHaveTextContent(s.protocol.mixed)
    expect(row).toHaveTextContent('0.0.0.0:1080')
    expect(row).toHaveTextContent('tokyo')
    expect(row).toHaveTextContent('u1')
    expect(row).toHaveTextContent(s.status.listening)
    expect(row).toHaveTextContent(t.listen.connections(2))
    expect(screen.getByTestId('proxy-row-tv')).toHaveTextContent(s.status.access_denied)
    expect(screen.getByTestId('proxy-row-tv')).toHaveTextContent(s.noAuth)
    expect(screen.getByTestId('proxy-row-old')).toHaveTextContent(s.status.disabled)
  })

  it('行里的开关只发 enabled', async () => {
    const user = userEvent.setup()
    const api = fakeApi({
      'GET /networks/1/proxies': [proxy()],
      'GET /networks/1/nodes': [nas, tokyo],
      'PATCH /networks/1/proxies/1': proxy({ enabled: false }),
    })
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    await user.click(await screen.findByRole('switch', { name: s.switchLabel('phone') }))
    await waitFor(() => expect(api.callsTo('PATCH /networks/1/proxies/1')).toHaveLength(1))
    expect(api.callsTo('PATCH /networks/1/proxies/1')[0]?.body).toEqual({ enabled: false })
  })

  it('新建：出口只能选打开了出口的节点；入口不能是出口；开在所有地址上要用户名密码；底下写出设备里填的地址', async () => {
    const user = userEvent.setup()
    const api = fakeApi({
      'GET /networks/1/proxies': [],
      'GET /networks/1/nodes': [nas, tokyo],
      'GET /networks/1/access/check': { from: nas, to: tokyo, allowed: true, rules: [] },
      'POST /networks/1/proxies': proxy({ name: 'phone' }),
    })
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: s.create }))
    const dialog = await screen.findByRole('dialog', { name: d.createTitle })

    // 出口：nas 没打开出口，选不了。
    await user.click(within(dialog).getByRole('combobox', { name: d.exit }))
    expect(await screen.findByRole('option', { name: /nas/ })).toHaveAttribute('aria-disabled', 'true')
    await user.click(screen.getByRole('option', { name: /tokyo/ }))
    await user.click(within(dialog).getByRole('combobox', { name: d.entry }))
    await user.click(await screen.findByRole('option', { name: /nas/ }))

    const username = within(dialog).getByRole('textbox', { name: d.username })
    await user.clear(username)
    await user.type(username, 'phone')
    const password = within(dialog).getByLabelText(d.password)
    await user.clear(password)
    await user.type(password, 'pw')
    expect(within(dialog).getByText(`socks5://phone:pw@<${s.lanHost}>:1080`)).toBeVisible()

    await user.type(within(dialog).getByRole('textbox', { name: d.name }), 'phone')
    await user.click(within(dialog).getByRole('button', { name: d.submitCreate }))
    await waitFor(() => expect(api.callsTo('POST /networks/1/proxies')).toHaveLength(1))
    expect(api.callsTo('POST /networks/1/proxies')[0]?.body).toEqual({
      name: 'phone',
      node: 2,
      bind: '0.0.0.0:1080',
      protocol: 'mixed',
      exit: 3,
      username: 'phone',
      password: 'pw',
      // ice3proxy 默认关。
      lane: false,
    })
  })

  it('controller 拒绝时按原因码写在对应的那一项下面', async () => {
    const user = userEvent.setup()
    fakeApi({
      'GET /networks/1/proxies': [proxy()],
      'GET /networks/1/nodes': [nas, tokyo],
      'GET /networks/1/access/check': { from: nas, to: tokyo, allowed: true, rules: [] },
      'PATCH /networks/1/proxies/1': () =>
        new Response(
          JSON.stringify({ error: { code: 'exit_not_enabled', message: '原话', what: null, field: 'exit' } }),
          { status: 409, headers: { 'Content-Type': 'application/json' } },
        ),
    })
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: t.common.actionsFor('phone') }))
    await user.click(await screen.findByRole('menuitem', { name: s.menuEdit }))
    const dialog = await screen.findByRole('dialog', { name: d.editTitle('phone') })
    const port = within(dialog).getByRole('textbox', { name: d.port })
    await user.clear(port)
    await user.type(port, '1081')
    await user.click(within(dialog).getByRole('button', { name: t.common.save }))
    await waitFor(() =>
      expect(within(dialog).getByRole('combobox', { name: d.exit })).toHaveAccessibleDescription(
        new RegExp(t.errors.codes.exit_not_enabled),
      ),
    )
  })

  it('策略不允许入口访问出口：当场说', async () => {
    const user = userEvent.setup()
    fakeApi({
      'GET /networks/1/proxies': [],
      'GET /networks/1/nodes': [nas, tokyo],
      'GET /networks/1/access/check': { from: nas, to: tokyo, allowed: false, rules: [] },
    })
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: s.create }))
    const dialog = await screen.findByRole('dialog', { name: d.createTitle })
    await user.click(within(dialog).getByRole('combobox', { name: d.exit }))
    await user.click(await screen.findByRole('option', { name: /tokyo/ }))
    await user.click(within(dialog).getByRole('combobox', { name: d.entry }))
    await user.click(await screen.findByRole('option', { name: /nas/ }))
    expect(await within(dialog).findByText(d.accessDenied('nas', 'tokyo'), { exact: false })).toBeVisible()
  })
})

describe('代理上的 ice3proxy', () => {
  const l = s.lane
  const ON = proxy({
    lane: true,
    lane_effect: { effective: true, blocked: [], dialers: [2], summary: '由 #2 拨对方的 TCP 端口' },
    lanes: [
      { node: 2, lanes: makePeerLanes(3, { dials: true, in_use: 2, claims_waited: 3, claim_wait_ms: 412 }) },
      { node: 3, lanes: makePeerLanes(2, { idle: 4 }), reported_secs_ago: 7 },
    ],
  })

  function setup(proxies: Proxy[]) {
    return fakeApi({
      'GET /networks/1/proxies': proxies,
      'GET /networks/1/nodes': [nas, tokyo],
      'GET /networks/1/nodes/2': {
        ...nas,
        lanes: {
          total: makePeerLanes(0),
          handshake_failures: { deadline: 0, unknown_key: 2, tls: 0, protocol: 0, busy: 0 },
          peers: [],
        },
      },
      'GET /networks/1/nodes/3': { ...tokyo, lanes: null },
      'GET /networks/1/access/check': { from: nas, to: tokyo, allowed: true, rules: [] },
      'PATCH /networks/1/proxies/1': (call) => ({ ...proxy(), ...(call.body as object) }),
    })
  }

  it('没有代理打开它时没有这一列；比界面旧的 controller 不写，也按关着', async () => {
    setup([proxy()])
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    await screen.findByTestId('proxy-row-phone')
    expect(screen.queryByRole('columnheader', { name: l.title })).toBeNull()
  })

  it('打开了、生效：写在用和谁拨号；详情里两端的计数并排，握手失败取自节点详情', async () => {
    const user = userEvent.setup()
    setup([ON, proxy({ id: 2, name: 'tv' })])
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    const cell = await screen.findByTestId('proxy-lane-phone')
    expect(cell).toHaveTextContent(l.inUse)
    expect(cell).toHaveTextContent(l.dials('nas'))
    // 没打开的那一行写"关"。
    expect(screen.getByTestId('proxy-row-tv')).toHaveTextContent(l.off)

    await user.click(within(cell).getByRole('button', { name: l.detailsFor('nas', 'tokyo') }))
    const table = screen.getByRole('table', { name: l.detailsTitle('nas', 'tokyo') })
    expect(within(table).getByRole('columnheader', { name: 'nas' })).toBeVisible()
    expect(within(table).getByRole('columnheader', { name: 'tokyo' })).toBeVisible()
    expect(within(table).getByText(l.row.waitedValue(3, t.common.ms(412)))).toBeVisible()
    await waitFor(() => expect(within(table).getByText(l.causeCount(l.handshakeCause.unknown_key, 2))).toBeVisible())
    expect(screen.getByText(l.sharedNote)).toBeVisible()
  })

  it('不生效：一条一条原因和修好它的那一步，用的是原因码而不是 controller 的原话', async () => {
    const summary = 'ice3proxy 不起作用，这个代理的流都走共用的连接（见 blocked）。'
    setup([
      proxy({
        lane: true,
        lane_effect: {
          effective: false,
          blocked: [{ reason: 'no_relay' }, { reason: 'node_too_old', node: 2 }],
          dialers: [],
          summary,
        },
        lanes: [],
      }),
    ])
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    const cell = await screen.findByTestId('proxy-lane-phone')
    expect(cell).toHaveTextContent(l.blocked)
    expect(cell).toHaveTextContent(l.reason.noRelay('nas', 'tokyo'))
    expect(cell).toHaveTextContent(l.reason.nodeTooOld('nas'))
    expect(cell).not.toHaveTextContent(summary)
    expect(within(cell).getByRole('link', { name: l.fix.update('nas') })).toHaveAttribute('href', '/n/1/nodes/2')
    // 两端都没报：没有详情。
    expect(within(cell).queryByRole('button', { name: l.detailsFor('nas', 'tokyo') })).toBeNull()
  })

  it('修改：开关在对话框里，和别的一起提交', async () => {
    const user = userEvent.setup()
    const api = setup([proxy()])
    renderWithApp(<ProxiesPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: t.common.actionsFor('phone') }))
    await user.click(await screen.findByRole('menuitem', { name: s.menuEdit }))
    const dialog = await screen.findByRole('dialog', { name: d.editTitle('phone') })
    const toggle = within(dialog).getByRole('switch', { name: l.title })
    expect(toggle).not.toBeChecked()
    expect(toggle).toHaveAccessibleDescription(l.switchHint)
    await user.click(toggle)
    await user.click(within(dialog).getByRole('button', { name: t.common.save }))
    await waitFor(() => expect(api.callsTo('PATCH /networks/1/proxies/1')).toHaveLength(1))
    expect(api.callsTo('PATCH /networks/1/proxies/1')[0]?.body).toMatchObject({ lane: true })
  })
})
