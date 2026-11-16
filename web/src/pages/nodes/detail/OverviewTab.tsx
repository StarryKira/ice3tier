import { useQuery } from '@tanstack/react-query'
import {
  Activity,
  ArrowDownUp,
  CalendarDays,
  ChevronDown,
  Cpu,
  Flag as FlagIcon,
  Globe,
  History,
  KeyRound,
  MapPin,
  MonitorSmartphone,
  Network as NetworkIcon,
  Pencil,
  RadioTower,
  RefreshCw,
  Waypoints,
  X,
} from 'lucide-react'
import { useCallback, useId, useRef } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type NodeDetail, type NodeId, type Reach, type RoutingMode, type UpdateNode } from '@/api/types'
import { Combobox } from '@/components/combobox'
import { CopyText } from '@/components/copy'
import { DateTimeValue, DetailList, DetailRow } from '@/components/detail-list'
import { Flag } from '@/components/flag'
import { GroupsLink } from '@/components/groups'
import { IceStack } from '@/components/layout/BrandMark'
import { NodeLink } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { OnlineDot } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { BoxCard, BoxCardBody, BoxCardHeader } from '@/components/ui/card'
import { NativeSelect } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t, useLocale } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { platformLabel } from '@/lib/platform'
import { countryName } from '@/lib/region'
import { MAX_PINNED_RELAYS } from '@/lib/product'

import { NodeGroupsEditor } from '../actions'
import { ExitRow, ForwardTargetRow, RelayRow } from './Capabilities'

const d = t.nodes.detail

const routingLabel = (mode: RoutingMode): string =>
  mode === 'direct_first' ? d.routingDirectFirst : d.routingLowestCost
const reachLabel = (reach: Reach): string =>
  reach === 'any' ? d.reachAny : reach === 'direct_only' ? d.reachDirectOnly : d.reachRelayOnly

/** "跟随网络"在下拉里的值。 */
const INHERIT = 'inherit'

interface TabProps {
  network: Network
  node: NodeDetail
}

/** controller 看到的地址（`1.2.3.4:20330`、`[2001:db8::1]:20330`）里的 IP。 */
function hostOf(endpoint: string): string {
  const bracket = /^\[(.+)\]:\d+$/.exec(endpoint)
  if (bracket !== null) return bracket[1] ?? endpoint
  const colon = endpoint.lastIndexOf(':')
  return colon > 0 ? endpoint.slice(0, colon) : endpoint
}

/**
 * 只读的信息卡：它是什么。一行一项，左边图标和名字，右边灰色的值，最右是改和复制。
 * 名字在页面的标题上，这里不再写一遍。
 */
