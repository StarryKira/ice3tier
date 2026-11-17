import { Boxes, Check, ChevronsUpDown, Search, Server, X } from 'lucide-react'
import { type KeyboardEvent, useId, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router'

import { type Group, type GroupRef, type NetworkId } from '@/api/types'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'
import { ALL_GROUP, byGroupName, groupRef, isAllGroup } from '@/lib/groups'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

interface GroupChipProps {
  name: string
  /** 是不是内置的 `All`（`GroupRef.builtin`）。没给时按名字认。 */
  builtin?: boolean
  /** 给了就在名字右边出现一个移除的叉。 */
  onRemove?: () => void
  muted?: boolean
  /** `sm` 用在列表和表格里，`md` 用在选分组的控件里。 */
  size?: 'sm' | 'md'
  className?: string
}

const CHIP_SIZE = {
  sm: 'h-5.5 max-w-44 gap-1 rounded-sm px-1.5 text-xs [&>svg]:size-3',
  md: 'h-7 max-w-60 gap-1.5 rounded-md px-2 text-[13px] [&>svg]:size-3.5',
} as const

/**
 * 分组的样子，全站一致：分组的图标加名字，浅灰底。内置的 `All` 是空心的细框，表示它不是管理员加上去的。
 */
export function GroupChip({ name, builtin: flag, onRemove, muted = false, size = 'sm', className }: GroupChipProps) {
  const builtin = isAllGroup({ name, builtin: flag })
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center border whitespace-nowrap',
        CHIP_SIZE[size],
        builtin ? 'border-border-strong bg-card text-foreground' : 'border-transparent bg-muted text-foreground',
        muted && 'opacity-60',
        className,
      )}
    >
      <Boxes className="shrink-0 text-muted-foreground" aria-hidden />
      <span className="truncate">{name}</span>
      {onRemove !== undefined ? (
        <button
          type="button"
          // 叉在选分组的框里面：点它只移除这一个，不打开候选列表。
          onClick={(event) => {
            event.stopPropagation()
            onRemove()
          }}
          aria-label={t.groups.picker.remove(name)}
          className="-mr-1 flex size-4 items-center justify-center rounded-sm text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
        >
          <X className="size-3" aria-hidden />
        </button>
      ) : null}
    </span>
  )
}

/** 一串分组：显示前几个，其余折成"+N"，悬停看全部。 */
export function GroupList({ groups, max = 2, empty }: { groups: readonly GroupRef[]; max?: number; empty?: string }) {
  if (groups.length === 0) return <span className="text-[13px] text-muted-foreground">{empty ?? t.common.none}</span>
  const shown = groups.slice(0, max)
  const rest = groups.slice(max).map((group) => group.name)
  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      {shown.map((group) => (
        <GroupChip key={group.id} name={group.name} builtin={group.builtin} />
      ))}
      {rest.length > 0 ? (
        <Tooltip content={formatList(rest)}>
          <span
            tabIndex={0}
            aria-label={t.groups.picker.more(rest.length, formatList(rest))}
            className="rounded-sm px-1 text-xs text-muted-foreground tabular"
          >
            +{rest.length}
          </span>
        </Tooltip>
      ) : null}
    </span>
  )
}

/** 去分组页的一个安静的链接：选分组的地方只能选已有的，要新建、改名就去那里。 */
export function GroupsLink({ network, onClick }: { network: NetworkId; onClick?: () => void }) {
  return (
    <Link
      to={networkPath(network, 'groups')}
      onClick={onClick}
      className="rounded-sm text-[13px] text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
    >
      {t.groups.picker.manage}
    </Link>
  )
}

