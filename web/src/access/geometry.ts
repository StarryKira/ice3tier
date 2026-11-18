/**
 * 关系图上连线的几何。点画成矩形，线从一个矩形的边连到另一个矩形的边，
 * 同一对点之间有几条线时各自弯向一侧，不叠在一起。
 */

export interface Rect {
  x: number
  y: number
  width: number
  height: number
}

export interface Point {
  x: number
  y: number
}

const center = (rect: Rect): Point => ({ x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 })

/** 从矩形中心朝 `toward` 走，和矩形边界相交的那一点。箭头就落在这里。 */
export function borderPoint(rect: Rect, toward: Point): Point {
  const c = center(rect)
  const dx = toward.x - c.x
  const dy = toward.y - c.y
  if (dx === 0 && dy === 0) return c
  const scale = Math.min(
    dx === 0 ? Number.POSITIVE_INFINITY : rect.width / 2 / Math.abs(dx),
    dy === 0 ? Number.POSITIVE_INFINITY : rect.height / 2 / Math.abs(dy),
  )
  return { x: c.x + dx * scale, y: c.y + dy * scale }
}

export interface EdgePath {
  /** SVG 的 path。 */
  d: string
  /** 放名字的位置：曲线的中点。 */
  label: Point
}

/** 相邻两条平行线之间错开多少。 */
const PARALLEL_GAP = 44

/**
 * 两个矩形之间的一条线。`index` / `siblings` 说明这是同一对点之间的第几条：
 * 只有一条时是直线；多条时围绕直线对称地弯开。
 */
export function curveBetween(source: Rect, target: Rect, index = 0, siblings = 1): EdgePath {
  const a = center(source)
  const b = center(target)
  const dx = b.x - a.x
  const dy = b.y - a.y
  const length = Math.hypot(dx, dy) || 1
  // 垂直于连线方向的单位向量。方向按两个中心的坐标固定下来，不随谁是 source 而翻转，
  // 这样同一对点之间方向相反的两条规则也会弯向不同的两侧。
  const flip = a.x < b.x || (a.x === b.x && a.y < b.y) ? 1 : -1
  const nx = (-dy / length) * flip
  const ny = (dx / length) * flip
  const offset = (index - (siblings - 1) / 2) * PARALLEL_GAP
  const control: Point = { x: (a.x + b.x) / 2 + nx * offset * 2, y: (a.y + b.y) / 2 + ny * offset * 2 }
  const start = borderPoint(source, control)
  const end = borderPoint(target, control)
  return {
    d: `M ${start.x} ${start.y} Q ${control.x} ${control.y} ${end.x} ${end.y}`,
    // 二次贝塞尔曲线在 t = 0.5 处的点。
    label: { x: 0.25 * start.x + 0.5 * control.x + 0.25 * end.x, y: 0.25 * start.y + 0.5 * control.y + 0.25 * end.y },
  }
}
