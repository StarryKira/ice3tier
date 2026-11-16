import { useQuery } from '@tanstack/react-query'
import { Route as RouteIcon } from 'lucide-react'

import { q } from '@/api/queries'
import { type Network, type NodeDetail, type NodeRoute } from '@/api/types'
import { NodeLink } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Card } from '@/components/ui/card'
import { t } from '@/i18n'
import { REPORT_INTERVAL_SECS } from '@/lib/product'

const d = t.nodes.detail

/**
 * 一条路径的写法。直连是绿色，经 relay 是蓝色：中转是正常的工作状态，不用警告色。只有无路可走是红色。
 */
function PathCell({ route, relayNames }: { route: NodeRoute['route']; relayNames: (ids: number[]) => string }) {
  switch (route.via) {
    case 'direct':
      return (
        <div>
          <StatusText tone="ok">{d.viaDirect}</StatusText>
          <div className="mt-0.5 font-mono text-xs text-muted-foreground">{d.viaDirectAddr(route.addr)}</div>
        </div>
      )
    case 'relay':
      return <StatusText tone="relay">{d.viaRelay(relayNames(route.relays))}</StatusText>
    case 'none':
      return (
        <div>
          <StatusText tone="danger">{d.viaNone}</StatusText>
          <div className="mt-0.5 text-xs text-muted-foreground">{d.viaNoneHint}</div>
        </div>
      )
  }
}

/** 这个节点到其他节点现在怎么走，按它自己最近一次的上报。 */
export function RoutesTab({ network, node }: { network: Network; node: NodeDetail }) {
  const routes = useQuery(q.routes(network.id, node.id))
  const nodes = useQuery(q.nodes(network.id))
  const relayNames = (ids: number[]) => ids.map((id) => nodeName(nodes.data ?? [], id)).join(' → ')

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
        <p className="text-sm text-muted-foreground">{d.routesLead}</p>
        <p className="text-[13px] text-muted-foreground">{t.common.updatedEvery(REPORT_INTERVAL_SECS)}</p>
      </div>
      <Card>
        {routes.isError ? (
          <ErrorState title={d.routesLoadFailed} error={routes.error} onRetry={() => void routes.refetch()} />
        ) : routes.isSuccess && !routes.data.online ? (
          <EmptyState icon={RouteIcon} title={d.routesOffline} />
        ) : routes.isSuccess && routes.data.routes.length === 0 ? (
          <EmptyState icon={RouteIcon} title={d.routesEmpty} description={d.routesEmptyHint} />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{d.colPeer}</Th>
                <Th>{d.colPath}</Th>
                <Th numeric>{d.colRtt}</Th>
              </tr>
            </thead>
            <tbody>
              {routes.isPending ? <SkeletonRows columns={3} rows={3} /> : null}
              {(routes.data?.routes ?? []).map((entry) => (
                <Tr key={entry.peer.id}>
                  <Td>
                    <NodeLink
                      network={network.id}
                      node={entry.peer}
                      online={nodes.data?.find((other) => other.id === entry.peer.id)?.online}
                    />
                    <div className="mt-0.5 font-mono text-xs text-muted-foreground">{entry.peer.virtual_ip}</div>
                  </Td>
                  <Td>
                    <PathCell route={entry.route} relayNames={relayNames} />
                  </Td>
                  <Td numeric>
                    {entry.rtt_ms === null ? (
                      <span className="text-muted-foreground">{d.rttUnknown}</span>
                    ) : (
                      t.common.ms(entry.rtt_ms)
                    )}
                  </Td>
                </Tr>
              ))}
            </tbody>
          </Table>
        )}
      </Card>
    </div>
  )
}
