import { type Group, type GroupId, type GroupRef } from '@/api/types'

/**
 * 分组的几件小事。
 *
 * 内置分组 `All`：每个网络都有，每个节点都在里面，不能改名、删除、增减成员；节点和注册令牌的 `groups` 里不列出它。
 */
export const ALL_GROUP = 'All'

/**
 * 是不是内置的 `All`：看 controller 给的 `builtin`。旧的 controller 不给，这时按名字认
 * （名字在一个网络里不分大小写唯一，`all` 被它占着）。
 */
export function isAllGroup(group: Pick<GroupRef, 'name' | 'builtin'>): boolean {
  return group.builtin ?? group.name === ALL_GROUP
}

/** 网络里 `All` 的编号：分组列表、关系图里 `builtin` 的那一个。数据还没取到时是 `undefined`。 */
export function allGroupId(groups: readonly Pick<Group, 'id' | 'builtin'>[] | undefined): GroupId | undefined {
  return groups?.find((group) => group.builtin)?.id
}

/** 节点所在的分组的编号：它自己的，加上 `All`。 */
export function nodeGroupIds(
  node: { groups: readonly GroupRef[] },
  groups: readonly Pick<Group, 'id' | 'builtin'>[] | undefined,
): Set<GroupId> {
  const all = allGroupId(groups)
  return new Set([...node.groups.map((group) => group.id), ...(all === undefined ? [] : [all])])
}

/** 按名字排，不分大小写：和 controller 列分组的顺序一样。 */
export function byGroupName(a: Pick<GroupRef, 'name'>, b: Pick<GroupRef, 'name'>): number {
  return a.name.localeCompare(b.name, undefined, { sensitivity: 'base', numeric: true })
}

/** 只要编号和名字：选分组的控件、节点上的分组都用这个形状。 */
export function groupRef(group: Pick<Group, 'id' | 'name' | 'builtin'>): GroupRef {
  return { id: group.id, name: group.name, builtin: group.builtin }
}
