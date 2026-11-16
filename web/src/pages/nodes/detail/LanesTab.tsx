import { useQuery } from '@tanstack/react-query'
import { Cable } from 'lucide-react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type Network, type NodeDetail } from '@/api/types'
import { NodeLink } from '@/components/node'
import { EmptyState } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { handshakeParts, handshakeTotal, laneBytes, peerGlance } from '@/lib/lanes'
import { nodeName } from '@/lib/nodes'
import { networkPath } from '@/pages/networks/context'

const d = t.nodes.detail
const l = t.pairs.lane

/**
 * 这个节点和各个对端之间的独占 TCP，取自它最近的上报（`NodeDetail.lanes`）。和「路由」同样是一行一个对端；
 * 回退、拨号失败是它自启动以来的累计。等待补拨、各种方式的字节数在节点对页的详情里。
 */
export function LanesTab({ network, node }: { network: Network; node: NodeDetail }) {
  const nodes = useQuery(q.nodes(network.id))
  const pairs = useQuery(q.pairs(network.id))
  const all = nodes.data ?? []
  const report = node.lanes ?? null
  const peers = report?.peers ?? []

  if (!node.online) return <EmptyState icon={Cable} title={d.lanesOffline} />
  if (report === null || peers.length === 0) {
    return (
      <EmptyState
        icon={Cable}
        title={d.lanesEmpty}
        action={
          <Button asChild>
            <Link to={networkPath(network.id, 'pairs')}>{t.pairs.title}</Link>
          </Button>
        }
      />
    )
  }

  const failures = handshakeTotal(report.handshake_failures)
  const hasPair = (peer: number) =>
    (pairs.data ?? []).some(
      (pair) => (pair.a === node.id && pair.b === peer) || (pair.b === node.id && pair.a === peer),
    )

  return (
    <div>
      <Table>
        <thead>
          <tr>
            <Th>{d.colPeer}</Th>
            <Th>
              <Tooltip content={l.row.controlTip}>
                <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
                  {l.row.control}
                </span>
              </Tooltip>
            </Th>
            <Th numeric>{l.colInUse}</Th>
            <Th numeric>{l.colIdle}</Th>
            <Th numeric>
              <Tooltip content={l.fallbacksTip}>
                <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
                  {l.colFallbacks}
                </span>
              </Tooltip>
            </Th>
            <Th numeric>{l.colDialFailures}</Th>
            <Th numeric>{d.colLaneTraffic}</Th>
            <Th>
              <span className="sr-only">{t.common.moreActions}</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {peers.map((peer) => {
            const glance = peerGlance(peer)
            const peerNode = all.find((other) => other.id === peer.peer)
            return (
              <Tr key={peer.peer} data-testid={`node-lane-${peer.peer}`}>
                <Td>
                  <NodeLink
                    network={network.id}
                    node={{ id: peer.peer, name: nodeName(all, peer.peer) }}
                    online={peerNode?.online}
                  />
                  {peerNode !== undefined ? (
                    <div className="mt-0.5 font-mono text-xs text-muted-foreground">{peerNode.virtual_ip}</div>
                  ) : null}
                </Td>
                <Td>
                  <StatusText tone={peer.control ? 'ok' : 'idle'}>
                    {peer.control ? l.row.controlUp : l.row.controlDown}
                  </StatusText>
                  <div className="mt-0.5 text-xs text-muted-foreground">
                    {peer.dials ? d.lanesDials : d.lanesPeerDials}
                  </div>
                </Td>
                <Td numeric>{glance.inUse}</Td>
                <Td numeric>{glance.idle}</Td>
                <Td numeric>{glance.fallbacks}</Td>
                <Td numeric>{glance.dialFailures}</Td>
                <Td numeric>
                  <Tooltip
                    content={d.laneTrafficTip(
                      formatBytes(peer.sealed_bytes),
                      formatBytes(peer.passthrough_bytes),
                      formatBytes(peer.spliced_bytes),
                    )}
                  >
                    <span tabIndex={0} className="rounded-sm" data-testid={`node-lane-traffic-${peer.peer}`}>
                      {formatBytes(laneBytes(peer))}
                    </span>
                  </Tooltip>
                </Td>
                <Td className="text-right text-[13px] whitespace-nowrap">
                  {hasPair(peer.peer) ? (
                    <Link
                      to={`${networkPath(network.id, 'pairs')}?a=${node.id}&b=${peer.peer}`}
                      className="rounded-sm text-muted-foreground hover:text-foreground hover:underline"
                    >
                      {d.lanesPair}
                    </Link>
                  ) : null}
                </Td>
              </Tr>
            )
          })}
        </tbody>
      </Table>
      {failures > 0 ? (
        <TableNote data-testid="lane-handshake-failures">
          {d.lanesHandshake(failures, handshakeParts(report.handshake_failures).join(' · '))}
        </TableNote>
      ) : null}
    </div>
  )
}
