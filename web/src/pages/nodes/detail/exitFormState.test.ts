import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { makeExitSettings } from '@/test/fixtures'

import { exitFormFrom, exitFormProblems, exitFormToInput, exitFormValid } from './exitFormState'

const GB = 1024 ** 3

describe('出口设置的表单', () => {
  it('第一次启用：默认值（端口不限），提交的是 PUT 要的全部字段', () => {
    expect(exitFormToInput(exitFormFrom(null))).toEqual({
      ports: null,
      lan: false,
      upstream_socks5: null,
      quota_bytes: null,
      quota_reset_day: 1,
      family: 'any',
      connects_per_minute: null,
      class: null,
    })
  })

  it('已有的设置原样读进来，原样交回去', () => {
    const settings = makeExitSettings({
      ports: null,
      lan: true,
      quota_bytes: 500 * GB,
      quota_reset_day: 15,
      family: 'ipv4',
      connects_per_minute: 120,
      class: 'bulk',
    })
    expect(exitFormToInput(exitFormFrom(settings))).toEqual({
      ports: null,
      lan: true,
      upstream_socks5: null,
      quota_bytes: 500 * GB,
      quota_reset_day: 15,
      family: 'ipv4',
      connects_per_minute: 120,
      class: 'bulk',
    })
  })

  it('配额不是整数个单位时，没动过就按原来的字节数交回去，不让四舍五入改掉它', () => {
    const odd = 123_456_789_012
    const state = exitFormFrom(makeExitSettings({ quota_bytes: odd }))
    expect(state.quota).not.toBe(String(odd))
    expect(exitFormToInput(state).quota_bytes).toBe(odd)
    expect(exitFormToInput({ ...state, quota: '200', quotaUnit: 'GB' }).quota_bytes).toBe(200 * GB)
  })

  it('配额清空就是不限，账期回到默认', () => {
    const state = exitFormFrom(makeExitSettings({ quota_bytes: GB, quota_reset_day: 9 }))
    const input = exitFormToInput({ ...state, quota: '' })
    expect(input.quota_bytes).toBeNull()
    expect(input.quota_reset_day).toBe(1)
  })

  it('账期只在有配额时检查', () => {
    const state = { ...exitFormFrom(null), resetDay: '31' }
    expect(exitFormProblems(state).resetDay).toBeNull()
    expect(exitFormProblems({ ...state, quota: '10' }).resetDay).toBe(t.validate.dayOfMonth(28))
  })

  it('配额、建连预算不能是 0（controller 不收），写法不对的拦下来', () => {
    const state = exitFormFrom(null)
    expect(exitFormValid({ ...state, quota: '0' })).toBe(false)
    expect(exitFormValid({ ...state, connects: '0' })).toBe(false)
    expect(exitFormValid({ ...state, connects: '60', quota: '1.5' })).toBe(true)
  })
})
