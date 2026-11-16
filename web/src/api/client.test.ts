import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { t } from '@/i18n'

import { ApiError, configureClient, errorMessage, request } from './client'
import { fieldError, formError, formFailure } from './formErrors'

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json', ...headers } })
}

function apiError(status: number, code: string, message: string): Response {
  return json(status, { error: { code, message } })
}

const fetchMock = vi.fn<typeof fetch>()
const onUnauthorized = vi.fn<() => void>()
const onReachability = vi.fn<(reachable: boolean) => void>()
let token: string | null

/** 最近一次请求发出去的样子。 */
function lastCall(): { url: string; init: RequestInit; headers: Record<string, string> } {
  const call = fetchMock.mock.calls.at(-1)
  if (call === undefined) throw new Error('没有发出请求')
  const [input, init = {}] = call
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url
  return { url, init, headers: init.headers as Record<string, string> }
}

async function failure(promise: Promise<unknown>): Promise<ApiError> {
  try {
    await promise
  } catch (error) {
    if (error instanceof ApiError) return error
    throw error
  }
  throw new Error('请求本该失败')
}

beforeEach(() => {
  token = 'admin-token'
  fetchMock.mockReset()
  onUnauthorized.mockReset()
  onReachability.mockReset()
  vi.stubGlobal('fetch', fetchMock)
  configureClient({ getToken: () => token, onUnauthorized, onReachability })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('请求怎么发', () => {
  it('路径前面加 /api/v1，带上管理令牌', async () => {
    fetchMock.mockResolvedValue(json(200, []))
    await request('GET', '/networks')
    const { url, init, headers } = lastCall()
    expect(url).toBe('/api/v1/networks')
    expect(init.method).toBe('GET')
    expect(headers.Authorization).toBe('Bearer admin-token')
  })

  it('没登录时不带 Authorization 头', async () => {
    token = null
    fetchMock.mockResolvedValue(json(200, []))
    await request('GET', '/networks')
    expect(lastCall().headers.Authorization).toBeUndefined()
  })

  it('GET 和 DELETE 不带请求体', async () => {
    fetchMock.mockImplementation(() => Promise.resolve(new Response(null, { status: 204 })))
    await request('GET', '/networks')
    expect(lastCall().init.body).toBeUndefined()
    expect(lastCall().headers['Content-Type']).toBeUndefined()
    await request('DELETE', '/networks/1')
    expect(lastCall().init.body).toBeUndefined()
  })

  it('POST、PUT、PATCH 总是带 JSON 请求体，没有字段时写 {}', async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(200, {})))
    await request('POST', '/networks/1/tokens')
    expect(lastCall().init.body).toBe('{}')
    expect(lastCall().headers['Content-Type']).toBe('application/json')

    await request('PATCH', '/networks/1/nodes/2', { body: { tags: ['servers'] } })
    expect(lastCall().init.body).toBe('{"tags":["servers"]}')

    await request('PUT', '/networks/1/nodes/2/exit', { body: { ports: null } })
    expect(lastCall().init.body).toBe('{"ports":null}')
  })

  it('查询参数里没有值的不写', async () => {
    fetchMock.mockResolvedValue(json(200, []))
    await request('GET', '/networks/1/access/check', { query: { from: 2, to: 3, node: undefined } })
    expect(lastCall().url).toBe('/api/v1/networks/1/access/check?from=2&to=3')
  })

  it('204 没有应答体，返回 undefined', async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }))
    await expect(request('DELETE', '/networks/1/rules/2')).resolves.toBeUndefined()
  })

  it('成功时返回解析好的应答体', async () => {
    fetchMock.mockResolvedValue(json(201, { id: 3, name: 'lab' }))
    await expect(request('POST', '/networks', { body: { name: 'lab' } })).resolves.toEqual({ id: 3, name: 'lab' })
  })
})

