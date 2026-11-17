import { Plus, X } from 'lucide-react'
import { useId, useState } from 'react'

import { errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type Network, type NodeDetail, type RelayEnabled, type RelayInput } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { Notice } from '@/components/notice'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { DEFAULT_NODE_PORT, MAX_RELAY_ADDRESSES } from '@/lib/product'
import { parseOptionalPositiveInt, validateSocketAddr } from '@/lib/validate'

const d = t.nodes.detail

interface RelayFormState {
  udp: string[]
  /** TCP 用和 UDP 一样的地址：多数机器是这样，勾着就不用再填一遍。 */
  tcpSame: boolean
  tcp: string[]
  region: string
  bandwidth: string
}

const sameList = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((item, index) => item === b[index])

function formFrom(node: NodeDetail): RelayFormState {
  if (node.relay !== null) {
    const { udp, tcp } = node.relay
    return {
      udp: udp.length > 0 ? [...udp] : [''],
      tcpSame: sameList(udp, tcp),
      tcp: tcp.length > 0 ? [...tcp] : [''],
      region: node.relay.region,
      bandwidth: node.relay.bandwidth_mbps?.toString() ?? '',
    }
  }
  // 第一次启用：用 controller 看到的地址预填，多数有公网 IP 的机器不用再改。
  const observed = node.observed_endpoint ?? ''
  return { udp: [observed], tcpSame: true, tcp: [''], region: '', bandwidth: '' }
}

const filled = (items: readonly string[]) => items.map((item) => item.trim()).filter((item) => item !== '')

/** 一组地址里第一个写错的，写成"地址：哪里不对"。 */
function listProblem(items: readonly string[]): string | null {
  for (const item of items) {
    const problem = validateSocketAddr(item)
    if (problem !== null) return d.relayFormAddress(item, problem)
  }
  return null
}

interface AddressInputsProps {
  /** `Field` 给的属性，放在第一个输入框上。 */
  field: { id: string; 'aria-describedby'?: string; 'aria-invalid'?: true }
  label: string
  values: string[]
  onChange: (values: string[]) => void
}

/** 一个地址一个输入框；多数只有一个，要第二个时点"添加地址"。 */
function AddressInputs({ field, label, values, onChange }: AddressInputsProps) {
  return (
    <div className="space-y-2">
      {values.map((value, index) => (
        // 地址的内容会变，位置不会：用位置当键，输入时焦点不会跳走。
        <div key={index} className="flex items-center gap-2">
          <Input
            {...(index === 0 ? field : { 'aria-label': `${label} ${index + 1}` })}
            className="tabular"
            placeholder={`203.0.113.7:${DEFAULT_NODE_PORT}`}
            value={value}
            onChange={(event) => onChange(values.map((other, at) => (at === index ? event.target.value : other)))}
          />
          {values.length > 1 ? (
            <Button
              variant="ghost"
              size="icon-sm"
              className="shrink-0 text-muted-foreground"
              aria-label={d.relayFormRemove(value === '' ? String(index + 1) : value)}
              onClick={() => onChange(values.filter((_, at) => at !== index))}
            >
              <X aria-hidden />
            </Button>
          ) : null}
        </div>
      ))}
      {values.length < MAX_RELAY_ADDRESSES ? (
        <Button variant="link" size="sm" onClick={() => onChange([...values, ''])}>
          <Plus aria-hidden />
          {d.relayFormAdd}
        </Button>
      ) : null}
    </div>
  )
}

interface RelayDialogProps {
  network: Network
  node: NodeDetail
  onClose: () => void
  /** 保存成功：controller 从自己这边试连的结果交给卡片显示。 */
  onSaved: (saved: RelayEnabled) => void
}

