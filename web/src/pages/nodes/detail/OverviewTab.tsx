import { useQuery } from '@tanstack/react-query'
import { ChevronDown, Pencil, X } from 'lucide-react'
import { useCallback, useId, useRef } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type NodeDetail, type NodeId, type Reach, type RoutingMode, type UpdateNode } from '@/api/types'
import { Combobox } from '@/components/combobox'
import { CopyText } from '@/components/copy'
import { NodeLink } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { InfoRow } from '@/components/page'
import { OnlineDot, RelativeTime } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardBody, CardHeader } from '@/components/ui/card'
import { NativeSelect } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { MAX_PINNED_RELAYS } from '@/lib/product'

import { NodeTagsEditor } from '../actions'
import { ExitRow, ForwardTargetRow, RelayRow } from './Capabilities'

const d = t.nodes.detail

const ROUTING_LABEL: Record<RoutingMode, string> = {
  direct_first: d.routingDirectFirst,
  lowest_cost: d.routingLowestCost,
}
const REACH_LABEL: Record<Reach, string> = {
  any: d.reachAny,
  direct_only: d.reachDirectOnly,
  relay_only: d.reachRelayOnly,
}

/** "跟随网络"在下拉里的值。 */
const INHERIT = 'inherit'

interface TabProps {
  network: Network
  node: NodeDetail
}

/** 只读的信息卡：它是什么。能复制的行点一下就复制。 */
function InfoCard({ network, node, onEditIp }: TabProps & { onEditIp: () => void }) {
  const relays = useQuery(q.nodes(network.id))
  const hasRelay = (relays.data ?? []).some((other) => other.relay !== null)
  const synced = node.acked_version >= node.netmap_version

  return (
    <Card>
      <CardHeader title={d.info} />
      <CardBody>
        <dl>
          <InfoRow label={d.name}>
            <CopyText value={node.name} mono={false} />
          </InfoRow>
          <InfoRow label={d.hostname}>
            {node.hostname === '' ? (
              <span className="text-muted-foreground">{d.hostnameNone}</span>
            ) : (
              <>
                <CopyText value={node.hostname} mono={false} />
                <span className="mt-0.5 block text-[13px] text-muted-foreground">{d.hostnameHint}</span>
              </>
            )}
          </InfoRow>
          <InfoRow label={d.fqdn}>
            <CopyText value={node.fqdn} />
          </InfoRow>
          <InfoRow label={d.virtualIp}>
            <span className="inline-flex items-center gap-1.5">
              <CopyText value={node.virtual_ip} />
              <Tooltip content={d.editIp}>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="-my-1 size-6"
                  onClick={onEditIp}
                  aria-label={d.editIp}
                >
                  <Pencil className="size-3.5!" aria-hidden />
                </Button>
              </Tooltip>
            </span>
          </InfoRow>
          <InfoRow label={d.key}>
            <CopyText value={node.key} wrap />
          </InfoRow>
          <InfoRow label={d.platform}>{node.platform === '' ? '—' : node.platform}</InfoRow>
          <InfoRow label={d.version}>
            <span className="tabular">{node.version === '' ? '—' : node.version}</span>
            {node.update.available !== null ? (
              <Link to="?tab=update" className="ml-2 rounded-sm">
                <Badge tone="warn">{t.updates.icon.available(node.update.available)}</Badge>
              </Link>
            ) : null}
          </InfoRow>
          <InfoRow label={d.joined}>
            <RelativeTime at={node.created_at} />
          </InfoRow>
          <InfoRow label={d.lastSeen}>
            {node.online ? (
              <span className="inline-flex items-center gap-2">
                <OnlineDot online />
                {d.onlineNow}
              </span>
            ) : node.last_seen_at === null ? (
              <span className="text-muted-foreground">
                {t.nodes.never}
                <span className="mt-0.5 block text-[13px]">{t.nodes.neverTip}</span>
              </span>
            ) : (
              <RelativeTime at={node.last_seen_at} />
            )}
          </InfoRow>
          <InfoRow label={d.config}>
            {synced ? (
              <span>{d.configSynced(node.netmap_version)}</span>
            ) : (
              <span className="text-warn">
                {d.configPending(node.acked_version, node.netmap_version)}
                <span className="block text-[13px]">
                  {node.online ? d.configPendingOnline : d.configPendingOffline}
                </span>
              </span>
            )}
          </InfoRow>
        </dl>
      </CardBody>

      <CardHeader title={d.live} description={node.online ? undefined : d.liveOffline} className="mt-10" />
      <CardBody>
        <dl>
          <InfoRow label={d.endpoints}>
            {node.endpoints.length === 0 ? (
              <span className="text-muted-foreground">{d.endpointsNone}</span>
            ) : (
              <ul className="space-y-0.5">
                {node.endpoints.map((endpoint) => (
                  <li key={endpoint}>
                    <CopyText value={endpoint} />
                  </li>
                ))}
              </ul>
            )}
          </InfoRow>
          <InfoRow label={d.observed}>
            {node.observed_endpoint === null ? (
              <span className="text-muted-foreground">—</span>
            ) : (
              <CopyText value={node.observed_endpoint} />
            )}
          </InfoRow>
          <InfoRow
            label={
              <Tooltip content={d.attachedHint}>
                <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
                  {d.attached}
                </span>
              </Tooltip>
            }
          >
            {node.attached.length === 0 ? (
              <span className="text-muted-foreground">{d.attachedNone}</span>
            ) : (
              <ul className="space-y-0.5">
                {node.attached.map((attachment) => (
                  <li key={attachment.relay} className="flex items-center gap-2">
                    <NodeLink
                      network={network.id}
                      node={{ id: attachment.relay, name: nodeName(relays.data ?? [], attachment.relay) }}
                    />
                    <span className="text-[13px] text-muted-foreground tabular">{t.common.ms(attachment.rtt_ms)}</span>
                  </li>
                ))}
              </ul>
            )}
          </InfoRow>
          <InfoRow label={d.flows}>
            <span className="tabular">{node.flows}</span>
          </InfoRow>
          <InfoRow label={d.traffic}>
            <Tooltip content={d.trafficHint}>
              <span tabIndex={0} className="rounded-sm tabular">
                {d.trafficValue(formatBytes(node.tx_bytes), formatBytes(node.rx_bytes))}
              </span>
            </Tooltip>
          </InfoRow>
        </dl>
        {/* 在有 relay 的网络里这只是一条说明，不是告警：它不需要用户做任何事。 */}
        {node.mapping_varies === true ? (
          <Notice tone={hasRelay ? 'relay' : 'warn'} className="mt-3">
            {hasRelay ? d.mappingVaries : d.mappingVariesNoRelay}
          </Notice>
        ) : null}
      </CardBody>
    </Card>
  )
}

