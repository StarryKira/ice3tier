import { ChevronRight } from 'lucide-react'
import { useEffect, useRef } from 'react'
import { Link } from 'react-router'

import { type AccessGraph, type GroupId, type Network } from '@/api/types'
import { GroupChip } from '@/components/groups'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

const p = t.access.groupsPanel

interface GroupPanelProps {
  network: Network
  graph: AccessGraph
  /** 图上点中的那个分组：这一行亮起来。 */
  active: GroupId | null
}

/**
 * 访问控制页上的分组：每个分组一行，有几个节点、被几条规则用到，对照着规则和图看。
 * 新建、改名、删除和增减成员在分组页上做：点一行就去分组页上的那一行。
 */
export function GroupPanel({ network, graph, active }: GroupPanelProps) {
  const rows = useRef(new Map<GroupId, HTMLLIElement>())

  useEffect(() => {
    if (active !== null) rows.current.get(active)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [active])

  return (
    <ul className="-mx-3 divide-y border-t">
      {graph.groups.map((group) => (
        <li
          key={group.id}
          ref={(element) => {
            if (element === null) rows.current.delete(group.id)
            else rows.current.set(group.id, element)
          }}
          data-testid={`group-row-${group.name}`}
        >
          <Link
            to={`${networkPath(network.id, 'groups')}?focus=${group.id}`}
            className={cn(
              'flex w-full items-center gap-2.5 rounded-md px-3 py-2.5 hover:bg-muted/60',
              active === group.id && 'bg-accent/60 hover:bg-accent',
            )}
          >
            <GroupChip name={group.name} builtin={group.builtin} />
            <span className="text-[13px] text-muted-foreground tabular">{p.members(group.nodes.length)}</span>
            <span className="ml-auto text-[13px] text-muted-foreground tabular">{p.usedBy(group.rules.length)}</span>
            <ChevronRight className="size-3.5 shrink-0 text-faint" aria-hidden />
          </Link>
        </li>
      ))}
    </ul>
  )
}
