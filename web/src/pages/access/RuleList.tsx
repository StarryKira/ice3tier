import { EllipsisVertical, Pencil, Trash2 } from 'lucide-react'
import { useEffect, useRef } from 'react'

import { type AccessGraph, type Network, type Rule } from '@/api/types'
import { isAllToAll } from '@/access/reach'
import { Notice } from '@/components/notice'
import { RuleSentence } from '@/components/rule'
import { ruleLabel } from '@/lib/rules'
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
import { cn } from '@/lib/utils'

import { useRuleActions } from './useRuleActions'

interface RuleListProps {
  network: Network
  graph: AccessGraph
  /** 图上点中的、或者鼠标停着的那条规则：这一行亮起来。 */
  highlighted: number | null
  /** 图上刚点中的规则：除了亮起来，还要滚动到看得见的地方。 */
  focused: number | null
  onHighlight: (ruleId: number | null) => void
  onEdit: (rule: Rule) => void
}

/** 规则列表。每条规则一行，写成一句话的形状；启用开关直接在行里拨。 */
export function RuleList({ network, graph, highlighted, focused, onHighlight, onEdit }: RuleListProps) {
  const actions = useRuleActions(network, graph)
  const rows = useRef(new Map<number, HTMLLIElement>())

  useEffect(() => {
    if (focused !== null) rows.current.get(focused)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [focused])

  // 文档里的那句提醒要出现在界面上：all ⇄ all 开着时，别的规则看不出效果。
  const allOpen = graph.rules.some((rule) => rule.enabled && isAllToAll(rule))
  const othersEnabled = graph.rules.some((rule) => rule.enabled && !isAllToAll(rule))

  return (
    <div>
      {allOpen && othersEnabled ? (
        <div className="border-b px-4 py-3">
          <Notice tone="warn">{t.access.allOpenNotice}</Notice>
        </div>
      ) : null}
      <ul className="divide-y" onMouseLeave={() => onHighlight(null)}>
        {graph.rules.map((rule) => {
          const label = ruleLabel(rule)
          const builtin = isAllToAll(rule)
          return (
            <li
              key={rule.id}
              ref={(element) => {
                if (element === null) rows.current.delete(rule.id)
                else rows.current.set(rule.id, element)
              }}
              onMouseEnter={() => onHighlight(rule.id)}
              data-testid={`rule-row-${rule.id}`}
              data-highlighted={highlighted === rule.id || undefined}
              className={cn(
                'flex items-center gap-3 px-4 py-2.5 transition-colors',
                highlighted === rule.id && 'bg-accent/60',
              )}
            >
              {/* 停用的规则整行变淡，开关保持清晰可点。 */}
              <Switch
                aria-label={t.access.ruleSwitch(label)}
                checked={rule.enabled}
                disabled={actions.pendingRuleId === rule.id}
                onCheckedChange={(checked) => void actions.toggle(rule, checked)}
              />
              <div
                className={cn(
                  'flex min-w-0 flex-1 flex-wrap items-center gap-x-2.5 gap-y-1',
                  !rule.enabled && 'opacity-55',
                )}
              >
                <RuleSentence rule={rule} />
                {rule.name !== '' ? (
                  <span className="truncate text-[13px] text-muted-foreground">{rule.name}</span>
                ) : null}
                {builtin ? (
                  <Tooltip content={t.access.defaultTip}>
                    <Badge tone="outline" tabIndex={0}>
                      {t.access.defaultBadge}
                    </Badge>
                  </Tooltip>
                ) : null}
                {!rule.enabled ? <span className="text-xs text-muted-foreground">{t.access.disabledBadge}</span> : null}
              </div>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon-sm" aria-label={t.access.ruleActions(label)}>
                    <EllipsisVertical aria-hidden />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent>
                  <DropdownMenuItem onSelect={() => onEdit(rule)}>
                    <Pencil aria-hidden />
                    {t.access.menuEdit}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem danger onSelect={() => void actions.destroy(rule)}>
                    <Trash2 aria-hidden />
                    {t.access.menuDelete}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