interface GroupPickerProps {
  id?: string
  /** 选中的分组，不含 `All`。 */
  value: readonly GroupRef[]
  onChange: (groups: GroupRef[]) => void
  /** 网络里已有的分组（分组列表的应答），从里面选。 */
  groups: readonly Pick<Group, 'id' | 'name' | 'builtin' | 'nodes'>[]
  /** 整个控件不能用：列表收起，框是灰的。 */
  disabled?: boolean
  /** 上一次改动还在保存：列表开着，但先不能勾。 */
  busy?: boolean
  autoFocus?: boolean
  'aria-label'?: string
  'aria-describedby'?: string
}

/** 列表里第一个能勾的（`All` 勾不掉）。 */
const firstPickable = (rows: readonly Pick<Group, 'builtin'>[]): number =>
  Math.max(
    0,
    rows.findIndex((group) => !group.builtin),
  )

/** 列表里的勾选框，只是样子：点的是整行。 */
function CheckMark({ checked, locked }: { checked: boolean; locked: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        'flex size-4 shrink-0 items-center justify-center rounded-[4px] border transition-colors',
        checked ? 'border-primary bg-primary text-primary-foreground' : 'border-faint bg-card',
        locked && 'opacity-50',
      )}
    >
      {checked ? <Check className="size-3" style={{ strokeWidth: 3 }} /> : null}
    </span>
  )
}

/**
 * 选分组：框里是选中的分组，点开是一个能搜索的列表，每行一个分组、它有几个节点、一个勾选框。
 * 只能从已有的分组里选，搜索只是筛选，不会新建。`All` 总在最前面，勾着、勾不掉。
 * 键盘：在框上按向下键或回车打开；列表里上下键移动，回车勾选或取消，Esc 收起。
 */
