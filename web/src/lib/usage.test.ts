import { describe, expect, it } from 'vitest'

import { setLocale, t } from '@/i18n'

import { BYTE_UNITS, bytesToField, formatPercent, unitBytes } from './format'
import { formatDuration, formatUtcDay } from './time'
import { NEARLY_USED, quotaView, usageView } from './usage'
import { MAX_QUOTA_RESET_DAY, parseOptionalBytes, parseResetDay } from './validate'

const GB = 1024 ** 3

describe('已用和上限怎么写', () => {
  it('没有上限：只写已用的数，不画进度条', () => {
    const view = usageView(5 * GB, null)
    expect(view.ratio).toBeNull()
    expect(view.percent).toBeNull()
    expect(view.text).toBe('5.0 GB')
    expect(view.exhausted).toBe(false)
    expect(view.tone).toBe('idle')
  })

  it('不到八成是绿色，写成"已用 / 上限"', () => {
    const view = usageView(2 * GB, 10 * GB)
    expect(view.text).toBe('2.0 GB / 10.0 GB')
    expect(view.percent).toBe(20)
    expect(view.tone).toBe('ok')
    expect(view.nearly).toBe(false)
  })

  it('到了八成是琥珀色，算"快用完了"', () => {
    const view = usageView(NEARLY_USED * 10 * GB, 10 * GB)
    expect(view.tone).toBe('warn')
    expect(view.nearly).toBe(true)
    expect(view.exhausted).toBe(false)
  })

  it('用到上限是红色；超出去的部分照实写，百分数可以过 100', () => {
    expect(usageView(10 * GB, 10 * GB)).toMatchObject({ tone: 'danger', exhausted: true, nearly: false, percent: 100 })
    expect(usageView(11 * GB, 10 * GB).percent).toBe(110)
  })

  it('百分数往下取整：99.9% 还没用完，不写成 100%', () => {
    expect(formatPercent(0.999)).toBe(99)
  })
})

describe('出口的配额', () => {
  // 2026-12-01 00:00 UTC。东八区是 12 月 1 日 8 点，西五区是 11 月 30 日晚上：写的是 UTC 的日子，和设置里填的那一天对得上。
  const DEC_1 = Date.UTC(2026, 11, 1) / 1000

  it('写出账期结束的那一天（按 UTC），什么时候重新开始算', () => {
    const view = quotaView({
      limit_bytes: 100 * GB,
      used_bytes: 42 * GB,
      period_start: DEC_1 - 30 * 86400,
      period_end: DEC_1,
    })
    expect(view.text).toBe('42.0 GB / 100 GB')
    expect(view.resets).toBe(t.exits.quotaResets('12月1日'))
    expect(view.resetsAt).toBe(DEC_1)
  })

  it('用完了：可能略超过配额', () => {
    const view = quotaView({ limit_bytes: GB, used_bytes: GB + 4096, period_start: 0, period_end: DEC_1 })
    expect(view.exhausted).toBe(true)
    expect(view.tone).toBe('danger')
  })

  it('formatUtcDay 不受浏览器时区影响，按语言写', () => {
    expect(formatUtcDay(DEC_1 - 1)).toBe('11月30日')
    expect(formatUtcDay(DEC_1)).toBe('12月1日')
    setLocale('en', { remember: false })
    expect(formatUtcDay(DEC_1 - 1)).toBe('Nov 30')
    expect(quotaView({ limit_bytes: GB, used_bytes: 0, period_start: 0, period_end: DEC_1 }).resets).toBe(
      'Resets Dec 1',
    )
  })
})

describe('流量上限的输入', () => {
  it('留空是不限', () => {
    expect(parseOptionalBytes('  ', 'GB')).toEqual({ value: null, error: null })
  })

  it('一个数乘上单位，按 1024 进位，可以有小数', () => {
    expect(parseOptionalBytes('10', 'GB').value).toBe(10 * GB)
    expect(parseOptionalBytes('1.5', 'TB').value).toBe(1.5 * 1024 * GB)
    expect(parseOptionalBytes('512', 'MB').value).toBe(512 * 1024 ** 2)
  })

  it('不能是 0、负数或者别的写法', () => {
    for (const raw of ['0', '0.0', '-1', 'abc', '1e9', '10 GB']) {
      expect(parseOptionalBytes(raw, 'GB').error, raw).toBe(t.validate.byteSize)
    }
  })

  it('已有的上限放回输入框：取能让数字不小于 1 的最大单位', () => {
    expect(bytesToField(null)).toEqual({ value: '', unit: 'GB' })
    expect(bytesToField(10 * GB)).toEqual({ value: '10', unit: 'GB' })
    expect(bytesToField(1536 * 1024 ** 2)).toEqual({ value: '1.5', unit: 'GB' })
    expect(bytesToField(3 * 1024 * GB)).toEqual({ value: '3', unit: 'TB' })
    expect(bytesToField(1024)).toEqual({ value: '0.001', unit: 'MB' })
  })

  it('放回去再读出来，整数个单位不变', () => {
    for (const unit of BYTE_UNITS) {
      const bytes = 7 * unitBytes(unit)
      const field = bytesToField(bytes)
      expect(parseOptionalBytes(field.value, field.unit).value).toBe(bytes)
    }
  })
})

describe('账期从每月几号开始', () => {
  it('1 到 28', () => {
    expect(parseResetDay('1')).toEqual({ value: 1, error: null })
    expect(parseResetDay(String(MAX_QUOTA_RESET_DAY))).toEqual({ value: MAX_QUOTA_RESET_DAY, error: null })
  })

  it('29 号以后不是每个月都有，不收', () => {
    for (const raw of ['0', '29', '31', '', 'x']) {
      expect(parseResetDay(raw).error, raw).toBe(t.validate.dayOfMonth(MAX_QUOTA_RESET_DAY))
    }
  })
})

describe('一段时长', () => {
  it('取整到最大的单位', () => {
    expect(formatDuration(30)).toBe(t.time.lessThanMinute)
    expect(formatDuration(150)).toBe(t.time.minutes(2))
    expect(formatDuration(3 * 3600 + 59 * 60)).toBe(t.time.hours(3))
    expect(formatDuration(9 * 86400)).toBe(t.time.days(9))
  })
})
