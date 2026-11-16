import { useQuery } from '@tanstack/react-query'
import { type ReactNode, useEffect, useState } from 'react'
import { Link } from 'react-router'

import { errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeDetail, type Reachability, type RelayInput } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock } from '@/components/copy'
import { DeclareHowTo } from '@/components/declare'
import { Notice } from '@/components/notice'
import { StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Field, FormError } from '@/components/ui/field'
import { Input, Textarea } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { exitUsage } from '@/lib/commands'
import { attachedCount } from '@/lib/nodes'
import { type Capability, DEFAULT_NODE_PORT, MAX_RELAY_ADDRESSES } from '@/lib/product'
import { cn } from '@/lib/utils'
import { parseOptionalPositiveInt, splitList, validateSocketAddr } from '@/lib/validate'
import { networkPath } from '@/pages/networks/context'

import { exitSummary, useSaveExit } from './exitActions'
import { ExitFormFields } from './exitForm'
import { exitFormFrom, type ExitFormState } from './exitFormState'

const d = t.nodes.detail

interface RowShellProps {
  title: string
  description: string
  /** 右端的开关，或者只读的状态。 */
  control: ReactNode
  /** 状态行：现在是什么情况、下一步做什么。 */
  status: ReactNode
  /** 声明刚到：轻微地亮一下，让刚从终端回来的人看到界面已经准备好了。 */
  flash?: boolean
  children?: ReactNode
  testId: string
}

/** 能力卡片里的一行：名字和一句话解释 · 开关 · 状态行。三行的结构相同。 */
function RowShell({ title, description, control, status, flash = false, children, testId }: RowShellProps) {
  return (
    <div data-testid={testId} className={cn('-mx-5 px-5 py-4', flash && 'animate-flash')}>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h3 className="text-sm font-semibold">{title}</h3>
          <p className="mt-0.5 text-[13px] text-muted-foreground">{description}</p>
        </div>
        <div className="shrink-0 pt-0.5">{control}</div>
      </div>
      <div className="mt-2.5 text-[13px]">{status}</div>
      {children}
    </div>
  )
}

interface NotDeclaredProps {
  capability: Capability
  node: Node
  open: boolean
  onToggle: () => void
}

/** "节点还没有声明"：原因写在灰掉的开关旁边，"怎么声明"点开是照着做的说明。 */
function NotDeclared({ capability, node, open, onToggle }: NotDeclaredProps) {
  return (
    <>
      <p data-testid={`not-declared-${capability}`}>
        {t.declare.notDeclared(node.name)}{' '}
        <Button variant="link" size="sm" onClick={onToggle} aria-expanded={open}>
          {open ? t.declare.hide : t.declare.howTo}
        </Button>
      </p>
      {open ? <DeclareHowTo capability={capability} nodeName={node.name} waiting className="mt-3" /> : null}
    </>
  )
}

/**
 * "怎么声明"开着的时候页面在等节点上报。声明一到，说明自己收起，这一行亮一下。
 * 返回值里的 `flash` 在亮过之后自己复位。
 */
function useDeclareWatch(declared: boolean, onWaiting: (waiting: boolean) => void) {
  const [open, setOpen] = useState(false)
  const [flash, setFlash] = useState(false)

  // 从取到的数据推出来的状态，在渲染时直接改：声明到了就收起说明。
  if (open && declared) {
    setOpen(false)
    setFlash(true)
  }

  useEffect(() => {
    onWaiting(open)
    return () => onWaiting(false)
  }, [open, onWaiting])

  useEffect(() => {
    if (!flash) return undefined
    const timer = window.setTimeout(() => setFlash(false), 1500)
    return () => window.clearTimeout(timer)
  }, [flash])

  return { open, flash, toggle: () => setOpen((value) => !value) }
}

interface CapabilityRowProps {
  network: Network
  node: NodeDetail
  /** 这一行正在等节点上报声明：页面据此把轮询调得勤一些。 */
  onWaiting: (waiting: boolean) => void
}

