import { useQuery } from '@tanstack/react-query'
import { ArrowRight, CircleCheck } from 'lucide-react'
import { useId, useState } from 'react'
import { Link } from 'react-router'

import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import {
  type Forward,
  type ForwardInput,
  type ForwardProtocol,
  type Network,
  type NodeId,
  type ServiceClass,
} from '@/api/types'
import { DeclareHowTo } from '@/components/declare'
import { FormDialog } from '@/components/form-dialog'
import { NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { ruleLabel } from '@/lib/rules'
import { Button } from '@/components/ui/button'
import { Field } from '@/components/ui/field'
import { Input, Textarea } from '@/components/ui/input'
import { RadioCards } from '@/components/ui/radio-cards'
import { Segmented } from '@/components/ui/segmented'
import { Switch } from '@/components/ui/switch'
import { ClassSelect } from '@/components/usage'
import { t } from '@/i18n'
import { formatSocketAddr, isLoopbackHost, parseIpv4, parseSocketAddr } from '@/lib/cidr'
import { WAITING_INTERVAL } from '@/lib/polling'
import { formatList } from '@/lib/format'
import { classLabel } from '@/lib/serviceClass'
import { normalizeCidrList, splitList, validateCidrList, validatePort, validateTitle } from '@/lib/validate'
import { canBeForwardTarget, forwardTargetBySwitch, nodeName } from '@/lib/nodes'
import { networkPath } from '@/pages/networks/context'

const d = t.forwards.dialog

/** 谁能用这个端口：监听在回环地址、监听在虚拟 IP、监听在局域网地址或 0.0.0.0。 */
export type ListenScope = 'self' | 'mesh' | 'open'

interface ForwardDialogProps {
  network: Network
  /** 要编辑的转发；新建时是 `null`。 */
  forward: Forward | null
  /** 新建时预选的监听方式（空状态里的两个例子带过来）。 */
  scope?: ListenScope
  open: boolean
  onClose: () => void
}

/**
 * 新建和编辑转发。左右两块，中间一个箭头，读起来就是"从这里 → 到那里"。
 */
export function ForwardDialog({ network, forward, scope: presetScope, open, onClose }: ForwardDialogProps) {
  const scopeLabel = useId()
  const targetKindLabel = useId()

  const bindParsed = forward === null ? null : parseSocketAddr(forward.bind)
  const targetParsed = forward === null ? null : parseSocketAddr(forward.target_addr)

  const [listenNode, setListenNode] = useState<NodeId | null>(forward?.listen_node ?? null)
  const [targetNode, setTargetNode] = useState<NodeId | null>(forward?.target_node ?? null)
  // 目标节点还没声明时页面在等它上报，取得勤一些。
  const [awaiting, setAwaiting] = useState(false)
  const nodes = useQuery({ ...q.nodes(network.id), refetchInterval: awaiting ? WAITING_INTERVAL : undefined })
  const all = nodes.data ?? []
  const listen = all.find((node) => node.id === listenNode)
  const target = all.find((node) => node.id === targetNode)

  const initialScope = (): ListenScope => {
    if (bindParsed === null) return presetScope ?? 'self'
    if (isLoopbackHost(bindParsed.host)) return 'self'
    const owner = all.find((node) => node.id === forward?.listen_node)
    return owner?.virtual_ip === bindParsed.host ? 'mesh' : 'open'
  }
  const [scope, setScope] = useState<ListenScope>(initialScope)
  const [openAddress, setOpenAddress] = useState(
    bindParsed !== null && initialScope() === 'open' ? bindParsed.host : '0.0.0.0',
  )
  const [allowFrom, setAllowFrom] = useState(forward?.allow_from.join('\n') ?? '')
  // 端口写 0 的转发由监听节点挑端口；编辑时输入框留空，表示还是让它挑。
  const [port, setPort] = useState(bindParsed === null || bindParsed.port === 0 ? '' : String(bindParsed.port))
  const [protocol, setProtocol] = useState<ForwardProtocol>(forward?.protocol ?? 'tcp')
  const [targetKind, setTargetKind] = useState<'self' | 'lan'>(
    targetParsed !== null && !isLoopbackHost(targetParsed.host) ? 'lan' : 'self',
  )
  const [targetIp, setTargetIp] = useState(
    targetParsed !== null && !isLoopbackHost(targetParsed.host) ? targetParsed.host : '',
  )
  const [targetPort, setTargetPort] = useState(targetParsed?.port.toString() ?? '')
  const [name, setName] = useState(forward?.name ?? '')
  const [enabled, setEnabled] = useState(forward?.enabled ?? true)
  const [serviceClass, setServiceClass] = useState<ServiceClass | null>(forward?.class ?? null)
  const [touched, setTouched] = useState(false)
  const [dirty, setDirty] = useState(false)

  const targetDeclared = target === undefined || canBeForwardTarget(target)
  // 0.1.11 起目标是管理员的开关：没打开时这里就能打开，不用去节点上声明。
  const turnOn = useAction(network.id, {
    mutationFn: (id: NodeId) => api.updateNode(network.id, id, { forward_target: true }),
    success: (_saved: unknown, id: NodeId) => t.nodes.detail.forwardOnToast(nodeName(all, id)),
  })
  if (awaiting !== !targetDeclared) setAwaiting(!targetDeclared)

  const check = useQuery({
    ...q.check(network.id, listenNode ?? 0, targetNode ?? 0),
    enabled: listenNode !== null && targetNode !== null && listenNode !== targetNode,
  })

  const bindHost = scope === 'self' ? '127.0.0.1' : scope === 'mesh' ? (listen?.virtual_ip ?? '') : openAddress.trim()
  const bind = formatSocketAddr(bindHost, Number(port) || 0)
  const targetHost = targetKind === 'self' ? '127.0.0.1' : targetIp.trim()
  const targetAddr = formatSocketAddr(targetHost, Number(targetPort) || 0)

  const problems = {
    listenNode: listenNode === null ? t.validate.pickNode : null,
    targetNode: targetNode === null ? t.validate.pickNode : null,
    openAddress:
      scope === 'open' && parseIpv4(bindHost) === null && !bindHost.includes(':') ? t.validate.ipFormat : null,
    allowFrom: scope === 'open' ? validateCidrList(splitList(allowFrom)) : null,
    // 留空让节点自己挑；监听在虚拟 IP 上的不行（controller 不收）。
    port: port.trim() === '' ? (scope === 'mesh' ? d.portMeshRequired : null) : validatePort(port),
    targetIp:
      targetKind === 'lan'
        ? parseIpv4(targetHost) === null
          ? t.validate.ipFormat
          : targetHost === '0.0.0.0'
            ? t.validate.socketAddrUnspecified
            : null
        : null,
    targetPort: validatePort(targetPort),
    name: validateTitle(name, false),
  }
  const valid = Object.values(problems).every((problem) => problem === null)
  const shown = (problem: string | null) => (touched ? problem : null)

  const save = useAction(network.id, {
    mutationFn: (input: ForwardInput) =>
      forward === null ? api.createForward(network.id, input) : api.replaceForward(network.id, forward.id, input),
    success: (saved) => {
      const listenName = all.find((node) => node.id === saved.listen_node)?.name ?? ''
      if (forward !== null) return d.saved
      if (!saved.enabled) return d.createdDisabled
      // 端口是节点挑的：这时还不知道是哪个，列表里那一行等它报上来再写出地址。
      return parseSocketAddr(saved.bind)?.port === 0 ? d.createdPicked(listenName) : d.created(listenName, saved.bind)
    },
    toastError: false,
  })
  const failure = formFailure(save.error, {
    conflict: 'port',
    not_declared: 'targetNode',
    forward_target_off: 'targetNode',
  } as const)

  const change =
    <T,>(setter: (value: T) => void) =>
    (value: T) => {
      setter(value)
      setDirty(true)
      save.reset()
    }

  const submit = () => {
    setTouched(true)
    if (!valid || listenNode === null || targetNode === null) return
    save.mutate(
      {
        name: name.trim(),
        protocol,
        listen_node: listenNode,
        bind,
        target_node: targetNode,
        target_addr: targetAddr,
        allow_from: scope === 'open' ? normalizeCidrList(splitList(allowFrom)) : [],
        enabled: enabled && targetDeclared,
        class: serviceClass,
      },
      { onSuccess: onClose },
    )
  }

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={forward === null ? d.createTitle : d.editTitle(forward.name === '' ? forward.bind : forward.name)}
      size="lg"
      dirty={dirty}
      requireDirty={forward !== null}
      submitLabel={forward === null ? d.submitCreate : d.submitEdit}
      pending={save.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
      onSubmit={submit}
    >
      <div className="grid grid-cols-[minmax(0,1fr)_2rem_minmax(0,1fr)] gap-x-4">
        <fieldset className="space-y-4">
          <legend className="mb-3 text-sm font-semibold">{d.listenSide}</legend>
          <Field label={d.node} error={shown(problems.listenNode)}>
            {(props) => <NodeSelect {...props} nodes={all} value={listenNode} onChange={change(setListenNode)} />}
          </Field>
          <div>
            <p id={scopeLabel} className="mb-2 text-[13px] font-medium">
              {d.who}
            </p>
            <RadioCards
              aria-labelledby={scopeLabel}
              value={scope}
              onChange={change(setScope)}
              options={[
                { value: 'self', title: d.whoSelf, description: d.whoSelfDesc },
                {
                  value: 'mesh',
                  title: d.whoMesh,
                  description: listen === undefined ? d.whoMeshDescNoNode : d.whoMeshDesc(listen.virtual_ip),
                },
                { value: 'open', title: d.whoOpen, description: d.whoOpenDesc },
              ]}
            />
          </div>
          {scope === 'open' ? (
            <>
              <Notice tone="warn">{d.openWarning}</Notice>
              <Field label={d.address} hint={d.addressHint} error={shown(problems.openAddress)}>
                {(props) => (
                  <Input
                    {...props}
                    className="font-mono"
                    value={openAddress}
                    onChange={(event) => change(setOpenAddress)(event.target.value)}
                  />
                )}
              </Field>
              <Field
                label={d.allowFrom}
                hint={d.allowFromHint}
                optional={t.common.optional}
                error={shown(problems.allowFrom)}
              >
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
            </>
          ) : null}
          <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-4">
            <Field
              label={d.port}
              hint={scope === 'mesh' ? undefined : d.portHint}
              error={shown(problems.port) ?? fieldError(failure, 'port')}
            >
              {(props) => (
                <Input
                  {...props}
                  name="listen-port"
                  inputMode="numeric"
                  className="w-32 font-mono"
                  placeholder={scope === 'mesh' ? '8080' : d.portPlaceholder}
                  value={port}
                  onChange={(event) => change(setPort)(event.target.value)}
                />
              )}
            </Field>
            <Field label={d.protocol}>
              {() => (
                <Segmented
                  aria-label={d.protocol}
                  value={protocol}
                  onChange={change(setProtocol)}
                  options={[
                    { value: 'tcp', label: d.tcp },
                    { value: 'udp', label: d.udp },
                  ]}
                  className="w-fit"
                />
              )}
            </Field>
          </div>
        </fieldset>

        <div className="flex items-start justify-center pt-24 text-faint">
          <ArrowRight className="size-5" aria-label={d.arrow} />
        </div>

        <fieldset className="space-y-4">
          <legend className="mb-3 text-sm font-semibold">{d.targetSide}</legend>
          <Field label={d.node} error={shown(problems.targetNode) ?? fieldError(failure, 'targetNode')}>
            {(props) => (
              <NodeSelect
                {...props}
                // 没有声明的节点也列出来，不然用户会以为它丢了；选中之后说明怎么声明。
                nodes={all}
                value={targetNode}
                onChange={change(setTargetNode)}
              />
            )}
          </Field>
          {target !== undefined && !canBeForwardTarget(target) ? (
            forwardTargetBySwitch(target) ? (
              <Notice
                tone="warn"
                action={
                  <Button size="sm" pending={turnOn.isPending} onClick={() => turnOn.mutate(target.id)}>
                    {d.targetTurnOn}
                  </Button>
                }
              >
                {d.targetOff(target.name)}
              </Notice>
            ) : (
              <div>
                <Notice tone="warn">{d.targetHowTo(target.name)}</Notice>
                <DeclareHowTo capability="forward" nodeName={target.name} waiting className="mt-2" />
              </div>
            )
          ) : null}
          <div>
            <p id={targetKindLabel} className="mb-2 text-[13px] font-medium">
              {d.targetAddress}
            </p>
            <RadioCards
              aria-labelledby={targetKindLabel}
              value={targetKind}
              onChange={change(setTargetKind)}
              options={[
                { value: 'self', title: d.targetSelf },
                { value: 'lan', title: d.targetLan },
              ]}
            />
          </div>
          <div className="grid grid-cols-[minmax(0,1fr)_8rem] gap-4">
            {targetKind === 'lan' ? (
              <Field label={d.targetIp} error={shown(problems.targetIp)}>
                {(props) => (
                  <Input
                    {...props}
                    name="target-ip"
                    className="font-mono"
                    placeholder="192.168.1.10"
                    value={targetIp}
                    onChange={(event) => change(setTargetIp)(event.target.value)}
                  />
                )}
              </Field>
            ) : (
              <div />
            )}
            <Field label={d.targetPort} error={shown(problems.targetPort)}>
              {(props) => (
                <Input
                  {...props}
                  name="target-port"
                  inputMode="numeric"
                  className="font-mono"
                  placeholder="80"
                  value={targetPort}
                  onChange={(event) => change(setTargetPort)(event.target.value)}
                />
              )}
            </Field>
          </div>
        </fieldset>
      </div>

      {/* 即时检查：转发不绕过访问规则。不拦着保存，因为规则以后还会变。 */}
      <div aria-live="polite">
        {listen !== undefined && target !== undefined ? (
          listen.id === target.id ? (
            <Notice>
              {scope === 'mesh' && problems.port === null
                ? t.common.sentences(d.checkSame, d.meshNote(bind))
                : d.checkSame}
            </Notice>
          ) : check.data === undefined ? null : check.data.allowed ? (
            <p className="flex items-center gap-1.5 text-[13px] text-ok" data-testid="forward-check">
              <CircleCheck className="size-4" aria-hidden />
              {d.checkAllowed(listen.name, target.name, formatList(check.data.rules.map(ruleLabel)))}
            </p>
          ) : (
            <Notice
              tone="warn"
              action={
                <Link
                  to={networkPath(network.id, 'access')}
                  className="rounded-sm text-[13px] font-medium text-primary underline-offset-4 hover:underline"
                >
                  {t.forwards.viewRules}
                </Link>
              }
            >
              <span data-testid="forward-check">{d.checkDenied(listen.name, target.name)}</span>
            </Notice>
          )
        ) : null}
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-end gap-6">
        <Field label={d.name} optional={t.common.optional} error={shown(problems.name)}>
          {(props) => (
            <Input
              {...props}
              name="name"
              placeholder="web"
              value={name}
              onChange={(event) => change(setName)(event.target.value)}
            />
          )}
        </Field>
        <label className="flex h-9 items-center gap-2.5 text-sm">
          <Switch checked={enabled && targetDeclared} disabled={!targetDeclared} onCheckedChange={change(setEnabled)} />
          {d.enabled}
        </label>
      </div>
      <Field label={d.classLabel}>
        {(props) => (
          <ClassSelect
            {...props}
            value={serviceClass}
            inheritLabel={t.serviceClass.inheritNetwork(classLabel(network.class ?? 'auto'))}
            onChange={change(setServiceClass)}
          />
        )}
      </Field>
    </FormDialog>
  )
}
