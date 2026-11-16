import { type Node } from '@/api/types'
import { t } from '@/i18n'

export interface NodeIssue {
  kind: 'relay_offline' | 'exit_offline' | 'pinned_gone'
  /** 悬停时显示的一句话：是什么、为什么、去哪里处理。 */
  message: string
}

/**
 * 一个节点上"需要处理"的事，按重要程度排好。列表里只显示第一条。
 *
 * 只包括从节点列表本身看得出来的：管理 API 现在没有更新状态、声明被撤回的记录这些信息。
 * "这台机器所在的网络打不了洞"在有 relay 的网络里不算问题，不上这里，只在详情页作为一条说明出现。
 */
export function nodeIssues(node: Node, all: readonly Node[]): NodeIssue[] {
  const issues: NodeIssue[] = []
  if (node.relay !== null && !node.online) {
    issues.push({ kind: 'relay_offline', message: t.nodes.issue.relayOffline })
  }
  if (node.exit !== null && !node.online) {
    issues.push({ kind: 'exit_offline', message: t.nodes.issue.exitOffline })
  }
  const gone = node.pinned_relays.filter((id) => all.find((other) => other.id === id)?.relay == null)
  if (gone.length > 0) {
    const names = gone.map((id) => all.find((other) => other.id === id)?.name ?? `#${id}`).join('、')
    issues.push({ kind: 'pinned_gone', message: t.nodes.issue.pinnedGone(names) })
  }
  return issues
}