describe('出错时', () => {
  it('controller 给的 code 和 message 原样放进 ApiError', async () => {
    const message = '节点 laptop 没有声明可以作为出口。在节点上执行 `i3t set can-exit on` 之后才能启用'
    fetchMock.mockResolvedValue(apiError(409, 'not_declared', message))
    const error = await failure(request('PUT', '/networks/1/nodes/3/exit', { body: {} }))
    expect(error.status).toBe(409)
    expect(error.code).toBe('not_declared')
    expect(error.message).toBe(message)
    expect(error.is('not_declared')).toBe(true)
    expect(error.is('conflict', 'name_taken')).toBe(false)
    expect(error.isNetwork).toBe(false)
    expect(errorMessage(error)).toBe(message)
  })

  it('不认识的错误码照样显示 controller 给的话', async () => {
    fetchMock.mockResolvedValue(apiError(409, 'some_future_code', '以后才有的一种错误'))
    const error = await failure(request('GET', '/networks'))
    expect(error.code).toBe('some_future_code')
    expect(error.message).toBe('以后才有的一种错误')
  })

  it('应答不是约定的格式（例如代理报的 502）时按状态码说一句', async () => {
    fetchMock.mockResolvedValue(new Response('<html>Bad Gateway</html>', { status: 502 }))
    const error = await failure(request('GET', '/status'))
    expect(error.status).toBe(502)
    expect(error.code).toBe('http_502')
    expect(error.message).toBe(t.errors.http(502))
  })

  it('JSON 里的 error 缺字段时不当成 controller 的错误', async () => {
    fetchMock.mockResolvedValue(json(500, { error: { code: 'internal' } }))
    const error = await failure(request('GET', '/status'))
    expect(error.code).toBe('http_500')
  })

  it('连不上：错误码是 network，并报告"连不上"', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'))
    const error = await failure(request('GET', '/status'))
    expect(error.status).toBe(0)
    expect(error.code).toBe('network')
    expect(error.isNetwork).toBe(true)
    expect(error.message).toBe(t.errors.network)
    expect(onReachability).toHaveBeenCalledExactlyOnceWith(false)
  })

  it('得到了应答就报告"连得上"，哪怕应答是个错误', async () => {
    fetchMock.mockResolvedValue(apiError(404, 'not_found', '没有这个节点'))
    await failure(request('GET', '/networks/1/nodes/9'))
    expect(onReachability).toHaveBeenCalledExactlyOnceWith(true)
  })

  it('被取消的请求抛出原来的 AbortError，不算连不上', async () => {
    const abort = new DOMException('aborted', 'AbortError')
    fetchMock.mockRejectedValue(abort)
    await expect(request('GET', '/status')).rejects.toBe(abort)
    expect(onReachability).not.toHaveBeenCalled()
  })

  it('成功的状态码配上读不出来的应答体：bad_response', async () => {
    fetchMock.mockResolvedValue(new Response('not json', { status: 200 }))
    const error = await failure(request('GET', '/status'))
    expect(error.code).toBe('bad_response')
    expect(error.message).toBe(t.errors.badResponse)
  })
})

describe('401', () => {
  it('带着已登录的令牌被拒绝：通知外面退出登录，并照常抛出错误', async () => {
    fetchMock.mockResolvedValue(apiError(401, 'unauthorized', '管理令牌不对'))
    const error = await failure(request('GET', '/networks'))
    expect(error.is('unauthorized')).toBe(true)
    expect(error.message).toBe('管理令牌不对')
    expect(onUnauthorized).toHaveBeenCalledOnce()
  })

  it('登录页拿一个令牌去试：用的是给的令牌，被拒绝不触发退出', async () => {
    fetchMock.mockResolvedValue(apiError(401, 'unauthorized', '管理令牌不对'))
    const error = await failure(request('GET', '/status', { token: 'typed-by-user' }))
    expect(lastCall().headers.Authorization).toBe('Bearer typed-by-user')
    expect(error.is('unauthorized')).toBe(true)
    expect(onUnauthorized).not.toHaveBeenCalled()
  })

  it('别的错误不触发退出', async () => {
    fetchMock.mockResolvedValue(apiError(409, 'name_taken', '名字 home 已经被占用'))
    await failure(request('POST', '/networks', { body: { name: 'home' } }))
    expect(onUnauthorized).not.toHaveBeenCalled()
  })
})

describe('errorMessage', () => {
  it('普通的 Error 用它自己的话；别的东西说"原因不明"', () => {
    expect(errorMessage(new Error('boom'))).toBe('boom')
    expect(errorMessage(new Error(''))).toBe(t.errors.unknown)
    expect(errorMessage('boom')).toBe(t.errors.unknown)
    expect(errorMessage(undefined)).toBe(t.errors.unknown)
  })
})

describe('把提交失败归到表单的字段上', () => {
  type FieldName = 'name' | 'cidr'
  const map = { name_taken: 'name', range_overlaps: 'cidr' } as const

  it('错误码在表里：显示在对应的字段下面', () => {
    const failed = formFailure<FieldName>(new ApiError(409, 'range_overlaps', '地址段和网络 home 的重叠'), map)
    expect(failed).toEqual({ field: 'cidr', message: '地址段和网络 home 的重叠' })
    expect(fieldError(failed, 'cidr')).toBe('地址段和网络 home 的重叠')
    expect(fieldError(failed, 'name')).toBeNull()
    expect(formError(failed)).toBeNull()
  })

  it('错误码不在表里，或者根本不是 controller 的错误：显示在表单底部', () => {
    const unknown = formFailure<FieldName>(new ApiError(500, 'internal', 'controller 内部出错'), map)
    expect(unknown).toEqual({ field: null, message: 'controller 内部出错' })
    expect(formError(unknown)).toBe('controller 内部出错')
    expect(fieldError(unknown, 'name')).toBeNull()

    expect(formError(formFailure<FieldName>(new ApiError(0, 'network', t.errors.network), map))).toBe(t.errors.network)
  })

  it('没有错误时什么都不显示', () => {
    expect(formFailure<FieldName>(null, map)).toBeNull()
    expect(formFailure<FieldName>(undefined, map)).toBeNull()
    expect(fieldError<FieldName>(null, 'name')).toBeNull()
    expect(formError<FieldName>(null)).toBeNull()
  })
})
