import { type NodeId, type PathLeg, type PathSource } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'

import { CLASS_TONE, classify, type RouteClass, routeText } from './route'

const p = t.path

/**
 * `GET /path` 一个方向的应答在界面上怎么说。纯函数。
 *
 * 四种来源分得很清楚：刚问到的（现在的结论）、最近一次上报（可能过时，标出多久之前）、
 * 不是对端（访问规则没有把它们连通）、没有数据。
 */

export type LegState = PathSource

export interface LegView {
  state: LegState
  /** 来源的短语和颜色。 */
  sourceLabel: string
  sourceTone: Tone
  sourceHint: string
  /** 路；没有数据时是 `null`。 */
  route: {
    cls: RouteClass
    tone: Tone
    text: string
    /** 经过的节点的名字，从发送方到接收方。 */
    hops: string[]
    /** 直连时对端的地址。 */
    addr: string | null
  } | null
  rtt: string | null
  /** "6 秒前的数据"。刚问到的不写（是现在）；没有数据时是 `null`。 */
  age: string | null
  /** 这条路是什么时候、为什么换上的。 */
  since: string | null
}

const SOURCE_TONE: Record<PathSource, Tone> = {
  query: 'ok',
  report: 'idle',
  not_peers: 'warn',
  none: 'idle',
  unknown: 'idle',
}

export function describeLeg(leg: PathLeg, name: (id: NodeId) => string): LegView {
  // 更新的 controller 可能给出这里还不认识的来源。
  const state: LegState = leg.source in SOURCE_TONE ? leg.source : 'unknown'
  const route =
    leg.route === null
      ? null
      : {
          cls: classify(leg.route),
          tone: CLASS_TONE[classify(leg.route)],
          text: routeText(leg.route, name),
          hops: [
            leg.from.name,
            ...(leg.route.via === 'relay' ? leg.route.relays.map(name) : []),
            ...(leg.route.via === 'none' ? [] : [leg.to.name]),
          ],
          addr: leg.route.via === 'direct' ? leg.route.addr : null,
        }
  const age = leg.age_secs === null || state === 'query' ? null : p.age(formatAge(leg.age_secs))
  let since: string | null = null
  if (leg.since !== null) since = p.since(formatAge(leg.since.secs_ago), leg.since.reason)
  else if (state === 'query' && route !== null) since = p.sinceUnknown
  return {
    state,
    sourceLabel: p.source[state] ?? state,
    sourceTone: SOURCE_TONE[state],
    sourceHint: p.sourceHint[state] ?? '',
    route,
    rtt: leg.rtt_ms === null ? null : t.routes.rtt(leg.rtt_ms),
    age,
    since,
  }
}
