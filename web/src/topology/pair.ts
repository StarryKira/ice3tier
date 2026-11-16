import { type NodeId } from '@/api/types'
import { networkPath } from '@/pages/networks/context'

/**
 * 选中的一对节点写在地址的查询参数里（`?a=3&b=7`）：拓扑和路径诊断共用这两个参数，
 * 两页之间来回跳时选中的那一对跟着走，浏览器的前进后退也能还原。
 */

export interface Pair {
  a: NodeId | null
  b: NodeId | null
}

function idParam(params: URLSearchParams, key: string): NodeId | null {
  const raw = params.get(key)
  if (raw === null) return null
  const value = Number(raw)
  return Number.isInteger(value) && value > 0 ? value : null
}

export function readPair(params: URLSearchParams): Pair {
  return { a: idParam(params, 'a'), b: idParam(params, 'b') }
}

/** 改选中的那一对，别的查询参数（例如视图）保留。 */
export function writePair(params: URLSearchParams, pair: Pair): URLSearchParams {
  const next = new URLSearchParams(params)
  for (const key of ['a', 'b'] as const) {
    const value = pair[key]
    if (value === null) next.delete(key)
    else next.set(key, String(value))
  }
  return next
}

/** 在图上先后点两个节点：第一下是 A，第二下是 B；两个都有了再点，就从 A 重新开始。 */
export function nextPick(pair: Pair, id: NodeId): Pair {
  if (pair.a === null) return { a: id, b: pair.b === id ? null : pair.b }
  if (pair.b === null) return id === pair.a ? pair : { a: pair.a, b: id }
  return { a: id, b: null }
}

export function pairPath(network: number, page: 'topology' | 'diagnosis', a: NodeId | null, b: NodeId | null): string {
  const query = writePair(new URLSearchParams(), { a, b }).toString()
  return `${networkPath(network, page)}${query === '' ? '' : `?${query}`}`
}
