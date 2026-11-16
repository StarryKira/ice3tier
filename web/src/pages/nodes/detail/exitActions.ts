import { errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type Network, type Node } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { t } from '@/i18n'

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
  return { submit, pending: save.isPending, error: save.isError ? errorMessage(save.error) : null, reset: save.reset }
}

/** 出口的限制写成一行摘要。 */
export function exitSummary(exit: NonNullable<Node['exit']>): string {
  return [
    exit.ports === null ? d.exitPortsAny : d.exitPorts(exit.ports.join(', ')),
    exit.lan ? d.exitLanOn : d.exitLanOff,
    exit.bandwidth_mbps === null ? d.relayUnlimited : d.relayBandwidth(exit.bandwidth_mbps),
    exit.upstream_socks5 === null ? null : d.exitUpstream(exit.upstream_socks5),
  ]
    .filter((part): part is string => part !== null)
    .join(' · ')
}
