import { useResolvedTheme } from '@/lib/theme'

/**
 * 拓扑图上线的颜色。画图的库要具体的颜色值（箭头是 SVG 的 marker），所以不能用 CSS 变量；和 index.css 里的状态色一致：
 * 直连绿，经 relay 紫（中转是正常的工作状态，只是绕了一段），不可达红，过期的灰。暗色里各提亮一档。
 */
const LIGHT = {
  direct: '#22a35a',
  relay: '#7c66dc',
  none: '#e5484d',
  stale: '#a1a1aa',
  /** 图例里画"A → B / B → A"的线型用，不代表哪一种路。 */
  neutral: '#52525b',
}

const DARK: LineColors = {
  direct: '#3fb67a',
  relay: '#9584f2',
  none: '#f0595e',
  stale: '#5f5f68',
  neutral: '#a1a1aa',
}

export type LineColors = typeof LIGHT

/** 跟着亮色、暗色换的那一套。 */
export function useLineColors(): LineColors {
  return useResolvedTheme() === 'dark' ? DARK : LIGHT
}
