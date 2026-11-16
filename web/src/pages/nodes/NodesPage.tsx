import { useQuery } from '@tanstack/react-query'
import {
  CircleArrowUp,
  EllipsisVertical,
  ListFilter,
  Pencil,
  Plus,
  Search,
  Server,
  Tags,
  Trash2,
  X,
} from 'lucide-react'
import { type MouseEvent, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router'

import { errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CopyText } from '@/components/copy'
import { ForceUpdateDialog } from '@/components/force-update'
import { FormDialog } from '@/components/form-dialog'
import { attachedCount, nodePath } from '@/lib/nodes'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, NoMatches, SkeletonRows } from '@/components/states'
import { Flag } from '@/components/flag'
import { IssueIcon, RelativeTime, StatusDot } from '@/components/status'
import { SortTh, Table, TableFooter, Td, Th, Toolbar, Tr } from '@/components/table'
import { TagInput, TagList } from '@/components/tags'
import { VersionCell } from '@/components/update'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Field } from '@/components/ui/field'
import { fieldBase } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Segmented } from '@/components/ui/segmented'
import { Switch } from '@/components/ui/switch'
import { toast } from '@/components/ui/toast'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { nodeIssues } from '@/lib/nodeIssues'
import { forceAbility } from '@/lib/nodeUpdate'
import { formatList } from '@/lib/format'
import { readStored, writeStored } from '@/lib/storage'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'

import { NodeTagsDialog, RenameNodeDialog } from './actions'
import { useDeleteNode, useKnownTags } from './hooks'
import { AddNodeDialog } from './AddNodeDialog'
import {
  DEFAULT_SORT,
  hasActiveFilters,
  isSort,
  matchesFilters,
  type NodeFilters,
  parseFilters,
  type RoleFilter,
  type Sort,
  type SortKey,
  sortNodes,
  type StatusFilter,
  writeFilters,
} from './filter'

const PAGE_SIZE = 25
const SORT_KEY = 'nodesSort'
/** 输入停顿这么久之后搜索才生效。 */
const SEARCH_DEBOUNCE_MS = 300

const roleLabel = (role: RoleFilter): string => (role === 'relay' ? t.nodes.roleRelay : t.nodes.roleExit)

/** 角色的小徽标，跟在名字后面。`attached` 给了时，relay 的徽标上写着有几个节点挂靠在它上面，悬停有一句话。 */
export function RoleBadges({ node, attached }: { node: Node; attached?: number }) {
  return (
    <>
      {node.relay !== null ? (
        <Tooltip content={attached === undefined ? null : t.nodes.roleRelayAttached(attached)}>
          <Badge tabIndex={attached === undefined ? undefined : 0} data-testid={`relay-badge-${node.name}`}>
            {t.nodes.roleRelay}
            {attached !== undefined && attached > 0 ? <span className="tabular opacity-80">· {attached}</span> : null}
          </Badge>
        </Tooltip>
      ) : null}
      {node.exit !== null ? <Badge>{t.nodes.roleExit}</Badge> : null}
    </>
  )
}

type RowDialog = { kind: 'rename' | 'tags' | 'update'; node: Node } | null

/**
 * 菜单里的"立即更新"。没有新版本时不出现；有新版本但现在不能点时置灰，原因写在下面一行
 * （菜单项不能悬停出说明，所以直接写出来）。
 */
function UpdateMenuItem({ node, onSelect }: { node: Node; onSelect: () => void }) {
  if (node.update.available === null) return null
  const ability = forceAbility(node)
  return (
    <DropdownMenuItem disabled={!ability.ok} onSelect={onSelect}>
      <CircleArrowUp aria-hidden />
      <span>
        {t.updates.force.menu}
        {ability.ok ? null : <span className="block text-xs text-muted-foreground">{ability.short}</span>}
      </span>
    </DropdownMenuItem>
  )
}

