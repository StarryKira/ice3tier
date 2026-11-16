import { useQuery } from '@tanstack/react-query'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Route, Routes, useLocation } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { configureClient } from '@/api/client'
import { api as endpoints } from '@/api/endpoints'
import { type Status } from '@/api/types'
import { t } from '@/i18n'
import { apiFailure, fakeApi, type FakeApi, renderWithApp, type Routes as ApiRoutes } from '@/test/harness'

import { RequireAuth } from './RequireAuth'
import { session } from './session'
import { SignInPage } from './SignInPage'

const KEY = 'i3t.adminToken'
const ADMIN_TOKEN = 'kq3ZbV2m0n8YwQ1r5t7u9x-AbCdEfGhIjKlMnOpQrSt'

const STATUS: Status = {
  version: '0.1.0',
  controller_key: 'LltlBKa6eUXymVVonwneDvQzlnt_0z1jqdmwdPsruNY',
  public_addr: 'ctl.example.com:20331',
  networks: 0,
  nodes: 0,
  online: 0,
  newer_version: null,
}

/** 登录之后的页面：写出现在的地址，并且带着令牌去取一次数据。 */
function Inside() {
  const location = useLocation()
  const networks = useQuery({ queryKey: ['networks'], queryFn: endpoints.networks, retry: false })
  return (
    <div>
      <p data-testid="where">{location.pathname + location.search}</p>
      <p data-testid="networks">{networks.isSuccess ? 'ok' : networks.status}</p>
    </div>
  )
}

function App() {
  return (
    <Routes>
      <Route path="/login" element={<SignInPage />} />
      <Route element={<RequireAuth />}>
        <Route path="*" element={<Inside />} />
      </Route>
    </Routes>
  )
}

/** 一个只认 `ADMIN_TOKEN` 的 controller，和应用启动时一样把登录状态接到 API 客户端上。 */
function controller(extra: ApiRoutes = {}): FakeApi {
  const api = fakeApi({
    'GET /status': (call) =>
      call.headers.Authorization === `Bearer ${ADMIN_TOKEN}` ? STATUS : apiFailure(401, 'unauthorized', '管理令牌不对'),
    'GET /networks': (call) =>
      call.headers.Authorization === `Bearer ${ADMIN_TOKEN}` ? [] : apiFailure(401, 'unauthorized', '管理令牌不对'),
    ...extra,
  })
  configureClient({
    getToken: session.token,
    onUnauthorized: () => session.signOut('expired'),
    onReachability: () => undefined,
  })
  return api
}

async function submit(user: ReturnType<typeof userEvent.setup>, token: string): Promise<void> {
  const input = screen.getByLabelText(t.signIn.tokenLabel)
  await user.clear(input)
  if (token !== '') await user.type(input, token)
  await user.click(screen.getByRole('button', { name: t.signIn.submit }))
}

beforeEach(() => {
  // 登录状态是模块里的一个变量，上一个测试留下的要清掉。
  session.signOut('manual')
  session.takeSignOutReason()
})

