import { ChevronDown, PanelLeftClose, PanelLeftOpen } from 'lucide-react'
import { useState } from 'react'
import { NavLink, useLocation } from 'react-router'

import { type Network } from '@/api/types'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { readStored, writeStored } from '@/lib/storage'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

import { BrandMark } from './BrandMark'
import { currentPage, NAV_GROUPS, type NavItem } from './nav'

const COLLAPSED_GROUPS_KEY = 'navCollapsedGroups'

const isStringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === 'string')

/** 导航的一行：14 像素的字，18 像素的图标，和字隔 11 像素，38 像素高，圆角 10 像素。 */
const ROW = 'flex items-center gap-[11px] rounded-[10px] py-[9px] text-sm leading-5 transition-colors'
const ROW_IDLE = 'text-foreground/80 hover:bg-card/70 hover:text-foreground [&_svg]:text-muted-foreground'
/** 当前那一项：一块白，从灰底上浮起来一点。 */
const ROW_ACTIVE = 'bg-card font-medium text-foreground shadow-nav'

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
  const name = planned ? t.common.withPlanned(label) : label
  const link = (
    <NavLink
      to={to}
      aria-label={collapsed || planned ? name : undefined}
      className={cn(
        ROW,
        collapsed ? 'justify-center px-[7px]' : 'px-2.5',
        active ? ROW_ACTIVE : ROW_IDLE,
        // 计划中的页面：整行淡一些，悬停时说明。
        planned && !active && 'text-faint [&_svg]:text-faint/80',
      )}
    >
      <Icon className="size-[18px] shrink-0" strokeWidth={1.75} aria-hidden />
      {collapsed ? null : <span className="min-w-0 flex-1 truncate">{label}</span>}
    </NavLink>
  )
  return collapsed || planned ? (
    <Tooltip content={name} side="right">
      {link}
    </Tooltip>
  ) : (
    link
  )
}

/**
 * 侧栏顶上的一行：标志和名字，最右是收起侧栏的按钮；高 24 像素，和右边 60 像素高的顶栏居中对齐。
 * 侧栏收起时只剩标志，悬停变成展开的按钮。
 */
function SidebarBrand({ collapsed, onToggle }: { collapsed: boolean; onToggle: () => void }) {
  const label = collapsed ? t.nav.expand : t.nav.collapse
  if (collapsed) {
    return (
      <Tooltip content={label} side="right">
        <button
          type="button"
          onClick={onToggle}
          aria-label={label}
          aria-pressed
          className="group/expand relative -my-1 flex size-8 shrink-0 items-center justify-center rounded-lg transition-colors hover:bg-foreground/[0.05]"
        >
          <BrandMark markOnly className="transition-opacity group-hover/expand:opacity-0" />
          <PanelLeftOpen
            className="absolute size-4 text-muted-foreground opacity-0 transition-opacity group-hover/expand:opacity-100"
            strokeWidth={1.75}
            aria-hidden
          />
        </button>
      </Tooltip>
    )
  }
  return (
    <div className="flex h-6 shrink-0 items-center gap-2 pl-1.5">
      <BrandMark className="min-w-0 flex-1" />
      <Tooltip content={label} side="bottom">
        <button
          type="button"
          onClick={onToggle}
          aria-label={label}
          aria-pressed={false}
          className="flex size-7 shrink-0 items-center justify-center rounded-md text-faint transition-colors hover:bg-foreground/[0.05] hover:text-foreground"
        >
          <PanelLeftClose className="size-4" strokeWidth={1.75} aria-hidden />
        </button>
      </Tooltip>
    </div>
  )
}

interface SidebarProps {
  current: Network
  collapsed: boolean
  onToggle: () => void
}

/**
 * 侧栏：整个窗口的高度，左边一列，没有自己的底色，直接坐在整页的浅灰底上。顶上是标志和收起按钮，下面是导航：
 * 图标和字都是灰的，当前页是一块浮起来的白；侧栏里不用强调色。分区的标题可以折叠。
 * 尺寸照着参考的界面：208 像素宽，上下留 18、左右留 14，分区之间 22，导航和标志之间再多 10；一行 38 像素，14 像素的字，图标和字隔 11。
 */
export function Sidebar({ current, collapsed, onToggle }: SidebarProps) {
  const location = useLocation()
  const page = currentPage(location.pathname)
  const [closedGroups, setClosedGroups] = useState<string[]>(() => readStored(COLLAPSED_GROUPS_KEY, [], isStringArray))

  const toggleGroup = (id: string) => {
    setClosedGroups((closed) => {
      const next = closed.includes(id) ? closed.filter((other) => other !== id) : [...closed, id]
      writeStored(COLLAPSED_GROUPS_KEY, next)
      return next
    })
  }

  return (
    <aside
      className={cn(
        'flex h-full shrink-0 flex-col gap-[22px] px-[14px] py-[18px] transition-[width]',
        collapsed ? 'w-[60px]' : 'w-[208px]',
      )}
    >
      <SidebarBrand collapsed={collapsed} onToggle={onToggle} />
      <nav aria-label={t.nav.label} className="mt-2.5 flex min-h-0 flex-1 flex-col gap-[22px] overflow-y-auto">
        {NAV_GROUPS.map((group) => {
          // 当前页面所在的分区总是展开的，不管记住的折叠状态。
          const containsCurrent = group.items.some((item) => item.page === page)
          const open = group.label === undefined || collapsed || containsCurrent || !closedGroups.includes(group.id)
          return (
            <div key={group.id}>
              {group.label !== undefined && !collapsed ? (
                <button
                  type="button"
                  onClick={() => toggleGroup(group.id)}
                  aria-expanded={open}
                  aria-label={t.nav.toggleGroup(group.label, open)}
                  disabled={containsCurrent}
                  className="group/label flex w-full items-center justify-between rounded-md px-2.5 pb-1 text-[11px] leading-4 font-medium text-muted-foreground transition-colors hover:text-foreground disabled:cursor-default disabled:hover:text-muted-foreground"
                >
                  {group.label}
                  <ChevronDown
                    className={cn(
                      'size-3 opacity-0 transition group-hover/label:opacity-100 group-focus-visible/label:opacity-100',
                      !open && '-rotate-90 opacity-100',
                      containsCurrent && 'hidden',
                    )}
                    aria-hidden
                  />
                </button>
              ) : null}
              {group.label !== undefined && collapsed ? (
                <div aria-hidden className="mx-1.5 mb-2 border-t border-border-strong/60" />
              ) : null}
              {open ? (
                <ul className="space-y-0.5">
                  {group.items.map((item) => (
                    <li key={item.page}>
                      <ItemLink
                        to={item.to ?? networkPath(current.id, item.page)}
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
    </aside>
  )
}
