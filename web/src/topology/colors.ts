/**
 * 拓扑图上线的颜色。画图的库要具体的颜色值（箭头是 SVG 的 marker），所以不能用 CSS 变量；和 index.css 里的状态色一致：
 * 直连绿，经 relay 紫（中转是正常的工作状态，只是绕了一段），不可达红，过期的灰。
 */
export const LINE_COLORS = {
  direct: '#22a35a',
  relay: '#7c66dc',
  none: '#e5484d',
  stale: '#a1a1aa',
  /** 图例里画"A → B / B → A"的线型用，不代表哪一种路。 */
  neutral: '#52525b',
} as const
