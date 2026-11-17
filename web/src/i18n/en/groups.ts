import type { Messages } from '../messages'

const count = (n: number, one: string, many: string) => (n === 1 ? `1 ${one}` : `${n} ${many}`)

export const groups: Messages['groups'] = {
  title: 'Groups',
  subtitle: 'Access policies apply between groups',
  create: 'Create group',
  loadFailed: "Couldn't load groups",
  colName: 'Name',
  colNodes: 'Nodes',
  colRules: 'Policies',
  searchPlaceholder: 'Search groups',
  noMatch: 'No matching groups',
  nodesCount: (n) => (n === 1 ? '1 node' : `${n} nodes`),
  rulesCount: (n) => (n === 1 ? '1 policy' : `${n} policies`),
  builtin: 'Built-in',
  builtinTip: "Contains every node; can't be renamed or deleted",
  members: 'Members',
  membersOf: (name) => `Members of ${name}`,
  membersHide: 'Hide members',
  allMembers: 'Every node is in All',
  noMembers: 'No nodes',
  addMember: 'Add node',
  addMemberTo: (name) => `Add node to ${name}`,
  noCandidates: 'Every node is in this group',
  removeMember: (node, group) => `Remove ${node} from ${group}`,
  added: (node, group) => `${node} added to ${group}`,
  removed: (node, group) => `${node} removed from ${group}`,
  menuRename: 'Rename…',
  menuDelete: 'Delete…',
  nameLabel: 'Name',
  createTitle: 'Create group',
  createSubmit: 'Create group',
  created: (name) => `Group ${name} created`,
  renameTitle: (name) => `Rename ${name}`,
  renameSubmit: 'Rename',
  renamed: (from, to) => `Group ${from} renamed to ${to}`,
  deleteTitle: (name) => `Delete group ${name}?`,
  deleteImpact: (nodes, rules) => {
    const parts = [
      nodes > 0 ? `It is removed from ${count(nodes, 'node', 'nodes')}.` : null,
      rules > 0
        ? `${count(rules, 'rule that uses it is', 'rules that use it are')} deleted, and the access they grant ends.`
        : null,
    ].filter((part): part is string => part !== null)
    return parts.length === 0 ? null : parts.join(' ')
  },
  deleteConfirm: 'Delete group',
  deleted: (name) => `Group ${name} deleted`,
  focusMissing: (id) => `Group #${id} has been deleted`,

  picker: {
    pickOne: 'Select a group',
    search: 'Search groups',
    options: 'Groups',
    remove: (group) => `Remove group ${group}`,
    allTip: "Every node is in All; it can't be removed",
    noneFound: 'No matching groups',
    noGroups: 'No other groups yet',
    nodes: (n) => count(n, 'node', 'nodes'),
    more: (n, list) => `${n} more: ${list}`,
    manage: 'Manage groups',
  },
}
