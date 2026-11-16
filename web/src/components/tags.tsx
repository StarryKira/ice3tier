import { Plus, X } from 'lucide-react'
import { type KeyboardEvent, useId, useMemo, useRef, useState } from 'react'

import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { ALL_TAG, normalizeTag, validateTag } from '@/lib/validate'

interface TagChipProps {
  tag: string
  /** 给了就在标签右边出现一个移除的叉。 */
  onRemove?: () => void
  muted?: boolean
  className?: string
}

/** 标签的样子，全站一致：浅灰底的小字。内置的 `all` 是空心的细框，表示它不是管理员打上去的。 */
export function TagChip({ tag, onRemove, muted = false, className }: TagChipProps) {
  const builtin = tag === ALL_TAG
  return (
    <span
      className={cn(
        'inline-flex h-5.5 max-w-44 items-center gap-0.5 rounded-sm border px-1.5 text-xs whitespace-nowrap',
        builtin ? 'border-border-strong bg-card text-foreground' : 'border-transparent bg-muted text-foreground',
        muted && 'opacity-60',
        className,
      )}
    >
      <span className="truncate">{tag}</span>
      {onRemove !== undefined ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label={t.tags.remove(tag)}
          className="-mr-1 flex size-4 items-center justify-center rounded-sm text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
        >
          <X className="size-3" aria-hidden />
        </button>
      ) : null}
    </span>
  )
}

/** 一串标签：显示前几个，其余折成"+N"，悬停看全部。 */
export function TagList({ tags, max = 2, empty }: { tags: readonly string[]; max?: number; empty?: string }) {
  if (tags.length === 0) return <span className="text-[13px] text-muted-foreground">{empty ?? t.common.none}</span>
  const shown = tags.slice(0, max)
  const rest = tags.slice(max)
  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      {shown.map((tag) => (
        <TagChip key={tag} tag={tag} />
      ))}
      {rest.length > 0 ? (
        <Tooltip content={rest.join('、')}>
          <span
            tabIndex={0}
            aria-label={t.tags.more(rest.length, rest.join('、'))}
            className="rounded-sm px-1 text-xs text-muted-foreground tabular"
          >
            +{rest.length}
          </span>
        </Tooltip>
      ) : null}
    </span>
  )
}

interface TagInputProps {
  id?: string
  value: readonly string[]
  onChange: (tags: string[]) => void
  /** 网络里已经有的标签，输入时从里面挑。 */
  suggestions: readonly string[]
  disabled?: boolean
  autoFocus?: boolean
  'aria-label'?: string
  'aria-describedby'?: string
}

type Row = { kind: 'existing'; tag: string } | { kind: 'create'; tag: string }

/**
 * 标签选择器：输入时过滤已有的标签；没有同名的时，第一项是"新建标签 xxx"。
 * 标签没有"先去某处建好"这一步，用到就存在。
 * 键盘：上下键选择，回车确认，退格删除最后一个。内置的 `all` 不能打在节点上，这里不出现。
 */
