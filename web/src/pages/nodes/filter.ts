import { type GroupId, type Node } from '@/api/types'
import { parseIpv4 } from '@/lib/cidr'

/**
 * 节点列表的筛选和排序。筛选条件和搜索词写在 URL 的查询参数里：刷新、前进后退、分享链接都保留，
 * 下次打开不会留着一个被筛过的表格让人以为节点丢了。只有排序方式记在浏览器本地。
 */
export type StatusFilter = 'all' | 'online' | 'offline'
export type RoleFilter = 'relay' | 'exit'

export interface NodeFilters {
  search: string
  status: StatusFilter
  /** 分组的编号：改名之后链接还是同一个分组。 */
  groups: GroupId[]
  roles: RoleFilter[]
  platforms: string[]
}

const STATUSES: readonly StatusFilter[] = ['all', 'online', 'offline']
const ROLES: readonly RoleFilter[] = ['relay', 'exit']

export function parseFilters(params: URLSearchParams): NodeFilters {
  const status = params.get('status') as StatusFilter | null
  return {
    search: params.get('q') ?? '',
    status: status !== null && STATUSES.includes(status) ? status : 'all',
    groups: params
      .getAll('group')
      .map(Number)
      .filter((id) => Number.isInteger(id) && id > 0),
    roles: params.getAll('role').filter((role): role is RoleFilter => (ROLES as readonly string[]).includes(role)),
    platforms: params.getAll('platform'),
  }
}

export function writeFilters(filters: NodeFilters): URLSearchParams {
  const params = new URLSearchParams()
  if (filters.search.trim() !== '') params.set('q', filters.search.trim())
  if (filters.status !== 'all') params.set('status', filters.status)
  for (const group of filters.groups) params.append('group', String(group))
  for (const role of filters.roles) params.append('role', role)
  for (const platform of filters.platforms) params.append('platform', platform)
  return params
}

export function hasActiveFilters(filters: NodeFilters): boolean {
  return (
    filters.search.trim() !== '' ||
    filters.status !== 'all' ||
    filters.groups.length > 0 ||
    filters.roles.length > 0 ||
    filters.platforms.length > 0
  )
}

/** 搜索匹配名字、全名、虚拟 IP 和分组的名字。 */
function matchesSearch(node: Node, search: string): boolean {
  const needle = search.trim().toLowerCase()
  if (needle === '') return true
  return [node.name, node.fqdn, node.virtual_ip, ...node.groups.map((group) => group.name)].some((value) =>
    value.toLowerCase().includes(needle),
  )
}

/** 条件之间是"并且"：每加一个条件，结果只会更少。同一类里选了几个平台则是"其中之一"，因为一个节点只有一个平台。 */
export function matchesFilters(node: Node, filters: NodeFilters): boolean {
  if (!matchesSearch(node, filters.search)) return false
  if (filters.status === 'online' && !node.online) return false
  if (filters.status === 'offline' && node.online) return false
  if (!filters.groups.every((id) => node.groups.some((group) => group.id === id))) return false
  if (filters.roles.includes('relay') && node.relay === null) return false
  if (filters.roles.includes('exit') && node.exit === null) return false
  if (filters.platforms.length > 0 && !filters.platforms.includes(node.platform)) return false
  return true
}

export type SortKey = 'name' | 'ip' | 'seen' | 'version'
export interface Sort {
  key: SortKey
  direction: 'asc' | 'desc'
}

/** 默认按名字排，不按"最后在线"：状态实时更新时，行不应该在鼠标底下跳动。 */
export const DEFAULT_SORT: Sort = { key: 'name', direction: 'asc' }

export function isSort(value: unknown): value is Sort {
  if (typeof value !== 'object' || value === null) return false
  const { key, direction } = value as Partial<Sort>
  return (
    (key === 'name' || key === 'ip' || key === 'seen' || key === 'version') &&
    (direction === 'asc' || direction === 'desc')
  )
}

function compare(a: Node, b: Node, key: SortKey): number {
  switch (key) {
    case 'name':
      return a.name.localeCompare(b.name, undefined, { numeric: true })
    case 'ip':
      return (parseIpv4(a.virtual_ip) ?? 0) - (parseIpv4(b.virtual_ip) ?? 0)
    case 'seen':
      // 在线的排在最前，然后按最后在线的时间从近到远。
      return Number(b.online) - Number(a.online) || (b.last_seen_at ?? 0) - (a.last_seen_at ?? 0)
    case 'version':
      return a.version.localeCompare(b.version, undefined, { numeric: true })
  }
}

export function sortNodes(nodes: readonly Node[], sort: Sort): Node[] {
  const sign = sort.direction === 'asc' ? 1 : -1
  // 排序键相同的按编号排，保证顺序稳定：轮询回来的数据不会让行换位置。
  return [...nodes].sort((a, b) => sign * compare(a, b, sort.key) || a.id - b.id)
}
