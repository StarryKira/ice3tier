import { createContext, useContext } from 'react'

import { type Route } from '@/api/types'

/**
 * 报告里的联动：鼠标停在任何一处的一条路上（延迟、吞吐、候选路径、明细、路径的表），所有图表里同一条路亮着，
 * 别的淡下去。路按 `routeKey` 认：直连一个，经中继的按经过的中继，两个方向倒过来写算同一条。
 */
export interface Focus {
  focus: string | null
  setFocus: (key: string | null) => void
}

export const FocusContext = createContext<Focus>({ focus: null, setFocus: () => undefined })

export function useFocus(): Focus {
  return useContext(FocusContext)
}

/** 一条路的键：`direct`、`relay:1.3`（两个方向倒过来写是同一个键）、`none`。 */
export function routeKey(route: Route | null | undefined): string {
  if (route === null || route === undefined || route.via === 'none') return 'none'
  if (route.via === 'direct') return 'direct'
  const forward = route.relays.join('.')
  const backward = [...route.relays].reverse().join('.')
  return `relay:${forward < backward ? forward : backward}`
}

/** 这一行要不要淡下去：有焦点、并且不是它。 */
export function dimmed(focus: string | null, key: string): boolean {
  return focus !== null && focus !== key
}