/** 启用中继，或者改中继的设置。地址、区域、带宽上限；防火墙要放行的端口写在页脚。 */
export function RelayDialog({ network, node, onClose, onSaved }: RelayDialogProps) {
  const enabled = node.relay !== null
  const [initial] = useState(() => formFrom(node))
  const [form, setForm] = useState(initial)
  const [submitted, setSubmitted] = useState(false)
  const sameId = useId()

  const save = useAction(network.id, {
    mutationFn: (input: RelayInput) => api.enableRelay(network.id, node.id, input),
    success: enabled ? d.relaySavedToast : d.relayEnabledToast(node.name),
    toastError: false,
  })

  const udp = filled(form.udp)
  const tcp = form.tcpSame ? udp : filled(form.tcp)
  const udpProblem = listProblem(udp)
  const tcpProblem = form.tcpSame ? null : listProblem(tcp)
  const needOne = udp.length + tcp.length === 0 ? d.relayFormNeedOne : null
  const bandwidth = parseOptionalPositiveInt(form.bandwidth)
  const shown = (problem: string | null) => (submitted ? problem : null)

  const submit = () => {
    setSubmitted(true)
    if (udpProblem !== null || tcpProblem !== null || needOne !== null || bandwidth.error !== null) return
    save.mutate(
      { udp, tcp, region: form.region.trim(), bandwidth_mbps: bandwidth.value },
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
      title={enabled ? d.relayEditTitle(node.name) : d.relaySwitch(node.name)}
      dirty={JSON.stringify(form) !== JSON.stringify(initial)}
      submitLabel={enabled ? d.save : d.relayEnable}
      pending={save.isPending}
      error={save.isError ? errorMessage(save.error) : null}
      errorDetail={save.isError ? errorDetail(save.error) : null}
      onSubmit={submit}
      footerNote={d.relayPortReminder(DEFAULT_NODE_PORT)}
    >
      {!enabled && node.observed_endpoint !== null ? <Notice>{d.relayPrefilled(node.observed_endpoint)}</Notice> : null}

      <div className="space-y-3">
        {/* 最多几个不用写：加满了"添加地址"自己就不见了。 */}
        <Field label={d.relayFormUdp} error={shown(udpProblem ?? needOne)}>
          {(props) => (
            <AddressInputs
              field={props}
              label={d.relayFormUdp}
              values={form.udp}
              onChange={(next) => setForm({ ...form, udp: next })}
            />
          )}
        </Field>
        <label htmlFor={sameId} className="flex w-fit items-center gap-2 text-[13px]">
          <Checkbox
            id={sameId}
            checked={form.tcpSame}
            onCheckedChange={(checked) => setForm({ ...form, tcpSame: checked === true })}
          />
          {d.relayFormTcpSame}
        </label>
      </div>

      {form.tcpSame ? null : (
        <Field label={d.relayFormTcp} error={shown(tcpProblem)}>
          {(props) => (
            <AddressInputs
              field={props}
              label={d.relayFormTcp}
              values={form.tcp}
              onChange={(next) => setForm({ ...form, tcp: next })}
            />
          )}
        </Field>
      )}

      <div className="grid grid-cols-2 gap-4">
        <Field label={d.relayFormRegion} optional={t.common.optional}>
          {(props) => (
            <Input
              {...props}
              placeholder="us-east"
              value={form.region}
              onChange={(event) => setForm({ ...form, region: event.target.value })}
            />
          )}
        </Field>
        <Field label={d.relayFormBandwidth} error={shown(bandwidth.error)} optional={t.common.optional}>
          {(props) => (
            <div className="flex items-center gap-2">
              <Input
                {...props}
                inputMode="numeric"
                placeholder={t.common.unlimited}
                className="tabular"
                value={form.bandwidth}
                onChange={(event) => setForm({ ...form, bandwidth: event.target.value })}
              />
              <span className="shrink-0 text-[13px] text-muted-foreground">{t.common.mbps}</span>
            </div>
          )}
        </Field>
      </div>

      {save.isPending ? (
        <p role="status" className="text-[13px] text-relay">
          {d.relayChecking}
        </p>
      ) : null}
    </FormDialog>
  )
}
