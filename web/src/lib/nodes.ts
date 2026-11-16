import { type Node, type NodeId } from '@/api/types'
import { t } from '@/i18n'
import { networkPath } from '@/pages/networks/context'

/** 节点详情页的地址。 */
export function nodePath(network: number, node: NodeId): string {
  return networkPath(network, `nodes/${node}`)
}

/** 按编号找节点的名字。节点已经不在了（刚被删掉、列表还没刷新）时给一个看得懂的说法。 */
export function nodeName(nodes: readonly Pick<Node, 'id' | 'name'>[], id: NodeId): string {
  return nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)
}

/** 挂靠在这个 relay 上的节点数：数的是各节点最近一次上报的挂靠（离线的节点是它离线前报的）。 */
export function attachedCount(nodes: readonly Pick<Node, 'id' | 'attached'>[], relay: NodeId): number {
  return nodes.filter((node) => node.id !== relay && node.attached.some((attachment) => attachment.relay === relay))
    .length
}
