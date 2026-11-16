import { useQuery } from '@tanstack/react-query'
import { useId, useState } from 'react'

import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import {
  type DefaultAction,
  type Listener,
  type ListenerInput,
  type ListenerProtocol,
  type Network,
  type NodeId,
  type ServiceClass,
} from '@/api/types'
import { CodeBlock } from '@/components/copy'
import { FormDialog } from '@/components/form-dialog'
import { NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { Field } from '@/components/ui/field'
import { Input, Textarea } from '@/components/ui/input'
import { RadioCards } from '@/components/ui/radio-cards'
import { Segmented } from '@/components/ui/segmented'
import { Switch } from '@/components/ui/switch'
import { ClassSelect } from '@/components/usage'
import { t } from '@/i18n'
import { formatSocketAddr, isLoopbackHost, parseIpv4, parseSocketAddr } from '@/lib/cidr'
import { listenerUsage } from '@/lib/commands'
import { normalizeCidrList, splitList, validateCidrList, validatePort, validateTitle } from '@/lib/validate'

import { effectiveDefaultAction } from './describe'

const d = t.listeners.dialog

type Where = 'local' | 'others'
type ActionKind = DefaultAction['action']
/** 乐观回复的三种：按目标（`null`）、一律不等（`true`）、一律等（`false`）。 */
type Optimistic = 'auto' | 'on' | 'off'

function optimisticFrom(value: boolean | null): Optimistic {
  return value === null ? 'auto' : value ? 'on' : 'off'
}

function optimisticTo(value: Optimistic): boolean | null {
  return value === 'auto' ? null : value === 'on'
}

interface ListenerDialogProps {
  network: Network
  /** 要编辑的监听器；新建时是 `null`。 */
  listener: Listener | null
  /** 新建时预选的节点。 */
  node?: NodeId
  open: boolean
  onClose: () => void
}

/**
 * 新建和编辑监听器。字段按顺序出现，后面的字段取决于前面的选择：
 * 选了"给别的机器用"，才出现监听地址和来源白名单，"要求凭据"变成打开且关不掉。
 */
export function ListenerDialog({ network, listener, node: presetNode, open, onClose }: ListenerDialogProps) {
  const nodes = useQuery(q.nodes(network.id))
  const credentials = useQuery(q.credentials(network.id))
  const whereLabel = useId()
  const actionLabel = useId()

  const parsed = listener === null ? null : parseSocketAddr(listener.bind)
  const initialLocal = parsed === null ? true : isLoopbackHost(parsed.host)
  const initialAction = listener === null ? null : effectiveDefaultAction(listener)

  const [node, setNode] = useState<NodeId | null>(listener?.node ?? presetNode ?? null)
  const [name, setName] = useState(listener?.name ?? '')
  const [protocol, setProtocol] = useState<ListenerProtocol>(listener?.protocol ?? 'socks5')
  const [where, setWhere] = useState<Where>(initialLocal ? 'local' : 'others')
  const [address, setAddress] = useState(parsed !== null && !initialLocal ? parsed.host : '0.0.0.0')
  const [port, setPort] = useState(parsed?.port.toString() ?? '')
  const [requireCredential, setRequireCredential] = useState(listener?.require_credential ?? false)
  const [allowFrom, setAllowFrom] = useState(listener?.allow_from.join('\n') ?? '')
  // 没有明确选过时，默认动作跟着"监听在哪"走：本机用是从本机出去，给别的机器用是拒绝。
  const [action, setAction] = useState<ActionKind | null>(initialAction?.action ?? null)
  const [exitNode, setExitNode] = useState<NodeId | null>(initialAction?.action === 'exit' ? initialAction.node : null)
  const [fixedExit, setFixedExit] = useState<NodeId | null>(listener?.fixed_exit ?? null)
  const [serviceClass, setServiceClass] = useState<ServiceClass | null>(listener?.class ?? null)
  const [optimistic, setOptimistic] = useState<Optimistic>(optimisticFrom(listener?.optimistic ?? null))
  const [touched, setTouched] = useState(false)
  const [dirty, setDirty] = useState(false)

  const all = nodes.data ?? []
  const exits = all.filter((candidate) => candidate.exit !== null)
  const host = all.find((candidate) => candidate.id === node)
  const external = where === 'others'
  const effectiveAction: ActionKind = action ?? (external ? 'reject' : 'local')
  const mustRequire = external
  const localNeedsExit = external && host?.exit === null

  const hostValue = external ? address.trim() : '127.0.0.1'
  const bind = formatSocketAddr(hostValue, Number(port) || 0)

  const nameProblem = validateTitle(name, true)
  const portProblem = validatePort(port)
  const addressProblem =
    external && parseIpv4(hostValue) === null && !hostValue.includes(':') ? t.validate.ipFormat : null
  const allowItems = splitList(allowFrom)
  const allowProblem = validateCidrList(allowItems)
  const exitProblem = effectiveAction === 'exit' && exitNode === null ? d.pickExit : null
  const nodeProblem = node === null ? t.validate.pickNode : null
  const valid = [nameProblem, portProblem, addressProblem, allowProblem, exitProblem, nodeProblem].every(
    (problem) => problem === null,
  )

  const save = useAction(network.id, {
    mutationFn: (input: ListenerInput) =>
      listener === null ? api.createListener(network.id, input) : api.replaceListener(network.id, listener.id, input),
    success: (saved) => (listener === null ? d.created(saved.name) : d.saved(saved.name)),
    toastError: false,
  })
  const failure = formFailure(save.error, { name_taken: 'name', conflict: 'port' } as const)

  const change =
    <T,>(setter: (value: T) => void) =>
    (value: T) => {
      setter(value)
      setDirty(true)
      save.reset()
    }

  const submit = () => {
    setTouched(true)
    if (!valid || node === null) return
    const defaultAction: DefaultAction =
      effectiveAction === 'exit'
        ? { action: 'exit', node: exitNode ?? 0 }
        : effectiveAction === 'local'
          ? { action: 'local' }
          : { action: 'reject' }
    save.mutate(
      {
        node,
        name: name.trim(),
        bind,
        protocol,
        allow_from: external ? normalizeCidrList(allowItems) : [],
        require_credential: mustRequire || requireCredential,
        default_action: defaultAction,
        fixed_exit: fixedExit,
        class: serviceClass,
        optimistic: optimisticTo(optimistic),
      },
      { onSuccess: onClose },
    )
  }

  const shown = (problem: string | null) => (touched ? problem : null)
  const firstExit = exits[0]
  const usage = listenerUsage(
    { bind: portProblem === null ? bind : formatSocketAddr(hostValue, 1080), protocol },
    {
      exit: fixedExit !== null ? undefined : (firstExit?.virtual_ip ?? t.credentials.usageExitPlaceholder),
      credential: mustRequire || requireCredential ? t.credentials.credentialPlaceholder : undefined,
      hostPlaceholder: host?.name ?? t.credentials.usageHostPlaceholder,
    },
  )

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={listener === null ? d.createTitle : d.editTitle(listener.name)}
      dirty={dirty}
      requireDirty={listener !== null}
      submitLabel={listener === null ? d.submitCreate : d.submitEdit}
      pending={save.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
      onSubmit={submit}
    >
      <div className="grid grid-cols-2 gap-4">
        <Field label={d.node} error={shown(nodeProblem)}>
          {(props) => <NodeSelect {...props} nodes={all} value={node} onChange={change(setNode)} />}
        </Field>
        <Field label={d.name} error={shown(nameProblem) ?? fieldError(failure, 'name')}>
          {(props) => (
            <Input {...props} name="name" value={name} onChange={(event) => change(setName)(event.target.value)} />
          )}
        </Field>
      </div>

      <Field label={d.type}>
        {() => (
          <Segmented
            aria-label={d.type}
            value={protocol}
            onChange={change(setProtocol)}
            options={[
              { value: 'socks5', label: t.listeners.socks5 },
              { value: 'http', label: t.listeners.http },
            ]}
          />
        )}
      </Field>

      <div>
        <p id={whereLabel} className="mb-2 text-[13px] font-medium">
          {d.where}
        </p>
        <RadioCards
          aria-labelledby={whereLabel}
          value={where}
          onChange={change(setWhere)}
          columns={2}
          options={[
            { value: 'local', title: d.whereLocal, description: d.whereLocalDesc },
            { value: 'others', title: d.whereOthers, description: d.whereOthersDesc },
          ]}
        />
      </div>

      <div className="grid grid-cols-2 gap-4">
        {external ? (
          <Field label={d.address} hint={d.addressHint} error={shown(addressProblem)}>
            {(props) => (
              <Input
                {...props}
                className="font-mono"
                value={address}
                onChange={(event) => change(setAddress)(event.target.value)}
              />
            )}
          </Field>
        ) : null}
        <Field label={d.port} error={shown(portProblem) ?? fieldError(failure, 'port')}>
          {(props) => (
            <Input
              {...props}
              name="port"
              inputMode="numeric"
              className="w-32 font-mono"
              placeholder="1088"
              value={port}
              onChange={(event) => change(setPort)(event.target.value)}
            />
          )}
        </Field>
      </div>

      <label className="flex items-start gap-3">
        <Switch
          checked={mustRequire || requireCredential}
          disabled={mustRequire}
          onCheckedChange={change(setRequireCredential)}
          className="mt-0.5"
        />
        <span>
          <span className="block text-[13px] font-medium">{d.requireCredential}</span>
          <span className="block text-[13px] text-muted-foreground">
            {mustRequire ? d.requireCredentialForced : d.requireCredentialHint}
          </span>
        </span>
      </label>
      {(mustRequire || requireCredential) && credentials.data?.length === 0 ? <Notice>{d.noCredentials}</Notice> : null}

      {external ? (
        <Field label={d.allowFrom} hint={d.allowFromHint} optional={t.common.optional} error={shown(allowProblem)}>
          {(props) => (
            <Textarea
              {...props}
              rows={2}
              className="font-mono"
              placeholder="192.168.1.0/24"
              value={allowFrom}
              onChange={(event) => change(setAllowFrom)(event.target.value)}
            />
          )}
        </Field>
      ) : null}

      <div>
        <p id={actionLabel} className="text-[13px] font-medium">
          {d.defaultAction}
        </p>
        <p className="mb-2 text-[13px] text-muted-foreground">{d.defaultActionHint}</p>
        <RadioCards
          aria-labelledby={actionLabel}
          value={effectiveAction}
          onChange={change(setAction)}
          options={[
            {
              value: 'local',
              title: d.actionLocal,
              disabled: localNeedsExit,
              disabledReason: host === undefined ? undefined : d.actionLocalNeedsExit(host.name),
            },
            { value: 'reject', title: d.actionReject, description: d.actionRejectDesc },
            { value: 'exit', title: d.actionExit },
          ]}
        />
        {effectiveAction === 'exit' ? (
          <Field label={d.exitNode} error={shown(exitProblem)} className="mt-3">
            {(props) => (
              <NodeSelect
                {...props}
                nodes={exits}
                value={exitNode}
                onChange={change(setExitNode)}
                placeholder={exits.length === 0 ? d.exitNone : d.exitPlaceholder}
                disabled={exits.length === 0}
              />
            )}
          </Field>
        ) : null}
      </div>

      <Field label={d.fixedExit} hint={d.fixedExitHint} optional={t.common.optional}>
        {(props) => (
          <div className="flex items-center gap-2">
            <div className="min-w-0 flex-1">
              <NodeSelect
                {...props}
                nodes={exits}
                value={fixedExit}
                onChange={change(setFixedExit)}
                placeholder={exits.length === 0 ? d.exitNone : d.fixedExitNone}
                disabled={exits.length === 0}
              />
            </div>
            {fixedExit !== null ? (
              <button
                type="button"
                onClick={() => change(setFixedExit)(null)}
                className="rounded-sm text-[13px] text-primary underline-offset-4 hover:underline"
              >
                {d.fixedExitNone}
              </button>
            ) : null}
          </div>
        )}
      </Field>

      <div className="space-y-4 border-t pt-4">
        <p className="text-[13px] font-semibold">{d.advanced}</p>
        <Field label={d.classLabel} hint={d.classHint}>
          {(props) => (
            <ClassSelect
              {...props}
              value={serviceClass}
              inheritLabel={d.classInherit}
              onChange={change(setServiceClass)}
            />
          )}
        </Field>
        <Field
          label={d.optimisticLabel}
          hint={
            optimistic === 'auto'
              ? d.optimisticAutoHint
              : optimistic === 'on'
                ? d.optimisticOnHint
                : d.optimisticOffHint
          }
        >
          {() => (
            <Segmented
              aria-label={d.optimisticLabel}
              size="sm"
              value={optimistic}
              onChange={change(setOptimistic)}
              options={[
                { value: 'auto', label: d.optimisticAuto },
                { value: 'on', label: d.optimisticOn },
                { value: 'off', label: d.optimisticOff },
              ]}
              className="w-fit"
            />
          )}
        </Field>
      </div>

      <div>
        <p className="mb-1.5 text-[13px] font-medium">{d.usage}</p>
        <CodeBlock copy={usage}>{usage}</CodeBlock>
      </div>
    </FormDialog>
  )
}
