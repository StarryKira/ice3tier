import {
  ArrowLeftRight,
  DoorOpen,
  KeyRound,
  LayoutDashboard,
  Link2,
  type LucideIcon,
  Network,
  RadioTower,
  Route,
  Server,
  Settings,
  ShieldCheck,
  Ticket,
  Waypoints,
} from 'lucide-react'

import { t } from '@/i18n'
import { PLANNED, type PlannedId } from '@/pages/planned/planned'

export interface NavItem {
  /** 路径里的那一段：`/n/<网络>/<page>`。 */
  page: string
  label: string
  icon: LucideIcon
  /** 计划中的页面：接口还没有，导航上带一个标记。 */
  planned?: boolean
}

export interface NavGroup {
  id: string
  /** 没有标题的组不能折叠。 */
  label?: string
  items: readonly NavItem[]
}

const planned = (id: PlannedId): NavItem => ({
  page: id,
  label: PLANNED[id].title,
  icon: PLANNED[id].icon,
  planned: true,
})

/**
 * 侧栏的结构：每天会看的三项在最上面，不加标题；然后按"在做哪一类事"分区；管理类的沉到底部。
 * 每个页面只有这一个入口。详情页没有自己的入口，打开时它所属的列表项保持高亮。
 */
export const NAV_GROUPS: readonly NavGroup[] = [
  {
    id: 'main',
    items: [
      { page: 'overview', label: t.nav.overview, icon: LayoutDashboard },
      { page: 'nodes', label: t.nav.nodes, icon: Server },
      { page: 'access', label: t.nav.access, icon: ShieldCheck },
    ],
  },
  {
    id: 'access',
    label: t.nav.groupAccess,
    items: [
      { page: 'exits', label: t.nav.exits, icon: DoorOpen },
      { page: 'listeners', label: t.nav.listeners, icon: RadioTower },
      { page: 'credentials', label: t.nav.credentials, icon: KeyRound },
      { page: 'forwards', label: t.nav.forwards, icon: ArrowLeftRight },
    ],
  },
  {
    id: 'diagnose',
    label: t.nav.groupDiagnose,
    items: [
      { page: 'topology', label: t.nav.topology, icon: Waypoints },
      { page: 'diagnosis', label: t.nav.diagnosis, icon: Route },
      planned('tests'),
    ],
  },
]

/** 底部的管理区。"设置"是全局的，其余属于当前网络。 */
export const NAV_ADMIN: readonly NavItem[] = [
  { page: 'tokens', label: t.nav.tokens, icon: Ticket },
  { page: 'pairs', label: t.nav.pairs, icon: Link2 },
  { page: 'network', label: t.nav.network, icon: Network },
]

export const NAV_SETTINGS = { label: t.nav.settings, icon: Settings, to: '/settings' } as const

/** 从地址里取出当前页面那一段：`/n/3/nodes/7` 是 `nodes`。不在某个网络的页面上时是 `null`。 */
export function currentPage(pathname: string): string | null {
  const match = /^\/n\/\d+\/([^/]+)/.exec(pathname)
  return match?.[1] ?? null
}
