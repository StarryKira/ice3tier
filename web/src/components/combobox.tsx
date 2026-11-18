import { Check, ChevronsUpDown, Plus } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, useId, useMemo, useState } from 'react'

import { PickerSearch } from '@/components/picker-search'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'

export interface ComboOption<V> {
  value: V
  label: string
  /** 右侧的灰色小字，例如节点的虚拟 IP。 */
  hint?: string
  /** 名字之外也参与搜索的文字。 */
  keywords?: string
  disabled?: boolean
  /** 不能选的原因。列出来但置灰，比不列出来好：不然用户会以为它丢了。 */
  disabledReason?: ReactNode
  leading?: ReactNode
}

interface CreateConfig {
  /** "新建标签 xxx"这一行的文字。 */
  label: (text: string) => string
  /** 返回错误信息表示这个名字不能用。 */
  validate: (text: string) => string | null
  normalize: (text: string) => string
}

interface ComboboxProps<V extends string | number> {
  id?: string
  value: V | null
  onChange: (value: V) => void
  options: readonly ComboOption<V>[]
  placeholder: string
  searchPlaceholder: string
  emptyText: string
  disabled?: boolean
  className?: string
  'aria-label'?: string
  'aria-describedby'?: string
  'aria-invalid'?: true
  /** 给了就允许就地新建：没有同名的项时，列表第一项是"新建 xxx"。只对字符串类型的值有意义。 */
  create?: V extends string ? CreateConfig : never
}

type Row<V> = { kind: 'option'; option: ComboOption<V> } | { kind: 'create'; text: string; error: string | null }

/** 可以搜索的单选下拉。键盘：上下键移动，回车选中，Esc 关闭。 */
export function Combobox<V extends string | number>({
  id,
  value,
  onChange,
  options,
  placeholder,
  searchPlaceholder,
  emptyText,
  disabled = false,
  className,
  create,
  ...aria
}: ComboboxProps<V>) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const listId = useId()

  const rows = useMemo<Row<V>[]>(() => {
    const needle = query.trim().toLowerCase()
    const matched = options.filter((option) =>
      needle === ''
        ? true
        : `${option.label} ${option.hint ?? ''} ${option.keywords ?? ''}`.toLowerCase().includes(needle),
    )
    const result: Row<V>[] = matched.map((option) => ({ kind: 'option', option }))
    const createConfig = create as CreateConfig | undefined
    if (createConfig !== undefined && needle !== '') {
      const text = createConfig.normalize(query)
      if (!options.some((option) => option.value === text)) {
        result.unshift({ kind: 'create', text, error: createConfig.validate(text) })
      }
    }
    return result
  }, [options, query, create])

  const selectable = (row: Row<V> | undefined): boolean =>
    row !== undefined && (row.kind === 'create' ? row.error === null : row.option.disabled !== true)

  const move = (step: 1 | -1) => {
    if (rows.length === 0) return
    // 跳过不能选的项；转一整圈都没有能选的就停在原地。
    let index = active
    let tries = rows.length
    while (tries > 0) {
      index = (index + step + rows.length) % rows.length
      if (selectable(rows[index])) break
      tries -= 1
    }
    setActive(tries === 0 ? active : index)
  }

  const choose = (row: Row<V> | undefined) => {
    if (row === undefined || !selectable(row)) return
    onChange(row.kind === 'create' ? (row.text as V) : row.option.value)
    setOpen(false)
  }

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      move(1)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      move(-1)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      choose(rows[active])
    }
  }

  const selected = options.find((option) => option.value === value)
  const selectedLabel = selected?.label ?? (value === null ? null : String(value))

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next)
        if (next) {
          setQuery('')
          setActive(
            Math.max(
              0,
              options.findIndex((option) => option.value === value),
            ),
          )
        }
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          id={id}
          role="combobox"
          aria-expanded={open}
          aria-controls={listId}
          aria-haspopup="listbox"
          disabled={disabled}
          {...aria}
          className={cn(
            'flex h-9 w-full items-center justify-between gap-2 rounded-md border border-input bg-card px-3 text-left text-sm transition-colors hover:border-border-strong disabled:bg-muted disabled:text-muted-foreground aria-invalid:border-danger data-[state=open]:border-primary',
            className,
          )}
        >
          <span
            className={cn(
              'flex min-w-0 items-center gap-2 truncate',
              selectedLabel === null && 'text-muted-foreground',
            )}
          >
            {selected?.leading}
            <span className="truncate">{selectedLabel ?? placeholder}</span>
            {selected?.hint !== undefined ? (
              <span className="truncate text-[13px] text-muted-foreground">{selected.hint}</span>
            ) : null}
          </span>
          <ChevronsUpDown className="size-3.5 shrink-0 text-faint" aria-hidden />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-(--radix-popover-trigger-width) min-w-64 p-0">
        <PickerSearch
          value={query}
          onChange={(next) => {
            setQuery(next)
            setActive(0)
          }}
          onKeyDown={onKeyDown}
          placeholder={searchPlaceholder}
          listId={listId}
          activeId={rows.length > 0 ? `${listId}-${active}` : undefined}
        />
        <ul id={listId} role="listbox" className="max-h-64 overflow-y-auto p-1">
          {rows.length === 0 ? (
            <li className="px-2.5 py-3 text-center text-[13px] text-muted-foreground">{emptyText}</li>
          ) : null}
          {rows.map((row, index) => {
            const isActive = index === active
            if (row.kind === 'create') {
              return (
                <li
                  key="__create"
                  id={`${listId}-${index}`}
                  role="option"
                  aria-selected={isActive}
                  aria-disabled={row.error !== null}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => choose(row)}
                  className={cn(
                    'flex flex-col rounded-md px-2 py-1.5 text-sm',
                    isActive && 'bg-muted',
                    row.error !== null && 'cursor-not-allowed',
                  )}
                >
                  <span className="flex items-center gap-2 font-medium text-primary">
                    <Plus className="size-3.5" aria-hidden />
                    {(create as CreateConfig).label(row.text)}
                  </span>
                  {row.error !== null ? <span className="mt-0.5 text-[13px] text-danger">{row.error}</span> : null}
                </li>
              )
            }
            const { option } = row
            const isSelected = option.value === value
            return (
              <li
                key={String(option.value)}
                id={`${listId}-${index}`}
                role="option"
                aria-selected={isSelected}
                aria-disabled={option.disabled === true}
                onMouseEnter={() => setActive(index)}
                onClick={() => choose(row)}
                className={cn(
                  'flex cursor-pointer flex-col rounded-md px-2 py-1.5 text-sm',
                  isActive && 'bg-muted',
                  option.disabled === true && 'cursor-not-allowed',
                )}
              >
                <span className={cn('flex items-center gap-2', option.disabled === true && 'opacity-55')}>
                  {option.leading}
                  <span className="min-w-0 flex-1 truncate">{option.label}</span>
                  {option.hint !== undefined ? (
                    <span className="shrink-0 text-[13px] text-muted-foreground tabular">{option.hint}</span>
                  ) : null}
                  <Check className={cn('size-3.5 shrink-0 text-primary', !isSelected && 'invisible')} aria-hidden />
                </span>
                {option.disabled === true && option.disabledReason !== undefined ? (
                  <span className="mt-0.5 text-[13px] leading-snug text-warn">{option.disabledReason}</span>
                ) : null}
              </li>
            )
          })}
        </ul>
      </PopoverContent>
    </Popover>
  )
}