interface RelayFormState {
  udp: string
  tcp: string
  region: string
  bandwidth: string
}

function relayFormFrom(node: NodeDetail): RelayFormState {
  if (node.relay !== null) {
    return {
      udp: node.relay.udp.join('\n'),
      tcp: node.relay.tcp.join('\n'),
      region: node.relay.region,
      bandwidth: node.relay.bandwidth_mbps?.toString() ?? '',
    }
  }
  // 第一次启用：用 controller 看到的地址预填，多数有公网 IP 的机器不用再改。
  const observed = node.observed_endpoint ?? ''
  return { udp: observed, tcp: observed, region: '', bandwidth: '' }
}

/**
 * 启用 relay 之后 controller 从自己这边试连的结果。这是一条建议，不是错误：relay 已经启用了，
 * controller 和节点不在同一个网络位置上，它连不上不等于节点连不上。
 */
function ReachabilityNotice({ results, onDismiss }: { results: Reachability[]; onDismiss: () => void }) {
  const failed = results.some((result) => !result.reachable)
  return (
    <Notice
      tone={failed ? 'warn' : 'info'}
      title={d.reachTitle}
      className="mt-3"
      action={
        <Button size="sm" variant="ghost" onClick={onDismiss}>
          {d.reachDismiss}
        </Button>
      }
    >
      <ul className="space-y-0.5" data-testid="relay-reachability">
        {results.map((result) => (
          <li key={`${result.protocol}-${result.addr}`} className={result.reachable ? 'text-ok' : 'text-warn'}>
            {result.reachable
              ? d.reachOk(
                  result.protocol.toUpperCase(),
                  result.addr,
                  result.rtt_ms === null ? '—' : t.common.ms(result.rtt_ms),
                )
              : d.reachFail(result.protocol.toUpperCase(), result.addr, result.error ?? '—')}
          </li>
        ))}
      </ul>
      {failed ? <p className="mt-1.5">{d.reachSomeFail}</p> : null}
    </Notice>
  )
}

function addressListProblem(raw: string): string | null {
  const items = splitList(raw)
  if (items.length > MAX_RELAY_ADDRESSES) return d.relayFormTooMany(MAX_RELAY_ADDRESSES)
  for (const item of items) {
    const problem = validateSocketAddr(item)
    if (problem !== null) return d.relayFormAddress(item, problem)
  }
  return null
}

