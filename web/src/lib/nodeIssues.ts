import { type Node } from '@/api/types'
import { t } from '@/i18n'

import { selfUpdateBlock } from './nodeUpdate'

export interface NodeIssue {
  kind: 'update_failed' | 'relay_offline' | 'exit_offline' | 'pinned_gone' | 'manual_update'
  /** 悬停时显示的一句话：是什么、为什么、去哪里处理。 */
  message: string
  tone: 'warn' | 'danger'
}

/**
 * 一个节点上"需要处理"的事，按重要程度排好。列表里只显示第一条。
 *
 * 只包括从节点列表本身看得出来的：声明被撤回的记录管理 API 现在没有。
 * 顺序照设计：更新失败最先，"不能自己更新而有新版本"最后。
 * "这台机器所在的网络打不了洞"在有 relay 的网络里不算问题，不上这里，只在详情页作为一条说明出现。
 */
export function nodeIssues(node: Node, all: readonly Node[]): NodeIssue[] {
  const issues: NodeIssue[] = []
  const { status, failure, available } = node.update
  if (status.state === 'failed' && status.kind === 'version_rejected') {
    issues.push({ kind: 'update_failed', message: t.updates.issue.failed(status.reason), tone: 'danger' })
  } else if (failure !== null && failure.version === available) {
    issues.push({ kind: 'update_failed', message: t.updates.issue.failed(failure.reason), tone: 'danger' })
  }
  if (node.relay !== null && !node.online) {
    issues.push({ kind: 'relay_offline', message: t.nodes.issue.relayOffline, tone: 'warn' })
  }
  if (node.exit !== null && !node.online) {
    issues.push({ kind: 'exit_offline', message: t.nodes.issue.exitOffline, tone: 'warn' })
  }
  const gone = node.pinned_relays.filter((id) => all.find((other) => other.id === id)?.relay == null)
  if (gone.length > 0) {
    const names = gone.map((id) => all.find((other) => other.id === id)?.name ?? `#${id}`).join('、')
    issues.push({ kind: 'pinned_gone', message: t.nodes.issue.pinnedGone(names), tone: 'warn' })
  }
  const block = selfUpdateBlock(node.update)
  if (available !== null && block !== null) {
    issues.push({ kind: 'manual_update', message: t.updates.issue.manual(available, block.reason), tone: 'warn' })
  }
  return issues
}
