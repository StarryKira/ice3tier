import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type AccessGraph, type Network, type Rule } from '@/api/types'
import { accessDiff } from '@/access/reach'
import { useConfirm } from '@/components/confirm'
import { ruleLabel } from '@/lib/rules'
import { t } from '@/i18n'

/**
 * 规则的启用、停用和删除，以及它们各自要不要确认：
 *
 * - 不会让任何一对节点失去访问：直接生效，弹一条提示。
 * - 会让节点失去访问：先确认，确认框里写数字。收回许可之后已有的连接会被断开，这是会打断别人的操作。
 * - 之后所有节点互不可达：危险级别。
 * - 删除总要确认一次，因为删掉的规则找不回来。
 */
export function useRuleActions(network: Network, graph: AccessGraph) {
  const confirm = useConfirm()
  const setEnabled = useAction(network.id, {
    mutationFn: ({ rule, enabled }: { rule: Rule; enabled: boolean }) =>
      api.updateRule(network.id, rule.id, { enabled }),
    success: (_data, { rule, enabled }) =>
      enabled ? t.access.enabledToast(ruleLabel(rule)) : t.access.disabledToast(ruleLabel(rule)),
  })
  const remove = useAction(network.id, {
    mutationFn: (rule: Rule) => api.deleteRule(network.id, rule.id),
    success: (_data, rule) => t.access.deletedToast(ruleLabel(rule)),
  })

  const without = (rule: Rule) =>
    accessDiff(
      graph.tags,
      graph.rules,
      graph.rules.filter((other) => other.id !== rule.id),
    )

  const toggle = async (rule: Rule, enabled: boolean) => {
    if (!enabled) {
      const diff = without(rule)
      if (diff.lost > 0) {
        const isolates = diff.connectedAfter === 0
        const confirmed = await confirm({
          tone: isolates ? 'danger' : 'normal',
          title: t.access.disableTitle(ruleLabel(rule)),
          body: isolates ? t.access.lossAll : t.access.lossBody(diff.lost),
          confirmLabel: t.access.disableConfirm,
        })
        if (!confirmed) return
      }
    }
    setEnabled.mutate({ rule, enabled })
  }

  const destroy = async (rule: Rule) => {
    const diff = rule.enabled ? without(rule) : null
    const isolates = diff !== null && diff.lost > 0 && diff.connectedAfter === 0
    const confirmed = await confirm({
      tone: isolates ? 'danger' : 'normal',
      title: t.access.deleteTitle(ruleLabel(rule)),
      // 没有谁会因此失去访问时不写正文：标题已经说清了。
      body: isolates ? t.access.lossAll : diff !== null && diff.lost > 0 ? t.access.lossBody(diff.lost) : undefined,
      confirmLabel: t.access.deleteConfirm,
    })
    if (confirmed) remove.mutate(rule)
  }

  return {
    toggle,
    destroy,
    pendingRuleId: setEnabled.isPending ? setEnabled.variables.rule.id : remove.isPending ? remove.variables.id : null,
  }
}
