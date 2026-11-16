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
import { plannedEntry, type PlannedId } from '@/pages/planned/planned'

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

/** 名字用取值函数（getter）写：每次渲染现读，换了语言就是新的名字。 */
const item = (page: string, label: () => string, icon: LucideIcon): NavItem => ({
  page,
  get label() {
    return label()
  },
  icon,
})

const planned = (id: PlannedId): NavItem => ({
  page: id,
  get label() {
    return plannedEntry(id).title
  },
  icon: plannedEntry(id).icon,
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
      item('overview', () => t.nav.overview, LayoutDashboard),
      item('nodes', () => t.nav.nodes, Server),
      item('access', () => t.nav.access, ShieldCheck),
    ],
  },
  {
    id: 'access',
    get label() {
      return t.nav.groupAccess
    },
    items: [
      item('exits', () => t.nav.exits, DoorOpen),
      item('listeners', () => t.nav.listeners, RadioTower),
      item('credentials', () => t.nav.credentials, KeyRound),
      item('forwards', () => t.nav.forwards, ArrowLeftRight),
    ],
  },
  {
    id: 'diagnose',
    get label() {
      return t.nav.groupDiagnose
    },
    items: [
      item('topology', () => t.nav.topology, Waypoints),
      item('diagnosis', () => t.nav.diagnosis, Route),
      planned('tests'),
    ],
  },
]

/** 底部的管理区。"设置"是全局的，其余属于当前网络。 */
export const NAV_ADMIN: readonly NavItem[] = [
  item('tokens', () => t.nav.tokens, Ticket),
  item('pairs', () => t.nav.pairs, Link2),
  item('network', () => t.nav.network, Network),
]

export const NAV_SETTINGS = {
  get label() {
    return t.nav.settings
  },
  icon: Settings,
  to: '/settings',
} as const

/** 从地址里取出当前页面那一段：`/n/3/nodes/7` 是 `nodes`。不在某个网络的页面上时是 `null`。 */
export function currentPage(pathname: string): string | null {
  const match = /^\/n\/\d+\/([^/]+)/.exec(pathname)
  return match?.[1] ?? null
}
