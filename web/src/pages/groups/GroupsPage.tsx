import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, EllipsisVertical, Pencil, Plus, Trash2, X } from 'lucide-react'
import { Fragment, useState } from 'react'

import { errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { fieldError, formError, formErrorDetail, groupNameFailure } from '@/api/formErrors'
import { q, useAction } from '@/api/queries'
import { type Group, type GroupId, type Network, type Node, type NodeRef } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink, NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { ErrorState, SkeletonRows } from '@/components/states'
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
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { toast } from '@/components/ui/toast'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { FOCUSED_ROW, useFocusRow } from '@/lib/useFocusRow'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { validateGroupName } from '@/lib/validate'
import { useNetwork } from '@/pages/networks/context'

const g = t.groups

/** 表格的列数：名字、节点、规则、操作。成员那一行横跨全部。 */
const COLUMNS = 4

interface NameDialogProps {
  network: Network
  /** 要改名的分组；新建时是 `null`。 */
  group: Group | null
  onClose: () => void
  onSaved: (group: Group) => void
}

/** 新建分组和改名是同一个表单：只有一个名字。改名不改变谁能访问谁，规则记的是编号。 */
function NameDialog({ network, group, onClose, onSaved }: NameDialogProps) {
  const [name, setName] = useState(group?.name ?? '')
  const [touched, setTouched] = useState(false)
  const save = useAction(network.id, {
    mutationFn: () =>
      group === null
        ? api.createGroup(network.id, { name: name.trim() })
        : api.updateGroup(network.id, group.id, { name: name.trim() }),
    success: (saved) => (group === null ? g.created(saved.name) : g.renamed(group.name, saved.name)),
    toastError: false,
  })
  const failure = groupNameFailure(save.error)
  const problem = validateGroupName(name)

  return (
    <FormDialog
      open
      onClose={onClose}
      title={group === null ? g.createTitle : g.renameTitle(group.name)}
      size="sm"
      dirty={group === null ? name !== '' : name.trim() !== group.name}
      requireDirty={group !== null}
      submitLabel={group === null ? g.createSubmit : g.renameSubmit}
      pending={save.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
      onSubmit={() => {
        setTouched(true)
        if (problem === null) save.mutate(undefined, { onSuccess: onSaved })
      }}
    >
      <Field label={g.nameLabel} error={(touched ? problem : null) ?? fieldError(failure, 'name')}>
        {(props) => (
          <Input
            {...props}
            name="name"
            value={name}
            autoFocus
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              setName(event.target.value)
              save.reset()
            }}
          />
        )}
      </Field>
    </FormDialog>
  )
}

interface MembersProps {
  network: Network
  group: Group
  nodes: readonly Node[]
}

