/**
 * 连线的颜色。画图的库要的是具体的颜色值（箭头是 SVG 的 marker），所以这里不能用 CSS 变量；
 * 和 index.css 一致：平时是中性的深灰，停用的浅灰，点中的那一条用强调色。
 */
export const EDGE_COLORS = { enabled: '#52525b', disabled: '#a1a1aa', highlighted: '#2a5bd7' } as const