export function TagInput({
  id,
  value,
  onChange,
  suggestions,
  disabled = false,
  autoFocus = false,
  ...aria
}: TagInputProps) {
  const [text, setText] = useState('')
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(0)
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const wrapperRef = useRef<HTMLDivElement>(null)
  const listId = useId()
  const errorId = useId()

  const rows = useMemo<Row[]>(() => {
    const needle = normalizeTag(text)
    const candidates = suggestions.filter((tag) => tag !== ALL_TAG && !value.includes(tag) && tag.includes(needle))
    const result: Row[] = candidates.map((tag) => ({ kind: 'existing', tag }))
    if (needle !== '' && !suggestions.includes(needle) && !value.includes(needle))
      result.unshift({ kind: 'create', tag: needle })
    return result
  }, [suggestions, text, value])

  const add = (raw: string): boolean => {
    const tag = normalizeTag(raw)
    if (tag === '') return true
    if (tag === ALL_TAG) {
      setError(t.tags.allIsBuiltin)
      return false
    }
    const problem = validateTag(tag)
    if (problem !== null) {
      setError(problem)
      return false
    }
    if (!value.includes(tag)) onChange([...value, tag].sort())
    setText('')
    setError(null)
    setActive(0)
    return true
  }

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' && rows.length > 0) {
      event.preventDefault()
      setOpen(true)
      setActive((index) => (index + 1) % rows.length)
    } else if (event.key === 'ArrowUp' && rows.length > 0) {
      event.preventDefault()
      setActive((index) => (index - 1 + rows.length) % rows.length)
    } else if (event.key === 'Enter') {
      const row = open ? rows[active] : undefined
      if (row === undefined && text.trim() === '') return
      event.preventDefault()
      add(row?.tag ?? text)
    } else if (event.key === ',' || event.key === '，' || event.key === ' ') {
      event.preventDefault()
      add(text)
    } else if (event.key === 'Backspace' && text === '' && value.length > 0) {
      onChange(value.slice(0, -1))
    } else if (event.key === 'Escape' && open) {
      // 只关掉下拉，不要连带把外面的对话框也关了。
      event.stopPropagation()
      setOpen(false)
    }
  }

  const describedBy = [error !== null ? errorId : null, aria['aria-describedby']].filter(Boolean).join(' ')

  return (
    <div>
      <Popover open={open && rows.length > 0 && !disabled} onOpenChange={setOpen}>
        <PopoverAnchor asChild>
          <div
            ref={wrapperRef}
            onClick={() => inputRef.current?.focus()}
            className={cn(
              'flex min-h-9 w-full cursor-text flex-wrap items-center gap-1 rounded-md border border-input bg-card px-2 py-1.5 transition-colors hover:border-border-strong focus-within:border-primary focus-within:outline-3 focus-within:outline-primary/20',
              disabled && 'cursor-not-allowed bg-muted',
              error !== null && 'border-danger',
            )}
          >
            {value.map((tag) => (
              <TagChip
                key={tag}
                tag={tag}
                onRemove={disabled ? undefined : () => onChange(value.filter((other) => other !== tag))}
              />
            ))}
            <input
              ref={inputRef}
              id={id}
              role="combobox"
              aria-expanded={open && rows.length > 0}
              aria-controls={listId}
              aria-autocomplete="list"
              aria-activedescendant={open && rows.length > 0 ? `${listId}-${active}` : undefined}
              aria-label={aria['aria-label']}
              aria-describedby={describedBy === '' ? undefined : describedBy}
              aria-invalid={error !== null ? true : undefined}
              value={text}
              disabled={disabled}
              autoFocus={autoFocus}
              autoComplete="off"
              spellCheck={false}
              placeholder={value.length === 0 ? t.tags.placeholder : ''}
              onChange={(event) => {
                setText(event.target.value)
                setError(null)
                setActive(0)
                setOpen(true)
              }}
              onFocus={() => setOpen(true)}
              onBlur={() => {
                // 输了一半就离开：算作确认，不让它悄悄丢掉。
                if (text.trim() !== '') add(text)
              }}
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
          <ul id={listId} role="listbox" aria-label={t.tags.suggestions} className="max-h-56 overflow-y-auto">
            {rows.map((row, index) => (
              <li
                key={`${row.kind}:${row.tag}`}
                id={`${listId}-${index}`}
                role="option"
                aria-selected={index === active}
                onMouseEnter={() => setActive(index)}
                // 用 mousedown 而不是 click：不让输入框先失去焦点，否则会先触发"离开时确认"。
                onMouseDown={(event) => {
                  event.preventDefault()
                  add(row.tag)
                  inputRef.current?.focus()
                }}
                className={cn(
                  'flex cursor-pointer items-center gap-2 rounded-md px-2.5 py-1.5 text-sm',
                  index === active && 'bg-muted',
                )}
              >
                {row.kind === 'create' ? (
                  <span className="flex items-center gap-1.5 font-medium text-primary">
                    <Plus className="size-3.5" aria-hidden />
                    {t.tags.create(row.tag)}
                  </span>
                ) : (
                  <TagChip tag={row.tag} />
                )}
              </li>
            ))}
          </ul>
        </PopoverContent>
      </Popover>
      {error !== null ? (
        <p id={errorId} role="alert" className="mt-1.5 text-[13px] text-danger">
          {error}
        </p>
      ) : null}
    </div>
  )
}
