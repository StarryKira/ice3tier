import { describe, expect, it } from 'vitest'

import { formatBytes, formatList, formatNumber } from '@/lib/format'
import { formatDuration, formatFullTime, formatRelative, formatUtcDay } from '@/lib/time'

import { en } from './en'
import { getLocale, initialLocale, LOCALES, matchLocale, type Messages, setLocale, t } from './index'
import { zhCN } from './zh-CN'

type Tree = Record<string, unknown>

/** 目录里每一项的路径和值。 */
function leaves(tree: unknown, path = ''): [string, unknown][] {
  if (typeof tree === 'object' && tree !== null && !Array.isArray(tree)) {
    return Object.entries(tree as Tree).flatMap(([key, value]) => leaves(value, path === '' ? key : `${path}.${key}`))
  }
  return [[path, tree]]
}

/**
 * 每个参数都给 2：数字参数是 2，字符串参数写进句子里是"2"。只是为了看函数能不能写出一句话。
 */
function sample(fn: (...args: unknown[]) => unknown): unknown {
  return fn(...Array.from({ length: fn.length }, () => 2))
}

/** 一种语言的每一句话：字符串原样，函数用样例参数调一次。 */
function sentences(messages: Messages): [string, unknown][] {
  return leaves(messages).flatMap(([path, value]): [string, unknown][] => {
    if (typeof value === 'function') return [[`${path}()`, sample(value as (...args: unknown[]) => unknown)]]
    if (Array.isArray(value)) return value.map((item, index) => [`${path}[${index}]`, item])
    return [[path, value]]
  })
}

const HAN = /\p{Script=Han}/u

describe('每种语言的目录都是完整的', () => {
  for (const locale of LOCALES) {
    it(`${locale.code}：没有空的字符串，函数都写得出一句话`, () => {
      const empty = sentences(locale.messages).filter(([, value]) => typeof value !== 'string' || value.trim() === '')
      // 有几个函数在没有可说的时候返回 null（例如没有关联配置时的删除说明），样例参数下它们也该有话。
      expect(empty.map(([path]) => path)).toEqual([])
    })
  }

  it('各语言的键一样（类型管不到的 Record 也一样）', () => {
    const keysOf = (messages: Messages) => leaves(messages).map(([path]) => path)
    for (const locale of LOCALES) expect(keysOf(locale.messages)).toEqual(keysOf(zhCN))
  })

  it('英文目录里没有中文', () => {
    const chinese = sentences(en).filter(([, value]) => typeof value === 'string' && HAN.test(value))
    expect(chinese).toEqual([])
  })

  it('英文：冒号后面的状态短语不大写开头，缩写不动', () => {
    expect(en.overview.proxyBroken('lan', en.proxies.status.port_in_use)).toBe('Proxy lan: port in use')
    expect(en.overview.forwardBlocked('web', en.forwards.status.targetOffline)).toMatch(/^Port forward web: target /)
  })

  it('每种语言用它自己的文字写名字', () => {
    expect(LOCALES.map((locale) => locale.name)).toEqual(['简体中文', 'English'])
  })
})

describe('第一次打开时的语言', () => {
  it('按浏览器的偏好挑第一种有的语言', () => {
    expect(matchLocale(['zh-CN', 'en'])).toBe('zh-CN')
    expect(matchLocale(['en-US', 'zh-CN'])).toBe('en')
    expect(matchLocale(['fr-FR', 'zh-TW'])).toBe('zh-CN')
    expect(matchLocale(['EN-gb'])).toBe('en')
  })

  it('一种也没有时用英文', () => {
    expect(matchLocale(['de-DE', 'ja'])).toBe('en')
    expect(matchLocale([])).toBe('en')
  })

  it('记住的选择优先于浏览器的偏好', () => {
    // jsdom 的浏览器偏好是 en-US。
    expect(initialLocale()).toBe('en')
    window.localStorage.setItem('i3t.locale', JSON.stringify('zh-CN'))
    expect(initialLocale()).toBe('zh-CN')
    window.localStorage.setItem('i3t.locale', JSON.stringify('xx'))
    expect(initialLocale()).toBe('en')
  })
})

describe('换语言', () => {
  it('就地改写目录：分区的简写跟着变', () => {
    const detail = t.nodes.detail
    const signIn = t.signIn
    setLocale('en')
    expect(t.nodes.detail).toBe(detail)
    expect(signIn.submit).toBe(en.signIn.submit)
    expect(detail.tabRoutes).toBe(en.nodes.detail.tabRoutes)
    setLocale('zh-CN')
    expect(signIn.submit).toBe(zhCN.signIn.submit)
  })

  it('改 <html lang>，并记住选择', () => {
    setLocale('en')
    expect(getLocale()).toBe('en')
    expect(document.documentElement.lang).toBe('en')
    expect(window.localStorage.getItem('i3t.locale')).toBe('"en"')
    setLocale('zh-CN')
    expect(document.documentElement.lang).toBe('zh-CN')
    expect(window.localStorage.getItem('i3t.locale')).toBe('"zh-CN"')
  })

  it('remember 为假时不记', () => {
    setLocale('en', { remember: false })
    expect(window.localStorage.getItem('i3t.locale')).toBeNull()
  })
})

describe('数、字节、时间按语言写', () => {
  const NOW = Date.UTC(2026, 10, 16, 8, 0, 0) / 1000

  it('数和字节数', () => {
    setLocale('en', { remember: false })
    expect(formatNumber(1234.5, 1)).toBe('1,234.5')
    expect(formatBytes(1536)).toBe('1.5 KB')
    expect(formatBytes(5 * 1024 ** 3)).toBe('5.0 GB')
    setLocale('zh-CN', { remember: false })
    expect(formatBytes(1536)).toBe('1.5 KB')
  })

  it('相对时间和时长', () => {
    setLocale('en', { remember: false })
    expect(formatRelative(NOW - 180, NOW)).toBe('3 min ago')
    expect(formatRelative(NOW - 3600, NOW)).toBe('1 hour ago')
    expect(formatRelative(NOW - 3 * 86400, NOW)).toBe('3 days ago')
    expect(formatDuration(7200)).toBe('2 hours')
    setLocale('zh-CN', { remember: false })
    expect(formatRelative(NOW - 180, NOW)).toBe('3 分钟前')
    expect(formatDuration(7200)).toBe('2 小时')
  })

  it('日期', () => {
    setLocale('en', { remember: false })
    expect(formatUtcDay(Date.UTC(2026, 11, 1) / 1000)).toBe('Dec 1')
    // 时刻按机器的时区写，只看日期的写法。
    expect(formatFullTime(NOW)).toMatch(/^11\/1\d\/2026, /)
    setLocale('zh-CN', { remember: false })
    expect(formatUtcDay(Date.UTC(2026, 11, 1) / 1000)).toBe('12月1日')
    expect(formatFullTime(NOW)).toMatch(/^2026\/11\/1\d /)
  })

  it('并列的名字', () => {
    setLocale('en', { remember: false })
    expect(formatList(['a', 'b', 'c'])).toBe('a, b, c')
    setLocale('zh-CN', { remember: false })
    expect(formatList(['a', 'b', 'c'])).toBe('a、b、c')
  })
})
