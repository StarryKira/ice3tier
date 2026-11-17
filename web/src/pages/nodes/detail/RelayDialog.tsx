import { useState } from 'react'

import { errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type Network, type NodeDetail, type RelayEnabled, type RelayInput } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { Notice } from '@/components/notice'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { DEFAULT_NODE_PORT } from '@/lib/product'
import { relayAddresses } from '@/lib/relay'
import { parseOptionalPositiveInt } from '@/lib/validate'

const d = t.nodes.detail

/**
 * 中继的地址由 controller 自动发现，界面上不能填也不能改。接口改成给出地址列表之前，用 controller 看到的地址。
 * 节点从没连上过、还没发现任何地址时是 `null`：这时不能启用。
 */
function relayAddress(node: Pick<NodeDetail, 'observed_endpoint' | 'relay'>): string | null {
  return node.observed_endpoint ?? (node.relay === null ? null : (relayAddresses(node.relay)[0]?.address ?? null))
}

interface RelayDialogProps {
  network: Network
  node: NodeDetail
  onClose: () => void
  /** 保存成功：controller 从自己这边试连的结果交给卡片显示。 */
  onSaved: (saved: RelayEnabled) => void
}

/**
 * 启用中继：写明其他节点会经哪个地址连它（自动发现，不能改），可以填一个带宽上限。
 * 防火墙要放行的端口写在页脚。启用之后没有"修改"：要改带宽上限就先停用再启用。
 */
export function RelayDialog({ network, node, onClose, onSaved }: RelayDialogProps) {
  const [bandwidthText, setBandwidthText] = useState('')
  const [submitted, setSubmitted] = useState(false)
  const address = relayAddress(node)
  const bandwidth = parseOptionalPositiveInt(bandwidthText)

  const save = useAction(network.id, {
    mutationFn: (input: RelayInput) => api.enableRelay(network.id, node.id, input),
    success: d.relayEnabledToast(node.name),
    toastError: false,
  })

  const submit = () => {
    setSubmitted(true)
    if (address === null || bandwidth.error !== null) return
    // 现在的接口还要地址：交的就是上面算出来的那一个，UDP 和 TCP 一样。后端改成只收带宽之后去掉。
    save.mutate(
      { udp: [address], tcp: [address], bandwidth_mbps: bandwidth.value },
      {
        onSuccess: (saved) => {
          onSaved(saved)
          onClose()
        },
      },
    )
  }

  return (
    <FormDialog
      open
      onClose={onClose}
      title={d.relaySwitch(node.name)}
      dirty={bandwidthText !== ''}
      submitLabel={d.relayEnable}
      pending={save.isPending}
      canSubmit={address !== null}
      error={save.isError ? errorMessage(save.error) : null}
      errorDetail={save.isError ? errorDetail(save.error) : null}
      onSubmit={submit}
      footerNote={d.relayPortReminder(DEFAULT_NODE_PORT)}
    >
      {address === null ? (
        <Notice tone="warn">{d.relayNoAddress}</Notice>
      ) : (
        <div className="rounded-lg bg-muted/60 px-4 py-3">
          <p className="text-[13px] text-muted-foreground">{d.relayAddressLabel}</p>
          <p className="mt-1 text-base font-semibold tabular">{address}</p>
          <p className="mt-1 text-[13px] text-muted-foreground">{d.relayAddressAuto}</p>
        </div>
      )}

      <Field label={d.relayFormBandwidth} error={submitted ? bandwidth.error : null} optional={t.common.optional}>
        {(props) => (
          <div className="flex items-center gap-2">
            <Input
              {...props}
              inputMode="numeric"
              placeholder={t.common.unlimited}
              className="w-32 tabular"
              value={bandwidthText}
              onChange={(event) => setBandwidthText(event.target.value)}
            />
            <span className="shrink-0 text-[13px] text-muted-foreground">{t.common.mbps}</span>
          </div>
        )}
      </Field>

      {save.isPending ? (
        <p role="status" className="text-[13px] text-relay">
          {d.relayChecking}
        </p>
      ) : null}
    </FormDialog>
  )
}