function InfoCard({ network, node, onEditIp }: TabProps & { onEditIp: () => void }) {
  const locale = useLocale()
  const relays = useQuery(q.nodes(network.id))
  const hasRelay = (relays.data ?? []).some((other) => other.relay !== null)
  const synced = node.acked_version >= node.netmap_version
  const publicIp = node.observed_endpoint === null ? null : hostOf(node.observed_endpoint)
  const country = node.country ?? null

  return (
    <div className="space-y-5">
      <BoxCard>
        <h2 className="sr-only">{d.info}</h2>
        <DetailList>
          <DetailRow
            icon={<MapPin />}
            label={d.virtualIp}
            copy={node.virtual_ip}
            actions={
              <Tooltip content={d.editIp}>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="size-7 text-muted-foreground hover:text-foreground [&_svg]:size-3.5"
                  onClick={onEditIp}
                  aria-label={d.editIp}
                >
                  <Pencil aria-hidden />
                </Button>
              </Tooltip>
            }
          >
            <span className="tabular">{node.virtual_ip}</span>
          </DetailRow>
          <DetailRow icon={<NetworkIcon />} label={d.publicAddr} copy={publicIp ?? undefined}>
            {publicIp ?? '—'}
          </DetailRow>
          <DetailRow icon={<Globe />} label={d.fqdn} copy={node.fqdn}>
            {node.fqdn}
          </DetailRow>
          <DetailRow
            icon={<MonitorSmartphone />}
            label={
              <span className="inline-flex items-center gap-1">
                {d.hostname}
                <InfoTip>{d.hostnameHint}</InfoTip>
              </span>
            }
            copy={node.hostname === '' ? undefined : node.hostname}
          >
            {node.hostname === '' ? d.hostnameNone : node.hostname}
          </DetailRow>
          <DetailRow icon={<FlagIcon />} label={d.region}>
            {country === null ? (
              '—'
            ) : (
              <span className="inline-flex items-center gap-2">
                <Flag code={country} />
                {countryName(country, locale)}
              </span>
            )}
          </DetailRow>
          <DetailRow icon={<Cpu />} label={d.system}>
            {platformLabel(node.platform)}
          </DetailRow>
          <DetailRow icon={<KeyRound />} label={d.key} copy={node.key}>
            <span className="font-mono text-[13px]">{node.key}</span>
          </DetailRow>
          <DetailRow icon={<CalendarDays />} label={d.joined}>
            <DateTimeValue at={node.created_at} />
          </DetailRow>
          <DetailRow icon={<History />} label={d.lastSeen}>
            {node.online ? (
              <span className="inline-flex items-center gap-2">
                <OnlineDot online />
                {d.onlineNow}
              </span>
            ) : node.last_seen_at === null ? (
              <span className="inline-flex items-center gap-1">
                {t.nodes.never}
                <InfoTip>{t.nodes.neverTip}</InfoTip>
              </span>
            ) : (
              <DateTimeValue at={node.last_seen_at} />
            )}
          </DetailRow>
          <DetailRow icon={<IceStack className="size-4" />} label={d.version}>
            <span className="inline-flex items-center gap-2">
              {node.update.available !== null ? (
                <Link to="?tab=update" className="rounded-sm">
                  <Badge tone="warn">{t.updates.icon.available(node.update.available)}</Badge>
                </Link>
              ) : null}
              <span className="tabular">{node.version === '' ? '—' : node.version}</span>
            </span>
          </DetailRow>
          <DetailRow icon={<RefreshCw />} label={d.config}>
            {synced ? (
              d.configSynced(node.netmap_version)
            ) : (
              <span className="text-warn">
                {d.configPending(node.acked_version, node.netmap_version)} ·{' '}
                {node.online ? d.configPendingOnline : d.configPendingOffline}
              </span>
            )}
          </DetailRow>
        </DetailList>
      </BoxCard>

      <BoxCard>
        <BoxCardHeader title={d.live} />
        <DetailList>
          <DetailRow icon={<Waypoints />} label={d.endpoints} multiline={node.endpoints.length > 1}>
            {node.endpoints.length === 0 ? (
              d.endpointsNone
            ) : (
              <ul className="space-y-1">
                {node.endpoints.map((endpoint) => (
                  <li key={endpoint}>
                    <CopyText value={endpoint} mono={false} className="tabular [&>svg]:hidden" />
                  </li>
                ))}
              </ul>
            )}
          </DetailRow>
          <DetailRow
            icon={<RadioTower />}
            label={
              <span className="inline-flex items-center gap-1">
                {d.attached}
                <InfoTip>{d.attachedHint}</InfoTip>
              </span>
            }
            multiline={node.attached.length > 1}
          >
            {node.attached.length === 0 ? (
              d.attachedNone
            ) : (
              <ul className="space-y-1">
                {node.attached.map((attachment) => (
                  <li key={attachment.relay} className="flex items-center justify-end gap-2">
                    <NodeLink
                      network={network.id}
                      node={{ id: attachment.relay, name: nodeName(relays.data ?? [], attachment.relay) }}
                    />
                    <span className="text-[13px] tabular">{t.common.ms(attachment.rtt_ms)}</span>
                  </li>
                ))}
              </ul>
            )}
          </DetailRow>
          <DetailRow icon={<Activity />} label={d.flows}>
            <span className="tabular">{node.flows}</span>
          </DetailRow>
          <DetailRow icon={<ArrowDownUp />} label={d.traffic}>
            <Tooltip content={d.trafficHint}>
              <span tabIndex={0} className="rounded-sm tabular">
                {d.trafficValue(formatBytes(node.tx_bytes), formatBytes(node.rx_bytes))}
              </span>
            </Tooltip>
          </DetailRow>
        </DetailList>
        {/* 在有 relay 的网络里这只是一条说明，不是告警：它不需要用户做任何事。 */}
        {node.mapping_varies === true ? (
          <div className="border-t px-5 py-4">
            <Notice tone={hasRelay ? 'relay' : 'warn'}>{hasRelay ? d.mappingVaries : d.mappingVariesNoRelay}</Notice>
          </div>
        ) : null}
      </BoxCard>
    </div>
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
    <BoxCard>
      <details className="group">
        <summary className="flex min-h-12 cursor-pointer list-none items-center justify-between gap-4 rounded-xl px-5 py-2.5 [&::-webkit-details-marker]:hidden">
          <span className="text-[15px] leading-6 font-semibold">{d.advanced}</span>
          <ChevronDown className="size-4 shrink-0 text-faint transition-transform group-open:rotate-180" aria-hidden />
        </summary>
        <div className="space-y-5 border-t px-5 py-5">
          <div className="grid grid-cols-[minmax(0,1fr)_15rem] items-start gap-x-4 gap-y-1">
            <div className="flex h-9 items-center gap-1.5">
              <label htmlFor={routingId} className="text-sm font-medium">
                {d.routingLabel}
              </label>
              <InfoTip>{d.routingHint}</InfoTip>
            </div>
            <NativeSelect
              id={routingId}
              value={node.routing ?? INHERIT}
              disabled={save.isPending}
              onChange={(event) =>
                save.mutate({ routing: event.target.value === INHERIT ? null : (event.target.value as RoutingMode) })
              }
            >
              <option value={INHERIT}>{d.routingInherit(routingLabel(network.routing))}</option>
              <option value="direct_first">{d.routingDirectFirst}</option>
              <option value="lowest_cost">{d.routingLowestCost}</option>
            </NativeSelect>
          </div>

          <div className="grid grid-cols-[minmax(0,1fr)_15rem] items-start gap-x-4 gap-y-1">
            <div className="flex h-9 items-center gap-1.5">
              <label htmlFor={reachId} className="text-sm font-medium">
                {d.reachLabel}
              </label>
              <InfoTip>{d.reachHint}</InfoTip>
            </div>
            <NativeSelect
              id={reachId}
              value={node.reach ?? INHERIT}
              disabled={save.isPending}
              onChange={(event) =>
                save.mutate({ reach: event.target.value === INHERIT ? null : (event.target.value as Reach) })
              }
            >
              <option value={INHERIT}>{d.reachInherit(reachLabel(network.reach))}</option>
              <option value="any">{d.reachAny}</option>
              <option value="direct_only">{d.reachDirectOnly}</option>
              <option value="relay_only">{d.reachRelayOnly}</option>
            </NativeSelect>
          </div>

          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-1.5">
              <label htmlFor={dialId} className="text-sm font-medium">
                {d.dialOutOnly}
              </label>
              <InfoTip>{d.dialOutOnlyHint}</InfoTip>
            </div>
            <Switch
              id={dialId}
              checked={node.dial_out_only}
              disabled={save.isPending}
              onCheckedChange={(checked) => save.mutate({ dial_out_only: checked })}
            />
          </div>

          <div>
            <p className="flex items-center gap-1.5">
              <span id={pinnedId} className="text-sm font-medium">
                {d.pinnedLabel}
              </span>
              <InfoTip>{d.pinnedHint(MAX_PINNED_RELAYS)}</InfoTip>
            </p>
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
    </BoxCard>
  )
}

