import { X } from 'lucide-react'
import { type KeyboardEvent, useId, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router'

import { type Group, type GroupRef, type NetworkId } from '@/api/types'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'
import { ALL_GROUP, byGroupName, isAllGroup } from '@/lib/groups'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

interface GroupChipProps {
  name: string
  /** 给了就在名字右边出现一个移除的叉。 */
  onRemove?: () => void
  muted?: boolean
  className?: string
}

/** 分组的样子，全站一致：浅灰底的小字。内置的 `All` 是空心的细框，表示它不是管理员加上去的。 */
export function GroupChip({ name, onRemove, muted = false, className }: GroupChipProps) {
  const builtin = isAllGroup({ name })
  return (
    <span
      className={cn(
        'inline-flex h-5.5 max-w-44 items-center gap-0.5 rounded-sm border px-1.5 text-xs whitespace-nowrap',
        builtin ? 'border-border-strong bg-card text-foreground' : 'border-transparent bg-muted text-foreground',
        muted && 'opacity-60',
        className,
      )}
    >
      <span className="truncate">{name}</span>
      {onRemove !== undefined ? (
        <button
          type="button"
          onClick={onRemove}
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
        <GroupChip key={group.id} name={group.name} />
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
  groups: readonly Pick<Group, 'id' | 'name' | 'builtin'>[]
  disabled?: boolean
  autoFocus?: boolean
  'aria-label'?: string
  'aria-describedby'?: string
}

/**
 * 选分组：只能从已有的分组里多选，输入只是筛选，不会新建。`All` 总在最前面，移不掉。
 * 键盘：上下键选择，回车确认，退格移除最后一个，Esc 收起列表。
 */
export function GroupPicker({
  id,
  value,
  onChange,
  groups,
  disabled = false,
  autoFocus = false,
  ...aria
}: GroupPickerProps) {
  const [text, setText] = useState('')
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const wrapperRef = useRef<HTMLDivElement>(null)
  const listId = useId()

  const chosen = useMemo(() => new Set(value.map((group) => group.id)), [value])
  const others = useMemo(() => groups.filter((group) => !group.builtin), [groups])
  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return others.filter((group) => !chosen.has(group.id) && group.name.toLowerCase().includes(needle))
  }, [others, chosen, text])

  const add = (group: GroupRef) => {
    if (!chosen.has(group.id)) onChange([...value, { id: group.id, name: group.name }].sort(byGroupName))
    setText('')
    setActive(0)
  }

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' && rows.length > 0) {
      event.preventDefault()
      // 列表收着时第一下只是打开它，停在第一项。
      if (!open) {
        setOpen(true)
        setActive(0)
      } else setActive((index) => (index + 1) % rows.length)
    } else if (event.key === 'ArrowUp' && rows.length > 0) {
      event.preventDefault()
      setActive((index) => (index - 1 + rows.length) % rows.length)
    } else if (event.key === 'Enter') {
      const row = open ? rows[active] : undefined
      if (row === undefined) return
      event.preventDefault()
      add(row)
    } else if (event.key === 'Backspace' && text === '' && value.length > 0) {
      onChange(value.slice(0, -1))
    } else if (event.key === 'Escape' && open) {
      // 只关掉下拉，不要连带把外面的对话框也关了。
      event.stopPropagation()
      setOpen(false)
    }
  }

  const listed = open && !disabled
  const empty = others.length === 0 ? t.groups.picker.noGroups : t.groups.picker.noneFound

  return (
    <Popover open={listed} onOpenChange={setOpen}>
      <PopoverAnchor asChild>
        <div
          ref={wrapperRef}
          onClick={() => inputRef.current?.focus()}
          className={cn(
            'flex min-h-9 w-full cursor-text flex-wrap items-center gap-1 rounded-md border border-input bg-card px-2 py-1.5 transition-colors hover:border-border-strong focus-within:border-primary focus-within:outline-3 focus-within:outline-primary/20',
            disabled && 'cursor-not-allowed bg-muted',
          )}
        >
          <Tooltip content={t.groups.picker.allTip}>
            <span tabIndex={0} className="rounded-sm">
              <GroupChip name={ALL_GROUP} />
            </span>
          </Tooltip>
          {value.map((group) => (
            <GroupChip
              key={group.id}
              name={group.name}
              onRemove={disabled ? undefined : () => onChange(value.filter((other) => other.id !== group.id))}
            />
          ))}
          <input
            ref={inputRef}
            id={id}
            role="combobox"
            aria-expanded={listed}
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={listed && rows.length > 0 ? `${listId}-${active}` : undefined}
            aria-label={aria['aria-label']}
            aria-describedby={aria['aria-describedby']}
            value={text}
            disabled={disabled}
            autoFocus={autoFocus}
            autoComplete="off"
            spellCheck={false}
            placeholder={value.length === 0 ? t.groups.picker.placeholder : ''}
            onChange={(event) => {
              setText(event.target.value)
              setActive(0)
              setOpen(true)
            }}
            // 离开时丢掉没选中的筛选文字：这里不会因为输入而新建分组。
            onBlur={() => setText('')}
            // 获得焦点时不打开列表：对话框一打开就聚焦在这里，列表会盖住下面的按钮。点它、输入或按向下键才打开。
            onClick={() => setOpen(true)}
            onKeyDown={onKeyDown}
            className="h-5.5 min-w-32 flex-1 bg-transparent px-1 text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>
      </PopoverAnchor>
      <PopoverContent
        onOpenAutoFocus={(event) => event.preventDefault()}
        onCloseAutoFocus={(event) => event.preventDefault()}
        onInteractOutside={(event) => {
          if (event.target instanceof Node && wrapperRef.current?.contains(event.target) === true)
            event.preventDefault()
        }}
        className="w-(--radix-popover-trigger-width) min-w-56 p-1"
      >
        <ul id={listId} role="listbox" aria-label={t.groups.picker.options} className="max-h-56 overflow-y-auto">
          {rows.length === 0 ? (
            <li className="px-2.5 py-3 text-center text-[13px] text-muted-foreground">{empty}</li>
          ) : null}
          {rows.map((group, index) => (
            <li
              key={group.id}
              id={`${listId}-${index}`}
              role="option"
              aria-selected={index === active}
              onMouseEnter={() => setActive(index)}
              // 用 mousedown 而不是 click：不让输入框先失去焦点（那会清掉筛选文字）。
              onMouseDown={(event) => {
                event.preventDefault()
                add(group)
                inputRef.current?.focus()
              }}
              className={cn(
                'flex cursor-pointer items-center gap-2 rounded-md px-2.5 py-1.5 text-sm',
                index === active && 'bg-muted',
              )}
            >
              <GroupChip name={group.name} />
            </li>
          ))}
        </ul>
      </PopoverContent>
    </Popover>
  )
}
