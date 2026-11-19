import { type MatrixEntry, type NodeId, type Route, type RouteDetail, type RouteMatrix } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { formatNumber } from '@/lib/format'
import { formatAge } from '@/lib/time'

/**
 * 一条路的几种说法。拓扑图、路由矩阵、路径诊断和节点详情的路由表用同一套：
 * 直连是绿色，经 relay 是紫色（中转是正常的工作状态，不用警告色），只有不可达是红色。
 */

/**
 * 按接口给的取值从目录里取一句话。接口的取值是封闭的联合类型，但比界面新的 controller 可能给别的值，
 * 那时返回 `undefined`，由调用的地方决定写什么。
 */
export function textFor<K extends string>(map: Readonly<Record<K, string>>, key: string): string | undefined {
  return Object.hasOwn(map, key) ? map[key as K] : undefined
}

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

/** 一条路经过的节点，从发送方到接收方，两头都算。不可达时是 `null`。 */
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

/** "直连""经 hk-1""经 hk-1 → sg-2""不可达"。 */
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

/**
 * 一条路的细节：直连是怎么建立起来的（"局域网""打洞"），经 relay 时第一段走什么传输（"第一段走 TCP"）。
 * 节点说不出来、或者 controller 比这个界面旧不写这两个字段时是 `null`，界面上就不写。
 */
export function routeDetail(route: Route, detail: RouteDetail): string | null {
  const direct = detail.direct ?? null
  const leg = detail.relay_leg ?? null
  if (route.via === 'direct' && direct !== null) return t.routes.directKind[direct]
  if (route.via === 'relay' && leg !== null) return t.routes.legTransport[leg]
  return null
}

/** 细节的一句解释，给悬停用。 */
export function routeDetailHint(route: Route, detail: RouteDetail): string | null {
  const direct = detail.direct ?? null
  if (route.via === 'direct' && direct !== null) return t.routes.directKindHint[direct]
  if (route.via === 'relay' && detail.relay_leg === 'tcp') return t.routes.relayLegTcpHint
  return null
}

/** "直连（局域网）""经 hk-1（第一段走 TCP）"；没有细节时和 {@link routeText} 一样。 */
export function routeTextWithDetail(route: Route, detail: RouteDetail, name: (id: NodeId) => string): string {
  const text = routeText(route, name)
  const extra = routeDetail(route, detail)
  return extra === null ? text : t.routes.withDetail(text, extra)
}

/** 矩阵一格的完整说法：路（和它的细节）、对端地址、延迟、多久之前的、过没过期。悬停和读屏都用它。 */
export function entryText(entry: MatrixEntry, name: (id: NodeId) => string): string {
  const parts = [routeTextWithDetail(entry.route, entry, name)]
  if (entry.route.via === 'direct') parts.push(t.routes.addr(entry.route.addr))
  if (entry.rtt_ms !== null) parts.push(t.routes.rtt(entry.rtt_ms))
  parts.push(t.routes.age(formatAge(entry.age_secs)))
  if (entry.stale) parts.push(t.routes.stale)
  return parts.join(t.routes.factSeparator)
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

/** 微秒写成毫秒：不到 1 毫秒两位小数，不到 100 毫秒一位，再大取整。 */
export function formatMicros(us: number): string {
  const ms = us / 1000
  const digits = ms < 1 ? 2 : ms < 100 ? 1 : 0
  return t.routes.ms(formatNumber(ms, digits))
}

/** 千分之几写成百分数：千分之 5 是 0.5%。 */
export function formatPermille(permille: number): string {
  return t.routes.percent(formatNumber(permille / 10, permille % 10 === 0 ? 0 : 1))
}
