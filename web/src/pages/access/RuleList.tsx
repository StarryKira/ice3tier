import { EllipsisVertical, MoveHorizontal, MoveRight, Pencil, Trash2 } from 'lucide-react'
import { useEffect, useRef } from 'react'

import { type AccessGraph, type Direction, type GroupRef, type Network, type Rule } from '@/api/types'
import { isAllToAll } from '@/access/reach'
import { GroupChip } from '@/components/groups'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { ruleLabel } from '@/lib/rules'
import { cn } from '@/lib/utils'

import { useRuleActions } from './useRuleActions'

const a = t.access

interface RuleListProps {
  network: Network
  graph: AccessGraph
  rules: readonly Rule[]
  /** 从操作记录、关系图点过来的那条规则：这一行亮起来，滚动到看得见的地方。 */
  focused: number | null
  onEdit: (rule: Rule) => void
}

/** 一个分组：分组的小标签，悬停写它现在有几个节点。 */
function GroupCell({ group, graph }: { group: GroupRef; graph: AccessGraph }) {
  const members = graph.groups.find((candidate) => candidate.id === group.id)?.nodes.length ?? 0
  return (
    <Tooltip content={a.members(members)}>
      <span tabIndex={0} className="inline-flex rounded-md">
        <GroupChip name={group.name} builtin={group.builtin} />
      </span>
    </Tooltip>
  )
}

/** 方向：一个带框的长箭头，双向两头都有。启用时是绿色的，停用时是灰的。 */
function DirectionBox({ direction, enabled }: { direction: Direction; enabled: boolean }) {
  const Icon = direction === 'both' ? MoveHorizontal : MoveRight
  const label = direction === 'both' ? a.directionBoth : a.directionOneWay
  return (
    <Tooltip content={label}>
      <span
        tabIndex={0}
        role="img"
        aria-label={label}
        className={cn(
          'inline-flex h-7 w-16 items-center justify-center rounded-md border',
          enabled ? 'border-ok/50 bg-ok-soft text-ok' : 'border-border text-faint',
        )}
      >
        <Icon className="size-5" strokeWidth={1.5} aria-hidden />
      </span>
    </Tooltip>
  )
}

/**
 * 策略表（照 NetBird 的样子）：一条策略一行，名字（前面一个圆点：启用是绿的）、启用开关、来源、方向、目标。
 * 停用的来源和目标变淡，开关照样能拨。策略只有"允许"，没有顺序和优先级，所以按编号排、不给拖动。
 */
export function RuleList({ network, graph, rules, focused, onEdit }: RuleListProps) {
  const actions = useRuleActions(network, graph)
  const rows = useRef(new Map<number, HTMLTableRowElement>())

  useEffect(() => {
    if (focused !== null) rows.current.get(focused)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [focused])

  return (
    <Table>
      <thead>
        <tr>
          <Th>{a.colName}</Th>
          <Th className="w-20">{a.colEnabled}</Th>
          <Th>{a.colSource}</Th>
          <Th>{a.colDirection}</Th>
          <Th>{a.colTarget}</Th>
          <Th className="w-0">
            <span className="sr-only">{t.common.moreActions}</span>
          </Th>
        </tr>
      </thead>
      <tbody>
        {rules.map((rule) => {
          const label = ruleLabel(rule)
          return (
            <Tr
              key={rule.id}
              ref={(element) => {
                if (element === null) rows.current.delete(rule.id)
                else rows.current.set(rule.id, element)
              }}
              data-testid={`rule-row-${rule.id}`}
              data-highlighted={focused === rule.id || undefined}
              className={cn('h-14', focused === rule.id && 'bg-accent/60')}
            >
              <Td>
                <span className="inline-flex items-center gap-2.5">
                  <span
                    aria-hidden
                    className={cn('size-2 shrink-0 rounded-full', rule.enabled ? 'bg-ok-dot' : 'bg-idle-dot')}
                  />
                  {rule.name === '' ? (
                    <span className="text-muted-foreground">{a.unnamed}</span>
                  ) : (
                    <span className="font-medium">{rule.name}</span>
                  )}
                  {isAllToAll(rule) ? (
                    <Tooltip content={a.defaultTip}>
                      <Badge tone="outline" tabIndex={0}>
                        {a.defaultBadge}
                      </Badge>
                    </Tooltip>
                  ) : null}
                </span>
              </Td>
              <Td>
                <Switch
                  aria-label={a.ruleSwitch(label)}
                  checked={rule.enabled}
                  disabled={actions.pendingRuleId === rule.id}
                  onCheckedChange={(checked) => void actions.toggle(rule, checked)}
                />
              </Td>
              <Td className={cn(!rule.enabled && 'opacity-55')}>
                <GroupCell group={rule.src} graph={graph} />
              </Td>
              <Td>
                <DirectionBox direction={rule.direction} enabled={rule.enabled} />
              </Td>
              <Td className={cn(!rule.enabled && 'opacity-55')}>
                <GroupCell group={rule.dst} graph={graph} />
              </Td>
              <Td>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="icon-sm" aria-label={a.ruleActions(label)}>
                      <EllipsisVertical aria-hidden />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent>
                    <DropdownMenuItem onSelect={() => onEdit(rule)}>
                      <Pencil aria-hidden />
                      {a.menuEdit}
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem danger onSelect={() => void actions.destroy(rule)}>
                      <Trash2 aria-hidden />
                      {a.menuDelete}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </Td>
            </Tr>
          )
        })}
      </tbody>
    </Table>
  )
}