interface FilterToggleProps {
  label: string
  checked: boolean
  onChange: (checked: boolean) => void
}

function FilterToggle({ label, checked, onChange }: FilterToggleProps) {
  return (
    <label className="flex items-center gap-2 rounded-md px-1.5 py-1 text-sm hover:bg-muted">
      <Checkbox checked={checked} onCheckedChange={(value) => onChange(value === true)} />
      <span className="truncate">{label}</span>
    </label>
  )
}

function toggled<T>(list: readonly T[], item: T, on: boolean): T[] {
  return on ? [...list.filter((other) => other !== item), item] : list.filter((other) => other !== item)
}

interface BatchTagDialogProps {
  network: Network
  nodes: readonly Node[]
  open: boolean
  onClose: (done: boolean) => void
}

/** 批量打标签。默认是追加：原有的标签保留；选了覆盖，提交前要确认一次。 */
function BatchTagDialog({ network, nodes, open, onClose }: BatchTagDialogProps) {
  const confirm = useConfirm()
  const known = useKnownTags(network)
  const [tags, setTags] = useState<string[]>([])
  const [overwrite, setOverwrite] = useState(false)

  const apply = useAction(network.id, {
    mutationFn: async () => {
      let ok = 0
      const failures: string[] = []
      for (const node of nodes) {
        const next = overwrite ? tags : [...new Set([...node.tags, ...tags])].sort()
        try {
          await api.updateNode(network.id, node.id, { tags: next })
          ok += 1
        } catch (error) {
          failures.push(errorMessage(error))
        }
      }
      return { ok, failures }
    },
  })

  const submit = async () => {
    if (overwrite) {
      const confirmed = await confirm({
        title: t.nodes.batchOverwriteTitle(nodes.length),
        body: t.nodes.batchOverwriteBody,
        confirmLabel: t.nodes.batchOverwriteConfirm,
      })
      if (!confirmed) return
    }
    const { ok, failures } = await apply.mutateAsync()
    if (failures.length === 0) toast.success(t.nodes.batchTagged(ok))
    else toast.error(t.nodes.batchTagFailed(ok, failures.length, failures[0] ?? ''))
    onClose(true)
  }

  return (
    <FormDialog
      open={open}
      onClose={() => onClose(false)}
      title={t.nodes.batchTagTitle(nodes.length)}
      size="sm"
      dirty={tags.length > 0}
      submitLabel={t.nodes.batchTagSubmit}
      pending={apply.isPending}
      canSubmit={tags.length > 0 || overwrite}
      onSubmit={() => void submit()}
    >
      <Field label={t.nodes.batchTagLabel}>
        {(props) => <TagInput {...props} value={tags} onChange={setTags} suggestions={known} autoFocus />}
      </Field>
      <label className="flex items-center gap-2.5 text-sm">
        <Switch checked={overwrite} onCheckedChange={setOverwrite} />
        {t.nodes.batchOverwrite}
      </label>
    </FormDialog>
  )
}

