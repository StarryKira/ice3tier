import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { makeToken } from '@/test/fixtures'

import { sortTokens, tokenLabel, tokenView } from './tokenState'

const k = t.tokens
const NOW = 1_794_050_000
const NAS = { id: 4, name: 'nas', virtual_ip: '10.66.0.4' }

describe('注册令牌的情况', () => {
  it('还能用：写还剩多久；删除这一步是收回', () => {
    const view = tokenView(makeToken(1, { expires_at: NOW + 3 * 3600 }), NOW)
    expect(view).toMatchObject({ tone: 'relay', label: k.unused(t.time.hours(3)), removal: 'revoke', usedBy: null })
  })

  it('controller 说还能用、浏览器的钟已经过了：按过期写，但还是收回', () => {
    expect(tokenView(makeToken(1, { expires_at: NOW - 10 }), NOW)).toMatchObject({
      label: k.expired,
      removal: 'revoke',
    })
  })

  it('用掉了：写是哪个节点、什么时候；删除这一步只是删掉记录', () => {
    const view = tokenView(makeToken(1, { state: 'used', used_at: NOW - 60, used_by: NAS }), NOW)
    expect(view).toMatchObject({ tone: 'ok', label: k.used, usedBy: NAS, usedAt: NOW - 60, removal: 'forget' })
    expect(view.usedByDeleted).toBe(false)
  })

  it('用掉了，那个节点后来被删掉了：说出来，不留一个空链接', () => {
    const view = tokenView(makeToken(1, { state: 'used', used_at: NOW - 60, used_by: null }), NOW)
    expect(view.usedBy).toBeNull()
    expect(view.usedByDeleted).toBe(true)
  })

  it('过期了、没用过', () => {
    expect(tokenView(makeToken(1, { state: 'expired' }), NOW)).toMatchObject({
      tone: 'idle',
      label: k.expired,
      removal: 'forget',
    })
  })

  it('不认识的状态', () => {
    expect(tokenView(makeToken(1, { state: 'unknown' }), NOW).label).toBe(k.unknownState)
  })
})

describe('令牌的叫法和顺序', () => {
  it('用开头的一段认；以前的令牌没有前缀，用编号', () => {
    expect(tokenLabel(makeToken(7, { prefix: 'i3t-eyJzIjoiSmFY' }))).toBe('i3t-eyJzIjoiSmFY…')
    expect(tokenLabel(makeToken(7, { prefix: '' }))).toBe('#7')
  })

  it('还能用的在前，其余的新的在前', () => {
    const tokens = [
      makeToken(1, { state: 'used' }),
      makeToken(2, { state: 'pending' }),
      makeToken(3, { state: 'expired' }),
      makeToken(4, { state: 'pending' }),
    ]
    expect(sortTokens(tokens).map((token) => token.id)).toEqual([4, 2, 3, 1])
  })
})
