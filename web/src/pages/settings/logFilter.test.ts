import { describe, expect, it } from 'vitest'

import { type LogEntry } from '@/api/types'

import {
  EMPTY_LOG_FILTER,
  hasServerFilter,
  isFiltered,
  logFilterToParams,
  logFilterToQuery,
  logPathFor,
  matchesSearch,
  parseLogFilter,
  parseRef,
  refTarget,
} from './logFilter'

const params = (text: string) => new URLSearchParams(text)

describe('操作记录的筛选写在地址里', () => {
  it('什么都没写：没有条件', () => {
    expect(parseLogFilter(params(''))).toEqual(EMPTY_LOG_FILTER)
    expect(isFiltered(EMPTY_LOG_FILTER)).toBe(false)
  })

  it('网络、操作、对象、搜索词都读得出来', () => {
    const filter = parseLogFilter(params('network=2&action=credential.revoke&ref=credential:5&q=phone'))
    expect(filter).toEqual({
      network: 2,
      action: 'credential.revoke',
      ref: { kind: 'credential', id: 5 },
      search: 'phone',
    })
  })

  it('写法不对的条件当作没写：链接可能是手改过的', () => {
    const filter = parseLogFilter(params('network=abc&action=Node!&ref=widget:3'))
    expect(filter).toEqual(EMPTY_LOG_FILTER)
    expect(parseLogFilter(params('network=0')).network).toBeNull()
    expect(parseLogFilter(params('network=1.5')).network).toBeNull()
    expect(parseLogFilter(params('ref=node:0')).ref).toBeNull()
    expect(parseLogFilter(params('ref=node:x')).ref).toBeNull()
  })

  it('写回地址再读出来是一样的；默认值不写进地址', () => {
    const filter = { network: 1, action: 'node', ref: { kind: 'node', id: 3 }, search: 'nas' }
    const written = logFilterToParams(filter)
    expect(written.toString()).toBe('network=1&action=node&ref=node%3A3&q=nas')
    expect(parseLogFilter(written)).toEqual(filter)
    expect(logFilterToParams(EMPTY_LOG_FILTER).toString()).toBe('')
  })
})

describe('交给 controller 的条件', () => {
  it('网络、操作、对象交给 controller；搜索不交', () => {
    const filter = { network: 1, action: 'node', ref: { kind: 'node', id: 3 }, search: 'nas' }
    expect(logFilterToQuery(filter)).toEqual({ network: 1, action: 'node', ref: 'node:3' })
    expect(logFilterToQuery({ ...EMPTY_LOG_FILTER, search: 'x' })).toEqual({
      network: undefined,
      action: undefined,
      ref: undefined,
    })
  })

  it('只有搜索词时不算 controller 那边的条件', () => {
    expect(hasServerFilter({ ...EMPTY_LOG_FILTER, search: 'x' })).toBe(false)
    expect(isFiltered({ ...EMPTY_LOG_FILTER, search: 'x' })).toBe(true)
    expect(hasServerFilter({ ...EMPTY_LOG_FILTER, action: 'tag' })).toBe(true)
  })

  it('对象的写法和 controller 认的一样', () => {
    expect(parseRef('forward:12')).toEqual({ kind: 'forward', id: 12 })
    expect(parseRef(' token:1 ')).toEqual({ kind: 'token', id: 1 })
    expect(parseRef('node')).toBeNull()
    expect(parseRef('route:1')).toBeNull()
  })
})

describe('搜索只在取回的记录里找', () => {
  const entry: LogEntry = {
    id: 9,
    at: 1,
    network: 1,
    action: 'credential.revoke',
    detail: '凭据 #1 phone',
    refs: [],
  }

  it('找说明、操作的名字和操作本身，不分大小写', () => {
    expect(matchesSearch(entry, 'PHONE', '吊销凭据')).toBe(true)
    expect(matchesSearch(entry, '吊销', '吊销凭据')).toBe(true)
    expect(matchesSearch(entry, 'credential.revoke', '吊销凭据')).toBe(true)
    expect(matchesSearch(entry, 'tablet', '吊销凭据')).toBe(false)
    expect(matchesSearch(entry, '  ', '吊销凭据')).toBe(true)
  })
})

describe('记录里提到的东西，点了去哪里', () => {
  it('网络里的东西去它所在的页面，列表页上那一行亮出来', () => {
    expect(refTarget({ kind: 'node', id: 3 }, 1)).toBe('/n/1/nodes/3')
    expect(refTarget({ kind: 'rule', id: 2 }, 1)).toBe('/n/1/access?rule=2')
    expect(refTarget({ kind: 'credential', id: 5 }, 1)).toBe('/n/1/credentials?focus=5')
    expect(refTarget({ kind: 'listener', id: 6 }, 1)).toBe('/n/1/listeners?focus=6')
    expect(refTarget({ kind: 'forward', id: 7 }, 1)).toBe('/n/1/forwards?focus=7')
    // 用掉的令牌默认不列出，要切到"全部"。
    expect(refTarget({ kind: 'token', id: 8 }, 1)).toBe('/n/1/tokens?show=all&focus=8')
  })

  it('网络本身去它的设置页，不需要记录属于哪个网络', () => {
    expect(refTarget({ kind: 'network', id: 4 }, null)).toBe('/n/4/network')
  })

  it('不知道是哪个网络的、不认识的种类：不做成链接', () => {
    expect(refTarget({ kind: 'node', id: 3 }, null)).toBeNull()
    expect(refTarget({ kind: 'gadget', id: 3 }, 1)).toBeNull()
  })

  it('一样东西的操作记录：按它筛、限定在它的网络里', () => {
    expect(logPathFor({ kind: 'node', id: 3 }, 1)).toBe('/settings/log?network=1&ref=node%3A3')
  })
})
