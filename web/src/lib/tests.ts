import { type Node, type NodeId, type NodeRef, type Route, type TestProblem } from '@/api/types'
import { t } from '@/i18n'

import { nodeName } from './nodes'

type Named = Pick<Node, 'id' | 'name'> | NodeRef

/** 微秒写成毫秒：10 毫秒以下留一位小数（局域网、同机房的延迟），以上取整。没有是一道横线。 */
export function msText(us: number | null | undefined): string {
  if (us === null || us === undefined) return '—'
  const ms = us / 1000
  return t.common.ms(ms < 10 ? Math.round(ms * 10) / 10 : Math.round(ms))
}

/** 千分之几写成百分比：整数就不带小数。 */
export function permilleText(permille: number | null | undefined): string {
  if (permille === null || permille === undefined) return '—'
  const percent = permille / 10
  return `${Number.isInteger(percent) ? percent : percent.toFixed(1)}%`
}

/** 比特每秒写成 Gbps / Mbps / Kbps，三位有效数字左右。 */
export function rateText(bitsPerSecond: number | null | undefined): string {
  if (bitsPerSecond === null || bitsPerSecond === undefined) return '—'
  if (bitsPerSecond >= 1e9) return `${(bitsPerSecond / 1e9).toFixed(2)} Gbps`
  if (bitsPerSecond >= 1e6) return `${(bitsPerSecond / 1e6).toFixed(1)} Mbps`
  if (bitsPerSecond >= 1e3) return `${Math.round(bitsPerSecond / 1e3)} Kbps`
  return `${Math.round(bitsPerSecond)} bps`
}

/** 一个原因：按原因码写界面自己的话，带上是哪个节点的；认不出的码用 controller 给的原话兜底。 */
export function problemText(problem: TestProblem, nodes: readonly Named[]): string {
  const known = Object.hasOwn(t.tests.problems, problem.code)
    ? t.tests.problems[problem.code as keyof typeof t.tests.problems]
    : problem.message || t.tests.problems.unknown
  return problem.node === null ? known : t.tests.problemOf(nodeName(nodes, problem.node), known)
}

/** 一条路的一句话：P2P、经 a → b，或者没有能走的路。 */
export function routeText(route: Route | null | undefined, nodes: readonly Named[]): string {
  if (route === null || route === undefined || route.via === 'none') return t.tests.noRoute
  if (route.via === 'direct') return t.nodes.detail.routesP2P
  return t.nodes.detail.viaRelay(route.relays.map((id: NodeId) => nodeName(nodes, id)).join(' → '))
}
