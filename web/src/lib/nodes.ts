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

/**
 * 能不能当端口转发的目标。0.1.11 起看管理员在界面上的开关（`forward_target`）；
 * 更早的 controller 没有这个字段，看节点自己的声明。
 */
export function canBeForwardTarget(node: Pick<Node, 'declared' | 'forward_target'>): boolean {
  return node.forward_target ?? node.declared.forward ?? false
}

/** 这个 controller 用的是开关（0.1.11 起），而不是节点的声明。 */
export function forwardTargetBySwitch(node: Pick<Node, 'forward_target'>): boolean {
  return node.forward_target !== undefined
}
