import { useQuery } from '@tanstack/react-query'
import { ChevronDown, LogOut, PanelLeft, Settings, UserRound } from 'lucide-react'
import { useState } from 'react'
import { NavLink, useLocation, useNavigate } from 'react-router'

import { q } from '@/api/queries'
import { type Network } from '@/api/types'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
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

/** 侧栏里一行的样子：导航项、账号菜单、收起按钮共用，高 32 像素。 */
const ROW = 'flex h-8 items-center gap-2.5 rounded-md text-sm transition-colors'
const ROW_IDLE = 'text-foreground/80 hover:bg-foreground/[0.05] hover:text-foreground [&_svg]:text-muted-foreground'

interface ItemLinkProps {
  to: string
  label: string
  icon: NavItem['icon']
  planned?: boolean
  collapsed: boolean
}

function ItemLink({ to, label, icon: Icon, planned = false, collapsed }: ItemLinkProps) {
  // 和 NavLink 自己的判断一样：地址就是它，或者在它下面。这里自己算，样式才能是一个字符串——
  // 收起时外面套着悬停说明，它会把自己的 className 和这里的拼起来，拼不了函数。
  const { pathname } = useLocation()
  const active = pathname === to || pathname.startsWith(`${to}/`)
  const link = (
    <NavLink
      to={to}
      aria-label={collapsed ? (planned ? `${label}（${t.common.planned}）` : label) : undefined}
      className={cn(
        ROW,
        collapsed ? 'w-10 justify-center' : 'px-2.5',
        active ? 'bg-foreground/[0.07] font-medium text-foreground' : ROW_IDLE,
      )}
    >
      <Icon className="size-4 shrink-0" aria-hidden />
      {collapsed ? null : (
        <>
          <span className="min-w-0 flex-1 truncate">{label}</span>
          {planned ? <span className="text-[11px] text-muted-foreground">{t.common.planned}</span> : null}
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
  onToggle: () => void
  onSignOut: () => void
  onCreateNetwork: () => void
}

/**
 * 侧栏：浅灰底，没有边线，和白色的正文靠底色分开。图标和字都是灰的，当前页加深、铺一层更深的灰；
 * 侧栏里不用强调色。最底下是账号菜单、收起按钮和版本。
 */
export function Sidebar({ networks, current, collapsed, onToggle, onSignOut, onCreateNetwork }: SidebarProps) {
  const location = useLocation()
  const navigate = useNavigate()
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

  const toggleLabel = collapsed ? t.nav.expand : t.nav.collapse

  return (
    // 外层撑满整页的高度（底色一直到底），里面那一层跟着视口走。
    <aside className={cn('shrink-0 bg-sidebar transition-[width]', collapsed ? 'w-16' : 'w-60')}>
      <div className="sticky top-0 flex h-screen flex-col">
        <div className="px-3 pt-4 pb-4">
          <NetworkSwitcher networks={networks} current={current} collapsed={collapsed} onCreate={onCreateNetwork} />
        </div>

        <nav aria-label={t.nav.label} className="flex-1 overflow-y-auto px-3 pb-4">
          {NAV_GROUPS.map((group) => {
            // 当前页面所在的分区总是展开的，不管记住的折叠状态。
            const containsCurrent = group.items.some((item) => item.page === page)
            const open = group.label === undefined || collapsed || containsCurrent || !closedGroups.includes(group.id)
            return (
              <div key={group.id} className="mt-5 first:mt-0">
                {group.label !== undefined && !collapsed ? (
                  <button
                    type="button"
                    onClick={() => toggleGroup(group.id)}
                    aria-expanded={open}
                    aria-label={t.nav.toggleGroup(group.label, open)}
                    disabled={containsCurrent}
                    className="group/label mb-1 flex h-6 w-full items-center justify-between rounded-md px-2.5 text-xs text-muted-foreground hover:text-foreground disabled:cursor-default disabled:hover:text-muted-foreground"
                  >
                    {group.label}
                    <ChevronDown
                      className={cn(
                        'size-3.5 opacity-0 transition group-hover/label:opacity-100 group-focus-visible/label:opacity-100',
                        !open && '-rotate-90 opacity-100',
                        containsCurrent && 'hidden',
                      )}
                      aria-hidden
                    />
                  </button>
                ) : null}
                {group.label !== undefined && collapsed ? <div aria-hidden className="mx-2 mb-3 border-t" /> : null}
                {open ? (
                  <ul className="space-y-px">
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

        <div className="px-3 pt-4 pb-4">
          <ul className="space-y-px">
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

          <div className={cn('mt-3 flex gap-1 border-t pt-3', collapsed && 'flex-col items-center')}>
            <DropdownMenu>
              <DropdownMenuTrigger
                aria-label={t.nav.accountMenu}
                className={cn(ROW, ROW_IDLE, collapsed ? 'w-10 justify-center' : 'min-w-0 flex-1 px-2.5')}
              >
                <UserRound className="size-4 shrink-0" aria-hidden />
                {collapsed ? null : <span className="truncate">{t.nav.account}</span>}
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" side="top">
                <DropdownMenuLabel>{t.nav.account}</DropdownMenuLabel>
                <DropdownMenuItem onSelect={() => void navigate('/settings')}>
                  <Settings aria-hidden />
                  {t.nav.settings}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={onSignOut}>
                  <LogOut aria-hidden />
                  {t.nav.signOut}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <Tooltip content={toggleLabel} side={collapsed ? 'right' : 'top'}>
              <button
                type="button"
                onClick={onToggle}
                aria-label={toggleLabel}
                aria-pressed={collapsed}
                className={cn(ROW, ROW_IDLE, 'w-8 shrink-0 justify-center', collapsed && 'w-10')}
              >
                <PanelLeft className="size-4" aria-hidden />
              </button>
            </Tooltip>
          </div>
          {collapsed ? null : <VersionFooter status={status.data} />}
        </div>
      </div>
    </aside>
  )
}
