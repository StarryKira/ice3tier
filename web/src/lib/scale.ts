/** 一把尺子：从 `min` 到 `max`，刻度在 `ticks`。 */
export interface Scale {
  min: number
  max: number
  ticks: number[]
}

/**
 * `lo` 到 `hi` 之间取整齐的刻度：一段是 1、2、2.5、5 乘以 10 的几次方，取段数不超过 `count + 1` 的最小一种，
 * 两头往外取整，盖住数据。给图表的轴用；条形图从 0 开始（`niceScale`），点和区间的图可以不从 0 开始。
 */
export function niceRange(lo: number, hi: number, count = 4): Scale {
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) return { min: 0, max: 1, ticks: [0, 1] }
  const low = Math.max(0, Math.min(lo, hi))
  const high = Math.max(lo, hi, low + 1e-9)
  if (high <= 0) return { min: 0, max: 1, ticks: [0, 1] }
  const power = 10 ** Math.floor(Math.log10((high - low) / count))
  const fits = (step: number) => Math.ceil(high / step - 1e-9) - Math.floor(low / step + 1e-9) <= count + 1
  const step = [1, 2, 2.5, 5, 10, 20].map((factor) => factor * power).find(fits) ?? 20 * power
  const min = Math.floor(low / step + 1e-9) * step
  const max = Math.ceil(high / step - 1e-9) * step
  const ticks: number[] = []
  for (let index = 0; min + index * step <= max + step / 2; index += 1) {
    ticks.push(Math.round((min + index * step) * 1e6) / 1e6)
  }
  return { min: Math.round(min * 1e6) / 1e6, max: Math.round(max * 1e6) / 1e6, ticks }
}

/** 从 0 开始的尺子：条形图的长短要从 0 量。没有数时是 0 到 1。 */
export function niceScale(max: number, count = 4): Scale {
  if (!Number.isFinite(max) || max <= 0) return { min: 0, max: 1, ticks: [0, 1] }
  return niceRange(0, max, count)
}