/** 高级设置。页面上的单个控件：改了立即生效。每一项旁边一句话，说的是后果。 */
function AdvancedCard({ network, node }: TabProps) {
  const nodes = useQuery(q.nodes(network.id))
  const routingId = useId()
  const reachId = useId()
  const dialId = useId()
  const pinnedId = useId()
  const save = useAction(network.id, {
    mutationFn: (body: UpdateNode) => api.updateNode(network.id, node.id, body),
    success: d.settingSaved,
  })

  const relays = (nodes.data ?? []).filter((other) => other.relay !== null && other.id !== node.id)
  const available = relays.filter((relay) => !node.pinned_relays.includes(relay.id))
  const setPinned = (pinned: NodeId[]) => save.mutate({ pinned_relays: pinned })

  return (
    <Card>
      <details className="group">
        <summary className="flex list-none items-center justify-between gap-4 rounded-md [&::-webkit-details-marker]:hidden">
          <span>
            <span className="block text-base font-semibold">{d.advanced}</span>
            <span className="mt-0.5 block text-[13px] text-muted-foreground">{d.advancedHint}</span>
          </span>
          <ChevronDown className="size-4 shrink-0 text-faint transition-transform group-open:rotate-180" aria-hidden />
        </summary>
        <div className="mt-5 space-y-6">
          <div className="grid grid-cols-[minmax(0,1fr)_13rem] items-start gap-x-4 gap-y-1">
            <div>
              <label htmlFor={routingId} className="text-sm font-medium">
                {d.routingLabel}
              </label>
              <p className="text-[13px] text-muted-foreground">{d.routingHint}</p>
            </div>
            <NativeSelect
              id={routingId}
              value={node.routing ?? INHERIT}
              disabled={save.isPending}
              onChange={(event) =>
                save.mutate({ routing: event.target.value === INHERIT ? null : (event.target.value as RoutingMode) })
              }
            >
              <option value={INHERIT}>{d.routingInherit(ROUTING_LABEL[network.routing])}</option>
              <option value="direct_first">{d.routingDirectFirst}</option>
              <option value="lowest_cost">{d.routingLowestCost}</option>
            </NativeSelect>
          </div>

          <div className="grid grid-cols-[minmax(0,1fr)_13rem] items-start gap-x-4 gap-y-1">
            <div>
              <label htmlFor={reachId} className="text-sm font-medium">
                {d.reachLabel}
              </label>
              <p className="text-[13px] text-muted-foreground">{d.reachHint}</p>
            </div>
            <NativeSelect
              id={reachId}
              value={node.reach ?? INHERIT}
              disabled={save.isPending}
              onChange={(event) =>
                save.mutate({ reach: event.target.value === INHERIT ? null : (event.target.value as Reach) })
              }
            >
              <option value={INHERIT}>{d.reachInherit(REACH_LABEL[network.reach])}</option>
              <option value="any">{d.reachAny}</option>
              <option value="direct_only">{d.reachDirectOnly}</option>
              <option value="relay_only">{d.reachRelayOnly}</option>
            </NativeSelect>
          </div>

          <div className="flex items-start justify-between gap-4">
            <div>
              <label htmlFor={dialId} className="text-sm font-medium">
                {d.dialOutOnly}
              </label>
              <p className="text-[13px] text-muted-foreground">{d.dialOutOnlyHint}</p>
            </div>
            <Switch
              id={dialId}
              checked={node.dial_out_only}
              disabled={save.isPending}
              onCheckedChange={(checked) => save.mutate({ dial_out_only: checked })}
            />
          </div>

          <div>
            <p id={pinnedId} className="text-sm font-medium">
              {d.pinnedLabel}
            </p>
            <p className="text-[13px] text-muted-foreground">{d.pinnedHint(MAX_PINNED_RELAYS)}</p>
            <ul aria-labelledby={pinnedId} className="mt-2 flex flex-wrap items-center gap-1.5">
              {node.pinned_relays.map((id) => {
                const relay = (nodes.data ?? []).find((other) => other.id === id)
                const name = nodeName(nodes.data ?? [], id)
                return (
                  <li key={id} className="inline-flex h-7 items-center gap-1 rounded-md bg-muted pr-1 pl-2.5 text-sm">
                    {name}
                    {relay?.relay == null ? <Badge tone="warn">{d.pinnedNotRelay}</Badge> : null}
                    <button
                      type="button"
                      disabled={save.isPending}
                      onClick={() => setPinned(node.pinned_relays.filter((other) => other !== id))}
                      aria-label={d.pinnedRemove(name)}
                      className="flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
                    >
                      <X className="size-3.5" aria-hidden />
                    </button>
                  </li>
                )
              })}
              {node.pinned_relays.length < MAX_PINNED_RELAYS ? (
                <li className="w-56">
                  <Combobox<NodeId>
                    value={null}
                    onChange={(id) => setPinned([...node.pinned_relays, id])}
                    options={available.map((relay) => ({
                      value: relay.id,
                      label: relay.name,
                      hint: relay.relay?.region === '' ? relay.virtual_ip : relay.relay?.region,
                      leading: <OnlineDot online={relay.online} />,
                    }))}
                    placeholder={d.pinnedAdd}
                    searchPlaceholder={t.select.nodeSearch}
                    emptyText={relays.length === 0 ? d.pinnedNone : t.select.nodeEmpty}
                    disabled={save.isPending}
                    aria-label={d.pinnedAdd}
                  />
                </li>
              ) : null}
            </ul>
          </div>
        </div>
      </details>
    </Card>
  )
}