export function RelayRow({ network, node, onWaiting }: CapabilityRowProps) {
  const confirm = useConfirm()
  const nodes = useQuery(q.nodes(network.id))
  const watch = useDeclareWatch(node.declared.relay, onWaiting)
  const [editing, setEditing] = useState(false)
  const [form, setForm] = useState<RelayFormState>(() => relayFormFrom(node))
  const [submitted, setSubmitted] = useState(false)
  const [reachability, setReachability] = useState<Reachability[] | null>(null)
  const enabled = node.relay !== null

  const save = useAction(network.id, {
    mutationFn: (input: RelayInput) => api.enableRelay(network.id, node.id, input),
    success: enabled ? d.relaySavedToast : d.relayEnabledToast(node.name),
    toastError: false,
  })
  const attached = attachedCount(nodes.data ?? [], node.id)
  const disable = useAction(network.id, {
    mutationFn: () => api.disableRelay(network.id, node.id),
    success: d.relayDisabledToast(node.name),
  })

  const udpProblem = addressListProblem(form.udp)
  const tcpProblem = addressListProblem(form.tcp)
  const needOne = splitList(form.udp).length + splitList(form.tcp).length === 0 ? d.relayFormNeedOne : null
  const bandwidth = parseOptionalPositiveInt(form.bandwidth)

  const openForm = () => {
    setForm(relayFormFrom(node))
    setSubmitted(false)
    setReachability(null)
    save.reset()
    setEditing(true)
  }

  const submit = () => {
    setSubmitted(true)
    if (udpProblem !== null || tcpProblem !== null || needOne !== null || bandwidth.error !== null) return
    save.mutate(
      {
        udp: splitList(form.udp),
        tcp: splitList(form.tcp),
        region: form.region.trim(),
        bandwidth_mbps: bandwidth.value,
      },
      {
        onSuccess: (saved) => {
          setEditing(false)
          setReachability(saved.reachability.length > 0 ? saved.reachability : null)
        },
      },
    )
  }

  const turnOff = async () => {
    const relays = (nodes.data ?? []).filter((other) => other.relay !== null)
    const pinnedBy = (nodes.data ?? []).filter((other) => other.pinned_relays.includes(node.id)).length
    const only = relays.length === 1
    // 唯一的 relay 关掉之后，打不通直连的节点对就互不可达了：这是危险级别。
    const confirmed = await confirm({
      tone: only ? 'danger' : 'normal',
      title: d.relayOffTitle(node.name),
      body: (
        <>
          <p>{only ? d.relayOffOnly(node.name) : d.relayOffGeneric}</p>
          {attached > 0 ? <p>{d.relayOffAttached(attached)}</p> : null}
          {pinnedBy > 0 ? <p>{d.relayOffPinned(pinnedBy)}</p> : null}
        </>
      ),
      confirmLabel: d.relayOffConfirm,
    })
    if (confirmed) {
      setEditing(false)
      setReachability(null)
      disable.mutate()
    }
  }

  const onSwitch = (checked: boolean) => {
    if (checked) openForm()
    else if (enabled) void turnOff()
    else setEditing(false)
  }

  const pinnedBy = (nodes.data ?? []).filter((other) => other.pinned_relays.includes(node.id)).length

  let status: ReactNode
  if (node.relay !== null) {
    const relay = node.relay
    // 第二行的几项用同一种分隔，和出口那一行的写法一致。
    const facts = [
      relay.region !== '' ? d.relayRegion(relay.region) : null,
      relay.bandwidth_mbps === null ? d.relayUnlimited : d.relayBandwidth(relay.bandwidth_mbps),
      pinnedBy > 0 ? d.relayPinnedBy(pinnedBy) : null,
    ].filter((fact): fact is string => fact !== null)
    status = (
      <div className="space-y-1.5">
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <StatusText tone="ok">{d.enabled}</StatusText>
          {relay.udp.length > 0 ? (
            <span>
              {d.relayUdp} <code className="font-mono text-xs">{relay.udp.join(', ')}</code>
            </span>
          ) : null}
          {relay.tcp.length > 0 ? (
            <span>
              {d.relayTcp} <code className="font-mono text-xs">{relay.tcp.join(', ')}</code>
            </span>
          ) : null}
        </p>
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground">
          <span>
            {facts.map((fact) => `${fact} · `).join('')}
            <Tooltip content={d.relayAttachedTip}>
              <span
                tabIndex={0}
                className="cursor-help rounded-sm underline decoration-dotted underline-offset-4"
                data-testid="relay-attached"
              >
                {d.relayAttached(attached)}
              </span>
            </Tooltip>
          </span>
          {editing ? null : (
            <Button variant="link" size="sm" onClick={openForm}>
              {d.edit}
            </Button>
          )}
        </p>
      </div>
    )
  } else if (!node.declared.relay) {
    status = <NotDeclared capability="relay" node={node} open={watch.open} onToggle={watch.toggle} />
  } else {
    status = <p className="text-muted-foreground">{node.online ? d.declaredReady : d.offlineNote}</p>
  }

  const canToggle = enabled || node.declared.relay

  return (
    <RowShell
      testId="capability-relay"
      title={d.relayTitle}
      description={d.relayDesc}
      flash={watch.flash}
      control={
        <Switch
          aria-label={d.relaySwitch(node.name)}
          checked={enabled || editing}
          disabled={!canToggle || disable.isPending || save.isPending}
          onCheckedChange={onSwitch}
        />
      }
      status={status}
    >
      {watch.flash ? (
        <p role="status" className="mt-2 text-[13px] font-medium text-ok">
          {t.declare.arrived}
        </p>
      ) : null}
      {reachability !== null && !editing ? (
        <ReachabilityNotice results={reachability} onDismiss={() => setReachability(null)} />
      ) : null}
      {editing ? (
        <form
          noValidate
          onSubmit={(event) => {
            event.preventDefault()
            submit()
          }}
          className="mt-4 space-y-4 rounded-lg bg-muted/60 p-4"
        >
          {!enabled && node.observed_endpoint !== null ? (
            <Notice>{d.relayPrefilled(node.observed_endpoint)}</Notice>
          ) : null}
          <div className="grid grid-cols-2 gap-4">
            <Field label={d.relayFormUdp} hint={d.relayFormUdpHint} error={submitted ? (udpProblem ?? needOne) : null}>
              {(props) => (
                <Textarea
                  {...props}
                  rows={2}
                  className="font-mono"
                  placeholder={`203.0.113.7:${DEFAULT_NODE_PORT}`}
                  value={form.udp}
                  onChange={(event) => setForm({ ...form, udp: event.target.value })}
                />
              )}
            </Field>
            <Field label={d.relayFormTcp} hint={d.relayFormTcpHint} error={submitted ? tcpProblem : null}>
              {(props) => (
                <Textarea
                  {...props}
                  rows={2}
                  className="font-mono"
                  placeholder={`203.0.113.7:${DEFAULT_NODE_PORT}`}
                  value={form.tcp}
                  onChange={(event) => setForm({ ...form, tcp: event.target.value })}
                />
              )}
            </Field>
            <Field label={d.relayFormRegion} optional={t.common.optional}>
              {(props) => (
                <Input
                  {...props}
                  placeholder="hk"
                  value={form.region}
                  onChange={(event) => setForm({ ...form, region: event.target.value })}
                />
              )}
            </Field>
            <Field label={d.relayFormBandwidth} error={submitted ? bandwidth.error : null} optional={t.common.optional}>
              {(props) => (
                <div className="flex items-center gap-2">
                  <Input
                    {...props}
                    inputMode="numeric"
                    placeholder={t.common.unlimited}
                    className="w-28 tabular"
                    value={form.bandwidth}
                    onChange={(event) => setForm({ ...form, bandwidth: event.target.value })}
                  />
                  <span className="text-[13px] text-muted-foreground">{t.common.mbps}</span>
                </div>
              )}
            </Field>
          </div>
          <p className="text-[13px] text-muted-foreground">{d.relayPortReminder(DEFAULT_NODE_PORT)}</p>
          {save.isPending ? (
            <p role="status" className="text-[13px] text-relay">
              {d.relayChecking}
            </p>
          ) : null}
          <FormError
            message={save.isError ? errorMessage(save.error) : null}
            detail={save.isError ? errorDetail(save.error) : null}
          />
          <div className="flex justify-end gap-2">
            <Button onClick={() => setEditing(false)} disabled={save.isPending}>
              {t.common.cancel}
            </Button>
            <Button type="submit" variant="primary" pending={save.isPending}>
              {enabled ? d.save : d.relayEnable}
            </Button>
          </div>
        </form>
      ) : null}
    </RowShell>
  )
}

