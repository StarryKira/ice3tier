import { useQuery } from '@tanstack/react-query'
import {
  CalendarDays,
  Cpu,
  Flag as FlagIcon,
  Globe,
  History,
  KeyRound,
  MapPin,
  MonitorSmartphone,
  Network as NetworkIcon,
  Pencil,
  RefreshCw,
} from 'lucide-react'
import { useId } from 'react'

import { q } from '@/api/queries'
import { type Network, type NodeDetail } from '@/api/types'
import { DateTimeValue, DetailList, DetailRow } from '@/components/detail-list'
import { Flag } from '@/components/flag'
import { GroupsLink } from '@/components/groups'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { OnlineDot } from '@/components/status'
import { Button } from '@/components/ui/button'
import { BoxCard } from '@/components/ui/card'
import { Tooltip } from '@/components/ui/tooltip'
import { t, useLocale } from '@/i18n'
import { platformLabel } from '@/lib/platform'
import { countryName } from '@/lib/region'

import { NodeGroupsEditor } from '../actions'
import { AdvancedCard } from './AdvancedCard'
import { VersionRow } from './NodeVersion'

const d = t.nodes.detail

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
        <VersionRow node={node} />
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
      {/* 在有 relay 的网络里这只是一条说明，不是告警：它不需要用户做任何事。 */}
      {node.mapping_varies === true ? (
        <div className="border-t px-5 py-4">
          <Notice tone={hasRelay ? 'relay' : 'warn'}>{hasRelay ? d.mappingVaries : d.mappingVariesNoRelay}</Notice>
        </div>
      ) : null}
    </BoxCard>
  )
}

/** 节点的分组：标题、一句它管什么、选分组的框。不套卡片，和左边的信息卡并排。 */
function GroupsSection({ network, node }: TabProps) {
  const titleId = useId()
  return (
    <section aria-labelledby={titleId}>
      <div className="flex items-center justify-between gap-4">
        <h2 id={titleId} className="text-[15px] leading-6 font-semibold">
          {d.groups}
        </h2>
        <GroupsLink network={network.id} />
      </div>
      <p className="mt-0.5 text-sm text-muted-foreground">{d.groupsDesc}</p>
      <div className="mt-3">
        <NodeGroupsEditor network={network} node={node} aria-label={d.groups} />
      </div>
    </section>
  )
}

interface OverviewTabProps extends TabProps {
  onEditIp: () => void
}

/** 概览：左边是只读的"它是什么"；右边是它在哪些分组里，和怎么选路、连谁。relay、出口、端口转发各有一页。 */
export function OverviewTab({ network, node, onEditIp }: OverviewTabProps) {
  return (
    <div className="grid grid-cols-1 items-start gap-x-8 gap-y-6 @4xl:grid-cols-2">
      <InfoCard network={network} node={node} onEditIp={onEditIp} />
      <div className="space-y-8">
        <GroupsSection network={network} node={node} />
        <AdvancedCard network={network} node={node} />
      </div>
    </div>
  )
}