interface OverviewTabProps extends TabProps {
  onEditIp: () => void
  /** 能力卡片里有一行正在等节点上报声明。 */
  onWaiting: (waiting: boolean) => void
}

/** 概览：左边是只读的"它是什么"，右边是"我可以改什么"。两边不混在一起。 */
export function OverviewTab({ network, node, onEditIp, onWaiting }: OverviewTabProps) {
  const tagsLabelId = useId()
  const tagsHintId = useId()
  // 三行各自报告是不是在等；有任何一行在等，页面就把轮询调勤。
  const waitingRows = useRef(new Set<string>())
  const report = useCallback(
    (key: string, value: boolean) => {
      if (value) waitingRows.current.add(key)
      else waitingRows.current.delete(key)
      onWaiting(waitingRows.current.size > 0)
    },
    [onWaiting],
  )
  const onRelay = useCallback((value: boolean) => report('relay', value), [report])
  const onExit = useCallback((value: boolean) => report('exit', value), [report])
  const onForward = useCallback((value: boolean) => report('forward', value), [report])

  return (
    <div className="grid grid-cols-1 items-start gap-x-14 gap-y-12 @4xl:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
      <InfoCard network={network} node={node} onEditIp={onEditIp} />
      <div className="space-y-12">
        <Card>
          <CardHeader
            title={<span id={tagsLabelId}>{d.tags}</span>}
            description={<span id={tagsHintId}>{t.nodes.tagsHint}</span>}
          />
          <CardBody>
            <NodeTagsEditor network={network} node={node} aria-label={d.tags} aria-describedby={tagsHintId} />
          </CardBody>
        </Card>

        <Card>
          <CardHeader title={d.capabilities} description={d.capabilitiesHint} />
          <div className="divide-y border-y">
            <RelayRow network={network} node={node} onWaiting={onRelay} />
            <ExitRow network={network} node={node} onWaiting={onExit} />
            <ForwardTargetRow network={network} node={node} onWaiting={onForward} />
          </div>
          <p className="pt-3 text-[13px] text-muted-foreground">
            {d.withdrawnNote(`${t.capability.relay.name} 和${t.capability.exit.name}`)}
          </p>
        </Card>

        <AdvancedCard network={network} node={node} />
      </div>
    </div>
  )
}