export function ExitRow({ network, node, onWaiting }: CapabilityRowProps) {
  const confirm = useConfirm()
  const watch = useDeclareWatch(node.declared.exit, onWaiting)
  const [editing, setEditing] = useState(false)
  const [form, setForm] = useState<ExitFormState>(() => exitFormFrom(node.exit))
  const [submitted, setSubmitted] = useState(false)
  const enabled = node.exit !== null
  const save = useSaveExit(network, node)
  const disable = useAction(network.id, {
    mutationFn: () => api.disableExit(network.id, node.id),
    success: d.exitDisabledToast(node.name),
  })

  const openForm = () => {
    setForm(exitFormFrom(node.exit))
    setSubmitted(false)
    save.reset()
    setEditing(true)
  }

  const turnOff = async () => {
    const confirmed = await confirm({
      title: d.exitOffTitle(node.name),
      body: d.exitOffBody,
      confirmLabel: d.exitOffConfirm,
    })
    if (confirmed) {
      setEditing(false)
      disable.mutate()
    }
  }

  const onSwitch = (checked: boolean) => {
    if (checked) openForm()
    else if (enabled) void turnOff()
    else setEditing(false)
  }

  let status: ReactNode
  if (node.exit !== null) {
    status = (
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <StatusText tone="ok">{d.enabled}</StatusText>
        <span className="text-muted-foreground">{exitSummary(node.exit)}</span>
        {editing ? null : (
          <Button variant="link" size="sm" onClick={openForm}>
            {d.edit}
          </Button>
        )}
      </p>
    )
  } else if (!node.declared.exit) {
    status = <NotDeclared capability="exit" node={node} open={watch.open} onToggle={watch.toggle} />
  } else {
    status = <p className="text-muted-foreground">{node.online ? d.declaredReady : d.offlineNote}</p>
  }

  return (
    <RowShell
      testId="capability-exit"
      title={d.exitTitle}
      description={d.exitDesc}
      flash={watch.flash}
      control={
        <Switch
          aria-label={d.exitSwitch(node.name)}
          checked={enabled || editing}
          disabled={!(enabled || node.declared.exit) || disable.isPending || save.pending}
          onCheckedChange={onSwitch}
        />
      }
      status={status}
    >
      {watch.flash ? (
        <p role="status" className="mt-2 text-[13px] font-medium text-ok">
          {t.declare.arrived}
        </p>
      ) : null}
      {editing ? (
        <form
          noValidate
          onSubmit={(event) => {
            event.preventDefault()
            setSubmitted(true)
            void save.submit(form).then((ok) => {
              if (ok) setEditing(false)
            })
          }}
          className="mt-4 space-y-4 rounded-lg bg-muted/60 p-4"
        >
          <ExitFormFields state={form} onChange={setForm} showProblems={submitted} />
          <FormError message={save.error} detail={save.errorDetail} />
          <div className="flex justify-end gap-2">
            <Button onClick={() => setEditing(false)} disabled={save.pending}>
              {t.common.cancel}
            </Button>
            <Button type="submit" variant="primary" pending={save.pending}>
              {enabled ? d.save : d.exitEnable}
            </Button>
          </div>
        </form>
      ) : null}
      {enabled && !editing ? (
        <div className="mt-3">
          <p className="text-[13px] text-muted-foreground">{d.exitUsageBody}</p>
          <CodeBlock copy={exitUsage(node.virtual_ip)} className="mt-1.5">
            {exitUsage(node.virtual_ip)}
          </CodeBlock>
        </div>
      ) : null}
    </RowShell>
  )
}

/** 端口转发的目标没有开关：声明过的节点可以被选作转发的目标。这里只读地显示声明的情况。 */
export function ForwardTargetRow({ network, node, onWaiting }: CapabilityRowProps) {
  const watch = useDeclareWatch(node.declared.forward, onWaiting)
  const forwards = useQuery(q.forwards(network.id))
  const count = (forwards.data ?? []).filter((forward) => forward.target_node === node.id).length

  return (
    <RowShell
      testId="capability-forward"
      title={d.forwardTitle}
      description={d.forwardDesc}
      flash={watch.flash}
      control={
        <StatusText tone={node.declared.forward ? 'ok' : 'idle'} className="text-[13px]">
          {node.declared.forward ? d.forwardDeclared : d.forwardNotDeclared}
        </StatusText>
      }
      status={
        node.declared.forward ? (
          <p className="text-muted-foreground">
            <Link
              to={networkPath(network.id, 'forwards')}
              className="rounded-sm text-primary underline-offset-4 hover:underline"
            >
              {d.forwardCount(count)}
            </Link>
          </p>
        ) : (
          <NotDeclared capability="forward" node={node} open={watch.open} onToggle={watch.toggle} />
        )
      }
    />
  )
}
