import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { t } from '@/i18n'
import { COOLDOWN_DAYS } from '@/lib/product'

/** 网络里已经在用的标签（节点上的、规则里提到的），给标签选择器当候选。 */
export function useKnownTags(network: Network): string[] {
  const graph = useQuery({ ...q.graph(network.id), refetchInterval: false })
  return useMemo(() => (graph.data?.tags ?? []).map((entry) => entry.tag), [graph.data])
}

/**
 * 删除节点：危险级别的确认，正文写清具体的后果。
 * 跟着一起删掉的配置先数出来列在确认框里，不等删完了才发现。
 */
export function useDeleteNode(network: Network): (node: Node) => Promise<boolean> {
  const confirm = useConfirm()
  const client = useQueryClient()
  const remove = useAction(network.id, {
    mutationFn: (node: Node) => api.deleteNode(network.id, node.id),
    success: (_data, node) => t.nodes.deleted(node.name),
  })

  return async (node) => {
    // 数不出来（请求失败）就不写这一句，不拿猜的数字充数。
    const [forwards, listeners, pairs] = await Promise.all([
      client.query(q.forwards(network.id)).catch(() => null),
      client.query(q.listeners(network.id)).catch(() => null),
      client.query(q.pairs(network.id)).catch(() => null),
    ])
    const refs =
      forwards === null || listeners === null || pairs === null
        ? null
        : t.nodes.deleteRefs(
            forwards.filter((forward) => forward.listen_node === node.id || forward.target_node === node.id).length,
            listeners.filter((listener) => listener.node === node.id).length,
            pairs.filter((pair) => pair.a === node.id || pair.b === node.id).length,
          )
    const confirmed = await confirm({
      tone: 'danger',
      title: t.nodes.deleteTitle(node.name),
      body: (
        <>
          <p>
            {t.nodes.deleteLead}
            {refs !== null ? ` ${refs}` : null}
          </p>
          <p>{t.nodes.deleteCooldown(node.virtual_ip, node.name, COOLDOWN_DAYS)}</p>
        </>
      ),
      confirmLabel: t.nodes.deleteConfirm,
    })
    if (!confirmed) return false
    try {
      await remove.mutateAsync(node)
      return true
    } catch {
      return false
    }
  }
}
