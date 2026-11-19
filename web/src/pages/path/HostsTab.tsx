import { ArrowUpRight } from 'lucide-react'
import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { type HostReport, type NodeDetail, type NodeRoute } from '@/api/types'
import { Flag } from '@/components/flag'
import { OsIcon } from '@/components/os-icon'
import { OnlineDot } from '@/components/status'
import { BoxCard } from '@/components/ui/card'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { nodePath } from '@/lib/nodes'
import { msText, permilleText, problemText } from '@/lib/tests'
import { formatAge, formatDuration } from '@/lib/time'
import { cn } from '@/lib/utils'
import { routeName } from '@/topology/diagnosis'

import { type Named } from './parts'

const d = t.diagnosis
const h = d.host

/** 一行：名字在左（灰），值在右。值是 `null` 时不画这一行。 */
function Row({ label, children, mono = false }: { label: string; children: ReactNode; mono?: boolean }) {
  if (children === null || children === undefined || children === '') return null
  return (
    <div className="grid grid-cols-[8.5rem_minmax(0,1fr)] gap-3 px-5 py-1.5 text-[13px]">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={cn('min-w-0 break-words', mono && 'font-mono text-xs leading-5')}>{children}</dd>
    </div>
  )
}

/** 一组：小标题，右边写数据从哪里来（节点的上报、诊断时）。 */
function Group({ title, source, children }: { title: string; source?: string; children: ReactNode }) {
  return (
    <section className="border-t py-2 first:border-t-0">
      <h3 className="flex items-center justify-between px-5 pt-1.5 pb-1 text-xs font-medium text-muted-foreground">
        <span className="text-foreground">{title}</span>
        {source !== undefined ? <span className="font-normal">{source}</span> : null}
      </h3>
      <dl>{children}</dl>
    </section>
  )
}

/** 诊断里这一项还没有、做不了时的一句话。 */
function Pending({ host, tested, nodes }: { host: HostReport | undefined; tested: boolean; nodes: Named }) {
  const text =
    host === undefined
      ? tested
        ? d.hostNotReported
        : d.hostNoTest
      : host.problem?.code === 'not_supported'
        ? d.hostTooOld
        : host.state === 'pending'
          ? t.tests.pending
          : host.problem !== null
            ? problemText(host.problem, nodes)
            : t.tests.failed
  return <p className="px-5 py-1.5 text-[13px] text-muted-foreground">{text}</p>
}

const pct = (value: number | null | undefined) => (value === null || value === undefined ? '—' : `${value}%`)
const count = (value: number | null | undefined) => (value === null || value === undefined ? null : String(value))

interface HostCardProps {
  network: number
  label: string
  node: NodeDetail | undefined
  /** 节点列表里的；详情还没取到时先用它。 */
  fallbackName: string
  host: HostReport | undefined
  /** 它的路由表里到对端的那一条。 */
  toPeer: NodeRoute | undefined
  /** 这一对诊断过（有 `host` 时才有主机数据）。 */
  tested: boolean
  nodes: Named
}

