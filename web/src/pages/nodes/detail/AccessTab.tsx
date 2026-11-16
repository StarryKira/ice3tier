import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type AccessEdge, type Network, type NodeDetail, type Rule } from '@/api/types'
import { NodeLink } from '@/components/node'
import { Notice } from '@/components/notice'
import { RuleSentence } from '@/components/rule'
import { ruleLabel } from '@/lib/rules'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { networkPath } from '@/pages/networks/context'

const d = t.nodes.detail

function EdgeList({ network, edges, rules }: { network: Network; edges: AccessEdge[]; rules: readonly Rule[] }) {
  const nodes = useQuery(q.nodes(network.id))
  if (edges.length === 0) return <p className="px-5 py-4 text-sm text-muted-foreground">{d.accessNone}</p>
  return (
    <ul className="divide-y">
      {edges.map((edge) => (
        <li key={edge.node.id} className="flex items-center justify-between gap-4 px-5 py-2.5">
          <span className="min-w-0">
            <NodeLink
              network={network.id}
              node={edge.node}
              online={nodes.data?.find((other) => other.id === edge.node.id)?.online}
            />
            <span className="ml-2 font-mono text-xs text-muted-foreground">{edge.node.virtual_ip}</span>
          </span>
          <span className="flex shrink-0 flex-wrap items-center justify-end gap-1.5" aria-label={d.accessByRules}>
            {edge.rules.map((id) => {
              const rule = rules.find((candidate) => candidate.id === id)
              return rule === undefined ? null : (
                <Tooltip key={id} content={`${d.accessByRules}：${ruleLabel(rule)}`}>
                  <span tabIndex={0} className="rounded-sm">
                    <RuleSentence rule={rule} />
                  </span>
                </Tooltip>
              )
            })}
          </span>
        </li>
      ))}
    </ul>
  )
}

/** 访问关系的表格版：这个节点能访问谁、谁能访问它、各是哪些规则允许的。 */
export function AccessTab({ network, node }: { network: Network; node: NodeDetail }) {
  const access = useQuery(q.nodeAccess(network.id, node.id))
  const rules = useQuery(q.rules(network.id))

  if (access.isError) {
    return (
      <Card>
        <ErrorState title={d.accessLoadFailed} error={access.error} onRetry={() => void access.refetch()} />
      </Card>
    )
  }
  if (access.isPending) {
    return (
      <Card>
        <SkeletonBlock lines={5} />
      </Card>
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-muted-foreground">{d.accessLead}</p>
        <Button asChild size="sm">
          <Link to={`${networkPath(network.id, 'access')}?view=node&node=${node.id}`}>{d.accessOpen}</Link>
        </Button>
      </div>
      {access.data.acknowledged ? null : <Notice tone="warn">{d.accessPending}</Notice>}
      <div className="grid grid-cols-1 items-start gap-5 @4xl:grid-cols-2">
        <Card>
          <CardHeader title={d.accessCanReach(access.data.can_reach.length)} />
          <EdgeList network={network} edges={access.data.can_reach} rules={rules.data ?? []} />
        </Card>
        <Card>
          <CardHeader title={d.accessReachableBy(access.data.reachable_by.length)} />
          <EdgeList network={network} edges={access.data.reachable_by} rules={rules.data ?? []} />
        </Card>
      </div>
    </div>
  )
}