/** 一个分组的成员：每个一枚，叉号移除；下面选一个节点添加。`All` 的成员是全部节点，只能看。 */
function Members({ network, group, nodes }: MembersProps) {
  const add = useAction(network.id, {
    mutationFn: (node: Node) => api.addGroupMember(network.id, group.id, node.id),
    success: (_data, node) => g.added(node.name, group.name),
  })
  const remove = useAction(network.id, {
    mutationFn: (member: NodeRef) => api.removeGroupMember(network.id, group.id, member.id),
    success: (_data, member) => g.removed(member.name, group.name),
  })
  const pending = add.isPending || remove.isPending
  const members = new Set(group.nodes.map((member) => member.id))
  const candidates = nodes.filter((node) => !members.has(node.id))

  return (
    <div className="space-y-2.5" data-testid={`group-members-${group.name}`}>
      {group.builtin ? <p className="text-[13px] text-muted-foreground">{g.allMembers}</p> : null}
      {group.nodes.length === 0 ? (
        <p className="text-[13px] text-muted-foreground">{g.noMembers}</p>
      ) : (
        <ul aria-label={g.membersOf(group.name)} className="flex flex-wrap gap-1.5">
          {group.nodes.map((member) => (
            <li
              key={member.id}
              className={cn(
                'inline-flex h-7 items-center gap-1 rounded-md bg-muted pl-2.5 text-[13px]',
                group.builtin ? 'pr-2.5' : 'pr-1',
              )}
            >
              <NodeLink
                network={network.id}
                node={member}
                online={nodes.find((node) => node.id === member.id)?.online}
                className="font-normal"
              />
              {group.builtin ? null : (
                <button
                  type="button"
                  disabled={pending}
                  onClick={() => remove.mutate(member)}
                  aria-label={g.removeMember(member.name, group.name)}
                  className="flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
                >
                  <X className="size-3.5" aria-hidden />
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {group.builtin ? null : (
        <div className="w-64">
          <NodeSelect
            nodes={candidates}
            value={null}
            onChange={(id) => {
              const node = nodes.find((candidate) => candidate.id === id)
              if (node !== undefined) add.mutate(node)
            }}
            placeholder={candidates.length === 0 ? g.noCandidates : g.addMember}
            disabled={candidates.length === 0 || pending}
            aria-label={g.addMemberTo(group.name)}
          />
        </div>
      )}
    </div>
  )
}

/**
 * 分组：一组节点，访问规则写在分组之间。每个网络都有内置的 `All`（全部节点），排在最前面。
 * 一行一个分组：有几个节点、被几条规则用到；展开看成员、增减成员；菜单里改名、删除。
 * 删除之前现取这个分组，说出它会从几个节点上移除、几条规则一起删掉。
 */
export function GroupsPage() {
  useDocumentTitle(g.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const client = useQueryClient()
  const groups = useQuery(q.groups(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const focus = useFocusRow(groups.data)
  // 从操作记录、访问控制页点过来的那个分组，一开始就展开成员。
  const [open, setOpen] = useState<GroupId | null>(focus.focus)
  const [editing, setEditing] = useState<{ group: Group | null } | null>(null)

  const remove = useAction(network.id, {
    mutationFn: (group: Group) => api.deleteGroup(network.id, group.id),
    success: (deleted) => g.deleted(deleted.name),
  })

  const onDelete = async (listed: Group) => {
    // 后果取自这个分组自己的应答，不用列表里那一份：列表可能是几秒之前的。
    let group: Group
    try {
      group = await client.query(q.group(network.id, listed.id))
    } catch (error) {
      toast.error(errorMessage(error), undefined, errorDetail(error))
      return
    }
    const confirmed = await confirm({
      tone: 'danger',
      title: g.deleteTitle(group.name),
      body: g.deleteImpact(group.nodes.length, group.rules.length) ?? undefined,
      confirmLabel: g.deleteConfirm,
    })
    if (confirmed) remove.mutate(group)
  }

  const list = groups.data ?? []
  const all = nodes.data ?? []

  return (
    <Page>
      <PageHeader
        title={g.title}
        description={g.subtitle}
        actions={
          <Button variant="primary" onClick={() => setEditing({ group: null })}>
            <Plus aria-hidden />
            {g.create}
          </Button>
        }
      />
      {focus.missing ? (
        <Notice className="mb-4" tone="warn">
          {g.focusMissing(focus.focus ?? 0)}
        </Notice>
      ) : null}
      {groups.isError ? (
        <ErrorState title={g.loadFailed} error={groups.error} onRetry={() => void groups.refetch()} />
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{g.colName}</Th>
              <Th numeric>{g.colNodes}</Th>
              <Th numeric>{g.colRules}</Th>
              <Th className="w-40">
                <span className="sr-only">{t.common.moreActions}</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {groups.isPending ? <SkeletonRows columns={COLUMNS} rows={3} /> : null}
            {list.map((group) => {
              const expanded = open === group.id
              const focused = focus.focus === group.id
              return (
                <Fragment key={group.id}>
                  <Tr
                    ref={focus.rowRef(group.id)}
                    className={cn(expanded && 'border-b-0', focused && FOCUSED_ROW)}
                    data-testid={`group-row-${group.name}`}
                  >
                    <Td>
                      <span className="inline-flex items-center gap-2">
                        <span className="font-medium">{group.name}</span>
                        {group.builtin ? (
                          <Tooltip content={g.builtinTip}>
                            <Badge tone="outline" tabIndex={0}>
                              {g.builtin}
                            </Badge>
                          </Tooltip>
                        ) : null}
                      </span>
                    </Td>
                    <Td numeric>{group.nodes.length}</Td>
                    <Td numeric>{group.rules.length}</Td>
                    <Td className="pl-0">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-muted-foreground"
                          aria-expanded={expanded}
                          aria-label={expanded ? g.membersHide : g.membersOf(group.name)}
                          onClick={() => setOpen(expanded ? null : group.id)}
                        >
                          {g.members}
                          <ChevronDown className={cn('transition-transform', expanded && 'rotate-180')} aria-hidden />
                        </Button>
                        {group.builtin ? (
                          // 和别的行的"⋮"一样宽，"成员"按钮上下对齐。
                          <span className="size-7 shrink-0" aria-hidden />
                        ) : (
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(group.name)}>
                                <EllipsisVertical aria-hidden />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent>
                              <DropdownMenuItem onSelect={() => setEditing({ group })}>
                                <Pencil aria-hidden />
                                {g.menuRename}
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem danger onSelect={() => void onDelete(group)}>
                                <Trash2 aria-hidden />
                                {g.menuDelete}
                              </DropdownMenuItem>
                            </DropdownMenuContent>
                          </DropdownMenu>
                        )}
                      </div>
                    </Td>
                  </Tr>
                  {expanded ? (
                    <tr className={cn('border-b last:border-b-0', focused && FOCUSED_ROW)}>
                      <td colSpan={COLUMNS} className="px-3 pt-0 pb-4">
                        <Members network={network} group={group} nodes={all} />
                      </td>
                    </tr>
                  ) : null}
                </Fragment>
              )
            })}
          </tbody>
        </Table>
      )}

      {editing !== null ? (
        <NameDialog
          key={editing.group?.id ?? 'new'}
          network={network}
          group={editing.group}
          onClose={() => setEditing(null)}
          onSaved={(saved) => {
            setEditing(null)
            // 新建的分组还没有成员：展开它，接着就能添加。
            if (editing.group === null) setOpen(saved.id)
          }}
        />
      ) : null}
    </Page>
  )
}
