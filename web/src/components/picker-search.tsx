import { Search } from 'lucide-react'
import { type KeyboardEvent } from 'react'

interface PickerSearchProps {
  value: string
  onChange: (value: string) => void
  onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => void
  placeholder: string
  /** 下面那个列表的 `id`。 */
  listId: string
  /** 键盘停在的那一行的 `id`；没有行时不给。 */
  activeId: string | undefined
}

/**
 * 下拉选择器顶上的搜索框：一条横栏，放大镜在左，底下一条细线。
 * 左边距 12 像素，和关着的框里的内容、列表里各行的内容（列表 4 像素 + 行 8 像素）对齐。
 */
export function PickerSearch({ value, onChange, onKeyDown, placeholder, listId, activeId }: PickerSearchProps) {
  return (
    <div className="flex items-center gap-2 border-b px-3">
      <Search className="size-4 shrink-0 text-faint" aria-hidden />
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={onKeyDown}
        placeholder={placeholder}
        aria-label={placeholder}
        aria-controls={listId}
        aria-activedescendant={activeId}
        autoComplete="off"
        spellCheck={false}
        className="h-10 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
      />
    </div>
  )
}
