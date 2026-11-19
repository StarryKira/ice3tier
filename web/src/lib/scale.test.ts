import { describe, expect, it } from 'vitest'

import { niceRange, niceScale } from './scale'

describe('整齐的刻度', () => {
  it('取 1、2、2.5、5 乘以 10 的几次方做一段，段数不超过 5，最大的刻度不小于数据', () => {
    expect(niceScale(236)).toEqual({ min: 0, max: 250, ticks: [0, 50, 100, 150, 200, 250] })
    expect(niceScale(92.5)).toEqual({ min: 0, max: 100, ticks: [0, 20, 40, 60, 80, 100] })
    expect(niceScale(0.4)).toEqual({ min: 0, max: 0.4, ticks: [0, 0.1, 0.2, 0.3, 0.4] })
    expect(niceScale(100)).toEqual({ min: 0, max: 100, ticks: [0, 20, 40, 60, 80, 100] })
  })

  it('没有数：0 到 1', () => {
    expect(niceScale(0)).toEqual({ min: 0, max: 1, ticks: [0, 1] })
  })

  it('点和区间的图不从 0 开始：两头往外取整，盖住数据，刻度不超过 6 个', () => {
    expect(niceRange(155, 182)).toEqual({ min: 150, max: 190, ticks: [150, 160, 170, 180, 190] })
    const range = niceRange(155, 260)
    expect(range.min).toBeLessThanOrEqual(155)
    expect(range.max).toBeGreaterThanOrEqual(260)
    expect(range.ticks.length).toBeLessThanOrEqual(6)
  })
})