export function NodesPage() {
  useDocumentTitle(t.nodes.title)
  const network = useNetwork()
  const navigate = useNavigate()
  const confirm = useConfirm()
  const nodes = useQuery(q.nodes(network.id))
  const knownTags = useKnownTags(network)
  const deleteNode = useDeleteNode(network)

  const [params, setParams] = useSearchParams()
  const filters = useMemo(() => parseFilters(params), [params])
  const page = Math.max(1, Number(params.get('page') ?? '1') || 1)

  const [sort, setSort] = useState<Sort>(() => readStored(SORT_KEY, DEFAULT_SORT, isSort))
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set())
  const [adding, setAdding] = useState(false)
  const [dialog, setDialog] = useState<RowDialog>(null)
  const [batchTagging, setBatchTagging] = useState(false)

  // 搜索框自己的内容；停顿一会儿才写进 URL。
  const [searchText, setSearchText] = useState(filters.search)
  const searchRef = useRef<HTMLInputElement>(null)

  /** 改筛选条件：回到第一页，并清空勾选，避免对看不见的行做批量操作。 */
  const applyFilters = (patch: Partial<NodeFilters>) => {
    setParams(writeFilters({ ...filters, ...patch }), { replace: true })
    setSelected(new Set())
  }

  // 搜索是延迟生效的：定时器到点时，别的筛选条件可能已经变了。
  // 所以到点时用的是"最新一次渲染"的 applyFilters，而不是输入那一刻的。
  const applyLatest = useRef(applyFilters)
  useEffect(() => {
    applyLatest.current = applyFilters
  })

  useEffect(() => {
    if (searchText.trim() === filters.search) return undefined
    const timer = window.setTimeout(() => applyLatest.current({ search: searchText }), SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(timer)
    // 只在输入变化时重新计时。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchText])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        searchRef.current?.focus()
        searchRef.current?.select()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const all = useMemo(() => nodes.data ?? [], [nodes.data])
  const issues = useMemo(() => new Map(all.map((node) => [node.id, nodeIssues(node, all)])), [all])
  const platforms = useMemo(
    () => [...new Set(all.map((node) => node.platform))].filter((platform) => platform !== '').sort(),
    [all],
  )
  const tagChoices = useMemo(() => knownTags.filter((tag) => tag !== 'all'), [knownTags])

  const visible = useMemo(
    () =>
      sortNodes(
        all.filter((node) => matchesFilters(node, filters)),
        sort,
      ),
    [all, filters, sort],
  )
  const pages = Math.max(1, Math.ceil(visible.length / PAGE_SIZE))
  const currentPage = Math.min(page, pages)
  const rows = visible.slice((currentPage - 1) * PAGE_SIZE, currentPage * PAGE_SIZE)
  const selectedNodes = all.filter((node) => selected.has(node.id))

  const setPage = (next: number) => {
    const updated = writeFilters(filters)
    if (next > 1) updated.set('page', String(next))
    setParams(updated, { replace: true })
  }

  const onSort = (key: SortKey) => {
    const next: Sort =
      sort.key === key ? { key, direction: sort.direction === 'asc' ? 'desc' : 'asc' } : { key, direction: 'asc' }
    setSort(next)
    writeStored(SORT_KEY, next)
  }

  const clearFilters = () => {
    setSearchText('')
    applyFilters({ search: '', status: 'all', tags: [], roles: [], platforms: [] })
  }

  const batchDelete = useAction(network.id, {
    mutationFn: async (targets: readonly Node[]) => {
      let ok = 0
      const failures: string[] = []
      for (const node of targets) {
        try {
          await api.deleteNode(network.id, node.id)
          ok += 1
        } catch (error) {
          failures.push(errorMessage(error))
        }
      }
      return { ok, failures }
    },
  })

  const onBatchDelete = async () => {
    const confirmed = await confirm({
      tone: 'danger',
      title: t.nodes.batchDeleteTitle(selectedNodes.length),
      body: (
        <>
          <p className="font-medium text-foreground">{formatList(selectedNodes.map((node) => node.name))}</p>
          <p>{t.nodes.batchDeleteBody}</p>
        </>
      ),
      confirmLabel: t.nodes.batchDeleteConfirm,
    })
    if (!confirmed) return
    const { ok, failures } = await batchDelete.mutateAsync(selectedNodes)
    if (failures.length === 0) toast.success(t.nodes.batchDeleted(ok))
    else toast.error(t.nodes.batchDeleteFailed(ok, failures.length, failures[0] ?? ''))
    setSelected(new Set())
  }

  const openRow = (event: MouseEvent<HTMLTableRowElement>, node: Node) => {
    // 行里的控件（复选框、复制、菜单、链接）点了不触发行的跳转。
    if (
      event.target instanceof Element &&
      event.target.closest('a, button, input, [role="checkbox"], [role="menuitem"]') !== null
    )
      return
    void navigate(nodePath(network.id, node.id))
  }

  const pageIds = rows.map((node) => node.id)
  const allOnPageSelected = pageIds.length > 0 && pageIds.every((id) => selected.has(id))
  const someOnPageSelected = pageIds.some((id) => selected.has(id))

  const statusOptions: { value: StatusFilter; label: string }[] = [
    { value: 'all', label: t.nodes.filterAll },
    { value: 'online', label: t.nodes.filterOnline },
    { value: 'offline', label: t.nodes.filterOffline },
  ]

  const facetCount = filters.tags.length + filters.roles.length + filters.platforms.length
  const chips: { label: string; remove: () => void }[] = [
    ...(filters.search.trim() !== ''
      ? [
          {
            label: t.nodes.chipSearch(filters.search),
            remove: () => {
              setSearchText('')
              applyFilters({ search: '' })
            },
          },
        ]
      : []),
    ...filters.tags.map((tag) => ({
      label: t.nodes.chipTag(tag),
      remove: () => applyFilters({ tags: toggled(filters.tags, tag, false) }),
    })),
    ...filters.roles.map((role) => ({
      label: t.nodes.chipRole(roleLabel(role)),
      remove: () => applyFilters({ roles: toggled(filters.roles, role, false) }),
    })),
    ...filters.platforms.map((platform) => ({
      label: t.nodes.chipPlatform(platform),
      remove: () => applyFilters({ platforms: toggled(filters.platforms, platform, false) }),
    })),
  ]

  const empty = nodes.isSuccess && all.length === 0

  return (
    <Page>
      <PageHeader
        title={t.nodes.title}
        description={t.nodes.subtitle}
        actions={
          // 没有任何节点时，页面上只留空状态卡片里那一个入口，不放两个一样的按钮。
          empty ? undefined : (
            <Button variant="primary" onClick={() => setAdding(true)}>
              <Plus aria-hidden />
              {t.nodes.add}
            </Button>
          )
        }
      />

      <div>
        {nodes.isError ? (
          <ErrorState title={t.nodes.loadFailed} error={nodes.error} onRetry={() => void nodes.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={Server}
            title={t.nodes.emptyTitle}
            action={
              <Button variant="primary" onClick={() => setAdding(true)}>
                <Plus aria-hidden />
                {t.nodes.add}
              </Button>
            }
          />
        ) : (
          <>
            <Toolbar>
              <div className="relative w-72">
                <Search
                  className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-faint"
                  aria-hidden
                />
                <input
                  ref={searchRef}
                  type="search"
                  value={searchText}
                  onChange={(event) => setSearchText(event.target.value)}
                  placeholder={t.nodes.searchPlaceholder}
                  aria-label={t.nodes.searchPlaceholder}
                  aria-keyshortcuts="Control+K"
                  autoComplete="off"
                  spellCheck={false}
                  className={cn(fieldBase, 'h-9 pr-14 pl-8.5 [&::-webkit-search-cancel-button]:hidden')}
                />
                <kbd
                  aria-hidden
                  className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded-sm bg-muted px-1 font-sans text-[11px] text-muted-foreground"
                >
                  {t.nodes.searchShortcut}
                </kbd>
              </div>
              <Segmented
                aria-label={t.nodes.filterStatus}
                value={filters.status}
                onChange={(status) => applyFilters({ status })}
                options={statusOptions}
              />
              <Popover>
                <PopoverTrigger asChild>
                  <Button variant="ghost">
                    <ListFilter aria-hidden />
                    {t.nodes.filter}
                    {facetCount > 0 ? (
                      <span className="rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground tabular">
                        {facetCount}
                      </span>
                    ) : null}
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-72 space-y-3 p-3">
                  <fieldset>
                    <legend className="mb-1 px-1.5 text-xs font-medium text-muted-foreground">
                      {t.nodes.filterTags}
                    </legend>
                    {tagChoices.length === 0 ? (
                      <p className="px-1.5 py-1 text-[13px] text-muted-foreground">{t.nodes.filterNoTags}</p>
                    ) : (
                      <div className="max-h-40 overflow-y-auto">
                        {tagChoices.map((tag) => (
                          <FilterToggle
                            key={tag}
                            label={tag}
                            checked={filters.tags.includes(tag)}
                            onChange={(on) => applyFilters({ tags: toggled(filters.tags, tag, on) })}
                          />
                        ))}
                      </div>
                    )}
                  </fieldset>
                  <fieldset>
                    <legend className="mb-1 px-1.5 text-xs font-medium text-muted-foreground">
                      {t.nodes.filterRoles}
                    </legend>
                    {(['relay', 'exit'] as const).map((role) => (
                      <FilterToggle
                        key={role}
                        label={roleLabel(role)}
                        checked={filters.roles.includes(role)}
                        onChange={(on) => applyFilters({ roles: toggled(filters.roles, role, on) })}
                      />
                    ))}
                  </fieldset>
                  {platforms.length > 1 ? (
                    <fieldset>
                      <legend className="mb-1 px-1.5 text-xs font-medium text-muted-foreground">
                        {t.nodes.filterPlatforms}
                      </legend>
                      {platforms.map((platform) => (
                        <FilterToggle
                          key={platform}
                          label={platform}
                          checked={filters.platforms.includes(platform)}
                          onChange={(on) => applyFilters({ platforms: toggled(filters.platforms, platform, on) })}
                        />
                      ))}
                    </fieldset>
                  ) : null}
                </PopoverContent>
              </Popover>
            </Toolbar>

            {chips.length > 0 ? (
              <ul aria-label={t.nodes.activeFilters} className="-mt-1 mb-4 flex flex-wrap items-center gap-1.5">
                {chips.map((chip) => (
                  <li
                    key={chip.label}
                    className="inline-flex h-6 items-center gap-0.5 rounded-full bg-muted pr-1 pl-2.5 text-[13px]"
                  >
                    {chip.label}
                    <button
                      type="button"
                      onClick={chip.remove}
                      aria-label={t.nodes.removeFilter(chip.label)}
                      className="flex size-4.5 items-center justify-center rounded-full text-muted-foreground hover:bg-border hover:text-foreground"
                    >
                      <X className="size-3" aria-hidden />
                    </button>
                  </li>
                ))}
                <li>
                  <Button variant="link" size="sm" onClick={clearFilters}>
                    {t.nodes.clearAll}
                  </Button>
                </li>
              </ul>
            ) : null}

            <Table>
              <thead>
                <tr>
                  <Th className="w-10 pr-0">
                    <Checkbox
                      aria-label={t.nodes.selectAll}
                      checked={allOnPageSelected ? true : someOnPageSelected ? 'indeterminate' : false}
                      onCheckedChange={(checked) =>
                        setSelected((current) => {
                          const next = new Set(current)
                          for (const id of pageIds) {
                            if (checked === true) next.add(id)
                            else next.delete(id)
                          }
                          return next
                        })
                      }
                    />
                  </Th>
                  <SortTh active={sort.key === 'name'} direction={sort.direction} onSort={() => onSort('name')}>
                    {t.nodes.colName}
                  </SortTh>
                  <SortTh active={sort.key === 'ip'} direction={sort.direction} onSort={() => onSort('ip')}>
                    {t.nodes.colAddress}
                  </SortTh>
                  <Th>{t.nodes.colTags}</Th>
                  <SortTh active={sort.key === 'seen'} direction={sort.direction} onSort={() => onSort('seen')}>
                    {t.nodes.colSeen}
                  </SortTh>
                  <SortTh active={sort.key === 'version'} direction={sort.direction} onSort={() => onSort('version')}>
                    {t.nodes.colVersion}
                  </SortTh>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {nodes.isPending ? <SkeletonRows columns={7} /> : null}
                {rows.map((node) => {
                  const issue = issues.get(node.id)?.[0]
                  return (
                    <Tr
                      key={node.id}
                      onClick={(event) => openRow(event, node)}
                      className={cn(
                        // 每行 88 像素高，行与行之间一根 1 像素的线。
                        'h-[88px] cursor-pointer hover:bg-muted/50',
                        selected.has(node.id) && 'bg-accent/60 hover:bg-accent',
                      )}
                      data-testid={`node-row-${node.name}`}
                    >
                      <Td className="pr-0">
                        <Checkbox
                          aria-label={t.nodes.selectOne(node.name)}
                          checked={selected.has(node.id)}
                          onCheckedChange={(checked) =>
                            setSelected((current) => {
                              const next = new Set(current)
                              if (checked === true) next.add(node.id)
                              else next.delete(node.id)
                              return next
                            })
                          }
                        />
                      </Td>
                      <Td>
                        <div className="flex items-center gap-2.5">
                          <Link
                            to={nodePath(network.id, node.id)}
                            className="truncate rounded-sm font-medium hover:underline"
                          >
                            {node.name}
                          </Link>
                          <RoleBadges node={node} attached={attachedCount(all, node.id)} />
                          {issue !== undefined ? <IssueIcon message={issue.message} tone={issue.tone} /> : null}
                        </div>
                      </Td>
                      <Td>
                        <span className="inline-flex items-center gap-1.5">
                          <Flag code={node.country} />
                          <CopyText value={node.virtual_ip} />
                        </span>
                        <div className="text-xs text-muted-foreground">
                          <CopyText value={node.fqdn} mono={false} className="text-xs" />
                        </div>
                      </Td>
                      <Td>
                        <button
                          type="button"
                          onClick={() => setDialog({ kind: 'tags', node })}
                          aria-label={t.nodes.editTagsOf(node.name)}
                          className={cn(
                            '-mx-1.5 rounded-md px-1.5 py-1 text-left hover:bg-muted',
                            node.tags.length === 0 && 'text-[13px] text-muted-foreground',
                          )}
                        >
                          {node.tags.length === 0 ? (
                            <span className="inline-flex items-center gap-1">
                              <Plus className="size-3" aria-hidden />
                              {t.nodes.addTags}
                            </span>
                          ) : (
                            <TagList tags={node.tags} />
                          )}
                        </button>
                      </Td>
                      <Td className="text-[13px] whitespace-nowrap">
                        {/* 在线与否的圆点在这一格的最前面，和"在线""3 分钟前"这句话放在一起。 */}
                        <span className="inline-flex items-center gap-2">
                          <StatusDot tone={node.online ? 'ok' : 'idle'} />
                          {node.online ? (
                            <span>{t.common.online}</span>
                          ) : node.last_seen_at === null ? (
                            <Tooltip content={t.nodes.neverTip}>
                              <span
                                tabIndex={0}
                                className="rounded-sm text-muted-foreground"
                                data-testid="never-connected"
                              >
                                {t.nodes.never}
                              </span>
                            </Tooltip>
                          ) : (
                            <RelativeTime at={node.last_seen_at} className="text-muted-foreground" />
                          )}
                        </span>
                      </Td>
                      <Td>
                        <VersionCell node={node} />
                      </Td>
                      <Td className="pl-0 text-right" data-keep>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(node.name)}>
                              <EllipsisVertical aria-hidden />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent>
                            <DropdownMenuItem onSelect={() => void navigate(nodePath(network.id, node.id))}>
                              <Server aria-hidden />
                              {t.nodes.menuDetails}
                            </DropdownMenuItem>
                            <DropdownMenuItem onSelect={() => setDialog({ kind: 'rename', node })}>
                              <Pencil aria-hidden />
                              {t.nodes.menuRename}
                            </DropdownMenuItem>
                            <DropdownMenuItem onSelect={() => setDialog({ kind: 'tags', node })}>
                              <Tags aria-hidden />
                              {t.nodes.menuTags}
                            </DropdownMenuItem>
                            <UpdateMenuItem node={node} onSelect={() => setDialog({ kind: 'update', node })} />
                            <DropdownMenuSeparator />
                            <DropdownMenuItem danger onSelect={() => void deleteNode(node)}>
                              <Trash2 aria-hidden />
                              {t.nodes.menuDelete}
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </Td>
                    </Tr>
                  )
                })}
              </tbody>
            </Table>

            {nodes.isSuccess && visible.length === 0 ? (
              <NoMatches message={t.nodes.noMatches} onClear={clearFilters} />
            ) : null}

            {visible.length > 0 ? (
              <TableFooter>
                <span className="tabular">
                  {t.common.rowsTotal(visible.length)}
                  {hasActiveFilters(filters) && visible.length !== all.length ? ` / ${all.length}` : ''}
                </span>
                {pages > 1 ? (
                  <div className="flex items-center gap-2">
                    <Button size="sm" disabled={currentPage <= 1} onClick={() => setPage(currentPage - 1)}>
                      {t.common.prevPage}
                    </Button>
                    <span className="tabular">{t.common.pageOf(currentPage, pages)}</span>
                    <Button size="sm" disabled={currentPage >= pages} onClick={() => setPage(currentPage + 1)}>
                      {t.common.nextPage}
                    </Button>
                  </div>
                ) : null}
              </TableFooter>
            ) : null}
          </>
        )}
      </div>

      {selectedNodes.length > 0 ? (
        <div
          role="region"
          aria-label={t.nodes.selected(selectedNodes.length)}
          className="fixed bottom-6 left-1/2 z-30 flex -translate-x-1/2 animate-rise items-center gap-2 rounded-xl border bg-foreground py-2 pr-2 pl-4 text-sm text-background shadow-pop"
        >
          <span className="mr-2 font-medium tabular" aria-live="polite">
            {t.nodes.selected(selectedNodes.length)}
          </span>
          <Button size="sm" onClick={() => setBatchTagging(true)}>
            <Tags aria-hidden />
            {t.nodes.batchTag}
          </Button>
          <Button
            size="sm"
            variant="danger-outline"
            pending={batchDelete.isPending}
            onClick={() => void onBatchDelete()}
          >
            <Trash2 aria-hidden />
            {t.nodes.batchDelete}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="text-background hover:bg-background/15 hover:text-background"
            onClick={() => setSelected(new Set())}
          >
            {t.nodes.batchCancel}
          </Button>
        </div>
      ) : null}

      <AddNodeDialog network={network} open={adding} onClose={() => setAdding(false)} />
      {dialog?.kind === 'rename' ? (
        <RenameNodeDialog
          key={dialog.node.id}
          network={network}
          node={dialog.node}
          open
          onClose={() => setDialog(null)}
        />
      ) : null}
      {dialog?.kind === 'tags' ? (
        <NodeTagsDialog
          key={dialog.node.id}
          network={network}
          // 对话框开着时节点的标签在变：用列表里最新的那一份。
          node={all.find((node) => node.id === dialog.node.id) ?? dialog.node}
          open
          onClose={() => setDialog(null)}
        />
      ) : null}
      {dialog?.kind === 'update' ? (
        <ForceUpdateDialog
          key={dialog.node.id}
          node={all.find((node) => node.id === dialog.node.id) ?? dialog.node}
          onClose={() => setDialog(null)}
        />
      ) : null}
      {batchTagging ? (
        <BatchTagDialog
          network={network}
          nodes={selectedNodes}
          open
          onClose={(done) => {
            setBatchTagging(false)
            if (done) setSelected(new Set())
          }}
        />
      ) : null}
    </Page>
  )
}
