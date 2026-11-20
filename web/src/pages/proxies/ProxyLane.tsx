import { useQuery } from '@tanstack/react-query'
import { ChevronDown } from 'lucide-react'
import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type LaneSide, type Network, type Node, type NodeId, type Proxy } from '@/api/types'
import { StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import {
  handshakeParts,
  handshakeTotal,
  type LaneFix,
  laneEffectView,
  lanesReported,
  peerGlance,
  proxyLane,
} from '@/lib/lanes'
import { nodeName, nodePath } from '@/lib/nodes'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

const l = t.proxies.lane

/** 带解释的名字：虚线下划线，悬停或聚焦时显示。 */
export function TipLabel({ label, tip }: { label: string; tip: string }) {
  return (
    <Tooltip content={tip}>
      <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
        {label}
      </span>
    </Tooltip>
  )
}

/** 修好一个原因的那一步，去它该去的页面。 */
function fixPath(network: number, fix: LaneFix): string {
  switch (fix.kind) {
    case 'enable_relay':
    case 'tcp_address':
      return `${nodePath(network, fix.node)}?tab=relay`
    case 'update':
      return nodePath(network, fix.node)
    case 'access':
      return `${networkPath(network, 'access')}?check=1&from=${fix.from}&to=${fix.to}`
  }
}

/**
 * 代理列表里 ice3proxy 那一格：没打开是"关"；打开了写起不起作用（起作用时下面是谁拨号，不起作用时一条一条原因和
 * 修好它的那一步）；两端报了计数时有"详情"，展开看两端的数。
 */
export function LaneCell({
  network,
  proxy,
  nodes,
  expanded,
  onToggle,
}: {
  network: Network
  proxy: Proxy
  nodes: readonly Node[]
  expanded: boolean
  onToggle: () => void
}) {
  const lane = proxyLane(proxy)
  if (!lane.on) return <span className="text-[13px] text-muted-foreground">{l.off}</span>
  const view =
    proxy.exit === null
      ? ({ kind: 'off' } as const)
      : laneEffectView(lane.effect, { listen: proxy.node, exit: proxy.exit }, lane.sides, nodes)
  const name = nodeName(nodes, proxy.node)
  const exit = proxy.exit === null ? '' : nodeName(nodes, proxy.exit)
  return (
    <div className="text-[13px]" data-testid={`proxy-lane-${proxy.name}`}>
      {view.kind === 'off' ? (
        <span>{l.on}</span>
      ) : (
        <StatusText tone={view.kind === 'blocked' ? 'warn' : 'ok'} className="whitespace-nowrap">
          {view.label}
        </StatusText>
      )}
      {view.kind === 'effective' && view.detail !== null ? (
        <div className="mt-0.5 text-xs text-muted-foreground">{view.detail}</div>
      ) : null}
      {view.kind === 'blocked' ? (
        <ul className="mt-0.5 max-w-72 space-y-0.5 text-xs leading-snug">
          {view.reasons.map((reason) => (
            <li key={reason.text}>
              <span className="text-muted-foreground">{reason.text}</span>
              {reason.fix !== null && reason.fixLabel !== null ? (
                <Link to={fixPath(network.id, reason.fix)} className="ml-2 rounded-sm text-primary hover:underline">
                  {reason.fixLabel}
                </Link>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      {lanesReported(proxy) ? (
        <Button
          variant="ghost"
          size="sm"
          className="-ml-2 h-6 px-2 text-xs text-muted-foreground"
          aria-expanded={expanded}
          aria-label={expanded ? l.detailsHide : l.detailsFor(name, exit)}
          onClick={onToggle}
        >
          {expanded ? l.detailsHide : l.details}
          <ChevronDown className={cn('size-3.5 transition-transform', expanded && 'rotate-180')} aria-hidden />
        </Button>
      ) : null}
    </div>
  )
}

/**
 * 代理两端（监听的节点、出口）各自报的、和对方之间的 ice3proxy，并排放着。计数属于这两个节点，
 * 它们之间选了 ice3proxy 的代理共用。握手失败分不到对端名下，是两个节点各自全节点的数，所以要取节点详情。
 */
export function LaneDetails({ network, proxy, nodes }: { network: Network; proxy: Proxy; nodes: readonly Node[] }) {
  const exitId = proxy.exit ?? 0
  const listen = useQuery(q.node(network.id, proxy.node))
  const exit = useQuery({ ...q.node(network.id, exitId), enabled: proxy.exit !== null })
  const reports = proxyLane(proxy).sides
  const sides: { id: NodeId; name: string; report: LaneSide | undefined; detail: typeof listen }[] = [
    {
      id: proxy.node,
      name: nodeName(nodes, proxy.node),
      report: reports.find((side) => side.node === proxy.node),
      detail: listen,
    },
    { id: exitId, name: nodeName(nodes, exitId), report: reports.find((side) => side.node === exitId), detail: exit },
  ]
  const dash = <span className="text-muted-foreground">{l.notReported}</span>
  const glance = (side: (typeof sides)[number]) => (side.report === undefined ? null : peerGlance(side.report.lanes))
  const rows: { label: ReactNode; value: (side: (typeof sides)[number]) => ReactNode }[] = [
    { label: l.colInUse, value: (side) => glance(side)?.inUse ?? dash },
    { label: l.colIdle, value: (side) => glance(side)?.idle ?? dash },
    {
      label: <TipLabel label={l.colFallbacks} tip={l.fallbacksTip} />,
      value: (side) => glance(side)?.fallbacks ?? dash,
    },
    { label: l.colDialFailures, value: (side) => glance(side)?.dialFailures ?? dash },
    {
      label: <TipLabel label={l.row.udpPath} tip={l.row.udpPathTip} />,
      value: ({ report }) =>
        report?.lanes.udp_path == null ? dash : report.lanes.udp_path ? l.row.udpPathYes : l.row.udpPathNo,
    },
    {
      label: <TipLabel label={l.row.control} tip={l.row.controlTip} />,
      value: ({ report }) => (report === undefined ? dash : report.lanes.control ? l.row.controlUp : l.row.controlDown),
    },
    { label: l.row.dialling, value: ({ report }) => report?.lanes.dialling ?? dash },
    {
      label: <TipLabel label={l.row.claims} tip={l.row.claimsTip} />,
      value: ({ report }) => report?.lanes.claims ?? dash,
    },
    {
      label: l.row.waited,
      value: ({ report }) =>
        report === undefined
          ? dash
          : report.lanes.claims_waited === 0
            ? 0
            : l.row.waitedValue(report.lanes.claims_waited, t.common.ms(report.lanes.claim_wait_ms)),
    },
    { label: l.row.claimsFailed, value: ({ report }) => report?.lanes.claims_failed ?? dash },
    {
      label: <TipLabel label={l.row.refused} tip={l.row.refusedTip} />,
      value: ({ report }) => report?.lanes.refused ?? dash,
    },
    {
      label: <TipLabel label={l.row.sealed} tip={l.row.sealedTip} />,
      value: ({ report }) => (report === undefined ? dash : formatBytes(report.lanes.sealed_bytes)),
    },
    {
      label: <TipLabel label={l.row.passthrough} tip={l.row.passthroughTip} />,
      value: ({ report }) => (report === undefined ? dash : formatBytes(report.lanes.passthrough_bytes)),
    },
    {
      label: <TipLabel label={l.row.spliced} tip={l.row.splicedTip} />,
      value: ({ report }) => (report === undefined ? dash : formatBytes(report.lanes.spliced_bytes)),
    },
    {
      label: <TipLabel label={l.row.handshake} tip={l.row.handshakeTip} />,
      value: ({ detail }) => {
        const failures = detail.data?.lanes?.handshake_failures
        if (failures === undefined) return dash
        const parts = handshakeParts(failures)
        return (
          <>
            {handshakeTotal(failures)}
            {parts.length > 0 ? <span className="ml-2 text-muted-foreground">{parts.join(' · ')}</span> : null}
          </>
        )
      },
    },
  ]
  const [first, second] = sides
  return (
    <div className="rounded-lg bg-muted/60 px-3 py-1" data-testid={`lane-details-${proxy.name}`}>
      <div className="overflow-x-auto">
        <table
          className="w-full border-collapse text-[13px]"
          aria-label={l.detailsTitle(first?.name ?? '', second?.name ?? '')}
        >
          <thead>
            <tr className="border-b border-border-strong/60 text-xs text-muted-foreground">
              <th scope="col" className="px-3 py-2 text-left font-medium">
                {l.detailsTitle(first?.name ?? '', second?.name ?? '')}
              </th>
              {sides.map((side) => (
                <th key={side.id} scope="col" className="px-3 py-2 text-right font-medium">
                  {side.name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, index) => (
              <tr key={index} className="border-b border-border-strong/60 last:border-b-0">
                <th scope="row" className="px-3 py-1.5 text-left font-normal text-muted-foreground">
                  {row.label}
                </th>
                {sides.map((side) => (
                  <td key={side.id} className="px-3 py-1.5 text-right whitespace-nowrap tabular">
                    {row.value(side)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="px-3 pt-1 pb-2 text-xs text-muted-foreground">{l.sharedNote}</p>
    </div>
  )
}
