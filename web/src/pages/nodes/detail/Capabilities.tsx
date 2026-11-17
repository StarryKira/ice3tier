import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, ArrowRight, DoorOpen, Pencil, RadioTower } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeDetail, type NodeId, type Reachability } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock } from '@/components/copy'
import { DeclareHowTo } from '@/components/declare'
import { FormDialog } from '@/components/form-dialog'
import { InfoTip } from '@/components/info-tip'
import { NodeLink } from '@/components/node'
import { Notice } from '@/components/notice'
import { SkeletonBlock } from '@/components/states'
import { StatusText } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { BoxCard } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import { t } from '@/i18n'
import { exitUsage } from '@/lib/commands'
import { formatBytes } from '@/lib/format'
import { attachedCount, nodeName } from '@/lib/nodes'
import { type Capability } from '@/lib/product'
import { classLabel } from '@/lib/serviceClass'
import { cn } from '@/lib/utils'
import { forwardLabel, forwardStatusView } from '@/pages/forwards/status'
import { networkPath } from '@/pages/networks/context'

import { useSaveExit } from './exitActions'
import { ExitFormFields } from './exitForm'
import { exitFormFrom, type ExitFormState } from './exitFormState'
import { RelayDialog } from './RelayDialog'

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

/** 一项设置：上面一行灰色的小字是名字，下面是值。几项排成一行，宽的时候四列。 */
function Stat({
  label,
  tip,
  wide = false,
  children,
}: {
  label: string
  tip?: string
  wide?: boolean
  children: ReactNode
}) {
  return (
    <div className={cn('min-w-0', wide && 'col-span-2')}>
      <dt className="flex items-center gap-1 text-[13px] text-muted-foreground">
        {label}
        {tip !== undefined ? <InfoTip>{tip}</InfoTip> : null}
      </dt>
      <dd className="mt-1.5 text-sm font-medium">{children}</dd>
    </div>
  )
}

