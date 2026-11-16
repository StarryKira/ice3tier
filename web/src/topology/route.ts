import { type MatrixEntry, type NodeId, type Route, type RouteMatrix } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'

/**
 * 一条路的几种说法。拓扑图、路由矩阵、路径诊断和节点详情的路由表用同一套：
 * 直连是绿色，经 relay 是蓝色（中转是正常的工作状态，不用警告色），只有无路可走是红色。
 */

/** 矩阵着色用的四类（文档 §15）：直连、一级中转、多级中转、不可达。 */
export type RouteClass = 'direct' | 'relay' | 'multi' | 'none'

export function classify(route: Route): RouteClass {
  switch (route.via) {
    case 'direct':
      return 'direct'
    case 'relay':
      return route.relays.length > 1 ? 'multi' : 'relay'
    case 'none':
      return 'none'
  }
}

export const CLASS_TONE: Record<RouteClass, Tone> = { direct: 'ok', relay: 'relay', multi: 'relay', none: 'danger' }

export const CLASS_LABEL: Record<RouteClass, string> = {
  direct: t.routes.classDirect,
  relay: t.routes.classRelay,
  multi: t.routes.classMulti,
  none: t.routes.classNone,
}

export const CLASS_SYMBOL: Record<RouteClass, string> = {
  direct: t.routes.symbolDirect,
  relay: t.routes.symbolRelay,
  multi: t.routes.symbolMulti,
  none: t.routes.symbolNone,
}

/** 一条路经过的节点，从发送方到接收方，两头都算。无路可走时是 `null`。 */
export function hopsOf(from: NodeId, to: NodeId, route: Route): NodeId[] | null {
  switch (route.via) {
    case 'direct':
      return [from, to]
    case 'relay':
      return [from, ...route.relays, to]
    case 'none':
      return null
  }
}

/** "直连""经 hk-1""经 hk-1 → sg-2""无路可走"。 */
export function routeText(route: Route, name: (id: NodeId) => string): string {
  switch (route.via) {
    case 'direct':
      return t.routes.direct
    case 'relay':
      return t.routes.via(route.relays.map(name).join(t.routes.hopSeparator))
    case 'none':
      return t.routes.none
  }
}

/** 矩阵一格的完整说法：路、对端地址、延迟、多久之前的、过没过期。悬停和读屏都用它。 */
export function entryText(entry: MatrixEntry, name: (id: NodeId) => string): string {
  const parts = [routeText(entry.route, name)]
  if (entry.route.via === 'direct') parts.push(t.routes.addr(entry.route.addr))
  if (entry.rtt_ms !== null) parts.push(t.routes.rtt(entry.rtt_ms))
  parts.push(t.routes.age(formatAge(entry.age_secs)))
  if (entry.stale) parts.push(t.routes.stale)
  return parts.join('，')
}

/** 经这个 relay 中转的节点对数：矩阵里现在（不过期的）路经过它的，两个方向算一对。 */
export function relayedPairs(matrix: RouteMatrix | undefined, relay: NodeId): number {
  const pairs = new Set<string>()
  for (const entry of matrix?.entries ?? []) {
    if (entry.stale || entry.route.via !== 'relay' || !entry.route.relays.includes(relay)) continue
    pairs.add(entry.from < entry.to ? `${entry.from}-${entry.to}` : `${entry.to}-${entry.from}`)
  }
  return pairs.size
}

/** 矩阵的格子按"从 > 到"索引。 */
export const cellKey = (from: NodeId, to: NodeId): string => `${from}>${to}`

export function indexEntries(entries: readonly MatrixEntry[]): Map<string, MatrixEntry> {
  return new Map(entries.map((entry) => [cellKey(entry.from, entry.to), entry]))
}