interface OverviewTabProps extends TabProps {
  onEditIp: () => void
  /** 能力卡片里有一行正在等节点上报声明。 */
  onWaiting: (waiting: boolean) => void
}

/** 概览：左边是只读的"它是什么"，右边是"我可以改什么"。两边不混在一起。 */
export function OverviewTab({ network, node, onEditIp, onWaiting }: OverviewTabProps) {
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
    // 两栏一样宽，每块都是同一种卡片：左边是它是什么（信息、连接），右边是能改的（分组、能力、高级）。
    <div className="grid grid-cols-1 items-start gap-5 @4xl:grid-cols-2">
      <InfoCard network={network} node={node} onEditIp={onEditIp} />
      <div className="space-y-5">
        <BoxCard>
          <BoxCardHeader title={d.groups} action={<GroupsLink network={network.id} />} />
          <BoxCardBody className="py-4">
            <NodeGroupsEditor network={network} node={node} aria-label={d.groups} />
          </BoxCardBody>
        </BoxCard>

        <BoxCard>
          <BoxCardHeader title={d.capabilities} tip={d.withdrawnNote} />
          <BoxCardBody className="divide-y">
            <RelayRow network={network} node={node} onWaiting={onRelay} />
            <ExitRow network={network} node={node} onWaiting={onExit} />
            <ForwardTargetRow network={network} node={node} onWaiting={onForward} />
          </BoxCardBody>
        </BoxCard>

        <AdvancedCard network={network} node={node} />
      </div>
    </div>
  )
}
