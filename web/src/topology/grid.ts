import { type CSSProperties } from 'react'

/** 表的宽度：铺满，但一列最宽 12rem（节点少时格子不被拉成一长条），最窄 4.5rem（多了横着滚）。 */
export function gridWidth(columns: number, head: string): CSSProperties {
  return {
    width: '100%',
    maxWidth: `calc(${head} + ${columns} * 12rem)`,
    minWidth: `calc(${head} + ${columns} * 4.5rem)`,
  }
}