function HostCard({ network, label, node, fallbackName, host, toPeer, tested, nodes }: HostCardProps) {
  const done = host?.state === 'done'
  const system = done ? host.system : null
  const peer = done ? host.to_peer : null
  const connections = done ? host.connections : null
  const name = (id: number) => nodes.find((other) => other.id === id)?.name ?? t.common.nodeMissing(id)
  const loss =
    peer?.sent_packets !== null &&
    peer?.sent_packets !== undefined &&
    peer.lost_packets !== null &&
    peer.sent_packets > 0
      ? permilleText(Math.round((peer.lost_packets / peer.sent_packets) * 1000))
      : '—'
  return (
    <BoxCard data-testid={`host-${label.toLowerCase()}`}>
      <header className="flex items-center gap-3 border-b px-5 py-3">
        <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
          {node?.country !== null && node?.country !== undefined ? (
            <Flag code={node.country} className="size-5" />
          ) : (
            <OsIcon
              platform={node?.platform ?? ''}
              className="size-4"
              fallback={<span className="text-[13px] font-semibold">{label}</span>}
            />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <p className="flex items-center gap-2">
            <span className="grid size-5 place-items-center rounded-full bg-primary text-[11px] font-semibold text-primary-foreground">
              {label}
            </span>
            <span className="truncate text-[15px] font-semibold">{node?.name ?? fallbackName}</span>
            {node?.relay !== null && node?.relay !== undefined ? (
              <span className="rounded-full bg-relay-soft px-1.5 text-[11px] leading-4 font-medium text-relay">
                {d.relay}
              </span>
            ) : null}
          </p>
          {node !== undefined ? (
            <p className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
              <OnlineDot online={node.online} />
              {node.online ? d.online : d.offline}
              <span aria-hidden>·</span>
              <span className="font-mono">{node.virtual_ip}</span>
            </p>
          ) : null}
        </div>
        {node !== undefined ? (
          <Link
            to={nodePath(network, node.id)}
            className="inline-flex shrink-0 items-center gap-1 rounded-sm text-xs text-muted-foreground hover:text-foreground"
          >
            {d.toNode}
            <ArrowUpRight className="size-3.5" aria-hidden />
          </Link>
        ) : null}
      </header>

      <Group title={d.hostSection.node} source={d.hostLive}>
        <Row label={h.version}>{node?.version}</Row>
        <Row label={h.platform}>{node?.platform}</Row>
        <Row label={h.hostname}>{node?.hostname}</Row>
        <Row label={h.reported}>
          {node?.reported_secs_ago === null || node?.reported_secs_ago === undefined
            ? null
            : formatAge(node.reported_secs_ago)}
        </Row>
        <Row label={h.profile}>
          {node?.performance === null || node?.performance === undefined
            ? null
            : `${node.performance.profile} · ${t.diagnosis.host.threads} ${node.performance.threads}`}
        </Row>
      </Group>

      <Group title={d.hostSection.system} source={d.hostTest}>
        {done ? (
          <>
            <Row label={h.os}>{system?.os}</Row>
            <Row label={h.kernel}>{system?.kernel}</Row>
            <Row label={h.cpuModel}>
              {system === null
                ? null
                : [system.cpu_model, system.cores === null ? null : h.cores(system.cores), system.arch]
                    .filter((part) => part !== null)
                    .join(' · ')}
            </Row>
            <Row label={h.uptime}>
              {system?.uptime_secs === null || system?.uptime_secs === undefined
                ? null
                : formatDuration(system.uptime_secs)}
            </Row>
          </>
        ) : (
          <Pending host={host} tested={tested} nodes={nodes} />
        )}
      </Group>

      <Group title={d.hostSection.resources} source={d.hostTest}>
        {done ? (
          <>
            <Row label={h.cpu}>
              {host.cpu === null ? null : h.cpuValue(pct(host.cpu.system_percent), pct(host.cpu.process_percent))}
            </Row>
            <Row label={h.cpuPeak}>
              {host.cpu_peak === null ? null : (
                <span
                  className={cn(
                    host.cpu_peak.process_percent !== null &&
                      host.cpu_peak.process_percent >= 85 &&
                      'font-medium text-warn',
                  )}
                >
                  {h.cpuValue(pct(host.cpu_peak.system_percent), pct(host.cpu_peak.process_percent))}
                </span>
              )}
            </Row>
            <Row label={h.load}>
              {host.load_x100 === null ? null : host.load_x100.map((value) => (value / 100).toFixed(2)).join(' · ')}
            </Row>
            <Row label={h.memory}>
              {host.memory === null
                ? null
                : h.memoryValue(formatBytes(host.memory.available_bytes), formatBytes(host.memory.total_bytes))}
            </Row>
            <Row label={h.rss}>
              {host.memory?.process_rss_bytes === null || host.memory?.process_rss_bytes === undefined
                ? null
                : formatBytes(host.memory.process_rss_bytes)}
            </Row>
            <Row label={h.processUptime}>
              {host.process?.uptime_secs === null || host.process?.uptime_secs === undefined
                ? null
                : formatDuration(host.process.uptime_secs)}
            </Row>
            <Row label={h.threads}>{count(host.process?.threads)}</Row>
          </>
        ) : (
          <Pending host={host} tested={tested} nodes={nodes} />
        )}
      </Group>

      <Group title={d.hostSection.connections} source={done ? d.hostTest : d.hostLive}>
        <Row label={h.peers}>{count(connections?.peers)}</Row>
        <Row label={h.relays}>{count(connections?.relays)}</Row>
        <Row label={h.relayClients}>{count(connections?.relay_clients)}</Row>
        <Row label={h.streams}>{count(connections?.streams ?? node?.flows)}</Row>
        <Row label={h.lanes}>
          {connections?.lanes_in_use !== null &&
          connections?.lanes_in_use !== undefined &&
          connections.lanes_idle !== null
            ? h.lanesValue(connections.lanes_in_use, connections.lanes_idle)
            : node?.lanes === null || node?.lanes === undefined
              ? null
              : h.lanesValue(
                  node.lanes.total.in_use,
                  node.lanes.peers.reduce((total, lane) => total + lane.idle + lane.ready, 0),
                )}
        </Row>
        <Row label={h.traffic}>
          {node === undefined ? null : h.trafficValue(formatBytes(node.tx_bytes), formatBytes(node.rx_bytes))}
        </Row>
      </Group>

      <Group title={d.hostSection.peer} source={peer !== null ? d.hostTest : d.hostLive}>
        <Row label={h.route}>
          {peer?.route !== null && peer?.route !== undefined
            ? routeName(peer.route, nodes)
            : toPeer === undefined
              ? null
              : routeName(toPeer.route, nodes)}
        </Row>
        <Row label={h.rtt}>
          {peer?.rtt_us !== null && peer?.rtt_us !== undefined
            ? msText(peer.rtt_us)
            : toPeer?.rtt_ms === null || toPeer?.rtt_ms === undefined
              ? null
              : t.common.ms(toPeer.rtt_ms)}
        </Row>
        {peer !== null ? (
          <>
            <Row label={h.cwnd}>{peer.cwnd_bytes === null ? null : formatBytes(peer.cwnd_bytes)}</Row>
            <Row label={h.mtu}>{count(peer.mtu)}</Row>
            <Row label={h.congestion}>{peer.congestion?.toUpperCase()}</Row>
            <Row label={h.packets}>
              {peer.sent_packets === null || peer.lost_packets === null
                ? null
                : h.packetsValue(String(peer.sent_packets), String(peer.lost_packets), loss)}
            </Row>
            <Row label={h.congestionEvents}>{count(peer.congestion_events)}</Row>
            <Row label={h.bytesDuring}>
              {peer.bytes_sent === null || peer.bytes_received === null
                ? null
                : h.trafficValue(formatBytes(peer.bytes_sent), formatBytes(peer.bytes_received))}
            </Row>
            <Row label={h.connectionAge}>{peer.age_secs === null ? null : formatDuration(peer.age_secs)}</Row>
          </>
        ) : null}
        <Row label={h.since}>{toPeer === undefined ? null : formatDuration(toPeer.since_secs)}</Row>
        <Row label={h.peerFlows}>{count(toPeer?.flows)}</Row>
        <Row label={h.peerTraffic}>
          {toPeer?.tx_bytes === null ||
          toPeer?.tx_bytes === undefined ||
          toPeer.rx_bytes === null ||
          toPeer.rx_bytes === undefined
            ? null
            : h.trafficValue(formatBytes(toPeer.tx_bytes), formatBytes(toPeer.rx_bytes))}
        </Row>
      </Group>

      <Group title={d.hostSection.network} source={d.hostLive}>
        <Row label={h.egress}>
          {node?.egress_ip === null || node?.egress_ip === undefined ? null : (
            <span className="inline-flex items-center gap-1.5">
              <Flag code={node.country} />
              <span className="font-mono text-xs">{node.egress_ip}</span>
            </span>
          )}
        </Row>
        <Row label={h.observed} mono>
          {node?.observed_endpoint}
        </Row>
        <Row label={h.endpoints} mono>
          {node === undefined || node.endpoints.length === 0 ? null : (
            <ul>
              {node.endpoints.map((endpoint) => (
                <li key={endpoint}>{endpoint}</li>
              ))}
            </ul>
          )}
        </Row>
        <Row label={h.nat}>
          {node?.mapping_varies === null || node?.mapping_varies === undefined ? null : node.mapping_varies ? (
            <span className="text-warn">{h.natVaries}</span>
          ) : (
            h.natStable
          )}
        </Row>
        <Row label={h.udp}>
          {node === undefined ? null : node.no_udp === true ? <span className="text-warn">{h.udpNo}</span> : h.udpOk}
        </Row>
        <Row label={h.attached}>
          {node === undefined
            ? null
            : node.attached.length === 0
              ? h.none
              : node.attached.map((relay) => `${name(relay.relay)} ${t.common.ms(relay.rtt_ms)}`).join(' · ')}
        </Row>
      </Group>
    </BoxCard>
  )
}

interface HostsTabProps {
  network: number
  a: {
    id: number
    name: string
    detail: NodeDetail | undefined
    host: HostReport | undefined
    toPeer: NodeRoute | undefined
  }
  b: {
    id: number
    name: string
    detail: NodeDetail | undefined
    host: HostReport | undefined
    toPeer: NodeRoute | undefined
  }
  tested: boolean
  nodes: Named
}

/** 两端主机并排：节点、系统、资源、连接、到对端的连接、网络。每组写着数据是节点的上报还是诊断时量的。 */
export function HostsTab({ network, a, b, tested, nodes }: HostsTabProps) {
  return (
    <div className="grid grid-cols-1 items-start gap-5 @3xl:grid-cols-2">
      {[
        { side: a, label: d.nodeA },
        { side: b, label: d.nodeB },
      ].map(({ side, label }) => (
        <HostCard
          key={label}
          network={network}
          label={label}
          node={side.detail}
          fallbackName={side.name}
          host={side.host}
          toPeer={side.toPeer}
          tested={tested}
          nodes={nodes}
        />
      ))}
    </div>
  )
}
