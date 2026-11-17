import { useQuery } from '@tanstack/react-query'
import {
  ArrowLeftRight,
  ArrowRight,
  Cable,
  ChartPie,
  DoorOpen,
  Earth,
  Forward,
  Gauge,
  Globe,
  Hash,
  House,
  Layers,
  Pencil,
  Pin,
  Radio,
  RadioTower,
  Users,
  Zap,
} from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { Link } from 'react-router'

import { errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeDetail, type Reachability, type RelayInput } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock } from '@/components/copy'
import { DeclareHowTo } from '@/components/declare'
import { DetailList, DetailRow } from '@/components/detail-list'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { SkeletonBlock } from '@/components/states'
import { StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { BoxCard } from '@/components/ui/card'
import { Field, FormError } from '@/components/ui/field'
import { Input, Textarea } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { t } from '@/i18n'
import { exitUsage } from '@/lib/commands'
import { formatBytes } from '@/lib/format'
import { attachedCount, nodeName } from '@/lib/nodes'
import { type Capability, DEFAULT_NODE_PORT, MAX_RELAY_ADDRESSES } from '@/lib/product'
import { classLabel } from '@/lib/serviceClass'
import { cn } from '@/lib/utils'
import { parseOptionalPositiveInt, splitList, validateSocketAddr } from '@/lib/validate'
import { forwardLabel, forwardStatusView } from '@/pages/forwards/status'
import { networkPath } from '@/pages/networks/context'

import { useSaveExit } from './exitActions'
import { ExitFormFields } from './exitForm'
import { exitFormFrom, type ExitFormState } from './exitFormState'

const d = t.nodes.detail

interface CapabilityCardProps {
  icon: ReactNode
  title: string
  /** 标题旁边悬停看的补充说明。 */
  tip?: string
  description: string
  /** 右端的开关，或者只读的状态。 */
  control: ReactNode
  /** 开关左边的按钮（修改）。 */
  action?: ReactNode
  /** 声明刚到：轻微地亮一下，让刚从终端回来的人看到界面已经准备好了。 */
  flash?: boolean
  children?: ReactNode
  testId: string
}

/**
 * 一种能力一张卡片：顶上是图标、名字、一句话解释和开关；下面一根线，是现在的设置（"名字：值"一行一项）、
 * 修改的表单，或者怎么声明。三张卡片的结构相同。
 */
function CapabilityCard({
  icon,
  title,
  tip,
  description,
  control,
  action,
  flash = false,
  children,
  testId,
}: CapabilityCardProps) {
  return (
    <BoxCard data-testid={testId} className={cn('overflow-hidden', flash && 'animate-flash')}>
      <header className="flex items-center justify-between gap-4 px-5 py-4">
        <div className="flex min-w-0 items-center gap-3.5">
          <span
            aria-hidden
            className="flex size-10 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/80 [&_svg]:size-5"
          >
            {icon}
          </span>
          <div className="min-w-0">
            <h2 className="flex items-center gap-1.5 text-[15px] leading-6 font-semibold">
              {title}
              {tip !== undefined ? <InfoTip>{tip}</InfoTip> : null}
            </h2>
            <p className="text-[13px] text-muted-foreground">{description}</p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          {action}
          {control}
        </div>
      </header>
      {children !== undefined && children !== null && children !== false ? (
        <div className="border-t">{children}</div>
      ) : null}
    </BoxCard>
  )
}

/** 卡片里一段说明文字（还没声明、已声明未启用）。 */
function CardNote({ children }: { children: ReactNode }) {
  return <div className="px-5 py-4 text-[13px]">{children}</div>
}

/** 开关左边的"修改"：一个安静的小按钮。 */
function EditButton({ onClick }: { onClick: () => void }) {
  return (
    <Button variant="ghost" size="sm" onClick={onClick} className="text-muted-foreground hover:text-foreground">
      <Pencil aria-hidden />
      {d.edit}
    </Button>
  )
}

/** 声明刚到时卡片里的一句话。 */
function Arrived() {
  return (
    <p role="status" className="px-5 pt-3 text-[13px] font-medium text-ok">
      {t.declare.arrived}
    </p>
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

interface CapabilityTabProps {
  network: Network
  node: NodeDetail
  /** 正在等节点上报声明：页面据此把轮询调得勤一些。 */
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
      className="mx-5 my-4"
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

export function RelayTab({ network, node, onWaiting }: CapabilityTabProps) {
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
  const canToggle = enabled || node.declared.relay
  const relay = node.relay

  let body: ReactNode
  if (editing) {
    body = (
      <form
        noValidate
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
        className="space-y-4 px-5 py-5"
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
    )
  } else if (relay !== null) {
    body = (
      <>
        <DetailList>
          <DetailRow icon={<Radio />} label={d.relayFormUdp} multiline={relay.udp.length > 1}>
            <AddressList addresses={relay.udp} />
          </DetailRow>
          <DetailRow icon={<Cable />} label={d.relayFormTcp} multiline={relay.tcp.length > 1}>
            <AddressList addresses={relay.tcp} />
          </DetailRow>
          <DetailRow icon={<Earth />} label={d.rowRegion}>
            {relay.region === '' ? '—' : relay.region}
          </DetailRow>
          <DetailRow icon={<Gauge />} label={d.rowBandwidth}>
            {bandwidthValue(relay.bandwidth_mbps)}
          </DetailRow>
          <DetailRow
            icon={<Users />}
            label={
              <span className="inline-flex items-center gap-1">
                {d.rowAttached}
                <InfoTip>{d.relayAttachedTip}</InfoTip>
              </span>
            }
          >
            <span data-testid="relay-attached" className="tabular">
              {d.rowNodes(attached)}
            </span>
          </DetailRow>
          {pinnedBy > 0 ? (
            <DetailRow icon={<Pin />} label={d.rowPinnedBy}>
              <span className="tabular">{d.rowNodes(pinnedBy)}</span>
            </DetailRow>
          ) : null}
        </DetailList>
      </>
    )
  } else if (!node.declared.relay) {
    body = (
      <CardNote>
        <NotDeclared capability="relay" node={node} open={watch.open} onToggle={watch.toggle} />
      </CardNote>
    )
  } else {
    body = (
      <CardNote>
        <p className="text-muted-foreground">{node.online ? d.declaredReady : d.offlineNote}</p>
      </CardNote>
    )
  }

  return (
    <CapabilityCard
      testId="capability-relay"
      icon={<RadioTower />}
      title={d.relayTitle}
      tip={d.withdrawnNote}
      description={d.relayDesc}
      flash={watch.flash}
      action={enabled && !editing ? <EditButton onClick={openForm} /> : null}
      control={
        <Switch
          aria-label={d.relaySwitch(node.name)}
          checked={enabled || editing}
          disabled={!canToggle || disable.isPending || save.isPending}
          onCheckedChange={onSwitch}
        />
      }
    >
      {watch.flash ? <Arrived /> : null}
      {body}
      {/* 保存之后马上就有：不等页面重新取到节点。 */}
      {reachability !== null && !editing ? (
        <ReachabilityNotice results={reachability} onDismiss={() => setReachability(null)} />
      ) : null}
    </CapabilityCard>
  )
}

export function ExitTab({ network, node, onWaiting }: CapabilityTabProps) {
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

  const exit = node.exit

  let body: ReactNode
  if (editing) {
    body = (
      <form
        noValidate
        onSubmit={(event) => {
          event.preventDefault()
          setSubmitted(true)
          void save.submit(form).then((ok) => {
            if (ok) setEditing(false)
          })
        }}
        className="space-y-4 px-5 py-5"
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
    )
  } else if (exit !== null) {
    body = (
      <>
        <DetailList>
          <DetailRow icon={<Hash />} label={d.rowPorts}>
            <span className="tabular">{exit.ports === null ? t.common.unlimited : exit.ports.join(', ')}</span>
          </DetailRow>
          <DetailRow icon={<House />} label={d.rowLan}>
            {exit.lan ? d.rowLanOn : d.rowLanOff}
          </DetailRow>
          <DetailRow icon={<Gauge />} label={d.rowBandwidth}>
            {bandwidthValue(exit.bandwidth_mbps)}
          </DetailRow>
          {exit.quota_bytes !== null ? (
            <DetailRow icon={<ChartPie />} label={d.rowQuota}>
              <span className="tabular">{d.rowQuotaValue(formatBytes(exit.quota_bytes), exit.quota_reset_day)}</span>
            </DetailRow>
          ) : null}
          {exit.family !== 'any' ? (
            <DetailRow icon={<Globe />} label={d.rowFamily}>
              {exit.family === 'ipv4' ? d.exitFamilyIpv4 : d.exitFamilyIpv6}
            </DetailRow>
          ) : null}
          {exit.connects_per_minute !== null ? (
            <DetailRow icon={<Zap />} label={d.rowConnects}>
              <span className="tabular">{d.rowConnectsValue(exit.connects_per_minute)}</span>
            </DetailRow>
          ) : null}
          {exit.class !== null ? (
            <DetailRow icon={<Layers />} label={d.rowClass}>
              {classLabel(exit.class)}
            </DetailRow>
          ) : null}
          {exit.upstream_socks5 !== null ? (
            <DetailRow icon={<Forward />} label={d.rowUpstream} copy={exit.upstream_socks5}>
              <span className="tabular">{exit.upstream_socks5}</span>
            </DetailRow>
          ) : null}
        </DetailList>
        <div className="border-t px-5 py-4">
          <p className="text-[13px] text-muted-foreground">{d.exitUsageBody}</p>
          <CodeBlock copy={exitUsage(node.virtual_ip)} className="mt-2">
            {exitUsage(node.virtual_ip)}
          </CodeBlock>
        </div>
      </>
    )
  } else if (!node.declared.exit) {
    body = (
      <CardNote>
        <NotDeclared capability="exit" node={node} open={watch.open} onToggle={watch.toggle} />
      </CardNote>
    )
  } else {
    body = (
      <CardNote>
        <p className="text-muted-foreground">{node.online ? d.declaredReady : d.offlineNote}</p>
      </CardNote>
    )
  }

  return (
    <CapabilityCard
      testId="capability-exit"
      icon={<DoorOpen />}
      title={d.exitTitle}
      tip={d.withdrawnNote}
      description={d.exitDesc}
      flash={watch.flash}
      action={enabled && !editing ? <EditButton onClick={openForm} /> : null}
      control={
        <Switch
          aria-label={d.exitSwitch(node.name)}
          checked={enabled || editing}
          disabled={!(enabled || node.declared.exit) || disable.isPending || save.pending}
          onCheckedChange={onSwitch}
        />
      }
    >
      {watch.flash ? <Arrived /> : null}
      {body}
    </CapabilityCard>
  )
}

/**
 * 端口转发的目标没有开关：声明过的节点可以被选作转发的目标。卡片顶上是声明的情况，下面是指向它的端口转发。
 */
export function ForwardTab({ network, node, onWaiting }: CapabilityTabProps) {
  const watch = useDeclareWatch(node.declared.forward, onWaiting)
  const nodes = useQuery(q.nodes(network.id))
  const forwards = useQuery(q.forwards(network.id))
  const all = nodes.data ?? []
  const mine = (forwards.data ?? []).filter((forward) => forward.target_node === node.id)
  const manage = networkPath(network.id, 'forwards')

  let body: ReactNode
  if (!node.declared.forward) {
    body = (
      <CardNote>
        <NotDeclared capability="forward" node={node} open={watch.open} onToggle={watch.toggle} />
      </CardNote>
    )
  } else if (forwards.isPending) {
    body = (
      <CardNote>
        <SkeletonBlock lines={2} />
      </CardNote>
    )
  } else {
    body = (
      <>
        <div className="flex min-h-11 items-center justify-between gap-4 px-5 py-2 text-[13px] text-muted-foreground">
          <span>{d.forwardCount(mine.length)}</span>
          <Link to={manage} className="rounded-sm text-primary underline-offset-4 hover:underline">
            {d.relatedManage}
          </Link>
        </div>
        {mine.length > 0 ? (
          <ul className="divide-y border-t">
            {mine.map((forward) => {
              const view = forwardStatusView(forward, all)
              return (
                <li
                  key={forward.id}
                  className="flex min-h-11 flex-wrap items-center gap-x-4 gap-y-1 px-5 py-2.5 text-sm"
                >
                  <span className="min-w-0 font-medium">{forwardLabel(forward)}</span>
                  <span className="flex min-w-0 flex-wrap items-center gap-1.5 text-[13px] text-muted-foreground">
                    <span>{nodeName(all, forward.listen_node)}</span>
                    <span className="tabular">{forward.bind}</span>
                    <ArrowRight className="size-3.5" aria-label={t.forwards.dialog.arrow} />
                    <span className="tabular">{forward.target_addr}</span>
                  </span>
                  <StatusText tone={view.tone} className="ml-auto text-[13px]">
                    {view.label}
                  </StatusText>
                </li>
              )
            })}
          </ul>
        ) : null}
      </>
    )
  }

  return (
    <CapabilityCard
      testId="capability-forward"
      icon={<ArrowLeftRight />}
      title={d.forwardTitle}
      description={d.forwardDesc}
      flash={watch.flash}
      control={
        <StatusText tone={node.declared.forward ? 'ok' : 'idle'} className="text-[13px]">
          {node.declared.forward ? d.forwardDeclared : d.forwardNotDeclared}
        </StatusText>
      }
    >
      {watch.flash ? <Arrived /> : null}
      {body}
    </CapabilityCard>
  )
}

/** 一个或几个地址，一行一个。 */
function AddressList({ addresses }: { addresses: readonly string[] }) {
  if (addresses.length === 0) return <>—</>
  return (
    <ul className="space-y-1">
      {addresses.map((address) => (
        <li key={address} className="tabular">
          {address}
        </li>
      ))}
    </ul>
  )
}

const bandwidthValue = (mbps: number | null): string =>
  mbps === null ? t.common.unlimited : `${mbps} ${t.common.mbps}`
