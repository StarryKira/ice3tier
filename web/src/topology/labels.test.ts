import { describe, expect, it } from 'vitest'

import { type Curve, pointOnCurve, type Rect } from '@/access/geometry'

import { LABEL_HEIGHT, labelWidth, placeLabels, textWidth } from './labels'

/** 一条直线：控制点在正中。 */
const straight = (x1: number, y1: number, x2: number, y2: number): Curve => ({
  start: { x: x1, y: y1 },
  control: { x: (x1 + x2) / 2, y: (y1 + y2) / 2 },
  end: { x: x2, y: y2 },
})

const boxOf = (curve: Curve, t: number, width: number): Rect => {
  const point = pointOnCurve(curve, t)
  return { x: point.x - width / 2, y: point.y - LABEL_HEIGHT / 2, width, height: LABEL_HEIGHT }
}

const overlaps = (a: Rect, b: Rect) =>
  Math.min(a.x + a.width, b.x + b.width) > Math.max(a.x, b.x) &&
  Math.min(a.y + a.height, b.y + b.height) > Math.max(a.y, b.y)

describe('线上的字往哪里放', () => {
  it('按字数估宽度：汉字比数字、字母宽', () => {
    expect(textWidth('经 h2')).toBeGreaterThan(textWidth('UDP'))
    expect(labelWidth(['UDP', '1 ms'])).toBeGreaterThan(textWidth('UDP') + textWidth('1 ms'))
  })

  it('不挤的时候在正中', () => {
    const placement = placeLabels([{ id: 'a', curve: straight(0, 0, 400, 0), width: 80, height: LABEL_HEIGHT }], [])
    expect(placement.get('a')).toBe(0.5)
  })

  it('两条挨着的竖线：字一前一后错开，不压在一起', () => {
    const left = straight(0, 0, 0, 400)
    const right = straight(40, 0, 40, 400)
    const width = 150
    const placement = placeLabels(
      [
        { id: 'left', curve: left, width, height: LABEL_HEIGHT },
        { id: 'right', curve: right, width, height: LABEL_HEIGHT },
      ],
      [],
    )
    const a = placement.get('left') ?? 0
    const b = placement.get('right') ?? 0
    expect(a).not.toBe(b)
    expect(overlaps(boxOf(left, a, width), boxOf(right, b, width))).toBe(false)
  })

  it('正中压着一张卡片：挪开', () => {
    const line = straight(0, 0, 600, 0)
    const card: Rect = { x: 250, y: -28, width: 100, height: 56 }
    const t = placeLabels([{ id: 'a', curve: line, width: 80, height: LABEL_HEIGHT }], [card]).get('a') ?? 0.5
    expect(t).not.toBe(0.5)
    expect(overlaps(boxOf(line, t, 80), card)).toBe(false)
  })
})
