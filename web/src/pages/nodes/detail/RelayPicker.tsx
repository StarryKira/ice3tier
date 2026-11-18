import { ChevronDown, TriangleAlert } from 'lucide-react'
import { type KeyboardEvent, useId, useState } from 'react'

import { type Node, type NodeId } from '@/api/types'
import { Flag } from '@/components/flag'
import { PickerSearch } from '@/components/picker-search'
import { OnlineDot } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { CheckMark } from '@/components/ui/check-mark'
import { fieldBase } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { t } from '@/i18n'
import { formatList } from '@/lib/format'
import { nodeName } from '@/lib/nodes'
import { MAX_PINNED_RELAYS } from '@/lib/product'
import { cn } from '@/lib/utils'

const d = t.nodes.detail

interface Row {
  id: NodeId
  name: string
  /** 名字右边的灰字：虚拟 IP。 */
  hint: string
  /** 公网 IP 所在的国家或地区，名字前面画国旗。 */
  country: string | null
  online: boolean
  /** 指定过、但现在已经不是中继了（或者节点已经删掉）。 */
  stale: boolean
}

interface RelayPickerProps {
  id?: string
  /** 网络里的全部节点。 */
  nodes: readonly Node[]
  /** 这个节点自己：不能指定自己。 */
  self: NodeId
  value: readonly NodeId[]
  onChange: (pinned: NodeId[]) => void
  /** 上一次改动还在保存：列表开着，但先不能勾。 */
  busy?: boolean
  'aria-label'?: string
}

/**
 * 指定的中继：和上面几项一样是一个下拉框，框里写着现在指定了哪几个（没有指定时是"自动"）。
 * 点开是能搜索的列表，每行一个中继、在不在线、国旗、虚拟 IP、一个勾选框；勾上、取消立即保存，列表不收起。
 * 选满了没勾的行变灰；指定过但已经不是中继的排在最后，带一个提醒，取消勾选就去掉了。
 */
export function RelayPicker({ id, nodes, self, value, onChange, busy = false, ...aria }: RelayPickerProps) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const listId = useId()

  const chosen = new Set(value)
  const relays: Row[] = nodes
    .filter((node) => node.relay !== null && node.id !== self)
    .map((node) => ({
      id: node.id,
      name: node.name,
      hint: node.virtual_ip,
      country: node.country ?? null,
      online: node.online,
      stale: false,
    }))
  const stale: Row[] = value
    .filter((pinned) => !relays.some((relay) => relay.id === pinned))
    .map((pinned) => ({
      id: pinned,
      name: nodeName(nodes, pinned),
      hint: '',
      country: null,
      online: nodes.find((node) => node.id === pinned)?.online ?? false,
      stale: true,
    }))
  const all = [...relays, ...stale]
  const needle = query.trim().toLowerCase()
  const rows = all.filter((row) => `${row.name} ${row.hint}`.toLowerCase().includes(needle))
  const full = value.length >= MAX_PINNED_RELAYS
  const locked = (row: Row) => busy || (full && !chosen.has(row.id))

  const toggle = (row: Row | undefined) => {
    if (row === undefined || locked(row)) return
    onChange(chosen.has(row.id) ? value.filter((other) => other !== row.id) : [...value, row.id])
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

  const names = value.map((pinned) => nodeName(nodes, pinned))

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (next) {
          setQuery('')
          setActive(0)
        }
        setOpen(next)
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
          aria-label={aria['aria-label']}
          className={cn(fieldBase, 'flex h-9 w-56 items-center gap-2 text-left data-[state=open]:border-primary')}
        >
          {stale.length > 0 ? (
            <TriangleAlert className="size-3.5 shrink-0 text-warn" aria-label={d.pinnedNotRelay} />
          ) : null}
          <span className={cn('min-w-0 flex-1 truncate', value.length === 0 && 'text-muted-foreground')}>
            {value.length === 0 ? d.pinnedAuto : formatList(names)}
          </span>
          <ChevronDown className="size-4 shrink-0 text-faint" aria-hidden />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 p-0">
        <PickerSearch
          value={query}
          onChange={(next) => {
            setQuery(next)
            setActive(0)
          }}
          onKeyDown={onSearchKey}
          placeholder={d.pinnedSearch}
          listId={listId}
          activeId={rows.length > 0 ? `${listId}-${active}` : undefined}
        />
        <ul
          id={listId}
          role="listbox"
          aria-multiselectable
          aria-label={d.pinnedLabel}
          className="max-h-72 overflow-y-auto p-1"
        >
          {rows.map((row, index) => {
            const checked = chosen.has(row.id)
            return (
              <li
                key={row.id}
                id={`${listId}-${index}`}
                role="option"
                aria-selected={checked}
                aria-disabled={locked(row)}
                onMouseEnter={() => setActive(index)}
                // 不让搜索框失去焦点：键盘接着能用。
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => toggle(row)}
                className={cn(
                  'flex min-h-9 items-center gap-2.5 rounded-md px-2 py-1.5 text-sm',
                  locked(row) ? 'cursor-not-allowed' : 'cursor-pointer',
                  index === active && 'bg-muted',
                )}
              >
                <OnlineDot online={row.online} />
                <Flag code={row.country} />
                <span className={cn('min-w-0 flex-1 truncate', locked(row) && !busy && 'text-muted-foreground')}>
                  {row.name}
                </span>
                {row.stale ? (
                  <Badge tone="warn">{d.pinnedNotRelay}</Badge>
                ) : (
                  <span className="shrink-0 text-[13px] text-muted-foreground tabular">{row.hint}</span>
                )}
                <CheckMark checked={checked} locked={locked(row)} />
              </li>
            )
          })}
        </ul>
        {rows.length === 0 ? (
          <p className="px-3 pb-3 text-center text-[13px] text-muted-foreground">
            {all.length === 0 ? d.pinnedNone : t.select.nodeEmpty}
          </p>
        ) : null}
        {/* 底下一行灰字：不选是什么意思、最多几个。选满了这一句就是没勾的行为什么是灰的。 */}
        <p className="border-t px-3 py-2 text-xs text-muted-foreground">{d.pinnedHint(MAX_PINNED_RELAYS)}</p>
      </PopoverContent>
    </Popover>
  )
}