function StatGrid({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-2 gap-x-8 gap-y-5 px-5 py-5 @3xl:grid-cols-4">{children}</dl>
}

/** 中继的地址：UDP 和 TCP 一样时每个地址写一次，后面注明两种都用；不一样时分开写。 */
function RelayAddresses({ udp, tcp }: { udp: readonly string[]; tcp: readonly string[] }) {
  const same = udp.length === tcp.length && udp.every((address, index) => address === tcp[index])
  const lines = same
    ? udp.map((address) => ({ address, protocols: 'UDP · TCP' }))
    : [
        ...udp.map((address) => ({ address, protocols: 'UDP' })),
        ...tcp.map((address) => ({ address, protocols: 'TCP' })),
      ]
  if (lines.length === 0) return <>—</>
  return (
    <ul className="space-y-1">
      {lines.map((line) => (
        <li key={`${line.protocols}-${line.address}`} className="flex flex-wrap items-baseline gap-x-2">
          <span className="tabular">{line.address}</span>
          <span className="text-xs font-normal text-muted-foreground">{line.protocols}</span>
        </li>
      ))}
    </ul>
  )
}

/**
 * 连着这个中继的节点：名字、虚拟 IP、延迟，按延迟排。指定用它、但现在没连着的也列出来，排在后面。
 */
function ConnectedNodes({ network, relay, nodes }: { network: Network; relay: NodeId; nodes: readonly Node[] }) {
  const rows = nodes
    .filter((node) => node.attached.some((entry) => entry.relay === relay) || node.pinned_relays.includes(relay))
    .map((node) => ({
      node,
      rtt: node.attached.find((entry) => entry.relay === relay)?.rtt_ms ?? null,
      pinned: node.pinned_relays.includes(relay),
    }))
    .sort((a, b) => (a.rtt ?? Infinity) - (b.rtt ?? Infinity))
  const connected = rows.filter((row) => row.rtt !== null).length

  return (
    <section className="border-t">
      <h3 className="flex min-h-11 items-center gap-1.5 px-5 py-2.5 text-sm font-medium">
        {d.relayConnectedTitle}
        <span data-testid="relay-attached" className="font-normal text-muted-foreground tabular">
          {connected}
        </span>
        <InfoTip>{d.relayAttachedTip}</InfoTip>
      </h3>
      {rows.length === 0 ? (
        <p className="border-t px-5 py-4 text-[13px] text-muted-foreground">{d.relayConnectedNone}</p>
      ) : (
        <ul className="divide-y border-t">
          {rows.map(({ node, rtt, pinned }) => (
            <li key={node.id} className="flex min-h-11 items-center gap-3 px-5 py-2 text-sm">
              <NodeLink network={network.id} node={node} online={node.online} />
              <span className="text-[13px] text-muted-foreground tabular">{node.virtual_ip}</span>
              {pinned ? <Badge tone="outline">{d.relayPinnedTag}</Badge> : null}
              <span className="ml-auto text-[13px] text-muted-foreground tabular">
                {rtt === null ? d.relayNotConnected : t.common.ms(rtt)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

export function RelayTab({ network, node, onWaiting }: CapabilityTabProps) {
  const confirm = useConfirm()
  const nodes = useQuery(q.nodes(network.id))
  const watch = useDeclareWatch(node.declared.relay, onWaiting)
  const [editing, setEditing] = useState(false)
  const [reachability, setReachability] = useState<Reachability[] | null>(null)
  const enabled = node.relay !== null
  const attached = attachedCount(nodes.data ?? [], node.id)
  const disable = useAction(network.id, {
    mutationFn: () => api.disableRelay(network.id, node.id),
    success: d.relayDisabledToast(node.name),
  })

  const openForm = () => {
    setReachability(null)
    setEditing(true)
  }

  const turnOff = async () => {
    const relays = (nodes.data ?? []).filter((other) => other.relay !== null)
    const pinnedBy = (nodes.data ?? []).filter((other) => other.pinned_relays.includes(node.id)).length
    const only = relays.length === 1
    // 唯一的中继关掉之后，打不通直连的节点对就互不可达了：这是危险级别。
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
      setReachability(null)
      disable.mutate()
    }
  }

  const onSwitch = (checked: boolean) => {
    if (checked) openForm()
    else if (enabled) void turnOff()
  }

  const relay = node.relay
  let body: ReactNode
  if (relay !== null) {
    body = (
      <>
        <StatGrid>
          <Stat label={d.statAddress} wide>
            <RelayAddresses udp={relay.udp} tcp={relay.tcp} />
          </Stat>
          <Stat label={d.rowRegion}>{relay.region === '' ? '—' : relay.region}</Stat>
          <Stat label={d.rowBandwidth}>{bandwidthValue(relay.bandwidth_mbps)}</Stat>
        </StatGrid>
        <ConnectedNodes network={network} relay={node.id} nodes={nodes.data ?? []} />
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
      action={enabled ? <EditButton onClick={openForm} /> : null}
      control={
        <Switch
          aria-label={d.relaySwitch(node.name)}
          checked={enabled || editing}
          disabled={!(enabled || node.declared.relay) || disable.isPending}
          onCheckedChange={onSwitch}
        />
      }
    >
      {watch.flash ? <Arrived /> : null}
      {/* 保存之后马上就有：不等页面重新取到节点。 */}
      {reachability !== null ? (
        <ReachabilityNotice results={reachability} onDismiss={() => setReachability(null)} />
      ) : null}
      {body}
      {editing ? (
        <RelayDialog
          network={network}
          node={node}
          onClose={() => setEditing(false)}
          onSaved={(saved) => setReachability(saved.reachability.length > 0 ? saved.reachability : null)}
        />
      ) : null}
    </CapabilityCard>
  )
}

interface ExitDialogProps {
  network: Network
  node: NodeDetail
  onClose: () => void
}

/** 启用出口，或者改出口的设置。和出口页上启用出口用的是同一组字段。 */
function ExitDialog({ network, node, onClose }: ExitDialogProps) {
  const enabled = node.exit !== null
  const [initial] = useState<ExitFormState>(() => exitFormFrom(node.exit))
  const [form, setForm] = useState(initial)
  const [submitted, setSubmitted] = useState(false)
  const save = useSaveExit(network, node)
  return (
    <FormDialog
      open
      onClose={onClose}
      title={enabled ? d.exitEditTitle(node.name) : d.exitSwitch(node.name)}
      dirty={JSON.stringify(form) !== JSON.stringify(initial)}
      submitLabel={enabled ? d.save : d.exitEnable}
      pending={save.pending}
      error={save.error}
      errorDetail={save.errorDetail}
      onSubmit={() => {
        setSubmitted(true)
        void save.submit(form).then((ok) => {
          if (ok) onClose()
        })
      }}
    >
      <ExitFormFields state={form} onChange={setForm} showProblems={submitted} />
    </FormDialog>
  )
}

export function ExitTab({ network, node, onWaiting }: CapabilityTabProps) {
  const confirm = useConfirm()
  const watch = useDeclareWatch(node.declared.exit, onWaiting)
  const [editing, setEditing] = useState(false)
  const enabled = node.exit !== null
  const disable = useAction(network.id, {
    mutationFn: () => api.disableExit(network.id, node.id),
    success: d.exitDisabledToast(node.name),
  })

  const turnOff = async () => {
    const confirmed = await confirm({
      title: d.exitOffTitle(node.name),
      body: d.exitOffBody,
      confirmLabel: d.exitOffConfirm,
    })
    if (confirmed) disable.mutate()
  }

  const onSwitch = (checked: boolean) => {
    if (checked) setEditing(true)
    else if (enabled) void turnOff()
  }

  const exit = node.exit
  let body: ReactNode
  if (exit !== null) {
    body = (
      <>
        <StatGrid>
          <Stat label={d.rowPorts}>
            <span className="tabular">{exit.ports === null ? t.common.unlimited : exit.ports.join(', ')}</span>
          </Stat>
          <Stat label={d.rowLan}>{exit.lan ? d.rowLanOn : d.rowLanOff}</Stat>
          <Stat label={d.rowBandwidth}>{bandwidthValue(exit.bandwidth_mbps)}</Stat>
          {exit.quota_bytes !== null ? (
            <Stat label={d.rowQuota}>
              <span className="tabular">{d.rowQuotaValue(formatBytes(exit.quota_bytes), exit.quota_reset_day)}</span>
            </Stat>
          ) : null}
          {exit.family !== 'any' ? (
            <Stat label={d.rowFamily}>{exit.family === 'ipv4' ? d.exitFamilyIpv4 : d.exitFamilyIpv6}</Stat>
          ) : null}
          {exit.connects_per_minute !== null ? (
            <Stat label={d.rowConnects}>
              <span className="tabular">{d.rowConnectsValue(exit.connects_per_minute)}</span>
            </Stat>
          ) : null}
          {exit.class !== null ? <Stat label={d.rowClass}>{classLabel(exit.class)}</Stat> : null}
          {exit.upstream_socks5 !== null ? (
            <Stat label={d.rowUpstream}>
              <span className="tabular">{exit.upstream_socks5}</span>
            </Stat>
          ) : null}
        </StatGrid>
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
      action={enabled ? <EditButton onClick={() => setEditing(true)} /> : null}
      control={
        <Switch
          aria-label={d.exitSwitch(node.name)}
          checked={enabled || editing}
          disabled={!(enabled || node.declared.exit) || disable.isPending}
          onCheckedChange={onSwitch}
        />
      }
    >
      {watch.flash ? <Arrived /> : null}
      {body}
      {editing ? <ExitDialog network={network} node={node} onClose={() => setEditing(false)} /> : null}
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

const bandwidthValue = (mbps: number | null): string =>
  mbps === null ? t.common.unlimited : `${mbps} ${t.common.mbps}`
