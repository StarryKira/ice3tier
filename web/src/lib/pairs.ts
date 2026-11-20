import { type Node, type NodeId, type PairView } from '@/api/types'

/** 一对节点的键：小的编号在前，`1-3`。 */
export function pairKey(a: NodeId, b: NodeId): string {
  return `${Math.min(a, b)}-${Math.max(a, b)}`
}

/**
 * 这一对有没有和默认不一样的设置（0.1.28 起的意思）。比 0.1.19 旧的 controller 不写 `stored`：
 * 那时只有存下的才答得出来。
 */
export function pairStored(pair: Pick<PairView, 'stored'>): boolean {
  return pair.stored ?? true
}

/**
 * 网络里的每一对节点，按 a、b 排：接口给的那一行，没有就是默认（连接不常驻、拥塞控制用节点的默认）。
 * 0.1.28 起接口本来就给全部；更早的 controller 只给有设置的，用节点列表补齐。节点列表还没取到时只有接口给的。
 */
export function allPairs(nodes: readonly Pick<Node, 'id'>[], listed: readonly PairView[]): PairView[] {
  const byKey = new Map(listed.map((pair) => [pairKey(pair.a, pair.b), pair]))
  const ids = nodes.map((node) => node.id).sort((x, y) => x - y)
  const rows: PairView[] = []
  for (const [index, a] of ids.entries()) {
    for (const b of ids.slice(index + 1)) {
      const key = pairKey(a, b)
      rows.push(byKey.get(key) ?? { a, b, keep_alive: false, congestion: null, stored: false })
      byKey.delete(key)
    }
  }
  // 接口给了、节点列表里没有的（列表还没取到，或者刚加入的节点）：照样列出来。
  return [...rows, ...byKey.values()].sort(
    (x, y) => Math.min(x.a, x.b) - Math.min(y.a, y.b) || Math.max(x.a, x.b) - Math.max(y.a, y.b),
  )
}
