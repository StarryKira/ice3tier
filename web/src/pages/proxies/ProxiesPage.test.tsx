import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type Proxy } from '@/api/types'
import { t } from '@/i18n'
import { proxyUrl } from '@/lib/proxy'
import { makeNode } from '@/test/fixtures'
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
        proxy({ id: 3, name: 'old', enabled: false }),
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
    expect(row).toHaveTextContent(s.connections(2))
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
    })
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
