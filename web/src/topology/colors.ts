/**
 * 拓扑图上线的颜色。画图的库要具体的颜色值（箭头是 SVG 的 marker），所以不能用 CSS 变量；和 index.css 里的一致：
 * 直连绿，经 relay 蓝（中转是正常的工作状态），不可达红，过期的灰。
 */
export const LINE_COLORS = {
  direct: '#1f9d63',
  relay: '#3b78d8',
  none: '#d0362d',
  stale: '#9aa5b1',
  /** 图例里画"A → B / B → A"的线型用，不代表哪一种路。 */
  neutral: '#46515f',
} as const