describe('登录', () => {
  it('没登录时任何地址都落到登录页', async () => {
    controller()
    renderWithApp(<App />, { route: '/n/1/nodes?status=online' })
    expect(await screen.findByRole('heading', { name: t.signIn.title })).toBeVisible()
    expect(screen.queryByTestId('where')).not.toBeInTheDocument()
  })

  it('填对令牌：拿它去取一次状态，成功后回到原来要去的地址，之后的请求带着它', async () => {
    const user = userEvent.setup()
    const api = controller()
    renderWithApp(<App />, { route: '/n/1/nodes?status=online' })
    await submit(user, ADMIN_TOKEN)

    expect(await screen.findByTestId('where')).toHaveTextContent('/n/1/nodes?status=online')
    expect(api.callsTo('GET /status')[0]?.headers.Authorization).toBe(`Bearer ${ADMIN_TOKEN}`)
    await waitFor(() => expect(screen.getByTestId('networks')).toHaveTextContent('ok'))
    expect(api.callsTo('GET /networks')[0]?.headers.Authorization).toBe(`Bearer ${ADMIN_TOKEN}`)
    expect(session.token()).toBe(ADMIN_TOKEN)
  })

  it('默认保持登录：令牌放在 localStorage', async () => {
    const user = userEvent.setup()
    controller()
    renderWithApp(<App />, { route: '/login' })
    await submit(user, ADMIN_TOKEN)
    await screen.findByTestId('where')
    expect(window.localStorage.getItem(KEY)).toBe(ADMIN_TOKEN)
    expect(window.sessionStorage.getItem(KEY)).toBeNull()
  })

  it('不勾"保持登录"：令牌只放在 sessionStorage，关掉标签页就没了', async () => {
    const user = userEvent.setup()
    controller()
    renderWithApp(<App />, { route: '/login' })
    await user.click(screen.getByRole('checkbox', { name: new RegExp(t.signIn.remember) }))
    await submit(user, ADMIN_TOKEN)
    await screen.findByTestId('where')
    expect(window.sessionStorage.getItem(KEY)).toBe(ADMIN_TOKEN)
    expect(window.localStorage.getItem(KEY)).toBeNull()
  })

  it('粘贴时带进来的首尾空白不算', async () => {
    const user = userEvent.setup()
    const api = controller()
    renderWithApp(<App />, { route: '/login' })
    await submit(user, `  ${ADMIN_TOKEN}  `)
    await screen.findByTestId('where')
    expect(api.callsTo('GET /status')[0]?.headers.Authorization).toBe(`Bearer ${ADMIN_TOKEN}`)
    expect(session.token()).toBe(ADMIN_TOKEN)
  })

  it('令牌不对：controller 的话写在输入框下面，留在登录页，什么都不存', async () => {
    const user = userEvent.setup()
    controller()
    renderWithApp(<App />, { route: '/login' })
    await submit(user, 'not-the-token')

    const input = screen.getByLabelText(t.signIn.tokenLabel)
    expect(await screen.findByRole('alert')).toHaveTextContent('管理令牌不对')
    expect(input).toHaveAttribute('aria-invalid', 'true')
    expect(screen.getByRole('heading', { name: t.signIn.title })).toBeVisible()
    expect(session.token()).toBeNull()
    expect(window.localStorage.getItem(KEY)).toBeNull()
    expect(window.sessionStorage.getItem(KEY)).toBeNull()
    // 这不是"登录已失效"：只是这一次填错了。
    expect(screen.queryByText(t.signIn.expired)).not.toBeInTheDocument()

    // 一改输入，错误就撤掉。
    await user.type(input, 'x')
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('没填就提交：不发请求', async () => {
    const user = userEvent.setup()
    const api = controller()
    renderWithApp(<App />, { route: '/login' })
    await submit(user, '')
    expect(screen.getByRole('alert')).toHaveTextContent(t.signIn.empty)
    expect(api.calls).toHaveLength(0)
  })

  it('连不上 controller：说连不上，不说令牌不对', async () => {
    const user = userEvent.setup()
    controller()
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')))
    renderWithApp(<App />, { route: '/login' })
    await submit(user, ADMIN_TOKEN)
    expect(await screen.findByRole('alert')).toHaveTextContent(t.errors.network)
    expect(screen.getByLabelText(t.signIn.tokenLabel)).not.toHaveAttribute('aria-invalid')
    expect(session.token()).toBeNull()
  })

  it('令牌默认不显示，可以点一下看', async () => {
    const user = userEvent.setup()
    controller()
    renderWithApp(<App />, { route: '/login' })
    const input = screen.getByLabelText(t.signIn.tokenLabel)
    expect(input).toHaveAttribute('type', 'password')
    await user.click(screen.getByRole('button', { name: t.signIn.show }))
    expect(input).toHaveAttribute('type', 'text')
    await user.click(screen.getByRole('button', { name: t.signIn.hide }))
    expect(input).toHaveAttribute('type', 'password')
  })

  it('next 只接受站内的路径', async () => {
    const user = userEvent.setup()
    controller()
    renderWithApp(<App />, { route: '/login?next=//evil.example/steal' })
    await submit(user, ADMIN_TOKEN)
    expect(await screen.findByTestId('where')).toHaveTextContent(/^\/$/)
  })

  it('已经登录时打开登录页，直接进去', async () => {
    controller()
    session.signIn(ADMIN_TOKEN, true)
    renderWithApp(<App />, { route: '/login?next=%2Fn%2F1%2Faccess' })
    expect(await screen.findByTestId('where')).toHaveTextContent('/n/1/access')
  })
})

describe('登录失效', () => {
  it('带着令牌的请求被拒绝（令牌被重新生成过）：回到登录页，说明原因，记着原来的地址', async () => {
    const user = userEvent.setup()
    controller()
    // 浏览器里存着的是一个已经作废的令牌。
    session.signIn('revoked-token', true)
    renderWithApp(<App />, { route: '/n/1/forwards' })

    expect(await screen.findByRole('heading', { name: t.signIn.title })).toBeVisible()
    expect(screen.getByText(t.signIn.expired)).toBeVisible()
    expect(session.token()).toBeNull()
    expect(window.localStorage.getItem(KEY)).toBeNull()

    await submit(user, ADMIN_TOKEN)
    expect(await screen.findByTestId('where')).toHaveTextContent('/n/1/forwards')
  })

  it('退出的原因只说一次', () => {
    session.signIn(ADMIN_TOKEN, false)
    session.signOut('expired')
    expect(session.takeSignOutReason()).toBe('expired')
    expect(session.takeSignOutReason()).toBeNull()
  })

  it('主动退出：两处存的令牌都清掉，订阅者得到通知', () => {
    const listener = vi.fn()
    const unsubscribe = session.subscribe(listener)
    session.signIn(ADMIN_TOKEN, true)
    expect(listener).toHaveBeenCalledTimes(1)
    session.signOut('manual')
    expect(listener).toHaveBeenCalledTimes(2)
    expect(session.token()).toBeNull()
    expect(window.localStorage.getItem(KEY)).toBeNull()
    expect(session.takeSignOutReason()).toBe('manual')
    unsubscribe()
  })

  it('换一种保持方式登录时，另一处存的旧令牌不留着', () => {
    session.signIn('old-token', true)
    session.signIn(ADMIN_TOKEN, false)
    expect(window.localStorage.getItem(KEY)).toBeNull()
    expect(window.sessionStorage.getItem(KEY)).toBe(ADMIN_TOKEN)
  })
})
