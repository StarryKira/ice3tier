import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, CircleCheck, CircleX } from 'lucide-react'

import { errorMessage } from '@/api/client'
import { q } from '@/api/queries'
import { type GroupId, type Network, type Node, type NodeId } from '@/api/types'
import { NodeSelect } from '@/components/node'
import { ruleLabel } from '@/lib/rules'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'

import { type RuleDraft } from './RuleDialog'

const c = t.access.checker

function Verdict({ network, from, to }: { network: Network; from: Node; to: Node }) {
  const check = useQuery(q.check(network.id, from.id, to.id))
  return (
    <li className="flex items-start gap-2.5 py-2" data-testid={`check-${from.name}-${to.name}`}>
      {check.isPending ? (
        <Skeleton className="mt-1 h-3.5 w-72" />
      ) : check.isError ? (
        <p role="alert" className="flex flex-wrap gap-x-2 text-sm">
          <span className="text-danger">{c.loadFailed}</span>
          <span className="text-muted-foreground">{errorMessage(check.error)}</span>
        </p>
      ) : (
        <>
          {check.data.allowed ? (
            <CircleCheck className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden />
          ) : (
            <CircleX className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />
          )}
          <p className="flex flex-wrap items-baseline gap-x-2.5 text-sm">
            <span className="font-medium">{c.direction(from.name, to.name)}</span>
            <span className={check.data.allowed ? 'font-medium' : 'font-medium text-danger'}>
              {check.data.allowed ? c.allowed : c.denied}
            </span>
            <span className="text-muted-foreground">
              {check.data.allowed ? c.byRules(formatList(check.data.rules.map(ruleLabel))) : c.noRule}
            </span>
          </p>
        </>
      )}
    </li>
  )
}

interface AccessCheckProps {
  network: Network
  nodes: readonly Node[]
  /** 内置分组 All 的编号：没有分组的节点，新建规则时预填它。 */
  all: GroupId | undefined
  from: NodeId | null
  to: NodeId | null
  onChange: (from: NodeId | null, to: NodeId | null) => void
  onClose: () => void
  onCreateRule: (draft: RuleDraft) => void
}

/**
 * 访问检查：一个对话框，选两个节点，立即出结果。方向有意义，所以两个方向各答一次。
 * 图上的每个结论在这里都能用文字得到。不允许时底下给出两边的分组和"新建策略"。
 */
export function AccessCheck({ network, nodes, all, from, to, onChange, onClose, onCreateRule }: AccessCheckProps) {
  const fromNode = nodes.find((node) => node.id === from)
  const toNode = nodes.find((node) => node.id === to)
  const same = from !== null && from === to
  const forward = useQuery({
    ...q.check(network.id, from ?? 0, to ?? 0),
    enabled: fromNode !== undefined && toNode !== undefined && !same,
  })
  const backward = useQuery({
    ...q.check(network.id, to ?? 0, from ?? 0),
    enabled: fromNode !== undefined && toNode !== undefined && !same,
  })
  const denied = forward.data?.allowed === false || backward.data?.allowed === false
  const groupsOf = (node: Node) =>
    node.groups.length === 0 ? c.noGroups : formatList(node.groups.map((group) => group.name))

  return (
    <Dialog open onOpenChange={(open) => (open ? undefined : onClose())}>
      <DialogContent title={c.title} data-testid="access-check">
        <DialogBody>
          <div className="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-end gap-2">
            <label className="flex min-w-0 flex-col gap-1.5 text-[13px] font-medium">
              {c.from}
              <NodeSelect nodes={nodes} value={from} onChange={(id) => onChange(id, to)} aria-label={c.from} />
            </label>
            <Tooltip content={c.swap}>
              <Button variant="ghost" size="icon" onClick={() => onChange(to, from)} aria-label={c.swap}>
                <ArrowLeftRight aria-hidden />
              </Button>
            </Tooltip>
            <label className="flex min-w-0 flex-col gap-1.5 text-[13px] font-medium">
              {c.to}
              <NodeSelect nodes={nodes} value={to} onChange={(id) => onChange(from, id)} aria-label={c.to} />
            </label>
          </div>

          {/* 结果的地方一直留着两行高：选好之后对话框不跳。 */}
          <div className="mt-4 min-h-20" aria-live="polite">
            {same ? (
              <p className="py-2 text-sm text-warn">{c.same}</p>
            ) : fromNode === undefined || toNode === undefined ? (
              <p className="py-2 text-sm text-muted-foreground">{c.pickTwo}</p>
            ) : (
              <ul className="divide-y">
                <Verdict network={network} from={fromNode} to={toNode} />
                <Verdict network={network} from={toNode} to={fromNode} />
              </ul>
            )}
          </div>
        </DialogBody>
        {denied && fromNode !== undefined && toNode !== undefined && !same ? (
          <DialogFooter className="justify-between text-[13px]">
            <p className="flex min-w-0 flex-col text-muted-foreground">
              <span className="truncate">{c.groupsOf(fromNode.name, groupsOf(fromNode))}</span>
              <span className="truncate">{c.groupsOf(toNode.name, groupsOf(toNode))}</span>
            </p>
            <Button
              variant="primary"
              className="shrink-0"
              onClick={() => onCreateRule({ src: fromNode.groups[0]?.id ?? all, dst: toNode.groups[0]?.id ?? all })}
            >
              {c.createRule}
            </Button>
          </DialogFooter>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}