export function GroupPicker({
  id,
  value,
  onChange,
  groups,
  disabled = false,
  busy = false,
  autoFocus = false,
  ...aria
}: GroupPickerProps) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const fieldRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const listId = useId()

  const chosen = useMemo(() => new Set(value.map((group) => group.id)), [value])
  const filter = (text: string) => {
    const needle = text.trim().toLowerCase()
    return groups.filter((group) => group.name.toLowerCase().includes(needle))
  }
  const rows = filter(query)
  const allName = groups.find((group) => group.builtin)?.name ?? ALL_GROUP
  const onlyAll = groups.every((group) => group.builtin)

  const show = (next: boolean) => {
    if (next) {
      setQuery('')
      setActive(firstPickable(groups))
    }
    setOpen(next)
  }

  const toggle = (group: (typeof groups)[number] | undefined) => {
    if (group === undefined || group.builtin || busy) return
    onChange(
      chosen.has(group.id)
        ? value.filter((other) => other.id !== group.id)
        : [...value, groupRef(group)].sort(byGroupName),
    )
  }

  const onSearchKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (rows.length === 0) return
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActive((index) => (index + 1) % rows.length)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActive((index) => (index - 1 + rows.length) % rows.length)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      toggle(rows[active])
    }
  }

  const listed = open && !disabled

  return (
    <Popover open={listed} onOpenChange={show}>
      <PopoverAnchor asChild>
        <div
          ref={fieldRef}
          onClick={() => {
            if (!disabled) show(!open)
          }}
          className={cn(
            'flex min-h-10 w-full cursor-pointer items-center gap-2 rounded-lg border border-input bg-card py-1.5 pr-1.5 pl-1.5 transition-colors hover:border-border-strong has-[[role=combobox]:focus-visible]:border-primary has-[[role=combobox]:focus-visible]:outline-3 has-[[role=combobox]:focus-visible]:outline-primary/20',
            listed && 'border-primary hover:border-primary',
            disabled && 'cursor-not-allowed bg-muted hover:border-input',
          )}
        >
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
            <GroupChip name={allName} builtin size="md" />
            {value.map((group) => (
              <GroupChip
                key={group.id}
                name={group.name}
                builtin={group.builtin}
                size="md"
                onRemove={disabled || busy ? undefined : () => onChange(value.filter((other) => other.id !== group.id))}
              />
            ))}
          </div>
          {/* 框本身是一块能点的地方；读屏和键盘用的是这个按钮。点它和点框一样（事件冒到框上）。 */}
          <button
            ref={triggerRef}
            type="button"
            id={id}
            role="combobox"
            aria-expanded={listed}
            aria-controls={listId}
            aria-haspopup="listbox"
            aria-label={aria['aria-label']}
            aria-describedby={aria['aria-describedby']}
            disabled={disabled}
            autoFocus={autoFocus}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown' && !open) {
                event.preventDefault()
                show(true)
              }
            }}
            className="flex size-7 shrink-0 items-center justify-center self-start rounded-md text-faint outline-none hover:text-foreground disabled:pointer-events-none"
          >
            <ChevronsUpDown className="size-4" aria-hidden />
          </button>
        </div>
      </PopoverAnchor>
      <PopoverContent
        onCloseAutoFocus={(event) => {
          // 没有别的东西拿到焦点（按了 Esc、点了空白处）时，焦点回到框上。
          event.preventDefault()
          if (document.activeElement === document.body) triggerRef.current?.focus()
        }}
        onInteractOutside={(event) => {
          // 点框本身由框来开关；不拦着的话，这里先关上，框又马上打开。
          if (event.target instanceof Node && fieldRef.current?.contains(event.target) === true) event.preventDefault()
        }}
        className="w-(--radix-popover-trigger-width) min-w-72 p-0"
      >
        <div className="flex items-center gap-2.5 border-b px-3">
          <Search className="size-4 shrink-0 text-faint" aria-hidden />
          <input
            value={query}
            onChange={(event) => {
              setQuery(event.target.value)
              setActive(firstPickable(filter(event.target.value)))
            }}
            onKeyDown={onSearchKey}
            placeholder={t.groups.picker.search}
            aria-label={t.groups.picker.search}
            aria-controls={listId}
            aria-activedescendant={rows.length > 0 ? `${listId}-${active}` : undefined}
            autoComplete="off"
            spellCheck={false}
            className="h-11 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>
        <ul
          id={listId}
          role="listbox"
          aria-multiselectable
          aria-label={t.groups.picker.options}
          className="max-h-72 overflow-y-auto p-1.5"
        >
          {rows.map((group, index) => {
            const checked = group.builtin || chosen.has(group.id)
            return (
              <li
                key={group.id}
                id={`${listId}-${index}`}
                role="option"
                aria-selected={checked}
                aria-disabled={group.builtin || busy}
                title={group.builtin ? t.groups.picker.allTip : undefined}
                onMouseEnter={() => setActive(index)}
                // 不让搜索框失去焦点：键盘接着能用。
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => toggle(group)}
                className={cn(
                  'flex items-center gap-3 rounded-md px-2 py-1.5',
                  group.builtin ? 'cursor-default' : busy ? 'cursor-progress' : 'cursor-pointer',
                  index === active && 'bg-muted',
                )}
              >
                <GroupChip
                  name={group.name}
                  builtin={group.builtin}
                  size="md"
                  // 这一行亮着时底色是灰的，分组换成白底才分得出来。
                  className={cn('min-w-0 shrink', index === active && !group.builtin && 'bg-card')}
                />
                <span className="ml-auto flex shrink-0 items-center gap-1.5 text-[13px] text-muted-foreground tabular">
                  <Server className="size-3.5" aria-hidden />
                  {t.groups.picker.nodes(group.nodes.length)}
                </span>
                <CheckMark checked={checked} locked={group.builtin} />
              </li>
            )
          })}
        </ul>
        {rows.length === 0 || (onlyAll && query.trim() === '') ? (
          <p className="px-3 pb-3 text-center text-[13px] text-muted-foreground">
            {rows.length === 0 ? t.groups.picker.noneFound : t.groups.picker.noGroups}
          </p>
        ) : null}
      </PopoverContent>
    </Popover>
  )
}
