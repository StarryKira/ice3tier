import { errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type Network, type Node } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { classLabel } from '@/lib/serviceClass'

import { type ExitFormState, exitFormToInput, exitFormValid } from './exitFormState'

const d = t.nodes.detail

/** 启用或修改出口：要打开"允许访问局域网"时先确认一次。出口页的对话框也用它。 */
export function useSaveExit(network: Network, node: Pick<Node, 'id' | 'name' | 'exit'>) {
  const confirm = useConfirm()
  const enabled = node.exit !== null
  const save = useAction(network.id, {
    mutationFn: (state: ExitFormState) => api.enableExit(network.id, node.id, exitFormToInput(state)),
    success: enabled ? d.exitSavedToast : d.exitEnabledToast(node.name),
    toastError: false,
  })

  /** 返回是否保存成功。 */
  const submit = async (state: ExitFormState): Promise<boolean> => {
    if (!exitFormValid(state)) return false
    if (state.lan && node.exit?.lan !== true) {
      const confirmed = await confirm({
        title: d.exitLanTitle(node.name),
        body: d.exitLanBody(node.name),
        confirmLabel: d.exitLanConfirm,
      })
      if (!confirmed) return false
    }
    try {
      await save.mutateAsync(state)
      return true
    } catch {
      return false
    }
  }
  return {
    submit,
    pending: save.isPending,
    error: save.isError ? errorMessage(save.error) : null,
    errorDetail: save.isError ? errorDetail(save.error) : null,
    reset: save.reset,
  }
}

/** 出口的限制写成一行摘要。默认值（两种地址都连、不限建连、不设等级）不写。 */
export function exitSummary(exit: NonNullable<Node['exit']>): string {
  return exitSummaryParts(exit).join(' · ')
}

/** 摘要的每一项：表格里一项不拆成两行，所以要分开排。 */
export function exitSummaryParts(exit: NonNullable<Node['exit']>): string[] {
  return [
    exit.ports === null ? d.exitPortsAny : d.exitPorts(exit.ports.join(', ')),
    exit.lan ? d.exitLanOn : d.exitLanOff,
    exit.quota_bytes === null ? null : d.exitQuota(formatBytes(exit.quota_bytes), exit.quota_reset_day),
    exit.family === 'ipv4' ? d.exitFamilyIpv4 : exit.family === 'ipv6' ? d.exitFamilyIpv6 : null,
    exit.connects_per_minute === null ? null : d.exitConnects(exit.connects_per_minute),
    exit.class === null ? null : t.serviceClass.value(classLabel(exit.class)),
    exit.upstream_socks5 === null ? null : d.exitUpstream(exit.upstream_socks5),
  ].filter((part): part is string => part !== null)
}
