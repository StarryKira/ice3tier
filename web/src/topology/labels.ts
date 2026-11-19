import { type Curve, type Point, pointOnCurve, type Rect } from '@/access/geometry'

/**
 * 拓扑图上线的字往哪里放：默认在线的正中，和别的字、节点卡片压在一起时沿着线往两头挪，挑最不挤的地方。纯函数。
 *
 * 字的大小按字数估：画布上的字是 11 像素，汉字一个字宽、别的半个字宽多一点，再加上胶囊两头的留白。
 * 估得宽一点没关系，只是挪得早一些。
 */

export interface LabelItem {
  id: string
  curve: Curve
  width: number
  height: number
}

/** 胶囊的高度。 */
export const LABEL_HEIGHT = 20

/** 字在 11 像素下大概多宽：汉字 11，数字和拉丁字母 6.6。 */
export function textWidth(text: string): number {
  let width = 0
  for (const char of text) width += (char.codePointAt(0) ?? 0) >= 0x2e80 ? 11 : 6.6
  return width
}

/** 几段字排成一个胶囊有多宽：段与段之间隔 4 像素，两头各留 7 像素，再加边框。 */
export function labelWidth(parts: readonly string[]): number {
  const text = parts.reduce((total, part) => total + textWidth(part), 0)
  return Math.ceil(text + Math.max(0, parts.length - 1) * 4 + 14 + 2)
}

/** 从正中开始，一左一右往两头试。 */
const CANDIDATES = [0.5, 0.42, 0.58, 0.34, 0.66, 0.27, 0.73, 0.2, 0.8] as const

/** 字和别的字、卡片之间至少留这么宽。 */
const GAP = 3

function boxAt(point: Point, width: number, height: number): Rect {
  return {
    x: point.x - width / 2 - GAP,
    y: point.y - height / 2 - GAP,
    width: width + 2 * GAP,
    height: height + 2 * GAP,
  }
}

function overlap(a: Rect, b: Rect): number {
  const x = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x)
  const y = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y)
  return x > 0 && y > 0 ? x * y : 0
}

/**
 * 每个字放在它那条线上的哪里（参数 t，0 是起点、1 是终点）。短的线可挪的地方少，先放；
 * 一个位置和已经放下的字、卡片都不压就用它，都压的话用压得最少的那个。压卡片比压字更糟，算两倍。
 */
export function placeLabels(items: readonly LabelItem[], cards: readonly Rect[]): Map<string, number> {
  const length = (curve: Curve) => Math.hypot(curve.end.x - curve.start.x, curve.end.y - curve.start.y)
  const order = [...items].sort((a, b) => length(a.curve) - length(b.curve) || a.id.localeCompare(b.id))
  const placed: Rect[] = []
  const result = new Map<string, number>()
  for (const item of order) {
    let best = {
      t: 0.5,
      score: Number.POSITIVE_INFINITY,
      box: boxAt(pointOnCurve(item.curve, 0.5), item.width, item.height),
    }
    for (const t of CANDIDATES) {
      const box = boxAt(pointOnCurve(item.curve, t), item.width, item.height)
      const score =
        placed.reduce((total, other) => total + overlap(box, other), 0) +
        2 * cards.reduce((total, card) => total + overlap(box, card), 0)
      if (score < best.score) best = { t, score, box }
      if (score === 0) break
    }
    placed.push(best.box)
    result.set(item.id, best.t)
  }
  return result
}
