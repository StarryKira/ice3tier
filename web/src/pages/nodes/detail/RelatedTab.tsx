import { useQuery } from '@tanstack/react-query'
import { ArrowRight } from 'lucide-react'
import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type Network, type NodeDetail } from '@/api/types'
import { NodeLink } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { SkeletonBlock } from '@/components/states'
import { StatusText } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { forwardLabel, forwardStatusView } from '@/pages/forwards/status'
import { describeNoUser, describeWho, PROTOCOL_LABEL } from '@/pages/listeners/describe'
import { networkPath } from '@/pages/networks/context'

const d = t.nodes.detail

interface SectionProps {
  title: string
  manageTo: string
  pending: boolean
  empty: string
  count: number
  children: ReactNode
}

function Section({ title, manageTo, pending, empty, count, children }: SectionProps) {
  return (
    <Card>
      <CardHeader
        title={title}
        action={
          <Button asChild size="sm" variant="ghost">
            <Link to={manageTo}>{d.relatedManage}</Link>
          </Button>
        }
      />
      {pending ? (
        <SkeletonBlock lines={2} />
      ) : count === 0 ? (
        <p className="border-t pt-3 text-sm text-muted-foreground">{empty}</p>
      ) : (
        <ul className="divide-y border-t">{children}</ul>
      )}
    </Card>
  )
}

/** 关联配置：这个节点上的监听器、和它有关的端口转发与节点对设置。只读，编辑去各自的页面。 */
export function RelatedTab({ network, node }: { network: Network; node: NodeDetail }) {
  const nodes = useQuery(q.nodes(network.id))
  const listeners = useQuery(q.listeners(network.id))
  const forwards = useQuery(q.forwards(network.id))
  const pairs = useQuery(q.pairs(network.id))
  const all = nodes.data ?? []

  const myListeners = (listeners.data ?? []).filter((listener) => listener.node === node.id)
  const myForwards = (forwards.data ?? []).filter(
    (forward) => forward.listen_node === node.id || forward.target_node === node.id,
  )
  const myPairs = (pairs.data ?? []).filter((pair) => pair.a === node.id || pair.b === node.id)

  return (
    <div className="space-y-12">
      <Section
        title={d.relatedForwards}
        manageTo={networkPath(network.id, 'forwards')}
        pending={forwards.isPending}
        empty={d.relatedForwardsEmpty}
        count={myForwards.length}
      >
        {myForwards.map((forward) => {
          const view = forwardStatusView(forward, all)
          return (
            <li key={forward.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-3 text-sm">
              <StatusText tone={view.tone} className="w-36 shrink-0 text-[13px]">
                {view.label}
              </StatusText>
              <span className="font-medium">{forwardLabel(forward)}</span>
              <span className="flex min-w-0 flex-wrap items-center gap-1.5 text-muted-foreground">
                <span>{nodeName(all, forward.listen_node)}</span>
                <code className="font-mono text-xs">{forward.bind}</code>
                <ArrowRight className="size-3.5" aria-label={t.forwards.dialog.arrow} />
                <span>{nodeName(all, forward.target_node)}</span>
                <code className="font-mono text-xs">{forward.target_addr}</code>
              </span>
              <Badge className="ml-auto">
                {forward.listen_node === node.id ? d.relatedListening : d.relatedTarget}
              </Badge>
            </li>
          )
        })}
      </Section>

      <Section
        title={d.relatedListeners}
        manageTo={networkPath(network.id, 'listeners')}
        pending={listeners.isPending}
        empty={d.relatedListenersEmpty}
        count={myListeners.length}
      >
        {myListeners.map((listener) => (
          <li key={listener.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-3 text-sm">
            <span className="font-medium">{listener.name}</span>
            <Badge>{PROTOCOL_LABEL[listener.protocol]}</Badge>
            <code className="font-mono text-xs">{listener.bind}</code>
            <span className="text-muted-foreground">{describeWho(listener)}</span>
            <span className="ml-auto text-muted-foreground">{describeNoUser(listener, all)}</span>
          </li>
        ))}
      </Section>

      <Section
        title={d.relatedPairs}
        manageTo={networkPath(network.id, 'pairs')}
        pending={pairs.isPending}
        empty={d.relatedPairsEmpty}
        count={myPairs.length}
      >
        {myPairs.map((pair) => {
          const other = pair.a === node.id ? pair.b : pair.a
          return (
            <li key={`${pair.a}-${pair.b}`} className="flex items-center justify-between gap-3 py-3 text-sm">
              <NodeLink
                network={network.id}
                node={{ id: other, name: nodeName(all, other) }}
                online={all.find((candidate) => candidate.id === other)?.online}
              />
              <Badge tone={pair.keep_alive ? 'outline' : 'neutral'}>
                {d.keepAlive}：{pair.keep_alive ? t.common.yes : t.common.no}
              </Badge>
            </li>
          )
        })}
      </Section>
    </div>
  )
}
