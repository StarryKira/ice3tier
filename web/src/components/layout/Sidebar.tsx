import { useQuery } from '@tanstack/react-query'
import { ChevronDown } from 'lucide-react'
import { useState } from 'react'
import { NavLink, useLocation } from 'react-router'

import { q } from '@/api/queries'
import { type Network } from '@/api/types'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { readStored, writeStored } from '@/lib/storage'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

import { currentPage, NAV_ADMIN, NAV_GROUPS, NAV_SETTINGS, type NavItem } from './nav'
import { NetworkSwitcher } from './NetworkSwitcher'
import { VersionFooter } from './VersionFooter'

const COLLAPSED_GROUPS_KEY = 'navCollapsedGroups'

const isStringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === 'string')

interface ItemLinkProps {
  to: string
  label: string
  icon: NavItem['icon']
  planned?: boolean
  collapsed: boolean
}

function ItemLink({ to, label, icon: Icon, planned = false, collapsed }: ItemLinkProps) {
  const link = (
    <NavLink
      to={to}
      aria-label={collapsed ? (planned ? `${label}（${t.common.planned}）` : label) : undefined}
      className={({ isActive }) =>
        cn(
          'group flex h-8.5 items-center gap-2.5 rounded-md text-sm transition-colors',
          collapsed ? 'w-9 justify-center' : 'px-2.5',
          isActive
            ? 'bg-accent font-medium text-accent-foreground'
            : 'text-foreground/85 hover:bg-muted hover:text-foreground',
        )
      }
    >
      <Icon className="size-4 shrink-0 opacity-80" aria-hidden />
      {collapsed ? null : (
        <>
          <span className="min-w-0 flex-1 truncate">{label}</span>
          {planned ? (
            <span className="rounded-sm border border-border-strong px-1 text-[11px] leading-4 font-normal text-muted-foreground">
              {t.common.planned}
            </span>
          ) : null}
        </>
      )}
    </NavLink>
  )
  return collapsed ? (
    <Tooltip content={planned ? `${label}（${t.common.planned}）` : label} side="right">
      {link}
    </Tooltip>
  ) : (
    link
  )
}

interface SidebarProps {
  networks: readonly Network[]
  current: Network
  collapsed: boolean
  onCreateNetwork: () => void
}

export function Sidebar({ networks, current, collapsed, onCreateNetwork }: SidebarProps) {
  const location = useLocation()
  const page = currentPage(location.pathname)
  const [closedGroups, setClosedGroups] = useState<string[]>(() => readStored(COLLAPSED_GROUPS_KEY, [], isStringArray))
  const status = useQuery(q.status())

  const toggleGroup = (id: string) => {
    setClosedGroups((closed) => {
      const next = closed.includes(id) ? closed.filter((other) => other !== id) : [...closed, id]
      writeStored(COLLAPSED_GROUPS_KEY, next)
      return next
    })
  }

  return (
    // 外层撑满整页的高度（背景和右边线一直到底），里面那一层跟着视口走。
    <aside className={cn('shrink-0 border-r bg-card transition-[width]', collapsed ? 'w-15' : 'w-58')}>
      <div className="sticky top-0 flex h-screen flex-col">
        <div className="px-3 pt-3 pb-2">
          <NetworkSwitcher networks={networks} current={current} collapsed={collapsed} onCreate={onCreateNetwork} />
        </div>

        <nav aria-label={t.nav.label} className="flex-1 overflow-y-auto px-3 pb-3">
          {NAV_GROUPS.map((group) => {
            // 当前页面所在的分区总是展开的，不管记住的折叠状态。
            const containsCurrent = group.items.some((item) => item.page === page)
            const open = group.label === undefined || collapsed || containsCurrent || !closedGroups.includes(group.id)
            return (
              <div key={group.id} className="mt-3 first:mt-1">
                {group.label !== undefined && !collapsed ? (
                  <button
                    type="button"
                    onClick={() => toggleGroup(group.id)}
                    aria-expanded={open}
                    aria-label={t.nav.toggleGroup(group.label, open)}
                    disabled={containsCurrent}
                    className="mb-0.5 flex h-7 w-full items-center justify-between rounded-md px-2.5 text-xs font-medium text-muted-foreground hover:text-foreground disabled:cursor-default disabled:hover:text-muted-foreground"
                  >
                    {group.label}
                    <ChevronDown
                      className={cn(
                        'size-3.5 transition-transform',
                        !open && '-rotate-90',
                        containsCurrent && 'opacity-0',
                      )}
                      aria-hidden
                    />
                  </button>
                ) : null}
                {group.label !== undefined && collapsed ? <div aria-hidden className="mx-1 mb-2 border-t" /> : null}
                {open ? (
                  <ul className="space-y-0.5">
                    {group.items.map((item) => (
                      <li key={item.page}>
                        <ItemLink
                          to={networkPath(current.id, item.page)}
                          label={item.label}
                          icon={item.icon}
                          planned={item.planned}
                          collapsed={collapsed}
                        />
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>
            )
          })}
        </nav>

        <div className="border-t px-3 py-2.5">
          <ul className="space-y-0.5">
            {NAV_ADMIN.map((item) => (
              <li key={item.page}>
                <ItemLink
                  to={networkPath(current.id, item.page)}
                  label={item.label}
                  icon={item.icon}
                  collapsed={collapsed}
                />
              </li>
            ))}
            <li>
              <ItemLink
                to={NAV_SETTINGS.to}
                label={NAV_SETTINGS.label}
                icon={NAV_SETTINGS.icon}
                collapsed={collapsed}
              />
            </li>
          </ul>
          {collapsed ? null : <VersionFooter status={status.data} />}
        </div>
      </div>
    </aside>
  )
}
