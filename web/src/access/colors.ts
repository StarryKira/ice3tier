import { useResolvedTheme } from '@/lib/theme'

/**
 * 连线的颜色。画图的库要的是具体的颜色值（箭头是 SVG 的 marker），所以这里不能用 CSS 变量；
 * 和 index.css 一致：平时是中性的深灰，停用的浅灰，点中的那一条用强调色。暗色里深浅对调。
 */
const LIGHT = { enabled: '#52525b', disabled: '#a1a1aa', highlighted: '#2a5bd7' }
const DARK: EdgeColors = { enabled: '#a1a1aa', disabled: '#52525b', highlighted: '#5585f0' }

export type EdgeColors = typeof LIGHT

/** 跟着亮色、暗色换的那一套。 */
export function useEdgeColors(): EdgeColors {
  return useResolvedTheme() === 'dark' ? DARK : LIGHT
}
